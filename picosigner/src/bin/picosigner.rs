#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;
use core::{cell::UnsafeCell, convert::Infallible};
use embedded_graphics::{
    geometry::{OriginDimensions, Size},
    mono_font::{ascii::FONT_6X10, MonoTextStyle},
    pixelcolor::{Rgb565, RgbColor},
    prelude::{DrawTarget, Drawable, IntoStorage, Pixel, Point, Primitive},
    primitives::{PrimitiveStyle, PrimitiveStyleBuilder, Rectangle},
    text::Text,
};
use embedded_hal::{delay::DelayNs, pwm::SetDutyCycle};
use fugit::RateExtU32;
use hal::{
    dma::{single_buffer, DMAExt, HalfWord, WriteTarget},
    gpio::FunctionPio0,
    pio::{PIOBuilder, PIOExt, ShiftDirection},
    Clock,
};
use panic_halt as _;
use picosigner::{app::AppState, heap};
use picosigner_board::{
    CoordinatedSeedStore, Cst816d, Orientation, Ov5640, Rp2350FlashCoordinator, Rp2350NorFlash,
    Rp2350Trng, SensorZoom, TouchPoint, TransactionalSeedStore, WaveshareLcd, CAMERA_GRAY_HEIGHT,
    CAMERA_GRAY_WIDTH, DISPLAY_HEIGHT, DISPLAY_WIDTH,
};
use picosigner_qr::{
    encode_descriptor_qr, encode_export_qr, extract_rgb565_volatile, BbqrCollector, BbqrExport,
    QuircDecoder, UrPsbtCollector, DESCRIPTOR_QR_MODULES, DESCRIPTOR_QR_SIDE, EXPORT_QR_MODULES,
    EXPORT_QR_SIDE, MAX_PSBT_BYTES,
};
use picosigner_ui::{Gesture, GestureConfig, Navigator, Screen, TouchTracker, UiEvent};
use picosigner_wallet::Network;
use rp235x_hal as hal;
use zeroize::Zeroize;

#[link_section = ".start_block"]
#[used]
pub static IMAGE_DEF: hal::block::ImageDef = hal::block::ImageDef::secure_exe();

const XTAL_FREQ_HZ: u32 = 12_000_000;

#[derive(Clone, Copy)]
enum HoldAction {
    Backup,
    Regenerate,
    Sign,
    Scan,
    Descriptor,
    Brightness,
    Back,
}

const HOME_SCAN_BUTTON: (i32, i32, u32, u32) = (16, 112, 208, 40);
const HOME_DESCRIPTOR_BUTTON: (i32, i32, u32, u32) = (16, 164, 208, 40);
const HOME_NEW_WALLET_BUTTON: (i32, i32, u32, u32) = (16, 216, 208, 40);
const HOME_NEW_WALLET_BAR_Y: i32 = 270;
// The descriptor QR occupies the display above this bottom return strip.
const DESCRIPTOR_BACK_BUTTON: (i32, i32, u32, u32) = (0, 296, 240, 24);
const REVIEW_BACK_BUTTON: (i32, i32, u32, u32) = (180, 0, 60, 32);
const REVIEW_SIGN_BUTTON: (i32, i32, u32, u32) = (16, 288, 208, 32);
const REVIEW_SIGN_BAR_Y: i32 = 304;
const BACKLIGHT_DUTY: [u16; 3] = [255, 40, 16];
const CAMERA_PIXELS: usize = DISPLAY_WIDTH * DISPLAY_HEIGHT;
const SCAN_CANCEL_BUTTON: (u16, u16, u16, u16) = (174, 0, 66, 24);
const SCAN_ZOOM_BUTTON: (u16, u16, u16, u16) = (70, 280, 100, 40);
const SCAN_PROGRESS_BAR: (i32, i32, u32, u32) = (12, 263, 216, 11);
const QR_PAYLOAD_CAPACITY: usize = 1024;

#[repr(align(4))]
struct CameraFrame(UnsafeCell<[u8; CAMERA_PIXELS * 2]>);

unsafe impl Sync for CameraFrame {}

static CAMERA_FRAME: CameraFrame = CameraFrame(UnsafeCell::new([0; CAMERA_PIXELS * 2]));

#[derive(Clone, Copy)]
struct CameraDmaTarget;

unsafe impl WriteTarget for CameraDmaTarget {
    type TransmittedWord = u16;

    fn tx_treq() -> Option<u8> {
        None
    }

    fn tx_address_count(&mut self) -> (u32, u32) {
        (
            CAMERA_FRAME.0.get().cast::<u16>() as u32,
            CAMERA_PIXELS as u32,
        )
    }

    fn tx_increment(&self) -> bool {
        true
    }
}

struct PortraitFrame<'a>(&'a mut [u8]);

impl OriginDimensions for PortraitFrame<'_> {
    fn size(&self) -> Size {
        Size::new(DISPLAY_WIDTH as u32, DISPLAY_HEIGHT as u32)
    }
}

impl DrawTarget for PortraitFrame<'_> {
    type Color = Rgb565;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            if point.x >= 0
                && point.y >= 0
                && point.x < DISPLAY_WIDTH as i32
                && point.y < DISPLAY_HEIGHT as i32
            {
                let offset = (point.y as usize * DISPLAY_WIDTH + point.x as usize) * 2;
                self.0[offset..offset + 2].copy_from_slice(&color.into_storage().to_be_bytes());
            }
        }
        Ok(())
    }
}

#[hal::entry]
fn main() -> ! {
    unsafe {
        heap::init();
    }

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

    // Waveshare RP2350-Touch-LCD-2: SPI0 CLK=18, MOSI=19.
    let clk = pins.gpio18.into_function::<hal::gpio::FunctionSpi>();
    let mosi = pins.gpio19.into_function::<hal::gpio::FunctionSpi>();
    let spi = hal::spi::Spi::<_, _, _, 8>::new(pac.SPI0, (mosi, clk)).init(
        &mut pac.RESETS,
        clocks.peripheral_clock.freq(),
        62_500.kHz(),
        embedded_hal::spi::MODE_0,
    );

    let dc = pins.gpio16.into_push_pull_output();
    let cs = pins.gpio17.into_push_pull_output();
    let rst = pins.gpio20.into_push_pull_output();
    let mut pwm_slices = hal::pwm::Slices::new(pac.PWM, &mut pac.RESETS);
    let backlight_pwm = &mut pwm_slices.pwm7;
    backlight_pwm.set_div_int(1);
    backlight_pwm.set_top(BACKLIGHT_DUTY[0]);
    backlight_pwm.enable();
    let backlight = &mut backlight_pwm.channel_b;
    backlight.output_to(pins.gpio15);
    backlight.set_duty_cycle(BACKLIGHT_DUTY[0]).unwrap();

    // Exact 25 MHz OV5640 XCLK on GPIO11. The camera remains initialized
    // while the wallet UI is active, but captures only run on the Scan screen.
    let xclk_pwm = &mut pwm_slices.pwm5;
    xclk_pwm.set_div_int(1);
    xclk_pwm.set_top(5);
    xclk_pwm.enable();
    let xclk = &mut xclk_pwm.channel_b;
    xclk.output_to(pins.gpio11);
    xclk.set_duty_cycle(3).unwrap();
    timer.delay_ms(50);

    let mut display = WaveshareLcd::new(spi, dc, cs, rst);
    display.init(&mut timer, Orientation::PortraitTouchAligned);

    display.clear(Rgb565::BLACK).unwrap();
    let title = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);
    let accent = MonoTextStyle::new(&FONT_6X10, Rgb565::CYAN);
    Text::new("PicoSigner", Point::new(24, 136), title)
        .draw(&mut display)
        .unwrap();
    Text::new("Loading wallet", Point::new(24, 160), accent)
        .draw(&mut display)
        .unwrap();

    let mut entropy = Rp2350Trng::new(pac.TRNG, &mut pac.RESETS);
    let flash = TransactionalSeedStore::new(Rp2350NorFlash::new());
    let mut store = CoordinatedSeedStore::new(flash, Rp2350FlashCoordinator::new());
    let mut app = AppState::new(Network::Signet);

    if app.boot(&mut entropy, &mut store).is_err() {
        display.clear(Rgb565::BLACK).unwrap();
        Text::new("Secure hardware unavailable", Point::new(24, 96), title)
            .draw(&mut display)
            .unwrap();
        Text::new("Wallet generation stopped", Point::new(24, 120), accent)
            .draw(&mut display)
            .unwrap();
        loop {
            timer.delay_ms(1_000);
        }
    }

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
    camera.set_fast_frame_timing(true).unwrap();
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
        "push block",
        "jmp x-- pixel",
        ".wrap"
    );
    let (mut pio, sm0, _, _, _) = pac.PIO0.split(&mut pac.RESETS);
    let installed = pio.install(&capture_program.program).unwrap();
    let (mut capture_sm, camera_rx, mut camera_tx) = PIOBuilder::from_installed_program(installed)
        .in_pin_base(0)
        .in_shift_direction(ShiftDirection::Left)
        .push_threshold(32)
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

    render_app(&mut display, &app, None, 0);
    let mut tracker = TouchTracker::new(GestureConfig::default());
    let mut was_touched = false;
    let mut last_touch = (0, 0);
    let mut hold_action = None;
    let mut rendered_hold_ticks = 0;
    let mut backlight_level = 0usize;
    loop {
        if app.screen() == Screen::Signed {
            let export = app
                .signed_psbt()
                .ok_or(())
                .and_then(|signed| BbqrExport::new_psbt(&signed.bytes, 47).map_err(|_| ()));
            let mut export_touch_active = true;
            let mut exit_export = false;

            if let Ok(export) = export {
                let mut frame = heapless::Vec::<u8, 84>::new();
                let mut frame_index = 0usize;
                while !exit_export {
                    if export.frame(frame_index, &mut frame).is_ok() {
                        let display_frame = unsafe { &mut *CAMERA_FRAME.0.get() };
                        draw_signed_psbt_qr(
                            display_frame,
                            &frame,
                            frame_index,
                            export.frame_count(),
                        );
                        display.write_frame(display_frame);
                    }
                    // About three frames per second, while polling touch often
                    // enough that the return control remains responsive.
                    for _ in 0..16 {
                        match touch.point() {
                            Ok(None) => export_touch_active = false,
                            Ok(Some(point)) if !export_touch_active => {
                                export_touch_active = true;
                                let (x, y) = portrait_touch(point);
                                if in_button(x, y, DESCRIPTOR_BACK_BUTTON) {
                                    exit_export = true;
                                    break;
                                } else if y < DESCRIPTOR_BACK_BUTTON.1 as u16 {
                                    backlight_level = (backlight_level + 1) % BACKLIGHT_DUTY.len();
                                    backlight
                                        .set_duty_cycle(BACKLIGHT_DUTY[backlight_level])
                                        .unwrap();
                                }
                            }
                            _ => {}
                        }
                        timer.delay_ms(20);
                    }
                    frame_index = (frame_index + 1) % export.frame_count();
                }
            } else {
                render_signed_export_error(&mut display);
                while !exit_export {
                    match touch.point() {
                        Ok(None) => export_touch_active = false,
                        Ok(Some(point)) if !export_touch_active => {
                            export_touch_active = true;
                            let (x, y) = portrait_touch(point);
                            if in_button(x, y, DESCRIPTOR_BACK_BUTTON) {
                                exit_export = true;
                            } else if y < DESCRIPTOR_BACK_BUTTON.1 as u16 {
                                backlight_level = (backlight_level + 1) % BACKLIGHT_DUTY.len();
                                backlight
                                    .set_duty_cycle(BACKLIGHT_DUTY[backlight_level])
                                    .unwrap();
                            }
                        }
                        _ => {}
                    }
                    timer.delay_ms(20);
                }
            }

            app.navigate(Screen::Home);
            was_touched = false;
            hold_action = None;
            rendered_hold_ticks = 0;
            render_app(&mut display, &app, None, 0);
            continue;
        }

        if app.screen() == Screen::Scan {
            let mut collector = BbqrCollector::<MAX_PSBT_BYTES>::new();
            let mut ur_collector = UrPsbtCollector::new();
            // 0 = unknown, 1 = BBQr, 2 = animated crypto-psbt UR.
            let mut scan_format = 0u8;
            let mut grayscale = vec![0u8; CAMERA_GRAY_WIDTH * CAMERA_GRAY_HEIGHT];
            let mut decoder =
                QuircDecoder::<CAMERA_GRAY_WIDTH, CAMERA_GRAY_HEIGHT, QR_PAYLOAD_CAPACITY>::new();
            let mut sensor_zoom = SensorZoom::X1;
            let _ = camera.set_zoom(sensor_zoom);
            let mut centered_zoom = false;
            let mut complete = false;
            let mut cancelled = false;
            // Require release of the touch that opened the scanner before a
            // scanner control can fire.
            let mut scan_touch_active = true;

            while !complete && !cancelled {
                let transfer =
                    single_buffer::Config::new(camera_dma, camera_rx, CameraDmaTarget).start();
                while !camera_tx.write(0) {}
                while !camera_tx.write((CAMERA_PIXELS - 1) as u32) {}
                while !transfer.is_done() {
                    core::hint::spin_loop();
                }
                let (channel, rx, _) = transfer.wait();
                camera_dma = channel;
                camera_rx = rx;

                let camera_pixels = CAMERA_FRAME.0.get().cast::<u16>();
                if unsafe {
                    extract_rgb565_volatile(
                        camera_pixels,
                        DISPLAY_WIDTH,
                        DISPLAY_HEIGHT,
                        centered_zoom,
                        &mut grayscale,
                    )
                }
                .is_ok()
                {
                    centered_zoom = !centered_zoom;
                    if let Ok((Some(payload), _)) = decoder.decode_with_timing(&grayscale, || 0) {
                        let is_ur = payload
                            .0
                            .get(..3)
                            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"ur:"));
                        if is_ur && scan_format != 1 {
                            scan_format = 2;
                            match ur_collector.push(payload.0) {
                                Ok(done) => complete = done,
                                Err(picosigner_qr::TransportError::MismatchedSession) => {
                                    ur_collector = UrPsbtCollector::new();
                                    complete = ur_collector.push(payload.0).unwrap_or(false);
                                }
                                Err(_) => {}
                            }
                        } else if !is_ur && scan_format != 2 {
                            scan_format = 1;
                            match collector.push(payload.0) {
                                Ok(done) => complete = done,
                                Err(picosigner_qr::TransportError::MismatchedSession) => {
                                    collector = BbqrCollector::new();
                                    complete = collector.push(payload.0).unwrap_or(false);
                                }
                                Err(_) => {}
                            }
                        }
                    }
                }

                let display_frame = unsafe { &mut *CAMERA_FRAME.0.get() };
                prepare_camera_preview(display_frame);
                let progress = match scan_format {
                    1 => collector.progress(),
                    2 => ur_collector.progress(),
                    _ => (0, 0),
                };
                draw_scan_overlay(display_frame, sensor_zoom, progress);
                display.write_frame(display_frame);

                match touch.point() {
                    Ok(None) => scan_touch_active = false,
                    Ok(Some(point)) if !scan_touch_active => {
                        scan_touch_active = true;
                        if point_in_u16_button(point, SCAN_CANCEL_BUTTON) {
                            cancelled = true;
                        } else if point_in_u16_button(point, SCAN_ZOOM_BUTTON) {
                            let next_zoom = sensor_zoom.toggle();
                            if camera.set_zoom(next_zoom).is_ok() {
                                sensor_zoom = next_zoom;
                            }
                        }
                    }
                    _ => {}
                }
            }

            // The decoder owns most of the QR heap. Release it before the app
            // copies the completed PSBT into its pending-review slot.
            drop(decoder);
            drop(grayscale);
            if complete {
                let received = if scan_format == 2 {
                    ur_collector
                        .finish()
                        .ok()
                        .is_some_and(|psbt| app.receive_psbt(&psbt).is_ok())
                } else {
                    collector
                        .finish()
                        .ok()
                        .is_some_and(|psbt| app.receive_psbt(psbt).is_ok())
                };
                if !received {
                    app.navigate(Screen::Home);
                }
            } else {
                app.navigate(Screen::Home);
            }
            was_touched = false;
            hold_action = None;
            rendered_hold_ticks = 0;
            render_app(&mut display, &app, None, 0);
            continue;
        }

        let point = touch.point().ok().flatten();
        if let Some(point) = point {
            if was_touched {
                last_touch = portrait_touch(point);
                let _ = tracker.event(UiEvent::Tick);
                let (progress, _) = tracker.hold_progress();
                if progress > rendered_hold_ticks {
                    if let Some(action) = hold_action {
                        if let Some(y) = hold_bar_y(action) {
                            draw_hold_bar_progress(&mut display, rendered_hold_ticks, progress, y);
                        }
                    }
                    rendered_hold_ticks = progress;
                }
            } else {
                hold_action = match app.screen() {
                    Screen::Backup => Some(HoldAction::Backup),
                    // The home button is intentionally a hold action so a
                    // stray tap can never replace the current wallet.
                    Screen::Home => home_action(point),
                    Screen::Descriptor => descriptor_action(point),
                    Screen::Review => review_action(point),
                    Screen::Signed => bottom_back_action(point),
                    _ => None,
                };
                if hold_action.is_some() {
                    last_touch = portrait_touch(point);
                    let _ = tracker.event(UiEvent::TouchDown {
                        x: last_touch.0,
                        y: last_touch.1,
                    });
                    was_touched = true;
                    rendered_hold_ticks = 0;
                    if !matches!(hold_action, Some(HoldAction::Brightness)) {
                        render_app(&mut display, &app, hold_action, 0);
                    }
                }
            }
        } else if was_touched {
            was_touched = false;
            let action = hold_action;
            let mut redraw_screen = false;
            if let Some(gesture) = tracker.event(UiEvent::TouchUp {
                x: last_touch.0,
                y: last_touch.1,
            }) {
                match (action, gesture.gesture) {
                    (Some(HoldAction::Backup), Gesture::Hold) => {
                        if app.acknowledge_backup_and_store(&mut store).is_ok() {
                            redraw_screen = true;
                        }
                    }
                    (Some(HoldAction::Regenerate), Gesture::Hold) => {
                        if app.generate_seed(&mut entropy, &mut store).is_ok() {
                            redraw_screen = true;
                        }
                    }
                    (Some(HoldAction::Sign), Gesture::Hold)
                        if in_button(last_touch.0, last_touch.1, REVIEW_SIGN_BUTTON) =>
                    {
                        if app.mark_all_outputs_reviewed().is_ok() {
                            // Show an explicit signing state before the
                            // synchronous cryptographic operation begins.
                            render_app(&mut display, &app, None, 0);
                            if app.sign_after_hold(true).is_err() {
                                app.navigate(Screen::Review);
                            }
                            redraw_screen = true;
                        }
                    }
                    (Some(HoldAction::Scan), Gesture::Tap) => {
                        if app.begin_scan().is_ok() {
                            redraw_screen = true;
                        }
                    }
                    (Some(HoldAction::Descriptor), Gesture::Tap) => {
                        if app.open_descriptor().is_ok() {
                            redraw_screen = true;
                        }
                    }
                    (Some(HoldAction::Brightness), Gesture::Tap) => {
                        backlight_level = (backlight_level + 1) % BACKLIGHT_DUTY.len();
                        backlight
                            .set_duty_cycle(BACKLIGHT_DUTY[backlight_level])
                            .unwrap();
                    }
                    (Some(HoldAction::Back), Gesture::Tap) => {
                        app.navigate(Screen::Home);
                        redraw_screen = true;
                    }
                    _ => {}
                }
            }
            if redraw_screen {
                render_app(&mut display, &app, None, 0);
            } else if let Some(action) = action.and_then(hold_bar_y) {
                // TouchUp consumes the gesture state. Any release short of the
                // hold threshold clears only the progress-bar rectangle.
                reset_hold_bar(&mut display, action);
            }
            hold_action = None;
            rendered_hold_ticks = 0;
        }
        timer.delay_ms(20);
    }
}

fn render_app<SPI, DC, CS, RST>(
    display: &mut WaveshareLcd<SPI, DC, CS, RST>,
    app: &AppState,
    hold_action: Option<HoldAction>,
    hold_ticks: u16,
) where
    SPI: embedded_hal::spi::SpiBus<u8>,
    DC: embedded_hal::digital::OutputPin,
    CS: embedded_hal::digital::OutputPin,
    RST: embedded_hal::digital::OutputPin,
{
    display.clear(Rgb565::BLACK).unwrap();
    let title = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);
    let accent = MonoTextStyle::new(&FONT_6X10, Rgb565::CYAN);
    let body = MonoTextStyle::new(&FONT_6X10, Rgb565::GREEN);
    let heading = match app.screen() {
        Screen::Backup => "BACKUP REQUIRED",
        Screen::Home => "",
        Screen::Scan => "SCAN PSBT QR",
        Screen::Descriptor => "DESCRIPTOR EXPORT",
        Screen::Review => "REVIEW TRANSACTION",
        Screen::Sign => "HOLD TO SIGN",
        Screen::Signed => "SIGNED PSBT",
        Screen::Boot | Screen::Network | Screen::Seed | Screen::Recovery => "INITIALIZING",
    };
    if app.screen() != Screen::Descriptor {
        if app.screen() == Screen::Home {
            if let Some(wallet) = app.wallet() {
                if let Some(fingerprint) = wallet.master_fingerprint() {
                    let identity = alloc::format!(
                        "Wallet {:02x}{:02x}{:02x}{:02x} [{}]",
                        fingerprint[0],
                        fingerprint[1],
                        fingerprint[2],
                        fingerprint[3],
                        network_name(wallet.network()),
                    );
                    Text::new(&identity, Point::new(16, 24), title)
                        .draw(display)
                        .unwrap();
                }
            }
        } else {
            Text::new(heading, Point::new(16, 24), title)
                .draw(display)
                .unwrap();
        }
    }

    match app.screen() {
        Screen::Backup => {
            Text::new("Write down these 12 words", Point::new(16, 48), accent)
                .draw(display)
                .unwrap();
            if let Ok(mut mnemonic) = app.mnemonic() {
                for (line, chunk) in mnemonic.as_bytes().chunks(34).enumerate() {
                    if let Ok(text) = core::str::from_utf8(chunk) {
                        Text::new(text, Point::new(16, 76 + line as i32 * 14), body)
                            .draw(display)
                            .unwrap();
                    }
                }
                mnemonic.zeroize();
            }
            Text::new("Hold to confirm backup", Point::new(16, 238), accent)
                .draw(display)
                .unwrap();
            draw_hold_bar(display, hold_ticks, 258);
        }
        Screen::Home => {
            Rectangle::new(
                Point::new(HOME_SCAN_BUTTON.0, HOME_SCAN_BUTTON.1),
                Size::new(HOME_SCAN_BUTTON.2, HOME_SCAN_BUTTON.3),
            )
            .into_styled(PrimitiveStyle::with_fill(Rgb565::BLUE))
            .draw(display)
            .unwrap();
            Text::new("SCAN PSBT", Point::new(93, 137), title)
                .draw(display)
                .unwrap();
            Rectangle::new(
                Point::new(HOME_DESCRIPTOR_BUTTON.0, HOME_DESCRIPTOR_BUTTON.1),
                Size::new(HOME_DESCRIPTOR_BUTTON.2, HOME_DESCRIPTOR_BUTTON.3),
            )
            .into_styled(PrimitiveStyle::with_fill(Rgb565::new(0, 32, 0)))
            .draw(display)
            .unwrap();
            Text::new("EXPORT DESCRIPTOR", Point::new(69, 189), title)
                .draw(display)
                .unwrap();
            Rectangle::new(
                Point::new(HOME_NEW_WALLET_BUTTON.0, HOME_NEW_WALLET_BUTTON.1),
                Size::new(HOME_NEW_WALLET_BUTTON.2, HOME_NEW_WALLET_BUTTON.3),
            )
            .into_styled(PrimitiveStyle::with_fill(Rgb565::RED))
            .draw(display)
            .unwrap();
            Text::new("HOLD FOR NEW WALLET", Point::new(63, 241), title)
                .draw(display)
                .unwrap();
            if matches!(hold_action, Some(HoldAction::Regenerate)) {
                draw_hold_bar(display, hold_ticks, HOME_NEW_WALLET_BAR_Y);
            }
        }
        Screen::Scan => {
            Text::new("Camera/BBQr receive next", Point::new(16, 76), accent)
                .draw(display)
                .unwrap();
            Text::new("No signing without review", Point::new(16, 102), body)
                .draw(display)
                .unwrap();
        }
        Screen::Review => {
            if let Some(review) = app.review() {
                Text::new("BACK", Point::new(196, 20), title)
                    .draw(display)
                    .unwrap();
                let mut y = 48;
                let mut recipients = 0usize;
                for output in review.outputs.iter().filter(|output| !output.is_change) {
                    recipients += 1;
                    let amount = alloc::format!("SEND {} sats", output.value_sat);
                    Text::new(&amount, Point::new(12, y), accent)
                        .draw(display)
                        .unwrap();
                    y += 14;
                    if let Some(address) = &output.address {
                        for chunk in address.as_bytes().chunks(36) {
                            if y > 242 {
                                break;
                            }
                            if let Ok(line) = core::str::from_utf8(chunk) {
                                Text::new(line, Point::new(12, y), title)
                                    .draw(display)
                                    .unwrap();
                            }
                            y += 12;
                        }
                    } else {
                        Text::new("UNKNOWN OUTPUT SCRIPT", Point::new(12, y), title)
                            .draw(display)
                            .unwrap();
                        y += 12;
                    }
                    y += 8;
                }

                let change_count = review
                    .outputs
                    .iter()
                    .filter(|output| output.is_change)
                    .count();
                let summary = alloc::format!("Recipients: {recipients}  Change: {change_count}");
                Text::new(&summary, Point::new(12, 266), body)
                    .draw(display)
                    .unwrap();
                let fee = match review.fee_sat {
                    Some(value) => alloc::format!("Fee: {value} sats"),
                    None => alloc::format!("Fee: unavailable"),
                };
                Text::new(&fee, Point::new(12, 282), body)
                    .draw(display)
                    .unwrap();
                Text::new("HOLD TO CONFIRM & SIGN", Point::new(54, 301), title)
                    .draw(display)
                    .unwrap();
                draw_hold_bar(
                    display,
                    if matches!(hold_action, Some(HoldAction::Sign)) {
                        hold_ticks
                    } else {
                        0
                    },
                    REVIEW_SIGN_BAR_Y,
                );
            }
        }
        Screen::Sign => {
            Text::new("Signing reviewed PSBT...", Point::new(16, 76), accent)
                .draw(display)
                .unwrap();
            Text::new("Verifying transaction digest", Point::new(16, 102), body)
                .draw(display)
                .unwrap();
        }
        Screen::Signed => {
            Text::new("PSBT signed successfully", Point::new(16, 76), accent)
                .draw(display)
                .unwrap();
            Text::new("Preparing animated BBQr...", Point::new(16, 102), body)
                .draw(display)
                .unwrap();
        }
        Screen::Descriptor => {
            if let Ok(export) = app.export_descriptor() {
                draw_descriptor_qr(display, export.descriptor.as_bytes());
            } else {
                Text::new(
                    "DESCRIPTOR UNAVAILABLE",
                    Point::new(54, 144),
                    MonoTextStyle::new(&FONT_6X10, Rgb565::RED),
                )
                .draw(display)
                .unwrap();
            }
            Text::new(
                "TAP TO RETURN",
                Point::new(81, 313),
                MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE),
            )
            .draw(display)
            .unwrap();
        }
        _ => {
            Text::new("State ready", Point::new(16, 76), accent)
                .draw(display)
                .unwrap();
        }
    }
}

fn draw_hold_bar<SPI, DC, CS, RST>(
    display: &mut WaveshareLcd<SPI, DC, CS, RST>,
    hold_ticks: u16,
    y: i32,
) where
    SPI: embedded_hal::spi::SpiBus<u8>,
    DC: embedded_hal::digital::OutputPin,
    CS: embedded_hal::digital::OutputPin,
    RST: embedded_hal::digital::OutputPin,
{
    draw_hold_bar_outline(display, y);
    draw_hold_bar_progress(display, 0, hold_ticks, y);
}

fn portrait_touch(point: TouchPoint) -> (u16, u16) {
    // CST816D coordinates already use the panel's native 240x320 portrait
    // axes, matching every application view.
    (point.x, point.y)
}

fn network_name(network: Network) -> &'static str {
    match network {
        Network::Mainnet => "mainnet",
        Network::Testnet4 => "testnet4",
        Network::Signet => "signet",
    }
}

fn home_action(point: TouchPoint) -> Option<HoldAction> {
    let (x, y) = portrait_touch(point);
    if in_button(x, y, HOME_SCAN_BUTTON) {
        Some(HoldAction::Scan)
    } else if in_button(x, y, HOME_DESCRIPTOR_BUTTON) {
        Some(HoldAction::Descriptor)
    } else if in_button(x, y, HOME_NEW_WALLET_BUTTON) {
        Some(HoldAction::Regenerate)
    } else {
        None
    }
}

fn descriptor_action(point: TouchPoint) -> Option<HoldAction> {
    let (x, y) = portrait_touch(point);
    if in_button(x, y, DESCRIPTOR_BACK_BUTTON) {
        Some(HoldAction::Back)
    } else if y < DESCRIPTOR_BACK_BUTTON.1 as u16 {
        Some(HoldAction::Brightness)
    } else {
        None
    }
}

fn review_action(point: TouchPoint) -> Option<HoldAction> {
    let (x, y) = portrait_touch(point);
    if in_button(x, y, REVIEW_BACK_BUTTON) {
        Some(HoldAction::Back)
    } else if in_button(x, y, REVIEW_SIGN_BUTTON) {
        Some(HoldAction::Sign)
    } else {
        None
    }
}

fn bottom_back_action(point: TouchPoint) -> Option<HoldAction> {
    let (_, y) = portrait_touch(point);
    (y >= DESCRIPTOR_BACK_BUTTON.1 as u16).then_some(HoldAction::Back)
}

fn in_button(x: u16, y: u16, button: (i32, i32, u32, u32)) -> bool {
    let (left, top, width, height) = button;
    let right = left + width as i32;
    let bottom = top + height as i32;
    (x as i32) >= left && (x as i32) < right && (y as i32) >= top && (y as i32) < bottom
}

fn hold_bar_y(action: HoldAction) -> Option<i32> {
    match action {
        HoldAction::Backup => Some(258),
        HoldAction::Regenerate => Some(HOME_NEW_WALLET_BAR_Y),
        HoldAction::Sign => Some(REVIEW_SIGN_BAR_Y),
        HoldAction::Scan | HoldAction::Descriptor | HoldAction::Brightness | HoldAction::Back => {
            None
        }
    }
}

fn draw_scan_overlay(frame: &mut [u8], zoom: SensorZoom, progress: (usize, usize)) {
    let mut target = PortraitFrame(frame);
    let panel = PrimitiveStyleBuilder::new()
        .fill_color(Rgb565::BLACK)
        .stroke_color(Rgb565::WHITE)
        .stroke_width(1)
        .build();
    Rectangle::new(Point::new(0, 0), Size::new(DISPLAY_WIDTH as u32, 24))
        .into_styled(panel)
        .draw(&mut target)
        .unwrap();
    Rectangle::new(Point::new(0, 256), Size::new(DISPLAY_WIDTH as u32, 24))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
        .draw(&mut target)
        .unwrap();
    Rectangle::new(
        Point::new(SCAN_PROGRESS_BAR.0, SCAN_PROGRESS_BAR.1),
        Size::new(SCAN_PROGRESS_BAR.2, SCAN_PROGRESS_BAR.3),
    )
    .into_styled(PrimitiveStyle::with_stroke(Rgb565::WHITE, 1))
    .draw(&mut target)
    .unwrap();
    let (received, total) = progress;
    let inner_width = SCAN_PROGRESS_BAR.2.saturating_sub(2);
    let filled_width = if total == 0 {
        0
    } else {
        (inner_width as usize * received.min(total) / total) as u32
    };
    if filled_width > 0 {
        Rectangle::new(
            Point::new(SCAN_PROGRESS_BAR.0 + 1, SCAN_PROGRESS_BAR.1 + 1),
            Size::new(filled_width, SCAN_PROGRESS_BAR.3.saturating_sub(2)),
        )
        .into_styled(PrimitiveStyle::with_fill(Rgb565::GREEN))
        .draw(&mut target)
        .unwrap();
    }
    Rectangle::new(
        Point::new(SCAN_ZOOM_BUTTON.0 as i32, SCAN_ZOOM_BUTTON.1 as i32),
        Size::new(SCAN_ZOOM_BUTTON.2 as u32, SCAN_ZOOM_BUTTON.3 as u32),
    )
    .into_styled(panel)
    .draw(&mut target)
    .unwrap();
    let white = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);
    Text::new("SCAN PSBT QR", Point::new(8, 16), white)
        .draw(&mut target)
        .unwrap();
    Text::new("CANCEL", Point::new(192, 16), white)
        .draw(&mut target)
        .unwrap();
    Text::new(
        match zoom {
            SensorZoom::X1 => "ZOOM 1x",
            SensorZoom::X2 => "ZOOM 2x",
        },
        Point::new(96, 305),
        white,
    )
    .draw(&mut target)
    .unwrap();
}

fn point_in_u16_button(point: TouchPoint, button: (u16, u16, u16, u16)) -> bool {
    let (x, y, width, height) = button;
    point.x >= x
        && point.x < x.saturating_add(width)
        && point.y >= y
        && point.y < y.saturating_add(height)
}

fn prepare_camera_preview(frame: &mut [u8]) {
    // Camera rows arrive in the opposite X direction from the touch-aligned
    // portrait LCD. Mirror each row while also converting native little-endian
    // DMA halfwords into the panel's high-byte-first RGB565 stream.
    for row in frame.chunks_exact_mut(DISPLAY_WIDTH * 2) {
        for x in 0..DISPLAY_WIDTH / 2 {
            let left = x * 2;
            let right = (DISPLAY_WIDTH - 1 - x) * 2;
            let left_pixel = [row[left], row[left + 1]];
            let right_pixel = [row[right], row[right + 1]];
            row[left] = right_pixel[1];
            row[left + 1] = right_pixel[0];
            row[right] = left_pixel[1];
            row[right + 1] = left_pixel[0];
        }
    }
}

fn draw_descriptor_qr<SPI, DC, CS, RST>(
    display: &mut WaveshareLcd<SPI, DC, CS, RST>,
    descriptor: &[u8],
) where
    SPI: embedded_hal::spi::SpiBus<u8>,
    DC: embedded_hal::digital::OutputPin,
    CS: embedded_hal::digital::OutputPin,
    RST: embedded_hal::digital::OutputPin,
{
    let mut modules = [false; DESCRIPTOR_QR_MODULES];
    if encode_descriptor_qr(descriptor, &mut modules).is_err() {
        return;
    }
    // Version 8 has 49 modules. At scale four the complete symbol, including
    // the required four-module quiet zone, is 228 pixels square and fits the
    // 240-pixel portrait width.
    const SCALE: u32 = 4;
    const QUIET: u32 = 4;
    let symbol = (DESCRIPTOR_QR_SIDE as u32 + QUIET * 2) * SCALE;
    let left = (DISPLAY_WIDTH as u32 - symbol) / 2;
    let top = 28;
    draw_qr_modules_on_white(
        display,
        &modules,
        DESCRIPTOR_QR_SIDE,
        SCALE,
        QUIET,
        left,
        top,
    );
}

fn draw_signed_psbt_qr(
    display_frame: &mut [u8],
    frame: &[u8],
    frame_index: usize,
    frame_count: usize,
) {
    // Assemble the complete screen off-panel, then let the caller transfer it
    // in one continuous write. This prevents scanners from seeing the white
    // square and individual black module runs as separate intermediate frames.
    display_frame.fill(0);
    let mut target = PortraitFrame(display_frame);
    let mut modules = [false; EXPORT_QR_MODULES];
    if encode_export_qr(frame, &mut modules).is_err() {
        return;
    }

    const SCALE: u32 = 5;
    const QUIET: u32 = 4;
    let symbol = (EXPORT_QR_SIDE as u32 + QUIET * 2) * SCALE;
    let left = (DISPLAY_WIDTH as u32 - symbol) / 2;
    draw_qr_modules_on_white(&mut target, &modules, EXPORT_QR_SIDE, SCALE, QUIET, left, 4);

    let title = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);
    let accent = MonoTextStyle::new(&FONT_6X10, Rgb565::CYAN);
    Text::new("SIGNED PSBT - BBQr", Point::new(66, 250), accent)
        .draw(&mut target)
        .unwrap();
    let part = alloc::format!("PART {}/{}", frame_index + 1, frame_count);
    Text::new(&part, Point::new(90, 270), title)
        .draw(&mut target)
        .unwrap();
    Text::new("TAP TO RETURN", Point::new(81, 313), title)
        .draw(&mut target)
        .unwrap();
}

fn render_signed_export_error<SPI, DC, CS, RST>(display: &mut WaveshareLcd<SPI, DC, CS, RST>)
where
    SPI: embedded_hal::spi::SpiBus<u8>,
    DC: embedded_hal::digital::OutputPin,
    CS: embedded_hal::digital::OutputPin,
    RST: embedded_hal::digital::OutputPin,
{
    display.clear(Rgb565::BLACK).unwrap();
    let title = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);
    let error = MonoTextStyle::new(&FONT_6X10, Rgb565::RED);
    Text::new("SIGNED PSBT", Point::new(16, 24), title)
        .draw(display)
        .unwrap();
    Text::new("QR EXPORT TOO LARGE", Point::new(48, 136), error)
        .draw(display)
        .unwrap();
    Text::new("TAP TO RETURN", Point::new(81, 313), title)
        .draw(display)
        .unwrap();
}

/// Draws a QR symbol on the only illuminated area of an otherwise black
/// export screen. The white square includes the complete QR quiet zone.
fn draw_qr_modules_on_white<D>(
    display: &mut D,
    modules: &[bool],
    side: usize,
    scale: u32,
    quiet: u32,
    left: u32,
    top: u32,
) where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    let symbol = (side as u32 + quiet * 2) * scale;
    Rectangle::new(
        Point::new(left as i32, top as i32),
        Size::new(symbol, symbol),
    )
    .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
    .draw(display)
    .unwrap();

    let black = PrimitiveStyle::with_fill(Rgb565::BLACK);
    for y in 0..side {
        let mut x = 0;
        while x < side {
            if !modules[y * side + x] {
                x += 1;
                continue;
            }

            let run_start = x;
            while x < side && modules[y * side + x] {
                x += 1;
            }
            Rectangle::new(
                Point::new(
                    (left + (quiet + run_start as u32) * scale) as i32,
                    (top + (quiet + y as u32) * scale) as i32,
                ),
                Size::new((x - run_start) as u32 * scale, scale),
            )
            .into_styled(black)
            .draw(display)
            .unwrap();
        }
    }
}

fn draw_hold_bar_outline<SPI, DC, CS, RST>(display: &mut WaveshareLcd<SPI, DC, CS, RST>, y: i32)
where
    SPI: embedded_hal::spi::SpiBus<u8>,
    DC: embedded_hal::digital::OutputPin,
    CS: embedded_hal::digital::OutputPin,
    RST: embedded_hal::digital::OutputPin,
{
    const WIDTH: u32 = 208;
    const HEIGHT: u32 = 14;
    Rectangle::new(Point::new(16, y), Size::new(WIDTH, HEIGHT))
        .into_styled(PrimitiveStyle::with_stroke(Rgb565::CYAN, 1))
        .draw(display)
        .unwrap();
}

fn draw_hold_bar_progress<SPI, DC, CS, RST>(
    display: &mut WaveshareLcd<SPI, DC, CS, RST>,
    previous_ticks: u16,
    current_ticks: u16,
    y: i32,
) where
    SPI: embedded_hal::spi::SpiBus<u8>,
    DC: embedded_hal::digital::OutputPin,
    CS: embedded_hal::digital::OutputPin,
    RST: embedded_hal::digital::OutputPin,
{
    const WIDTH: u32 = 208;
    const HEIGHT: u32 = 14;
    const HOLD_TICKS: u32 = 30;
    let previous = WIDTH.saturating_sub(2) * u32::from(previous_ticks.min(30)) / HOLD_TICKS;
    let current = WIDTH.saturating_sub(2) * u32::from(current_ticks.min(30)) / HOLD_TICKS;
    if current > previous {
        Rectangle::new(
            Point::new(17 + previous as i32, y + 1),
            Size::new(current - previous, HEIGHT - 2),
        )
        .into_styled(PrimitiveStyle::with_fill(Rgb565::GREEN))
        .draw(display)
        .unwrap();
    }
}

fn reset_hold_bar<SPI, DC, CS, RST>(display: &mut WaveshareLcd<SPI, DC, CS, RST>, y: i32)
where
    SPI: embedded_hal::spi::SpiBus<u8>,
    DC: embedded_hal::digital::OutputPin,
    CS: embedded_hal::digital::OutputPin,
    RST: embedded_hal::digital::OutputPin,
{
    const WIDTH: u32 = 208;
    const HEIGHT: u32 = 14;
    Rectangle::new(Point::new(16, y), Size::new(WIDTH, HEIGHT))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
        .draw(display)
        .unwrap();
    draw_hold_bar_outline(display, y);
}
