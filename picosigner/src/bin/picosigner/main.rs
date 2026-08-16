#![no_std]
#![no_main]

extern crate alloc;

use core::sync::atomic::Ordering;
use embedded_graphics::{
    pixelcolor::{Rgb565, RgbColor},
    prelude::Drawable,
    text::Text,
};
use embedded_hal::{delay::DelayNs, pwm::SetDutyCycle};
use fugit::RateExtU32;
use hal::{
    dma::{single_buffer, DMAExt, HalfWord},
    gpio::FunctionPio0,
    multicore::Multicore,
    pio::{PIOBuilder, PIOExt, ShiftDirection},
    Clock,
};
use picosigner::{
    app::AppState,
    heap,
    ui::{
        action_at_for_app, draw_hold_bar_progress as shared_draw_hold_bar_progress, hold_bar_y,
        in_button, point_in_u16_button, render_app as draw_shared_app,
        reset_hold_bar as shared_reset_hold_bar, Fonts, HoldAction, REVIEW_SIGN_BUTTON,
        SCAN_CANCEL_BUTTON, SCAN_ZOOM_BUTTON,
    },
};
use picosigner_board::{
    CoordinatedSeedStore, Cst816d, FlashCoordinator, Orientation, Ov5640, Rp2350FlashCoordinator,
    Rp2350NorFlash, Rp2350Trng, SensorZoom, TransactionalSeedStore, WaveshareLcd,
};
use picosigner_qr::{BbqrCollector, UrPsbtCollector, MAX_PSBT_BYTES};
use picosigner_ui::{Gesture, GestureConfig, Navigator, Screen, TouchTracker, UiEvent};
use picosigner_wallet::Network;
use rp235x_hal as hal;
#[cfg(feature = "debug")]
use usb_device::{class_prelude::UsbBusAllocator, prelude::*};
#[cfg(feature = "debug")]
use usbd_serial::SerialPort;

mod export;
mod framebuffer;
mod qr_views;
#[cfg(feature = "debug")]
mod serial_debug;
use export::run_signed_export;
use framebuffer::{
    core1_live, frame_mut, prepare_camera_preview, take_payload, wait_decode_finished_bounded,
    wait_frame_release_bounded, wait_frame_released,
    CameraDmaTarget, PortraitFrame, SyncDecoder, CAMERA_PIXELS, DECODER_IDLE, DECODER_STATE,
    FRAME_READY, RESULT_READY,
};
use picosigner::ui::text_point;
use qr_views::draw_scan_overlay;

#[link_section = ".start_block"]
#[used]
pub static IMAGE_DEF: hal::block::ImageDef = hal::block::ImageDef::secure_exe();

const XTAL_FREQ_HZ: u32 = 12_000_000;

const BACKLIGHT_DUTY: [u16; 3] = [255, 40, 16];

/// Flash coordinator that parks core 1 (the QR decode pipeline) around any
/// XIP-touching ROM flash operation. Core 1 executes flash-backed code, so it
/// must be parked in a WFE loop before XIP is disabled; the base coordinator
/// handles the core-0 critical section and interrupt masking. Parking is a
/// no-op until core 1 has booted, and `park_core1` uses a bounded wait, so a
/// wedged decoder core can never freeze wallet storage.
struct ParkingCoordinator<C> {
    inner: C,
}

impl<C: FlashCoordinator> FlashCoordinator for ParkingCoordinator<C> {
    type Error = C::Error;

    fn with_flash_paused<F, T>(&mut self, op: F) -> Result<T, Self::Error>
    where
        F: FnOnce() -> T,
    {
        framebuffer::park_core1();
        let result = self.inner.with_flash_paused(op);
        framebuffer::unpark_core1();
        result
    }
}

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
    let mut sio = hal::Sio::new(pac.SIO);
    let pins = hal::gpio::Pins::new(
        pac.IO_BANK0,
        pac.PADS_BANK0,
        sio.gpio_bank0,
        &mut pac.RESETS,
    );

    let mut pwm_slices = hal::pwm::Slices::new(pac.PWM, &mut pac.RESETS);
    let backlight_pwm = &mut pwm_slices.pwm7;
    backlight_pwm.set_div_int(1);
    backlight_pwm.set_top(BACKLIGHT_DUTY[0]);
    backlight_pwm.enable();
    let backlight = &mut backlight_pwm.channel_b;
    backlight.output_to(pins.gpio15);

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

    let mut display = picosigner::init_display!(
        pac,
        pins,
        delay: &mut timer,
        freq: clocks.peripheral_clock.freq(),
        orientation: Orientation::PortraitTouchAligned,
    );

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

    // Turn backlight on after initializing display to hide artifacts
    backlight.set_duty_cycle(BACKLIGHT_DUTY[0]).unwrap();

    let mut entropy = Rp2350Trng::new(pac.TRNG, &mut pac.RESETS);
    let flash = TransactionalSeedStore::new(Rp2350NorFlash::new());
    let mut store = CoordinatedSeedStore::new(
        flash,
        ParkingCoordinator {
            inner: Rp2350FlashCoordinator::new(),
        },
    );
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

    // Core 1 runs the QR decode pipeline (grayscale extraction + quircs).
    // Spawned after boot-time storage reads and all peripheral init, with a
    // boot handshake: if it does not come up, the scanner degrades to
    // synchronous decoding on core 0 instead of risking a park deadlock.
    let mut multicore = Multicore::new(&mut pac.PSM, &mut pac.PPB, &mut sio.fifo);
    let spawned = multicore.cores()[1]
        .spawn(
            framebuffer::CORE1_STACK.take().unwrap(),
            framebuffer::decoder_core,
        )
        .is_ok();
    let mut core1_booted = false;
    if spawned {
        for _ in 0..200 {
            if framebuffer::CORE1_BOOTED.load(Ordering::Acquire) {
                core1_booted = true;
                break;
            }
            timer.delay_ms(1);
        }
    }
    framebuffer::set_core1_live(core1_booted);

    render_shared_to_lcd(&mut display, &fonts, &app, None, 0);
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
            render_shared_to_lcd(&mut display, &fonts, &app, None, 0);
            continue;
        }

        if matches!(app.screen(), Screen::Scan | Screen::SeedQrScan) {
            let seed_qr = app.screen() == Screen::SeedQrScan;
            let mut collector = BbqrCollector::<MAX_PSBT_BYTES>::new();
            let mut ur_collector = UrPsbtCollector::new();
            // 0 = unknown, 1 = BBQr, 2 = animated crypto-psbt UR.
            let mut scan_format = 0u8;
            let mut sensor_zoom = SensorZoom::X1;
            let _ = camera.set_zoom(sensor_zoom);
            let mut complete = false;
            let mut cancelled = false;
            // Require release of the touch that opened the scanner before a
            // scanner control can fire.
            let mut scan_touch_active = true;

            // If core 1 never came up, decode synchronously on core 0 so the
            // scanner still works (at reduced FPS).
            let mut sync_decoder = if core1_live() {
                None
            } else {
                Some(SyncDecoder::new())
            };

            // Discard a stale result from a previous scan session so an old
            // payload cannot complete a new one.
            if DECODER_STATE.load(Ordering::Acquire) == RESULT_READY {
                DECODER_STATE.store(DECODER_IDLE, Ordering::Release);
            }

            while !complete && !cancelled {
                // Core 1 reads CAMERA_FRAME only while it holds FRAME_READY;
                // camera DMA must not overwrite the buffer in that window. A
                // wedged core 1 is declared dead and decoding falls back to
                // this core instead of freezing the scanner.
                if sync_decoder.is_none() && !wait_frame_release_bounded() {
                    sync_decoder = Some(SyncDecoder::new());
                }

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

                // Decode path: core-1 pipeline when available, otherwise a
                // synchronous decode on this core. Either way the read must
                // finish before the display pass mutates the shared buffer.
                if sync_decoder.is_none() && !core1_live() {
                    // Core 1 died mid-session: take over decoding on core 0
                    // and reclaim the state machine.
                    sync_decoder = Some(SyncDecoder::new());
                    DECODER_STATE.store(DECODER_IDLE, Ordering::Release);
                }
                let sync_payload = match sync_decoder.as_mut() {
                    Some(sd) => sd.decode_current_frame(),
                    None => {
                        // Hand the fresh frame to core 1 BEFORE the display
                        // pass mutates it: extraction needs the DMA-native
                        // little-endian layout. A frame captured while core 1
                        // is still decoding is skipped; its pending result is
                        // consumed below.
                        if wait_decode_finished_bounded() {
                            let submit_state = DECODER_STATE.load(Ordering::Acquire);
                            if submit_state == DECODER_IDLE {
                                let _ = DECODER_STATE.compare_exchange(
                                    DECODER_IDLE,
                                    FRAME_READY,
                                    Ordering::AcqRel,
                                    Ordering::Acquire,
                                );
                            }
                        }
                        // Wait until core 1 has copied the frame to grayscale
                        // before the preview pass reuses the same storage.
                        wait_frame_released();
                        None
                    }
                };

                let display_frame = unsafe { frame_mut() };
                prepare_camera_preview(display_frame);
                let progress = match scan_format {
                    1 => collector.progress(),
                    2 => ur_collector.progress(),
                    _ => (0, 0),
                };
                draw_scan_overlay(display_frame, &fonts, sensor_zoom, progress, seed_qr);
                display.write_frame(display_frame);

                // Consume a finished decode: either the synchronous result or
                // the core-1 result that landed during capture/display.
                let payload: Option<&[u8]> = match sync_payload.as_deref() {
                    Some(p) => Some(p),
                    None if sync_decoder.is_none()
                        && DECODER_STATE.load(Ordering::Acquire) == RESULT_READY =>
                    {
                        Some(take_payload())
                    }
                    None => None,
                };
                if let Some(payload) = payload {
                    if seed_qr {
                        complete = app.accept_seed_qr(payload).is_ok();
                    } else {
                        let is_ur = payload
                            .get(..3)
                            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"ur:"));
                        if is_ur && scan_format != 1 {
                            scan_format = 2;
                            match ur_collector.push(payload) {
                                Ok(done) => complete = done,
                                Err(picosigner_qr::TransportError::MismatchedSession) => {
                                    ur_collector = UrPsbtCollector::new();
                                    complete = ur_collector.push(payload).unwrap_or(false);
                                }
                                Err(_) => {}
                            }
                        } else if !is_ur && scan_format != 2 {
                            scan_format = 1;
                            match collector.push(payload) {
                                Ok(done) => complete = done,
                                Err(picosigner_qr::TransportError::MismatchedSession) => {
                                    collector = BbqrCollector::new();
                                    complete = collector.push(payload).unwrap_or(false);
                                }
                                Err(_) => {}
                            }
                        }
                    }
                    if sync_decoder.is_none() {
                        DECODER_STATE.store(DECODER_IDLE, Ordering::Release);
                    }
                }
                #[cfg(feature = "debug")]
                serial_debug::poll(&mut usb_dev, &mut serial);

                match touch.point() {
                    Ok(None) => scan_touch_active = false,
                    Ok(Some(point)) if !scan_touch_active => {
                        scan_touch_active = true;
                        if point_in_u16_button(point.x, point.y, SCAN_CANCEL_BUTTON) {
                            cancelled = true;
                        } else if point_in_u16_button(point.x, point.y, SCAN_ZOOM_BUTTON) {
                            let next_zoom = sensor_zoom.toggle();
                            if camera.set_zoom(next_zoom).is_ok() {
                                sensor_zoom = next_zoom;
                            }
                        }
                    }
                    _ => {}
                }
            }
            if seed_qr {
                if !complete {
                    app.navigate(Screen::ImportWallet);
                }
            } else if complete {
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
            render_shared_to_lcd(&mut display, &fonts, &app, None, 0);
            continue;
        }

        let point = touch.point().ok().flatten();
        if let Some(point) = point {
            if was_touched {
                last_touch = (point.x, point.y);
                let _ = tracker.event(UiEvent::Tick);
                let (progress, _) = tracker.hold_progress();
                if progress > rendered_hold_ticks {
                    if let Some(action) = hold_action {
                        if let Some(y) = hold_bar_y(action) {
                            draw_shared_progress_to_lcd(
                                &mut display,
                                rendered_hold_ticks,
                                progress,
                                y,
                            );
                        }
                    }
                    rendered_hold_ticks = progress;
                }
            } else {
                hold_action = action_at_for_app(&app, point.x, point.y);
                if hold_action.is_some() {
                    last_touch = (point.x, point.y);
                    let _ = tracker.event(UiEvent::TouchDown {
                        x: last_touch.0,
                        y: last_touch.1,
                    });
                    was_touched = true;
                    rendered_hold_ticks = 0;
                    if !matches!(hold_action, Some(HoldAction::Brightness)) {
                        render_shared_to_lcd(&mut display, &fonts, &app, hold_action, 0);
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
                    (Some(HoldAction::DeleteWallet), Gesture::Hold) => {
                        if app.delete_wallet(&mut store).is_ok() {
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
                            render_shared_to_lcd(&mut display, &fonts, &app, None, 0);
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
                    (Some(HoldAction::Load), Gesture::Tap) => {
                        if app.wallets().len() == 1 {
                            if let Some(id) = app.wallets().get(0).map(|wallet| wallet.id) {
                                // Redraw without the hold bar before the
                                // synchronous key derivation + flash write so
                                // the user sees the device is working.
                                render_shared_to_lcd(&mut display, &fonts, &app, None, 0);
                                match app.load_wallet_id(id, &mut store) {
                                    Ok(()) => redraw_screen = true,
                                    Err(error) => {
                                        app.show_error(error);
                                        redraw_screen = true;
                                    }
                                }
                            }
                        } else if app.wallets().len() > 1 {
                            app.navigate(Screen::WalletList);
                            redraw_screen = true;
                        }
                    }
                    (Some(HoldAction::Wallet(index)), Gesture::Tap) => {
                        if let Some(id) = app.wallets().get(index).map(|wallet| wallet.id) {
                            render_shared_to_lcd(&mut display, &fonts, &app, None, 0);
                            match app.load_wallet_id(id, &mut store) {
                                Ok(()) => redraw_screen = true,
                                Err(error) => {
                                    app.show_error(error);
                                    redraw_screen = true;
                                }
                            }
                        }
                    }
                    (Some(HoldAction::Import), Gesture::Tap) => {
                        app.begin_import();
                        redraw_screen = true;
                    }
                    (Some(HoldAction::Create), Gesture::Tap) => {
                        app.begin_create();
                        redraw_screen = true;
                    }
                    (Some(HoldAction::ScanSeedQr), Gesture::Tap) => {
                        app.choose_seed_qr_import();
                        redraw_screen = true;
                    }
                    (Some(HoldAction::EnterMnemonic), Gesture::Tap) => {
                        app.choose_mnemonic_import();
                        redraw_screen = true;
                    }
                    (Some(HoldAction::MnemonicWords(words)), Gesture::Tap) => {
                        redraw_screen = app.start_mnemonic_entry(words).is_ok();
                    }
                    (Some(HoldAction::Network(network)), Gesture::Tap) => {
                        app.select_network(network);
                        redraw_screen = true;
                    }
                    (Some(HoldAction::Derivation), Gesture::Tap) => {
                        match app.confirm_derivation(&mut entropy, &mut store) {
                            Ok(()) => redraw_screen = true,
                            Err(error) => {
                                app.show_error(error);
                                redraw_screen = true;
                            }
                        }
                    }
                    (Some(HoldAction::Suggestion(index)), Gesture::Tap) => {
                        let _ = app.choose_mnemonic_suggestion(index);
                        redraw_screen = true;
                    }
                    (Some(HoldAction::Key(letter)), Gesture::Tap) => {
                        app.mnemonic_letter(letter);
                        redraw_screen = true;
                    }
                    (Some(HoldAction::Backspace), Gesture::Tap) => {
                        app.mnemonic_backspace();
                        redraw_screen = true;
                    }
                    (Some(HoldAction::Cancel), Gesture::Tap) => {
                        app.cancel_setup();
                        redraw_screen = true;
                    }
                    (Some(HoldAction::Scan), Gesture::Tap) => {
                        if app.begin_scan().is_ok() {
                            redraw_screen = true;
                        }
                    }
                    (Some(HoldAction::BackupMenu), Gesture::Tap) => {
                        if app.open_backup_menu().is_ok() {
                            redraw_screen = true;
                        }
                    }
                    (Some(HoldAction::SaveWallet), Gesture::Tap) => {
                        match app.save_wallet(&mut store) {
                            Ok(()) => redraw_screen = true,
                            Err(error) => {
                                app.show_error(error);
                                redraw_screen = true;
                            }
                        }
                    }
                    (Some(HoldAction::MnemonicBackup), Gesture::Tap) => {
                        redraw_screen = app.open_backup().is_ok();
                    }
                    (Some(HoldAction::SeedQrBackup), Gesture::Tap) => {
                        redraw_screen = app.open_backup_seed_qr().is_ok();
                    }
                    (Some(HoldAction::Logout), Gesture::Tap) => {
                        redraw_screen = app.logout(&mut store).is_ok();
                    }
                    (Some(HoldAction::BackupPage), Gesture::SwipeLeft) => {
                        app.next_backup_page();
                        redraw_screen = true;
                    }
                    (Some(HoldAction::BackupPage), Gesture::SwipeRight) => {
                        app.previous_backup_page();
                        redraw_screen = true;
                    }
                    (Some(HoldAction::BackupPage), Gesture::Tap) => {
                        app.navigate(Screen::BackupMenu);
                        redraw_screen = true;
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
                        match app.screen() {
                            Screen::WalletList => app.navigate(Screen::LoadWallet),
                            Screen::BackupSeedQr | Screen::BackupWords => {
                                app.navigate(Screen::BackupMenu)
                            }
                            Screen::Descriptor => app.navigate(Screen::BackupMenu),
                            Screen::BackupMenu => app.navigate(Screen::Home),
                            _ => app.navigate(Screen::Home),
                        }
                        redraw_screen = true;
                    }
                    _ => {}
                }
            }
            if redraw_screen {
                render_shared_to_lcd(&mut display, &fonts, &app, None, 0);
            } else if let Some(action) = action.and_then(hold_bar_y) {
                // TouchUp consumes the gesture state. Any release short of the
                // hold threshold clears only the progress-bar rectangle.
                reset_shared_hold_to_lcd(&mut display, action);
            }
            hold_action = None;
            rendered_hold_ticks = 0;
        }
        timer.delay_ms(20);
    }
}

fn render_shared_to_lcd<SPI, DC, CS, RST>(
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
    let mut target = PortraitFrame(display_frame);
    draw_shared_app(&mut target, fonts, app, hold_action, hold_ticks);
    drop(target);
    display.write_frame(display_frame);
}

fn draw_shared_progress_to_lcd<SPI, DC, CS, RST>(
    display: &mut WaveshareLcd<SPI, DC, CS, RST>,
    previous: u16,
    current: u16,
    y: i32,
) where
    SPI: embedded_hal::spi::SpiBus<u8>,
    DC: embedded_hal::digital::OutputPin,
    CS: embedded_hal::digital::OutputPin,
    RST: embedded_hal::digital::OutputPin,
{
    let display_frame = unsafe { frame_mut() };
    let mut target = PortraitFrame(display_frame);
    shared_draw_hold_bar_progress(&mut target, previous, current, y);
    drop(target);
    display.write_frame(display_frame);
}

fn reset_shared_hold_to_lcd<SPI, DC, CS, RST>(display: &mut WaveshareLcd<SPI, DC, CS, RST>, y: i32)
where
    SPI: embedded_hal::spi::SpiBus<u8>,
    DC: embedded_hal::digital::OutputPin,
    CS: embedded_hal::digital::OutputPin,
    RST: embedded_hal::digital::OutputPin,
{
    let display_frame = unsafe { frame_mut() };
    let mut target = PortraitFrame(display_frame);
    shared_reset_hold_bar(&mut target, y);
    drop(target);
    display.write_frame(display_frame);
}
