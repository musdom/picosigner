#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;
use core::{
    cell::UnsafeCell,
    convert::Infallible,
    fmt::Write as _,
    hint::spin_loop,
    sync::atomic::{AtomicU32, AtomicU8, AtomicUsize, Ordering},
};

use embedded_graphics::{
    geometry::{OriginDimensions, Size},
    mono_font::{ascii::FONT_6X10, MonoTextStyle},
    pixelcolor::{Rgb565, RgbColor},
    prelude::{DrawTarget, IntoStorage, Pixel, Point, Primitive},
    primitives::{PrimitiveStyleBuilder, Rectangle},
    text::Text,
    Drawable,
};
use embedded_hal::{delay::DelayNs, digital::OutputPin, pwm::SetDutyCycle};
use fugit::RateExtU32;
use hal::{
    clocks::Clock,
    dma::{single_buffer, DMAExt, HalfWord, ReadTarget, WriteTarget},
    gpio::FunctionPio0,
    multicore::{Multicore, Stack},
    pio::{PIOBuilder, PIOExt, ShiftDirection},
};
use heapless::{String as FixedString, Vec as FixedVec};
// Panic handler comes from the `picosigner` library, which renders the
// panic location and message on the LCD.
use picosigner as _;
use picosigner::heap;
use picosigner_board::{
    Cst816d, Orientation, Ov5640, SensorZoom, WaveshareLcd, CAMERA_GRAY_HEIGHT, CAMERA_GRAY_WIDTH,
    DISPLAY_HEIGHT, DISPLAY_WIDTH,
};
use picosigner_qr::{extract_rgb565_volatile, QuircDecoder};
use rp235x_hal as hal;
use usb_device::{class_prelude::*, prelude::*};
use usbd_serial::SerialPort;

#[link_section = ".start_block"]
#[used]
pub static IMAGE_DEF: hal::block::ImageDef = hal::block::ImageDef::secure_exe();

const XTAL_FREQ_HZ: u32 = 12_000_000;
const WIDTH: usize = DISPLAY_WIDTH;
const HEIGHT: usize = DISPLAY_HEIGHT;
const PIXELS: usize = WIDTH * HEIGHT;
const QR_WIDTH: usize = CAMERA_GRAY_WIDTH;
const QR_HEIGHT: usize = CAMERA_GRAY_HEIGHT;
const ZOOM_X: usize = (WIDTH - QR_WIDTH) / 2;
const ZOOM_Y: usize = (HEIGHT - QR_HEIGHT) / 2;
const BUTTON_X: u16 = 20;
const BUTTON_Y: u16 = 268;
const BUTTON_WIDTH: u16 = 200;
const BUTTON_HEIGHT: u16 = 40;
const ZOOM_BUTTON_Y: u16 = 274;
const ZOOM_BUTTON_WIDTH: u16 = 100;
const ZOOM_BUTTON_HEIGHT: u16 = 38;
const ZOOM_BUTTON_X: u16 = (WIDTH as u16 - ZOOM_BUTTON_WIDTH) / 2;
const PAYLOAD_CAPACITY: usize = 1024;
// The capture loop below uses autopush instead of an explicit `push block`,
// which cuts the worst gap between PCLK edges from 3 instructions to 2 and lets
// PIO keep up with the doubled PCLK this enables.
const FAST_CAMERA_TIMING: bool = true;

/// Drive the LCD with DMA on channel 1 instead of a blocking SPI write.
/// Set false to fall back to the blocking path.
const LCD_DMA: bool = true;

/// Halve the sensor's vertical frame period (VTS 1944 -> 996).
const FAST_FRAME_PERIOD: bool = true;
const INITIAL_SENSOR_ZOOM: SensorZoom = SensorZoom::X1;

const DECODER_IDLE: u8 = 0;
const FRAME_READY: u8 = 1;
const DECODING: u8 = 2;
const RESULT_READY: u8 = 3;

#[repr(align(4))]
struct CameraFrame(UnsafeCell<[u8; PIXELS * 2]>);

unsafe impl Sync for CameraFrame {}

static CAMERA_FRAME: CameraFrame = CameraFrame(UnsafeCell::new([0; PIXELS * 2]));

struct SharedPayload(UnsafeCell<[u8; PAYLOAD_CAPACITY]>);

unsafe impl Sync for SharedPayload {}

static DECODER_STATE: AtomicU8 = AtomicU8::new(DECODER_IDLE);
static PAYLOAD_LENGTH: AtomicUsize = AtomicUsize::new(0);
static DECODED_PAYLOAD: SharedPayload = SharedPayload(UnsafeCell::new([0; PAYLOAD_CAPACITY]));
static GRAYSCALE_US: AtomicU32 = AtomicU32::new(0);
static IDENTIFY_US: AtomicU32 = AtomicU32::new(0);
static DECODE_US: AtomicU32 = AtomicU32::new(0);

// `quircs::Code::decode()` currently puts an approximately 18 KiB
// `Datastream` on the stack. Give core 1 enough room for that plus extraction,
// error correction, and the multicore startup frame.
static CORE1_STACK: Stack<8192> = Stack::new();

/// DMA read source over the camera/display framebuffer.
///
/// Byte-wise so the panel receives the buffer exactly as laid out: the display
/// pass has already byte-swapped it into the panel's high-byte-first order.
#[derive(Clone, Copy)]
struct LcdSource;

unsafe impl ReadTarget for LcdSource {
    type ReceivedWord = u8;

    fn rx_treq() -> Option<u8> {
        None
    }

    fn rx_address_count(&self) -> (u32, u32) {
        (CAMERA_FRAME.0.get() as u32, (PIXELS * 2) as u32)
    }

    fn rx_increment(&self) -> bool {
        true
    }
}

#[derive(Clone, Copy)]
struct CameraDmaTarget;

struct UiFrame<'a>(&'a mut [u8]);

impl OriginDimensions for UiFrame<'_> {
    fn size(&self) -> Size {
        Size::new(WIDTH as u32, HEIGHT as u32)
    }
}

impl DrawTarget for UiFrame<'_> {
    type Color = Rgb565;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            if point.x >= 0 && point.y >= 0 && point.x < WIDTH as i32 && point.y < HEIGHT as i32 {
                // The panel's portrait full-frame scan traverses each row in
                // the opposite X direction from embedded-graphics. Reverse X
                // here so glyphs are displayed left-to-right without changing
                // the camera preview's proven MADCTL configuration.
                let framebuffer_x = WIDTH - 1 - point.x as usize;
                let offset = (point.y as usize * WIDTH + framebuffer_x) * 2;
                self.0[offset..offset + 2].copy_from_slice(&color.into_storage().to_be_bytes());
            }
        }
        Ok(())
    }
}

fn draw_result(frame: &mut [u8], payload: &[u8]) {
    frame.fill(0);
    let mut target = UiFrame(frame);
    let white = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);

    Text::new("QR decoded", Point::new(8, 16), white)
        .draw(&mut target)
        .unwrap();

    let mut line = FixedString::<38>::new();
    let mut baseline = 38;
    for &byte in payload {
        if byte == b'\n' || line.len() == 37 {
            Text::new(&line, Point::new(8, baseline), white)
                .draw(&mut target)
                .unwrap();
            line.clear();
            baseline += 11;
            if baseline > 252 {
                break;
            }
            if byte == b'\n' {
                continue;
            }
        }
        line.push(if byte.is_ascii_graphic() || byte == b' ' {
            byte as char
        } else {
            '.'
        })
        .unwrap();
    }
    if !line.is_empty() && baseline <= 252 {
        Text::new(&line, Point::new(8, baseline), white)
            .draw(&mut target)
            .unwrap();
    }

    Rectangle::new(
        Point::new(BUTTON_X as i32, BUTTON_Y as i32),
        Size::new(BUTTON_WIDTH as u32, BUTTON_HEIGHT as u32),
    )
    .into_styled(
        PrimitiveStyleBuilder::new()
            .stroke_color(Rgb565::WHITE)
            .stroke_width(2)
            .build(),
    )
    .draw(&mut target)
    .unwrap();
    Text::new("Scan QR", Point::new(99, 294), white)
        .draw(&mut target)
        .unwrap();
}

fn scan_button_contains(x: u16, y: u16) -> bool {
    (BUTTON_X..BUTTON_X + BUTTON_WIDTH).contains(&x)
        && (BUTTON_Y..BUTTON_Y + BUTTON_HEIGHT).contains(&y)
}

fn draw_viewfinder_overlay(frame: &mut [u8], zoom: SensorZoom) {
    let mut target = UiFrame(frame);
    let button_style = PrimitiveStyleBuilder::new()
        .fill_color(Rgb565::BLACK)
        .stroke_color(Rgb565::WHITE)
        .stroke_width(1)
        .build();
    let white = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);

    Rectangle::new(
        Point::new(ZOOM_X as i32, ZOOM_Y as i32),
        Size::new(QR_WIDTH as u32, QR_HEIGHT as u32),
    )
    .into_styled(
        PrimitiveStyleBuilder::new()
            .stroke_color(Rgb565::WHITE)
            .stroke_width(1)
            .build(),
    )
    .draw(&mut target)
    .unwrap();

    Rectangle::new(
        Point::new(ZOOM_BUTTON_X as i32, ZOOM_BUTTON_Y as i32),
        Size::new(ZOOM_BUTTON_WIDTH as u32, ZOOM_BUTTON_HEIGHT as u32),
    )
    .into_styled(button_style)
    .draw(&mut target)
    .unwrap();
    Text::new(
        match zoom {
            SensorZoom::X1 => "Zoom 1x",
            SensorZoom::X2 => "Zoom 2x",
        },
        Point::new((ZOOM_BUTTON_X + 29) as i32, (ZOOM_BUTTON_Y + 24) as i32),
        white,
    )
    .draw(&mut target)
    .unwrap();
}

fn zoom_button_contains(x: u16, y: u16) -> bool {
    (ZOOM_BUTTON_X..ZOOM_BUTTON_X + ZOOM_BUTTON_WIDTH).contains(&x)
        && (ZOOM_BUTTON_Y..ZOOM_BUTTON_Y + ZOOM_BUTTON_HEIGHT).contains(&y)
}

unsafe impl WriteTarget for CameraDmaTarget {
    type TransmittedWord = u16;

    fn tx_treq() -> Option<u8> {
        None
    }

    fn tx_address_count(&mut self) -> (u32, u32) {
        (CAMERA_FRAME.0.get().cast::<u16>() as u32, PIXELS as u32)
    }

    fn tx_increment(&self) -> bool {
        true
    }
}

#[inline]
fn timer_micros() -> u32 {
    // Shared free-running 1 MHz counter, readable from either core.
    unsafe { (*hal::pac::TIMER0::ptr()).timerawl().read().bits() }
}

fn poll_usb<B: UsbBus>(
    usb_dev: &mut UsbDevice<'_, B>,
    serial: &mut SerialPort<'_, B>,
    event: &FixedVec<u8, 1152>,
    event_offset: &mut usize,
    report: &FixedString<192>,
    report_offset: &mut usize,
) {
    if usb_dev.poll(&mut [serial]) {
        if *event_offset < event.len() {
            if let Ok(written) = serial.write(&event[*event_offset..]) {
                *event_offset += written;
            }
        } else if *report_offset < report.len() {
            if let Ok(written) = serial.write(&report.as_bytes()[*report_offset..]) {
                *report_offset += written;
            }
        }
    }
}

fn decoder_core() {
    let mut grayscale = vec![0u8; QR_WIDTH * QR_HEIGHT];
    let mut decoder = QuircDecoder::<QR_WIDTH, QR_HEIGHT, PAYLOAD_CAPACITY>::new();
    let mut centered_zoom = false;

    loop {
        while DECODER_STATE.load(Ordering::Acquire) != FRAME_READY {
            spin_loop();
        }

        let grayscale_started = timer_micros();
        let camera_pixels = CAMERA_FRAME.0.get().cast::<u16>();
        // Keep extraction in the QR library so production scanning and host
        // tests exercise the same bounded grayscale seam.
        if unsafe {
            extract_rgb565_volatile(camera_pixels, WIDTH, HEIGHT, centered_zoom, &mut grayscale)
        }
        .is_err()
        {
            DECODER_STATE.store(DECODER_IDLE, Ordering::Release);
            continue;
        }
        centered_zoom = !centered_zoom;
        GRAYSCALE_US.store(
            timer_micros().wrapping_sub(grayscale_started),
            Ordering::Relaxed,
        );

        // The decoder no longer reads CAMERA_FRAME after this release. Core 0
        // may start the next DMA capture while identify/decode uses grayscale.
        DECODER_STATE.store(DECODING, Ordering::Release);

        let decoded = decoder.decode_with_timing(&grayscale, timer_micros);
        match decoded {
            Ok((Some(payload), timing)) => {
                IDENTIFY_US.store(timing.identify_us, Ordering::Relaxed);
                DECODE_US.store(timing.decode_us, Ordering::Relaxed);
                let length = payload.0.len();
                unsafe {
                    (&mut *DECODED_PAYLOAD.0.get())[..length].copy_from_slice(payload.0);
                }
                PAYLOAD_LENGTH.store(length, Ordering::Relaxed);
                DECODER_STATE.store(RESULT_READY, Ordering::Release);
            }
            Ok((None, timing)) => {
                IDENTIFY_US.store(timing.identify_us, Ordering::Relaxed);
                DECODE_US.store(timing.decode_us, Ordering::Relaxed);
                DECODER_STATE.store(DECODER_IDLE, Ordering::Release);
            }
            Err(_) => {
                DECODER_STATE.store(DECODER_IDLE, Ordering::Release);
            }
        }
    }
}

#[hal::entry]
fn main() -> ! {
    unsafe { heap::init() };

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
    let mut sio = hal::Sio::new(pac.SIO);
    let pins = hal::gpio::Pins::new(
        pac.IO_BANK0,
        pac.PADS_BANK0,
        sio.gpio_bank0,
        &mut pac.RESETS,
    );

    // Exact 25 MHz OV5640 XCLK on GPIO11.
    let mut pwm_slices = hal::pwm::Slices::new(pac.PWM, &mut pac.RESETS);
    let xclk_pwm = &mut pwm_slices.pwm5;
    xclk_pwm.set_div_int(1);
    xclk_pwm.set_top(5);
    xclk_pwm.enable();
    let xclk = &mut xclk_pwm.channel_b;
    xclk.output_to(pins.gpio11);
    xclk.set_duty_cycle(3).unwrap();
    timer.delay_ms(50);

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
    let mut sensor_zoom = INITIAL_SENSOR_ZOOM;
    camera.set_zoom(sensor_zoom).unwrap();
    camera.set_fast_frame_timing(FAST_CAMERA_TIMING, &mut timer).unwrap();
    camera.set_fast_frame_period(FAST_FRAME_PERIOD).unwrap();
    camera.set_test_pattern(false).unwrap();

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

    // One-shot frame capture. After PIXELS pushes, wrapping to OUT stalls until
    // the CPU supplies the next two setup words. This keeps the single camera
    // buffer stable while it is converted to preview and grayscale images.
    let program = pio::pio_asm!(
        ".wrap_target",
        "out x, 32",
        "out y, 32",
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
        "jmp x-- pixel",
        ".wrap"
    );
    let (mut pio, sm0, _, _, _) = pac.PIO0.split(&mut pac.RESETS);
    let installed = pio.install(&program.program).unwrap();
    let (mut capture_sm, camera_rx, mut camera_tx) = PIOBuilder::from_installed_program(installed)
        .in_pin_base(0)
        .in_shift_direction(ShiftDirection::Left)
        .push_threshold(16)
        .autopush(true)
        .autopull(true)
        .pull_threshold(32)
        .clock_divisor_fixed_point(1, 0)
        .build(sm0);
    capture_sm.set_pindirs((0..=10).map(|pin| (pin, hal::pio::PinDir::Input)));
    capture_sm.clear_fifos();
    let _capture_sm = capture_sm.start();
    let camera_rx = camera_rx.transfer_size(HalfWord);

    let lcd_clk = pins.gpio18.into_function::<hal::gpio::FunctionSpi>();
    let lcd_mosi = pins.gpio19.into_function::<hal::gpio::FunctionSpi>();
    let lcd_spi = hal::spi::Spi::<_, _, _, 8>::new(pac.SPI0, (lcd_mosi, lcd_clk)).init(
        &mut pac.RESETS,
        clocks.peripheral_clock.freq(),
        75.MHz(),
        embedded_hal::spi::MODE_0,
    );
    let mut display = WaveshareLcd::new(
        lcd_spi,
        pins.gpio16.into_push_pull_output(),
        pins.gpio17.into_push_pull_output(),
        pins.gpio20.into_push_pull_output(),
    );
    display.init(&mut timer, Orientation::Portrait);
    let mut backlight = pins.gpio15.into_push_pull_output();
    backlight.set_high().unwrap();

    let touch_sda: hal::gpio::Pin<_, hal::gpio::FunctionI2C, hal::gpio::PullUp> =
        pins.gpio12.reconfigure();
    let touch_scl: hal::gpio::Pin<_, hal::gpio::FunctionI2C, hal::gpio::PullUp> =
        pins.gpio13.reconfigure();
    let touch_i2c = hal::I2C::i2c0(
        pac.I2C0,
        touch_sda,
        touch_scl,
        400.kHz(),
        &mut pac.RESETS,
        &clocks.system_clock,
    );
    let mut touch = Cst816d::new(touch_i2c);
    touch.init().unwrap();
    let _touch_interrupt = pins.gpio29.into_pull_up_input();

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
            .product("OV5640 QR scanner timings")
            .serial_number("QR")])
        .unwrap()
        .max_packet_size_0(64)
        .unwrap()
        .device_class(2)
        .build();
    let mut timing_report = FixedString::<192>::new();
    let _ = write!(
        timing_report,
        "QR timing telemetry ready; camera={} zoom={}\r\n",
        if FAST_CAMERA_TIMING {
            "2x-pclk/half-vts"
        } else {
            "waveshare-default"
        },
        INITIAL_SENSOR_ZOOM.factor()
    );
    let mut timing_report_offset = 0usize;
    let mut event_report = FixedVec::<u8, 1152>::new();
    let mut event_report_offset = 0usize;

    let dma = pac.DMA.split(&mut pac.RESETS);
    let mut camera_dma = dma.ch0;
    // Channel 1 drives the LCD. It never runs concurrently with camera DMA on
    // ch0: the capture transfer is waited on before the display pass starts.
    let mut lcd_dma = Some(dma.ch1);
    let mut camera_rx = camera_rx;

    let mut multicore = Multicore::new(&mut pac.PSM, &mut pac.PPB, &mut sio.fifo);
    multicore.cores()[1]
        .spawn(CORE1_STACK.take().unwrap(), decoder_core)
        .unwrap();
    let mut last_timing_report = timer.get_counter_low();
    let mut zoom_touch_active = false;

    loop {
        poll_usb(
            &mut usb_dev,
            &mut serial,
            &event_report,
            &mut event_report_offset,
            &timing_report,
            &mut timing_report_offset,
        );
        let frame_started = timer.get_counter_low();

        // FRAME_READY means core 1 is still copying CAMERA_FRAME. Do not let
        // DMA overwrite it until core 1 publishes DECODING or DECODER_IDLE.
        let pre_capture_wait_started = timer.get_counter_low();
        let mut decoder_state = DECODER_STATE.load(Ordering::Acquire);
        while decoder_state == FRAME_READY {
            poll_usb(
                &mut usb_dev,
                &mut serial,
                &event_report,
                &mut event_report_offset,
                &timing_report,
                &mut timing_report_offset,
            );
            spin_loop();
            decoder_state = DECODER_STATE.load(Ordering::Acquire);
        }
        let pre_capture_wait_us = timer
            .get_counter_low()
            .wrapping_sub(pre_capture_wait_started);

        if decoder_state == RESULT_READY {
            let length = PAYLOAD_LENGTH.load(Ordering::Relaxed);
            let payload = unsafe { &(&*DECODED_PAYLOAD.0.get())[..length] };

            // Payloads may contain PSBTs or other sensitive material. Keep
            // USB diagnostics useful without copying decoded contents into a
            // log stream.
            event_report.clear();
            event_report_offset = 0;
            event_report.extend_from_slice(b"QR decoded\r\n").unwrap();

            let display_frame = unsafe { &mut *CAMERA_FRAME.0.get() };
            draw_result(display_frame, payload);
            display.write_frame(display_frame);

            // Require a fresh press so a touch already in progress cannot
            // dismiss the decoded value immediately.
            let mut released = false;
            loop {
                poll_usb(
                    &mut usb_dev,
                    &mut serial,
                    &event_report,
                    &mut event_report_offset,
                    &timing_report,
                    &mut timing_report_offset,
                );
                match touch.point() {
                    Ok(None) => released = true,
                    Ok(Some(point)) if released && scan_button_contains(point.x, point.y) => {
                        zoom_touch_active = true;
                        break;
                    }
                    _ => {}
                }
                timer.delay_ms(20);
            }
            DECODER_STATE.store(DECODER_IDLE, Ordering::Release);
            continue;
        }

        let capture_started = timer.get_counter_low();
        let transfer = single_buffer::Config::new(camera_dma, camera_rx, CameraDmaTarget).start();
        while !camera_tx.write(0) {}
        while !camera_tx.write((PIXELS - 1) as u32) {}

        while !transfer.is_done() {
            poll_usb(
                &mut usb_dev,
                &mut serial,
                &event_report,
                &mut event_report_offset,
                &timing_report,
                &mut timing_report_offset,
            );
        }
        let (channel, rx, _) = transfer.wait();
        camera_dma = channel;
        camera_rx = rx;
        let capture_us = timer.get_counter_low().wrapping_sub(capture_started);

        // If the decoder is idle, hand this stable frame to core 1. Core 1
        // copies it to grayscale before publishing DECODING; only then may
        // core 0 reuse the same storage as the LCD framebuffer.
        let mut submitted = DECODER_STATE
            .compare_exchange(
                DECODER_IDLE,
                FRAME_READY,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok();

        if submitted {
            while DECODER_STATE.load(Ordering::Acquire) == FRAME_READY {
                poll_usb(
                    &mut usb_dev,
                    &mut serial,
                    &event_report,
                    &mut event_report_offset,
                    &timing_report,
                    &mut timing_report_offset,
                );
                spin_loop();
            }
        }

        // DMA writes halfwords in the RP2350's native little-endian order,
        // while the panel expects RGB565 high byte first. The QR core has
        // released CAMERA_FRAME after the grayscale copy, so the one camera
        // allocation can now be byte-swapped in place and presented directly.
        let display_frame = unsafe { &mut *CAMERA_FRAME.0.get() };
        for pair in display_frame.chunks_exact_mut(2) {
            pair.swap(0, 1);
        }
        draw_viewfinder_overlay(display_frame, sensor_zoom);
        let lcd_started = timer.get_counter_low();
        // Stream the frame to the panel with DMA on ch1 instead of a blocking
        // SPI write. CS stays asserted across the transfer so the panel sees one
        // continuous pixel stream, exactly as in the blocking path. Measured
        // 47.8 ms -> 19.5 ms at 75 MHz SPI.
        if LCD_DMA {
            let (spi, dc, cs, rst, size) = display.begin_dma_frame();
            let channel = lcd_dma.take().unwrap();
            let transfer = single_buffer::Config::new(channel, LcdSource, spi).start();
            let (channel, _src, spi) = transfer.wait();
            lcd_dma = Some(channel);
            display = WaveshareLcd::from_parts(spi, dc, cs, rst, size);
            display.end_dma_frame();
        } else {
            display.write_frame(display_frame);
        }
        let lcd_us = timer.get_counter_low().wrapping_sub(lcd_started);

        poll_usb(
            &mut usb_dev,
            &mut serial,
            &event_report,
            &mut event_report_offset,
            &timing_report,
            &mut timing_report_offset,
        );

        match touch.point() {
            Ok(None) => zoom_touch_active = false,
            Ok(Some(point)) if !zoom_touch_active => {
                zoom_touch_active = true;
                if zoom_button_contains(point.x, point.y) {
                    let next_zoom = sensor_zoom.toggle();
                    camera.set_zoom(next_zoom).unwrap();
                    sensor_zoom = next_zoom;
                }
            }
            _ => {}
        }

        // If this frame was captured while core 1 was decoding the previous
        // one, retain it until that decode completes, then submit it. A result
        // takes priority and is presented on the next loop iteration.
        let wait_started = timer.get_counter_low();
        while !submitted {
            poll_usb(
                &mut usb_dev,
                &mut serial,
                &event_report,
                &mut event_report_offset,
                &timing_report,
                &mut timing_report_offset,
            );
            match DECODER_STATE.load(Ordering::Acquire) {
                DECODER_IDLE => {
                    submitted = DECODER_STATE
                        .compare_exchange(
                            DECODER_IDLE,
                            FRAME_READY,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok();
                }
                RESULT_READY => break,
                DECODING => spin_loop(),
                FRAME_READY => unreachable!(),
                _ => unreachable!(),
            }
        }
        let now = timer.get_counter_low();
        let wait_us = pre_capture_wait_us + now.wrapping_sub(wait_started);
        let frame_us = now.wrapping_sub(frame_started);

        if now.wrapping_sub(last_timing_report) >= 1_000_000
            && timing_report_offset >= timing_report.len()
        {
            let grayscale_us = GRAYSCALE_US.load(Ordering::Relaxed);
            let identify_us = IDENTIFY_US.load(Ordering::Relaxed);
            let decode_us = DECODE_US.load(Ordering::Relaxed);
            timing_report.clear();
            let _ = write!(
                timing_report,
                "CAP={capture_us}us LCD={lcd_us}us GRAY={grayscale_us}us IDENTIFY={identify_us}us DECODE={decode_us}us QR={}us WAIT={wait_us}us FRAME={frame_us}us ZOOM={}x\r\n",
                identify_us + decode_us,
                sensor_zoom.factor()
            );
            timing_report_offset = 0;
            last_timing_report = now;
        }
    }
}

#[link_section = ".bi_entries"]
#[used]
pub static PICOTOOL_ENTRIES: [hal::binary_info::EntryAddr; 5] = [
    hal::binary_info::rp_cargo_bin_name!(),
    hal::binary_info::rp_cargo_version!(),
    hal::binary_info::rp_program_description!(c"OV5640 QR scanner"),
    hal::binary_info::rp_cargo_homepage_url!(),
    hal::binary_info::rp_program_build_attribute!(),
];
