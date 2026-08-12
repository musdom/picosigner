#![no_std]
#![no_main]

use core::fmt::Write;

use embedded_hal::{delay::DelayNs, digital::OutputPin, pwm::SetDutyCycle};
use fugit::RateExtU32;
use hal::{
    clocks::Clock,
    dma::{double_buffer, DMAExt, HalfWord, WriteTarget},
    gpio::FunctionPio0,
    pio::{PIOBuilder, PIOExt, ShiftDirection},
};
use heapless::String;
use panic_halt as _;
use picosigner::{
    lcd::{Orientation, WaveshareLcd},
    ov5640::Ov5640,
};
use rp235x_hal as hal;
use usb_device::{class_prelude::*, prelude::*};
use usbd_serial::SerialPort;

#[link_section = ".start_block"]
#[used]
pub static IMAGE_DEF: hal::block::ImageDef = hal::block::ImageDef::secure_exe();

const XTAL_FREQ_HZ: u32 = 12_000_000;
const WIDTH: usize = 240;
const HEIGHT: usize = 320;
const PIXELS: usize = WIDTH * HEIGHT;
// Diagnostic mode: deterministic bars distinguish capture corruption from
// live sensor/exposure behavior. Set false after the capture path is stable.
const CAMERA_TEST_PATTERN: bool = false;

/// One camera frame. Camera DMA writes 16-bit pixels; LCD DMA reads bytes.
struct CameraFrame([u16; PIXELS]);

/// Panel-ready RGB565 bytes, matching the C example's `BlackImage` buffer.
struct DisplayFrame([u8; PIXELS * 2]);

fn frame_hash(frame: &[u16]) -> u32 {
    let mut hash = 0x811c_9dc5u32;
    for &pixel in frame {
        for byte in pixel.to_ne_bytes() {
            hash = (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193);
        }
    }
    hash
}

fn differing_adjacent_rows(frame: &[u16]) -> usize {
    frame
        .chunks_exact(WIDTH)
        .zip(frame.chunks_exact(WIDTH).skip(1))
        .filter(|(previous, current)| previous != current)
        .count()
}

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

    // CDC diagnostics remain responsive while the foreground waits for DMA.
    // At 0x503d=0x80 the standard vertical color bars should produce identical
    // rows, so row_changes should be zero and the frame hash should be stable.
    let usb_bus = UsbBusAllocator::new(hal::usb::UsbBus::new(
        pac.USB,
        pac.USB_DPRAM,
        clocks.usb_clock,
        true,
        &mut pac.RESETS,
    ));
    let mut serial = SerialPort::new(&usb_bus);
    let mut usb_dev = UsbDeviceBuilder::new(&usb_bus, UsbVidPid(0x16c0, 0x27dd))
        .strings(&[StringDescriptors::default()
            .manufacturer("Picosigner")
            .product("OV5640 diagnostics")
            .serial_number("CAMERA")])
        .unwrap()
        .max_packet_size_0(64)
        .unwrap()
        .device_class(2)
        .build();
    let mut report: String<256> = String::new();
    let mut report_offset = 0usize;
    let mut frame_number = 0u32;

    let dma = pac.DMA.split(&mut pac.RESETS);
    let camera_frame_a = hal::singleton!(: CameraFrame = CameraFrame([0; PIXELS])).unwrap();
    let camera_frame_b = hal::singleton!(: CameraFrame = CameraFrame([0; PIXELS])).unwrap();
    let display_frame = hal::singleton!(: DisplayFrame = DisplayFrame([0; PIXELS * 2])).unwrap();

    // Match the C example's continuously rearmed camera DMA. Two chained
    // buffers let DMA fill one frame while the CPU consumes the other without
    // the data race present in the vendor's single cam_ptr buffer.
    let mut camera_transfer =
        double_buffer::Config::new((dma.ch0, dma.ch1), camera_rx, camera_frame_a)
            .start()
            .write_next(camera_frame_b);
    while !camera_tx.write(0) {}
    while !camera_tx.write((PIXELS - 1) as u32) {}

    loop {
        while !camera_transfer.is_done() {
            if usb_dev.poll(&mut [&mut serial]) && report_offset < report.len() {
                if let Ok(written) = serial.write(&report.as_bytes()[report_offset..]) {
                    report_offset += written;
                }
            }
        }
        let (captured_frame, active_transfer) = camera_transfer.wait();
        let captured_hash = frame_hash(&captured_frame.0);

        if report_offset >= report.len() {
            report.clear();
            report_offset = 0;
            frame_number = frame_number.wrapping_add(1);
            let pixels = &captured_frame.0;
            let _ = writeln!(
                &mut report,
                "frame={frame_number} hash={:08x} row_changes={} first={:04x},{:04x},{:04x},{:04x}",
                captured_hash,
                differing_adjacent_rows(pixels),
                pixels[0],
                pixels[1],
                pixels[2],
                pixels[3],
            );
        }

        // This deliberately mirrors Waveshare's Paint_DrawImage path: DMA
        // stores each camera pixel as a native-endian u16, then the CPU writes
        // its high byte followed by its low byte into the LCD image buffer.
        for (source, destination) in captured_frame
            .0
            .iter()
            .zip(display_frame.0.chunks_exact_mut(2))
        {
            destination.copy_from_slice(&source.to_be_bytes());
        }

        // The conversion no longer reads captured_frame, so queue it behind
        // the DMA transfer already filling the other camera buffer. This is
        // the safe double-buffered equivalent of the C IRQ rearming cam_ptr.
        camera_transfer = active_transfer.write_next(captured_frame);

        // Match the C example: camera input uses DMA, while the completed
        // framebuffer is sent to the LCD with a blocking SPI write. This avoids
        // concurrent camera/LCD DMA arbitration corrupting panel transfers.
        display.write_frame(&display_frame.0);
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
