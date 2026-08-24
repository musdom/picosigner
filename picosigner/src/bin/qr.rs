#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;
use embedded_hal::{delay::DelayNs, pwm::SetDutyCycle};
use fugit::RateExtU32;
use hal::{
    clocks::Clock,
    dma::{single_buffer, DMAExt, HalfWord},
    gpio::FunctionPio0,
    pio::{PIOBuilder, PIOExt, ShiftDirection},
};
use heapless::Vec as FixedVec;
use picosigner as _;
use picosigner::{
    heap,
    ui::{
        draw_qr_result, draw_scan_overlay, in_button, point_in_u16_button, Fonts,
        QR_RESULT_CANCEL_BUTTON, QR_RESULT_CONFIRM_BUTTON, SCAN_CANCEL_BUTTON, SCAN_ZOOM_BUTTON,
    },
};
use picosigner_board::{
    Cst816d, Orientation, Ov5640, SensorZoom, WaveshareLcd, CAMERA_GRAY_HEIGHT, CAMERA_GRAY_WIDTH,
};
use picosigner_qr::{extract_rgb565_volatile, QuircDecoder};
use picosigner_ui::{Gesture, GestureConfig, TouchTracker, UiEvent};
use rp235x_hal as hal;

#[path = "picosigner/framebuffer.rs"]
mod framebuffer;

use framebuffer::{
    camera_pixels, frame_mut, prepare_camera_preview, CameraDmaTarget, LcdSource, PortraitFrame,
    CAMERA_PIXELS,
};

#[link_section = ".start_block"]
#[used]
pub static IMAGE_DEF: hal::block::ImageDef = hal::block::ImageDef::secure_exe();

const XTAL_FREQ_HZ: u32 = 12_000_000;
const QR_PAYLOAD_CAPACITY: usize = 1024;
const BACKLIGHT_DUTY: u16 = 80;

type Lcd = WaveshareLcd<
    hal::spi::Spi<
        hal::spi::Enabled,
        hal::pac::SPI0,
        (
            hal::gpio::Pin<hal::gpio::bank0::Gpio19, hal::gpio::FunctionSpi, hal::gpio::PullDown>,
            hal::gpio::Pin<hal::gpio::bank0::Gpio18, hal::gpio::FunctionSpi, hal::gpio::PullDown>,
        ),
        8,
    >,
    hal::gpio::Pin<hal::gpio::bank0::Gpio16, hal::gpio::FunctionSioOutput, hal::gpio::PullDown>,
    hal::gpio::Pin<hal::gpio::bank0::Gpio17, hal::gpio::FunctionSioOutput, hal::gpio::PullDown>,
    hal::gpio::Pin<hal::gpio::bank0::Gpio20, hal::gpio::FunctionSioOutput, hal::gpio::PullDown>,
>;

fn send_frame_dma<CH: hal::dma::SingleChannel>(display: &mut Lcd, lcd_dma: &mut Option<CH>) {
    let channel = lcd_dma.take().expect("LCD DMA channel in use");
    let channel = display.write_frame_dma(|spi| {
        let transfer = single_buffer::Config::new(channel, LcdSource, spi).start();
        let (channel, _source, spi) = transfer.wait();
        (spi, channel)
    });
    *lcd_dma = Some(channel);

    let device = unsafe { &*hal::pac::SPI0::ptr() };
    while device.sspsr().read().rne().bit_is_set() {
        let _ = device.sspdr().read().data().bits();
    }
    device.sspicr().write(|w| w.roric().clear_bit_by_one());
}

fn wait_for_result_ack<I2C, E, D>(touch: &mut Cst816d<I2C>, timer: &mut hal::Timer<D>)
where
    I2C: embedded_hal::i2c::I2c<Error = E>,
    D: hal::timer::TimerDevice,
{
    let mut released = false;
    loop {
        match touch.point() {
            Ok(None) => released = true,
            Ok(Some(point))
                if released
                    && (in_button(point.x, point.y, QR_RESULT_CANCEL_BUTTON)
                        || in_button(point.x, point.y, QR_RESULT_CONFIRM_BUTTON)) =>
            {
                return;
            }
            _ => {}
        }
        timer.delay_ms(20);
    }
}

#[hal::entry]
fn main() -> ! {
    unsafe { heap::init() };
    let fonts = Fonts::load();

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

    let mut pwm_slices = hal::pwm::Slices::new(pac.PWM, &mut pac.RESETS);
    let backlight_pwm = &mut pwm_slices.pwm7;
    backlight_pwm.set_div_int(1);
    backlight_pwm.set_top(BACKLIGHT_DUTY);
    backlight_pwm.enable();
    let backlight = &mut backlight_pwm.channel_b;
    backlight.output_to(pins.gpio15);

    let xclk_pwm = &mut pwm_slices.pwm5;
    xclk_pwm.set_div_int(1);
    xclk_pwm.set_top(5);
    xclk_pwm.enable();
    let xclk = &mut xclk_pwm.channel_b;
    xclk.output_to(pins.gpio11);
    xclk.set_duty_cycle(3).unwrap();
    timer.delay_ms(50);

    let mut display = picosigner::init_display!(
        pac,
        pins,
        delay: &mut timer,
        freq: clocks.peripheral_clock.freq(),
        orientation: Orientation::PortraitTouchAligned,
    );
    backlight.set_duty_cycle(BACKLIGHT_DUTY).unwrap();

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
    let _ = touch.init();

    let camera_sda: hal::gpio::Pin<_, hal::gpio::FunctionI2C, hal::gpio::PullUp> =
        pins.gpio22.reconfigure();
    let camera_scl: hal::gpio::Pin<_, hal::gpio::FunctionI2C, hal::gpio::PullUp> =
        pins.gpio23.reconfigure();
    let camera_i2c = hal::I2C::i2c1(
        pac.I2C1,
        camera_sda,
        camera_scl,
        100.kHz(),
        &mut pac.RESETS,
        &clocks.system_clock,
    );
    let mut camera = Ov5640::new(camera_i2c);
    camera.init(&mut timer).unwrap();
    camera.set_zoom(SensorZoom::X1).unwrap();
    camera.set_fast_frame_timing(true, &mut timer).unwrap();
    camera.set_fast_frame_period(true).unwrap();
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

    let capture_program = pio::pio_asm!(
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
    let installed = pio.install(&capture_program.program).unwrap();
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
    let mut camera_rx = camera_rx.transfer_size(HalfWord);

    let dma = pac.DMA.split(&mut pac.RESETS);
    let mut camera_dma = dma.ch0;
    let mut lcd_dma = Some(dma.ch1);
    let mut grayscale = vec![0u8; CAMERA_GRAY_WIDTH * CAMERA_GRAY_HEIGHT];
    let mut decoder =
        QuircDecoder::<CAMERA_GRAY_WIDTH, CAMERA_GRAY_HEIGHT, QR_PAYLOAD_CAPACITY>::new();
    let mut sensor_zoom = SensorZoom::X1;
    let mut centered_zoom = false;
    let mut scan_touch_active = false;
    let mut scan_last_touch = (0u16, 0u16);
    let mut scan_tracker = TouchTracker::new(GestureConfig::default());

    loop {
        let transfer = single_buffer::Config::new(camera_dma, camera_rx, CameraDmaTarget).start();
        while !camera_tx.write(0) {}
        while !camera_tx.write((CAMERA_PIXELS - 1) as u32) {}
        while !transfer.is_done() {
            core::hint::spin_loop();
        }
        let (channel, rx, _) = transfer.wait();
        camera_dma = channel;
        camera_rx = rx;

        let mut decoded_payload = FixedVec::<u8, QR_PAYLOAD_CAPACITY>::new();
        if unsafe {
            extract_rgb565_volatile(
                camera_pixels(),
                picosigner_board::DISPLAY_WIDTH,
                picosigner_board::DISPLAY_HEIGHT,
                centered_zoom,
                &mut grayscale,
            )
        }
        .is_ok()
        {
            centered_zoom = !centered_zoom;
            if let Ok((Some(payload), _)) = decoder.decode_with_timing(&grayscale, || 0) {
                let _ = decoded_payload.extend_from_slice(payload.0);
            }
        }

        let display_frame = unsafe { frame_mut() };
        prepare_camera_preview(display_frame);
        {
            let mut target = PortraitFrame(display_frame);
            draw_scan_overlay(&mut target, &fonts, sensor_zoom.factor(), (0, 0));
        }
        send_frame_dma(&mut display, &mut lcd_dma);

        if !decoded_payload.is_empty() {
            let display_frame = unsafe { frame_mut() };
            {
                let mut target = PortraitFrame(display_frame);
                draw_qr_result(&mut target, &fonts, &decoded_payload);
            }
            send_frame_dma(&mut display, &mut lcd_dma);
            wait_for_result_ack(&mut touch, &mut timer);
            scan_touch_active = true;
            continue;
        }

        match touch.point() {
            Ok(None) if scan_touch_active => {
                scan_touch_active = false;
                if let Some(event) = scan_tracker.event(UiEvent::TouchUp {
                    x: scan_last_touch.0,
                    y: scan_last_touch.1,
                }) {
                    match event.gesture {
                        Gesture::Tap if point_in_u16_button(event.x, event.y, SCAN_ZOOM_BUTTON) => {
                            let next_zoom = sensor_zoom.toggle();
                            if camera.set_zoom(next_zoom).is_ok() {
                                sensor_zoom = next_zoom;
                            }
                        }
                        Gesture::Tap
                            if point_in_u16_button(event.x, event.y, SCAN_CANCEL_BUTTON) =>
                        {
                            decoder = QuircDecoder::new();
                            centered_zoom = false;
                        }
                        _ => {}
                    }
                }
            }
            Ok(None) => {}
            Ok(Some(point)) if !scan_touch_active => {
                scan_touch_active = true;
                scan_last_touch = (point.x, point.y);
                let _ = scan_tracker.event(UiEvent::TouchDown {
                    x: point.x,
                    y: point.y,
                });
            }
            Ok(Some(point)) => scan_last_touch = (point.x, point.y),
            _ => {}
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
