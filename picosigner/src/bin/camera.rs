#![no_std]
#![no_main]

use embedded_hal::{delay::DelayNs, digital::OutputPin, pwm::SetDutyCycle, spi::SpiBus};
use fugit::RateExtU32;
use hal::{
    clocks::Clock,
    dma::{single_buffer, DMAExt, HalfWord, WriteTarget},
    gpio::FunctionPio0,
    pio::{PIOBuilder, PIOExt, ShiftDirection},
};
use picosigner_board::{
    display::{Orientation, WaveshareLcd},
    ov5640::Ov5640,
    ResourceArena, FRAMEBUFFER_BYTES,
};
use rp235x_hal as hal;

#[link_section = ".start_block"]
#[used]
pub static IMAGE_DEF: hal::block::ImageDef = hal::block::ImageDef::secure_exe();

const XTAL_FREQ_HZ: u32 = 12_000_000;
const WIDTH: usize = 240;
const HEIGHT: usize = 320;
const PIXELS: usize = WIDTH * HEIGHT;
const ROW_BYTES: usize = WIDTH * 2;
// Diagnostic mode: deterministic bars distinguish capture corruption from
// live sensor/exposure behavior. Set false after the capture path is stable.
const CAMERA_TEST_PATTERN: bool = false;

/// A DMA target owns the mutable camera view until the transfer is returned
/// by `wait()`. Consequently the CPU cannot read the arena-backed frame while
/// DMA owns it, and DMA is never rearmed until the preview has finished.
struct CameraDmaTarget<'a> {
    pixels: &'a mut [u16],
}

impl<'a> CameraDmaTarget<'a> {
    fn new(pixels: &'a mut [u16]) -> Self {
        Self { pixels }
    }
}

// Safety: the mutable slice is the sole access path to the claimed arena
// storage while this target is held by the transfer. The camera DMA channel
// is the only producer configured for it.
unsafe impl<'a> WriteTarget for CameraDmaTarget<'a> {
    type TransmittedWord = u16;

    fn tx_treq() -> Option<u8> {
        None
    }

    fn tx_address_count(&mut self) -> (u32, u32) {
        (self.pixels.as_mut_ptr() as u32, self.pixels.len() as u32)
    }

    fn tx_increment(&self) -> bool {
        true
    }
}

/// Stream one stable camera frame through the LCD's existing continuous
/// window. The only CPU-side staging is one row, so the camera capture arena
/// is not duplicated by a second full-screen framebuffer.
fn stream_frame<SPI, DC, CS, RST>(
    mut display: WaveshareLcd<SPI, DC, CS, RST>,
    pixels: &[u16],
    row: &mut [u8; ROW_BYTES],
) -> WaveshareLcd<SPI, DC, CS, RST>
where
    SPI: SpiBus<u8>,
    DC: OutputPin,
    CS: OutputPin,
    RST: OutputPin,
{
    debug_assert_eq!(pixels.len(), PIXELS);
    display.begin_frame_write();
    let (mut spi, dc, mut cs, rst, size) = display.release();
    debug_assert_eq!(size.width as usize, WIDTH);
    debug_assert_eq!(size.height as usize, HEIGHT);

    for y in 0..HEIGHT {
        for (destination, &pixel) in row.chunks_exact_mut(2).zip(&pixels[y * WIDTH..][..WIDTH]) {
            destination.copy_from_slice(&pixel.to_be_bytes());
        }
        let _ = spi.write(row);
    }
    let _ = spi.flush();
    let _ = cs.set_high();
    WaveshareLcd::from_parts(spi, dc, cs, rst, size)
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

    // Camera XCLK: GPIO11, PWM slice 5 channel B. The Waveshare example asks
    // for 37 MHz (which becomes 37.5 MHz), but the OV5640 specifies a 27 MHz
    // maximum input clock. Use an exact 25 MHz to keep XCLK in specification
    // and give the asynchronous PIO input substantially more sampling margin.
    let mut pwm_slices = hal::pwm::Slices::new(pac.PWM, &mut pac.RESETS);
    let xclk_pwm = &mut pwm_slices.pwm5;
    xclk_pwm.set_div_int(1);
    xclk_pwm.set_top(5);
    xclk_pwm.enable();
    let xclk = &mut xclk_pwm.channel_b;
    xclk.output_to(pins.gpio11);
    xclk.set_duty_cycle(3).unwrap();
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

    // Resynchronize every DMA block to VSYNC. OV5640 register 0x4740 gates
    // PCLK outside active HREF, so every PCLK transition observed here is a
    // valid data byte and the state machine doesn't need to sample HREF.
    let program = pio::pio_asm!(
        ".wrap_target",
        "out x, 32",
        "out y, 32",
        "frame:",
        "wait 0 pin 8",
        "wait 1 pin 8",
        "mov x, y",
        "pixel:",
        "wait 1 pin 10",
        "in pins, 8",
        "wait 0 pin 10",
        "wait 1 pin 10",
        "in pins, 8",
        "wait 0 pin 10",
        "push block",
        "jmp x-- pixel",
        "jmp frame",
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
        .autopull(true)
        .pull_threshold(32)
        .clock_divisor_fixed_point(1, 0)
        .build(sm0);
    capture_sm.set_pindirs((0..=10).map(|pin| (pin, hal::pio::PinDir::Input)));
    capture_sm.clear_fifos();
    // The first OUT stalls until the two setup words are supplied below.
    let _capture_sm = capture_sm.start();
    let camera_rx = camera_rx.transfer_size(HalfWord);

    // LCD: SPI0 CLK=18, MOSI=19, DC=16, CS=17, reset=20, backlight=15.
    let mut display = picosigner::init_display!(
        pac,
        pins,
        delay: &mut timer,
        freq: clocks.peripheral_clock.freq(),
        orientation: Orientation::Portrait,
    );
    let mut backlight = pins.gpio15.into_push_pull_output();
    backlight.set_high().unwrap();

    let dma = pac.DMA.split(&mut pac.RESETS);
    let arena = hal::singleton!(: ResourceArena<FRAMEBUFFER_BYTES> = ResourceArena::new()).unwrap();
    let mut camera_resource = arena.claim().unwrap();
    let mut row = [0u8; ROW_BYTES];
    let mut camera_transfer = single_buffer::Config::new(
        dma.ch0,
        camera_rx,
        CameraDmaTarget::new(camera_resource.words().unwrap()),
    )
    .start();
    while !camera_tx.write(0) {}
    while !camera_tx.write((PIXELS - 1) as u32) {}

    loop {
        let (camera_dma, camera_rx, camera_target) = camera_transfer.wait();
        drop(camera_target);

        // DMA is stopped and its mutable arena lease has been returned before
        // this shared view is created. The frame remains stable for the whole
        // row-streaming operation.
        let camera_pixels = camera_resource.words_shared().unwrap();
        display = stream_frame(display, camera_pixels, &mut row);

        // Rearm only after the CPU has finished reading the frame. This is the
        // ownership boundary that the former two-full-framebuffer design did
        // not provide.
        camera_transfer = single_buffer::Config::new(
            camera_dma,
            camera_rx,
            CameraDmaTarget::new(camera_resource.words().unwrap()),
        )
        .start();
        while !camera_tx.write(0) {}
        while !camera_tx.write((PIXELS - 1) as u32) {}
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
