#![no_std]
#![no_main]

use embedded_hal::{delay::DelayNs, digital::OutputPin, pwm::SetDutyCycle};
use fugit::RateExtU32;
use hal::{
    clocks::Clock,
    dma::{single_buffer, DMAExt, HalfWord, ReadTarget, WriteTarget},
    gpio::FunctionPio0,
    pio::{PIOBuilder, PIOExt, ShiftDirection},
};
use panic_halt as _;
use picosigner::{
    lcd::{Orientation, WaveshareLcd},
    ov5640::Ov5640,
};
use rp235x_hal as hal;

#[link_section = ".start_block"]
#[used]
pub static IMAGE_DEF: hal::block::ImageDef = hal::block::ImageDef::secure_exe();

const XTAL_FREQ_HZ: u32 = 12_000_000;
const WIDTH: usize = 240;
const HEIGHT: usize = 320;
const PIXELS: usize = WIDTH * HEIGHT;
// Diagnostic mode: deterministic bars distinguish capture corruption from
// live sensor/exposure behavior. Set false after the capture path is stable.
const CAMERA_TEST_PATTERN: bool = true;

/// One camera frame. Camera DMA writes 16-bit pixels; LCD DMA reads bytes.
struct CameraFrame([u16; PIXELS]);

/// Panel-ready RGB565 bytes, matching the C example's `BlackImage` buffer.
struct DisplayFrame([u8; PIXELS * 2]);

// Safety: CameraFrame is allocated in static SRAM and the DMA transfer owns
// this handle exclusively until wait() returns it.
unsafe impl WriteTarget for &'static mut CameraFrame {
    type TransmittedWord = u16;

    fn tx_treq() -> Option<u8> {
        None
    }

    fn tx_address_count(&mut self) -> (u32, u32) {
        (self.0.as_mut_ptr() as u32, PIXELS as u32)
    }

    fn tx_increment(&self) -> bool {
        true
    }
}

// Safety: DisplayFrame is allocated in static SRAM and the DMA transfer owns
// this handle exclusively until wait() returns it.
unsafe impl ReadTarget for &'static mut DisplayFrame {
    type ReceivedWord = u8;

    fn rx_treq() -> Option<u8> {
        None
    }

    fn rx_address_count(&self) -> (u32, u32) {
        (self.0.as_ptr() as u32, self.0.len() as u32)
    }

    fn rx_increment(&self) -> bool {
        true
    }
}

#[hal::entry]
fn main() -> ! {
    let mut pac = hal::pac::Peripherals::take().unwrap();
    let mut watchdog = hal::Watchdog::new(pac.WATCHDOG);
    let clocks = hal::clocks::init_clocks_and_plls(
        XTAL_FREQ_HZ,
        pac.XOSC,
        pac.CLOCKS,
        pac.PLL_SYS,
        pac.PLL_USB,
        &mut pac.RESETS,
        &mut watchdog,
    )
    .unwrap();
    let mut timer = hal::Timer::new_timer0(pac.TIMER0, &mut pac.RESETS, &clocks);
    let sio = hal::Sio::new(pac.SIO);
    let pins = hal::gpio::Pins::new(
        pac.IO_BANK0,
        pac.PADS_BANK0,
        sio.gpio_bank0,
        &mut pac.RESETS,
    );

    // Camera XCLK: GPIO11, PWM slice 5 channel B. At the 150 MHz system
    // clock, TOP=3 produces 37.5 MHz, matching the Waveshare example's
    // requested 37 MHz as closely as the integer PWM divider permits.
    let mut pwm_slices = hal::pwm::Slices::new(pac.PWM, &mut pac.RESETS);
    let xclk_pwm = &mut pwm_slices.pwm5;
    xclk_pwm.set_div_int(1);
    xclk_pwm.set_top(3);
    xclk_pwm.enable();
    let xclk = &mut xclk_pwm.channel_b;
    xclk.output_to(pins.gpio11);
    xclk.set_duty_cycle(2).unwrap();
    timer.delay_ms(50);

    // OV5640 SCCB control bus: I2C1 SDA=GPIO22, SCL=GPIO23.
    let sda: hal::gpio::Pin<_, hal::gpio::FunctionI2C, hal::gpio::PullUp> =
        pins.gpio22.reconfigure();
    let scl: hal::gpio::Pin<_, hal::gpio::FunctionI2C, hal::gpio::PullUp> =
        pins.gpio23.reconfigure();
    let i2c = hal::I2C::i2c1(
        pac.I2C1,
        sda,
        scl,
        100.kHz(),
        &mut pac.RESETS,
        &clocks.system_clock,
    );
    let mut camera = Ov5640::new(i2c);
    camera.init(&mut timer).unwrap();
    camera.set_test_pattern(CAMERA_TEST_PATTERN).unwrap();

    // Camera DVP bus: D2..D9=GPIO0..7, VSYNC=8, HREF=9, PCLK=10.
    let _camera_pins = (
        pins.gpio0.into_function::<FunctionPio0>(),
        pins.gpio1.into_function::<FunctionPio0>(),
        pins.gpio2.into_function::<FunctionPio0>(),
        pins.gpio3.into_function::<FunctionPio0>(),
        pins.gpio4.into_function::<FunctionPio0>(),
        pins.gpio5.into_function::<FunctionPio0>(),
        pins.gpio6.into_function::<FunctionPio0>(),
        pins.gpio7.into_function::<FunctionPio0>(),
        pins.gpio8.into_function::<FunctionPio0>(),
        pins.gpio9.into_function::<FunctionPio0>(),
        pins.gpio10.into_function::<FunctionPio0>(),
    );

    // Capture one RGB565 pixel (two DVP bytes) per RX FIFO entry. Keep the
    // vendor program's flat frame counter and gate *every pixel* with HREF.
    // The latter matters because PCLK is not gated during horizontal blanking
    // by the sensor's 0x4740 setting. Counting an assumed 240 pixels per HREF
    // interval can therefore slowly walk into blanking if a line is missed.
    // After PIXELS pushes the program wraps to `pull` and waits for the CPU,
    // so capture cannot resume in the middle of a frame while the LCD is busy.
    let program = pio::pio_asm!(
        ".wrap_target",
        "pull block",
        "mov y, osr",
        "wait 0 pin 8",
        "wait 1 pin 8",
        "mov x, y",
        "pixel:",
        "wait 1 pin 9",
        "wait 1 pin 10",
        "in pins, 8",
        "wait 0 pin 10",
        "wait 1 pin 10",
        "in pins, 8",
        "wait 0 pin 10",
        "push block",
        "jmp x-- pixel",
        ".wrap"
    );
    let (mut pio, sm0, _, _, _) = pac.PIO0.split(&mut pac.RESETS);
    let installed = pio.install(&program.program).unwrap();
    let (mut capture_sm, camera_rx, mut camera_tx) = PIOBuilder::from_installed_program(installed)
        .in_pin_base(0)
        .in_shift_direction(ShiftDirection::Left)
        // Match the SDK's sm_config_set_in_shift(..., false, false, 32).
        // IN_COUNT is intentionally left at its 32-pin reset default.
        .push_threshold(32)
        .clock_divisor_fixed_point(1, 0)
        .build(sm0);
    capture_sm.set_pindirs((0..=10).map(|pin| (pin, hal::pio::PinDir::Input)));
    capture_sm.clear_fifos();
    // The program immediately stalls on its first `pull`. Supplying the frame
    // pixel count after arming DMA releases it for exactly one frame.
    let _capture_sm = capture_sm.start();
    let mut camera_rx = camera_rx.transfer_size(HalfWord);

    // LCD: SPI0 CLK=18, MOSI=19, DC=16, CS=17, reset=20, backlight=15.
    let lcd_clk = pins.gpio18.into_function::<hal::gpio::FunctionSpi>();
    let lcd_mosi = pins.gpio19.into_function::<hal::gpio::FunctionSpi>();
    let lcd_spi = hal::spi::Spi::<_, _, _, 8>::new(pac.SPI0, (lcd_mosi, lcd_clk)).init(
        &mut pac.RESETS,
        clocks.peripheral_clock.freq(),
        62_500.kHz(),
        embedded_hal::spi::MODE_0,
    );
    let dc = pins.gpio16.into_push_pull_output();
    let cs = pins.gpio17.into_push_pull_output();
    let rst = pins.gpio20.into_push_pull_output();
    let mut backlight = pins.gpio15.into_push_pull_output();
    let mut display = WaveshareLcd::new(lcd_spi, dc, cs, rst);
    display.init(&mut timer, Orientation::Portrait);
    backlight.set_high().unwrap();

    let dma = pac.DMA.split(&mut pac.RESETS);
    let mut camera_dma = dma.ch0;
    let mut display_dma = dma.ch1;
    let mut camera_frame = hal::singleton!(: CameraFrame = CameraFrame([0; PIXELS])).unwrap();
    let mut display_frame =
        hal::singleton!(: DisplayFrame = DisplayFrame([0; PIXELS * 2])).unwrap();

    loop {
        // Arm DMA before releasing the PIO program. At the end of the frame,
        // it wraps to `pull block` and stalls until the next iteration.
        let transfer = single_buffer::Config::new(camera_dma, camera_rx, camera_frame).start();
        while !camera_tx.write((PIXELS - 1) as u32) {}

        let (channel, rx, captured_frame) = transfer.wait();
        camera_dma = channel;
        camera_rx = rx;
        camera_frame = captured_frame;

        // This deliberately mirrors Waveshare's Paint_DrawImage path: DMA
        // stores each camera pixel as a native-endian u16, then the CPU writes
        // its high byte followed by its low byte into the LCD image buffer.
        for (source, destination) in camera_frame
            .0
            .iter()
            .zip(display_frame.0.chunks_exact_mut(2))
        {
            destination.copy_from_slice(&source.to_be_bytes());
        }

        display.begin_frame_write();
        let (spi, dc, cs, rst, size) = display.release();
        let transfer = single_buffer::Config::new(display_dma, display_frame, spi).start();
        let (channel, returned_display_frame, spi) = transfer.wait();
        display_dma = channel;
        display_frame = returned_display_frame;
        display = WaveshareLcd::from_parts(spi, dc, cs, rst, size);
        display.end_frame_write();
    }
}

#[link_section = ".bi_entries"]
#[used]
pub static PICOTOOL_ENTRIES: [hal::binary_info::EntryAddr; 5] = [
    hal::binary_info::rp_cargo_bin_name!(),
    hal::binary_info::rp_cargo_version!(),
    hal::binary_info::rp_program_description!(c"OV5640 camera preview"),
    hal::binary_info::rp_cargo_homepage_url!(),
    hal::binary_info::rp_program_build_attribute!(),
];
