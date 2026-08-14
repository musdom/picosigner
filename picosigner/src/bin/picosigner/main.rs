#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;
use embedded_graphics::{
    geometry::Size,
    pixelcolor::{Rgb565, RgbColor},
    prelude::{DrawTarget, Drawable, Point, Primitive},
    primitives::{PrimitiveStyle, Rectangle},
    text::{Alignment, Text},
};
use embedded_hal::{delay::DelayNs, pwm::SetDutyCycle};
use fugit::RateExtU32;
use hal::{
    dma::{single_buffer, DMAExt, HalfWord},
    gpio::FunctionPio0,
    pio::{PIOBuilder, PIOExt, ShiftDirection},
    Clock,
};
use panic_halt as _;
use picosigner::{app::AppState, heap};
use picosigner_board::{
    CoordinatedSeedStore, Cst816d, Orientation, Ov5640, Rp2350FlashCoordinator, Rp2350NorFlash,
    Rp2350Trng, SensorZoom, TransactionalSeedStore, WaveshareLcd, CAMERA_GRAY_HEIGHT,
    CAMERA_GRAY_WIDTH, DISPLAY_HEIGHT, DISPLAY_WIDTH,
};
use picosigner_qr::{
    extract_rgb565_volatile, BbqrCollector, QuircDecoder, UrPsbtCollector, MAX_PSBT_BYTES,
};
use picosigner_ui::{Gesture, GestureConfig, Navigator, Screen, TouchTracker, UiEvent};
use picosigner_wallet::Network;
use rp235x_hal as hal;
#[cfg(feature = "debug")]
use usb_device::{class_prelude::UsbBusAllocator, prelude::*};
#[cfg(feature = "debug")]
use usbd_serial::SerialPort;
use zeroize::Zeroize;

mod controls;
mod export;
mod fonts;
mod framebuffer;
mod qr_views;
#[cfg(feature = "debug")]
mod serial_debug;
use controls::*;
use export::run_signed_export;
use fonts::{text_point, Fonts};
use framebuffer::{
    camera_pixels, frame_mut, prepare_camera_preview, CameraDmaTarget, PortraitFrame, CAMERA_PIXELS,
};
use qr_views::{draw_descriptor_qr, draw_scan_overlay};

#[link_section = ".start_block"]
#[used]
pub static IMAGE_DEF: hal::block::ImageDef = hal::block::ImageDef::secure_exe();

const XTAL_FREQ_HZ: u32 = 12_000_000;

const BACKLIGHT_DUTY: [u16; 3] = [255, 40, 16];
const QR_PAYLOAD_CAPACITY: usize = 1024;

#[hal::entry]
fn main() -> ! {
    unsafe {
        heap::init();
    }
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

    let display_frame = unsafe { frame_mut() };
    display_frame.fill(0);
    {
        let mut target = PortraitFrame(display_frame);
        Text::new(
            "PicoSigner",
            text_point(24, 136),
            fonts.regular(Rgb565::WHITE),
        )
        .draw(&mut target)
        .unwrap();
        Text::new(
            "Loading wallet",
            text_point(24, 160),
            fonts.regular(Rgb565::CYAN),
        )
        .draw(&mut target)
        .unwrap();
    }
    display.write_frame(display_frame);

    let mut entropy = Rp2350Trng::new(pac.TRNG, &mut pac.RESETS);
    let flash = TransactionalSeedStore::new(Rp2350NorFlash::new());
    let mut store = CoordinatedSeedStore::new(flash, Rp2350FlashCoordinator::new());
    let mut app = AppState::new(Network::Signet);

    if app.boot(&mut entropy, &mut store).is_err() {
        let display_frame = unsafe { frame_mut() };
        display_frame.fill(0);
        {
            let mut target = PortraitFrame(display_frame);
            Text::new(
                "Secure hardware unavailable",
                text_point(24, 96),
                fonts.regular(Rgb565::WHITE),
            )
            .draw(&mut target)
            .unwrap();
            Text::new(
                "Wallet generation stopped",
                text_point(24, 120),
                fonts.regular(Rgb565::CYAN),
            )
            .draw(&mut target)
            .unwrap();
        }
        display.write_frame(display_frame);
        loop {
            timer.delay_ms(1_000);
        }
    }

    #[cfg(feature = "debug")]
    let usb_bus = UsbBusAllocator::new(hal::usb::UsbBus::new(
        pac.USB,
        pac.USB_DPRAM,
        clocks.usb_clock,
        true,
        &mut pac.RESETS,
    ));
    #[cfg(feature = "debug")]
    let mut serial = SerialPort::new(&usb_bus);
    #[cfg(feature = "debug")]
    let mut usb_dev = UsbDeviceBuilder::new(&usb_bus, UsbVidPid(0x16c0, 0x27dd))
        .strings(&[StringDescriptors::default()
            .manufacturer("PicoSigner")
            .product("PicoSigner debug")
            .serial_number("SIGNER")])
        .unwrap()
        .max_packet_size_0(64)
        .unwrap()
        .device_class(2)
        .build();

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

    render_app(&mut display, &fonts, &app, None, 0);
    let mut tracker = TouchTracker::new(GestureConfig::default());
    let mut was_touched = false;
    let mut last_touch = (0, 0);
    let mut hold_action = None;
    let mut rendered_hold_ticks = 0;
    let mut backlight_level = 0usize;
    loop {
        #[cfg(feature = "debug")]
        serial_debug::poll(&mut usb_dev, &mut serial);
        if app.screen() == Screen::Signed {
            run_signed_export(
                &app,
                &fonts,
                &mut display,
                &mut touch,
                backlight,
                &mut timer,
                &mut backlight_level,
            );
            app.navigate(Screen::Home);
            was_touched = false;
            hold_action = None;
            rendered_hold_ticks = 0;
            render_app(&mut display, &fonts, &app, None, 0);
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

                let camera_pixels = unsafe { camera_pixels() };
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

                let display_frame = unsafe { frame_mut() };
                prepare_camera_preview(display_frame);
                let progress = match scan_format {
                    1 => collector.progress(),
                    2 => ur_collector.progress(),
                    _ => (0, 0),
                };
                draw_scan_overlay(display_frame, &fonts, sensor_zoom, progress);
                display.write_frame(display_frame);
                #[cfg(feature = "debug")]
                serial_debug::poll(&mut usb_dev, &mut serial);

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
            render_app(&mut display, &fonts, &app, None, 0);
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
                        render_app(&mut display, &fonts, &app, hold_action, 0);
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
                            render_app(&mut display, &fonts, &app, None, 0);
                            match app.sign_after_hold(true) {
                                Ok(signed) => {
                                    #[cfg(feature = "debug")]
                                    serial_debug::log_signed_psbt(
                                        &mut usb_dev,
                                        &mut serial,
                                        &mut timer,
                                        &signed.bytes,
                                        &signed.taproot_debug,
                                    );
                                    #[cfg(not(feature = "debug"))]
                                    let _ = signed;
                                }
                                Err(picosigner::app::AppError::Wallet(error)) => {
                                    #[cfg(feature = "debug")]
                                    serial_debug::log_signing_error(
                                        &mut usb_dev,
                                        &mut serial,
                                        &mut timer,
                                        &error,
                                    );
                                    #[cfg(not(feature = "debug"))]
                                    let _ = error;
                                    app.navigate(Screen::Review);
                                }
                                Err(_) => app.navigate(Screen::Review),
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
                render_app(&mut display, &fonts, &app, None, 0);
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
    fonts: &Fonts,
    app: &AppState,
    hold_action: Option<HoldAction>,
    hold_ticks: u16,
) where
    SPI: embedded_hal::spi::SpiBus<u8>,
    DC: embedded_hal::digital::OutputPin,
    CS: embedded_hal::digital::OutputPin,
    RST: embedded_hal::digital::OutputPin,
{
    let display_frame = unsafe { frame_mut() };
    display_frame.fill(0);
    let mut target = PortraitFrame(display_frame);
    let title = || fonts.regular(Rgb565::WHITE);
    let accent = || fonts.regular(Rgb565::CYAN);
    let body = || fonts.regular(Rgb565::GREEN);
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
                    Text::new(&identity, text_point(16, 24), title())
                        .draw(&mut target)
                        .unwrap();
                }
            }
        } else {
            Text::new(heading, text_point(16, 24), title())
                .draw(&mut target)
                .unwrap();
        }
    }

    match app.screen() {
        Screen::Backup => {
            Text::new("Write down these 12 words", text_point(16, 48), accent())
                .draw(&mut target)
                .unwrap();
            if let Ok(mut mnemonic) = app.mnemonic() {
                for (line, chunk) in mnemonic.as_bytes().chunks(34).enumerate() {
                    if let Ok(text) = core::str::from_utf8(chunk) {
                        Text::new(text, text_point(16, 76 + line as i32 * 14), body())
                            .draw(&mut target)
                            .unwrap();
                    }
                }
                mnemonic.zeroize();
            }
            Text::new("Hold to confirm backup", text_point(16, 238), accent())
                .draw(&mut target)
                .unwrap();
            draw_hold_bar(&mut target, hold_ticks, 258);
        }
        Screen::Home => {
            let descriptor_button_color = Rgb565::new(0, 32, 0);
            Rectangle::new(
                Point::new(HOME_SCAN_BUTTON.0, HOME_SCAN_BUTTON.1),
                Size::new(HOME_SCAN_BUTTON.2, HOME_SCAN_BUTTON.3),
            )
            .into_styled(PrimitiveStyle::with_fill(Rgb565::BLUE))
            .draw(&mut target)
            .unwrap();
            Text::with_alignment(
                "SCAN PSBT",
                text_point(120, 137),
                fonts.regular_on(Rgb565::WHITE, Rgb565::BLUE),
                Alignment::Center,
            )
            .draw(&mut target)
            .unwrap();
            Rectangle::new(
                Point::new(HOME_DESCRIPTOR_BUTTON.0, HOME_DESCRIPTOR_BUTTON.1),
                Size::new(HOME_DESCRIPTOR_BUTTON.2, HOME_DESCRIPTOR_BUTTON.3),
            )
            .into_styled(PrimitiveStyle::with_fill(descriptor_button_color))
            .draw(&mut target)
            .unwrap();
            Text::with_alignment(
                "EXPORT DESCRIPTOR",
                text_point(120, 189),
                fonts.regular_on(Rgb565::WHITE, descriptor_button_color),
                Alignment::Center,
            )
            .draw(&mut target)
            .unwrap();
            Rectangle::new(
                Point::new(HOME_NEW_WALLET_BUTTON.0, HOME_NEW_WALLET_BUTTON.1),
                Size::new(HOME_NEW_WALLET_BUTTON.2, HOME_NEW_WALLET_BUTTON.3),
            )
            .into_styled(PrimitiveStyle::with_fill(Rgb565::RED))
            .draw(&mut target)
            .unwrap();
            Text::with_alignment(
                "HOLD FOR NEW WALLET",
                text_point(120, 241),
                fonts.regular_on(Rgb565::WHITE, Rgb565::RED),
                Alignment::Center,
            )
            .draw(&mut target)
            .unwrap();
            if matches!(hold_action, Some(HoldAction::Regenerate)) {
                draw_hold_bar(&mut target, hold_ticks, HOME_NEW_WALLET_BAR_Y);
            }
        }
        Screen::Scan => {
            Text::new("Camera/BBQr receive next", text_point(16, 76), accent())
                .draw(&mut target)
                .unwrap();
            Text::new("No signing without review", text_point(16, 102), body())
                .draw(&mut target)
                .unwrap();
        }
        Screen::Review => {
            if let Some(review) = app.review() {
                Text::new("BACK", text_point(196, 20), title())
                    .draw(&mut target)
                    .unwrap();
                let mut y = 48;
                let mut recipients = 0usize;
                for output in review.outputs.iter().filter(|output| !output.is_change) {
                    recipients += 1;
                    let amount = alloc::format!("SEND {} sats", output.value_sat);
                    Text::new(&amount, text_point(12, y), accent())
                        .draw(&mut target)
                        .unwrap();
                    y += 14;
                    if let Some(address) = &output.address {
                        for chunk in address.as_bytes().chunks(29) {
                            if y > 242 {
                                break;
                            }
                            if let Ok(line) = core::str::from_utf8(chunk) {
                                Text::new(line, text_point(12, y), title())
                                    .draw(&mut target)
                                    .unwrap();
                            }
                            y += 12;
                        }
                    } else {
                        Text::new("UNKNOWN OUTPUT SCRIPT", text_point(12, y), title())
                            .draw(&mut target)
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
                Text::new(&summary, text_point(12, 266), body())
                    .draw(&mut target)
                    .unwrap();
                let fee = match review.fee_sat {
                    Some(value) => alloc::format!("Fee: {value} sats"),
                    None => alloc::format!("Fee: unavailable"),
                };
                Text::new(&fee, text_point(12, 282), body())
                    .draw(&mut target)
                    .unwrap();
                Text::with_alignment(
                    "HOLD TO CONFIRM & SIGN",
                    text_point(120, 301),
                    title(),
                    Alignment::Center,
                )
                .draw(&mut target)
                .unwrap();
                draw_hold_bar(
                    &mut target,
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
            Text::new("Signing reviewed PSBT...", text_point(16, 76), accent())
                .draw(&mut target)
                .unwrap();
            Text::new("Verifying transaction \ndigest", text_point(16, 102), body())
                .draw(&mut target)
                .unwrap();
        }
        Screen::Signed => {
            Text::new("PSBT signed successfully", text_point(16, 76), accent())
                .draw(&mut target)
                .unwrap();
            Text::new("Preparing animated BBQr...", text_point(16, 102), body())
                .draw(&mut target)
                .unwrap();
        }
        Screen::Descriptor => {
            if let Ok(export) = app.export_descriptor() {
                draw_descriptor_qr(&mut target, export.descriptor.as_bytes());
            } else {
                Text::with_alignment(
                    "DESCRIPTOR UNAVAILABLE",
                    text_point(120, 144),
                    fonts.regular(Rgb565::RED),
                    Alignment::Center,
                )
                .draw(&mut target)
                .unwrap();
            }
            Text::with_alignment(
                "TAP TO RETURN",
                text_point(120, 313),
                title(),
                Alignment::Center,
            )
            .draw(&mut target)
            .unwrap();
        }
        _ => {
            Text::new("State ready", text_point(16, 76), accent())
                .draw(&mut target)
                .unwrap();
        }
    }
    drop(target);
    display.write_frame(display_frame);
}

fn draw_hold_bar<D>(display: &mut D, hold_ticks: u16, y: i32)
where
    D: DrawTarget<Color = Rgb565, Error = core::convert::Infallible>,
{
    draw_hold_bar_outline(display, y);
    draw_hold_bar_progress(display, 0, hold_ticks, y);
}

fn network_name(network: Network) -> &'static str {
    match network {
        Network::Mainnet => "mainnet",
        Network::Testnet4 => "testnet4",
        Network::Signet => "signet",
    }
}

fn draw_hold_bar_outline<D>(display: &mut D, y: i32)
where
    D: DrawTarget<Color = Rgb565, Error = core::convert::Infallible>,
{
    const WIDTH: u32 = 208;
    const HEIGHT: u32 = 14;
    Rectangle::new(Point::new(16, y), Size::new(WIDTH, HEIGHT))
        .into_styled(PrimitiveStyle::with_stroke(Rgb565::CYAN, 1))
        .draw(display)
        .unwrap();
}

fn draw_hold_bar_progress<D>(display: &mut D, previous_ticks: u16, current_ticks: u16, y: i32)
where
    D: DrawTarget<Color = Rgb565, Error = core::convert::Infallible>,
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

fn reset_hold_bar<D>(display: &mut D, y: i32)
where
    D: DrawTarget<Color = Rgb565, Error = core::convert::Infallible>,
{
    const WIDTH: u32 = 208;
    const HEIGHT: u32 = 14;
    Rectangle::new(Point::new(16, y), Size::new(WIDTH, HEIGHT))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
        .draw(display)
        .unwrap();
    draw_hold_bar_outline(display, y);
}
