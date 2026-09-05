#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;
use embedded_graphics::{prelude::Drawable, text::Text};
use embedded_hal::{delay::DelayNs, pwm::SetDutyCycle};
use fugit::RateExtU32;
use hal::{
    dma::{single_buffer, DMAExt, HalfWord},
    gpio::FunctionPio0,
    pio::{PIOBuilder, PIOExt, ShiftDirection},
    Clock,
};
// Panic handler comes from the `picosigner` library, which renders the
// panic location and message on the LCD.
use picosigner as _;
use picosigner::{
    app::AppState,
    dispatch::{dispatch, finish_derivation, Effect},
    heap, palette,
    ui::{
        action_at_for_app, draw_hold_bar_progress as shared_draw_hold_bar_progress,
        hold_bar_bounds, in_button, point_in_u16_button, render_app as draw_shared_app,
        reset_hold_bar as shared_reset_hold_bar, Fonts, HoldAction, REVIEW_SIGN_BUTTON,
        SCAN_CANCEL_BUTTON, SCAN_ZOOM_BUTTON,
    },
};
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

mod export;
mod qr_views;
#[cfg(feature = "debug")]
mod serial_debug;
use export::run_signed_export;
use picosigner::ui::text_point;
use picosigner_board::framebuffer::{
    camera_pixels, frame_mut, prepare_camera_preview, CameraDmaTarget, LcdSource, PortraitFrame,
    CAMERA_PIXELS,
};
use qr_views::draw_scan_overlay;

#[link_section = ".start_block"]
#[used]
pub static IMAGE_DEF: hal::block::ImageDef = hal::block::ImageDef::secure_exe();

const XTAL_FREQ_HZ: u32 = 12_000_000;

const BACKLIGHT_DUTY: [u16; 3] = [80, 20, 5];
const QR_PAYLOAD_CAPACITY: usize = 1024;

/// Sends the shared framebuffer to the panel over DMA.
///
/// Replaces the blocking `write_frame`: the CPU no longer feeds the SPI FIFO
/// byte by byte, which measured 47.8 ms -> 19.5 ms at 75 MHz SPI. CS stays
/// asserted across the transfer so the panel sees one continuous pixel stream.
///
/// A macro rather than a function because `begin_dma_frame` consumes the
/// display to hand out the SPI peripheral, and naming that fully-generic
/// `WaveshareLcd` type in a signature costs four type aliases for no gain.
///
/// Camera DMA (ch0) is always waited on before this runs, so the two channels
/// never touch the framebuffer at the same time.
/// Concrete display type for the Waveshare panel on SPI0.
///
/// The DMA path must own the SPI peripheral for the duration of a transfer, so
/// helpers that send frames cannot stay generic over `SPI` — a `&mut SPI` can
/// never satisfy `single_buffer::Config::new`, which takes its target by value.
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

/// Sends the shared framebuffer to the panel over DMA.
///
/// Replaces the blocking `write_frame`: the CPU no longer feeds the SPI FIFO
/// byte by byte, measured 47.8 ms -> 19.5 ms at 75 MHz SPI.
///
/// Camera DMA (ch0) is always waited on before any display pass, so the two
/// channels never touch the framebuffer at the same time.
fn send_frame_dma<CH: hal::dma::SingleChannel>(display: &mut Lcd, lcd_dma: &mut Option<CH>) {
    let ch = lcd_dma.take().expect("LCD DMA channel in use");
    let ch = display.write_frame_dma(|spi| {
        let transfer = single_buffer::Config::new(ch, LcdSource, spi).start();
        let (ch, _src, spi) = transfer.wait();
        (spi, ch)
    });
    *lcd_dma = Some(ch);

    // The PL022 is full duplex: every byte DMA shifted out also clocked one in,
    // and nothing drained it, so the RX FIFO is left full with a sticky
    // overrun. Those stale words are read back by the next command and
    // desynchronise the panel. `SpiBus::write` avoids this by discarding one
    // word per word sent; `SpiBus::read` cannot be used to clean up because it
    // clocks dummy bytes out to the panel, so drain the FIFO directly.
    //
    // This runs after `write_frame_dma` has flushed and released CS: draining
    // while the shift register is still busy discards words that are still
    // arriving and leaves the FIFO dirty anyway.
    let device = unsafe { &*hal::pac::SPI0::ptr() };
    while device.sspsr().read().rne().bit_is_set() {
        let _ = device.sspdr().read().data().bits();
    }
    device.sspicr().write(|w| w.roric().clear_bit_by_one());
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

    // Same bring-up the panic handler uses, so the two cannot drift apart.
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
            fonts.regular(palette::TEXT),
        )
        .draw(&mut target)
        .unwrap();
        Text::new(
            "Loading wallet",
            text_point(24, 160),
            fonts.regular(palette::TEXT_MUTED),
        )
        .draw(&mut target)
        .unwrap();
    }
    display.write_frame(display_frame);

    // Turn backlight on after initializing display to hide artifacts
    backlight.set_duty_cycle(BACKLIGHT_DUTY[0]).unwrap();

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
                fonts.regular(palette::TEXT),
            )
            .draw(&mut target)
            .unwrap();
            Text::new(
                "Wallet generation stopped",
                text_point(24, 120),
                fonts.regular(palette::TEXT_MUTED),
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
    // Dump the clock/timing registers before and after the changes: writes to
    // the clock path are silently ignored while the sensor streams, so the
    // only way to know whether they took effect is to read them back.
    #[cfg(feature = "debug")]
    if let Ok(regs) = camera.read_clock_timing() {
        serial_debug::log_camera_clocks(&mut usb_dev, &mut serial, &mut timer, &regs, 25_000_000);
    }
    camera.set_fast_frame_timing(true, &mut timer).unwrap();
    camera.set_fast_frame_period(true).unwrap();
    #[cfg(feature = "debug")]
    let camera_clock_regs = camera.read_clock_timing().ok();
    #[cfg(feature = "debug")]
    if let Some(regs) = camera_clock_regs.as_ref() {
        serial_debug::log_camera_clocks(&mut usb_dev, &mut serial, &mut timer, regs, 25_000_000);
    }
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
    // Autopush at 16 bits replaces the explicit `push block`. The push was the
    // longest stall in the inner loop: after the second `wait 0 pin 10` the
    // program had to run push then jmp before reaching the next `wait 1`, a
    // 3-instruction gap. At 150 MHz PIO that is 20 ns, which exceeds the
    // 14.5 ns PCLK half-period once PCLK is raised to 34.38 MHz, and is why
    // raising PCLK previously corrupted frames. Dropping the push leaves a
    // 2-instruction (13.3 ns) gap, which fits with a narrow margin.
    //
    // Byte order is unchanged: with ShiftDirection::Left each `in pins, 8`
    // enters the low bits, so after two the ISR holds 0x0000_B1B2 and the
    // HalfWord DMA reads the same 0xB1B2 it did with the explicit push.
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
    // Channel 1 drives the LCD. It never runs concurrently with camera DMA on
    // ch0: the capture transfer is always waited on before a display pass.
    let mut lcd_dma = Some(dma.ch1);

    render_shared_to_lcd(&mut display, &mut lcd_dma, &fonts, &app, None, 0);
    let mut tracker = TouchTracker::new(GestureConfig::default());
    let mut was_touched = false;
    let mut last_touch = (0, 0);
    let mut hold_action = None;
    let mut rendered_hold_ticks = 0;
    let mut backlight_level = 0usize;
    const SESSION_TIMEOUT_US: u32 = 5 * 60 * 1_000_000;
    let mut last_activity_us = timer.get_counter_low();
    loop {
        #[cfg(feature = "debug")]
        serial_debug::poll(&mut usb_dev, &mut serial);
        let now_us = timer.get_counter_low();
        if app.screen() != Screen::LoadWallet
            && now_us.wrapping_sub(last_activity_us) >= SESSION_TIMEOUT_US
        {
            app.expire_session();
            was_touched = false;
            hold_action = None;
            rendered_hold_ticks = 0;
            last_activity_us = timer.get_counter_low();
            render_shared_to_lcd(&mut display, &mut lcd_dma, &fonts, &app, None, 0);
            continue;
        }
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
            backlight_level = 0;
            backlight
                .set_duty_cycle(BACKLIGHT_DUTY[backlight_level])
                .unwrap();
            render_shared_to_lcd(&mut display, &mut lcd_dma, &fonts, &app, None, 0);
            continue;
        }

        if matches!(
            app.screen(),
            Screen::Scan | Screen::SeedQrScan | Screen::CameraEntropy
        ) {
            let seed_qr = app.screen() == Screen::SeedQrScan;
            let camera_entropy = app.screen() == Screen::CameraEntropy;
            // Entering a scan is the tightest moment for the heap: the decoder
            // wants ~70 KiB of 128 KiB. Report what the UI left behind.
            #[cfg(feature = "debug")]
            serial_debug::log_heap(&mut usb_dev, &mut serial, "scan-enter");
            let mut collector = BbqrCollector::<MAX_PSBT_BYTES>::new();
            let mut ur_collector = UrPsbtCollector::new();
            // 0 = unknown, 1 = BBQr, 2 = animated crypto-psbt UR.
            let mut scan_format = 0u8;
            let mut grayscale = vec![0u8; CAMERA_GRAY_WIDTH * CAMERA_GRAY_HEIGHT];
            let mut decoder =
                QuircDecoder::<CAMERA_GRAY_WIDTH, CAMERA_GRAY_HEIGHT, QR_PAYLOAD_CAPACITY>::new();
            #[cfg(feature = "debug")]
            serial_debug::log_heap(&mut usb_dev, &mut serial, "scan-decoder");
            let mut sensor_zoom = SensorZoom::X1;
            let _ = camera.set_zoom(sensor_zoom);
            // Read the sensor live as capture begins. The boot-time dump lands
            // before the host has opened the CDC port and is lost; by this
            // point a terminal is attached and this reflects what the sensor is
            // actually running rather than what was written to it.
            #[cfg(feature = "debug")]
            if let Ok(regs) = camera.read_clock_timing() {
                serial_debug::log_camera_clocks(
                    &mut usb_dev,
                    &mut serial,
                    &mut timer,
                    &regs,
                    25_000_000,
                );
            }
            let mut centered_zoom = false;
            let mut complete = false;
            let mut cancelled = false;
            // Require release of the touch that opened the scanner before a
            // scanner control can fire.
            let mut scan_touch_active = true;
            let mut scan_last_touch = (0u16, 0u16);
            let mut scan_tracker = TouchTracker::new(GestureConfig::default());
            #[cfg(feature = "debug")]
            let mut scan_timing = serial_debug::ScanTiming::default();
            #[cfg(feature = "debug")]
            let mut last_timing_report = timer.get_counter_low();

            while !complete && !cancelled {
                #[cfg(feature = "debug")]
                let frame_started = timer.get_counter_low();
                #[cfg(feature = "debug")]
                let capture_started = timer.get_counter_low();
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
                #[cfg(feature = "debug")]
                let capture_us = timer.get_counter_low().wrapping_sub(capture_started);

                #[cfg(feature = "debug")]
                let extract_started = timer.get_counter_low();
                #[cfg(feature = "debug")]
                let mut decode_us = 0u32;
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
                    #[cfg(feature = "debug")]
                    let decode_started = timer.get_counter_low();
                    let decoded = decoder.decode_with_timing(&grayscale, || 0);
                    #[cfg(feature = "debug")]
                    {
                        decode_us = timer.get_counter_low().wrapping_sub(decode_started);
                    }
                    if camera_entropy {
                        app.accept_camera_entropy(&grayscale);
                        complete = true;
                    } else if let Ok((Some(payload), _)) = decoded {
                        if seed_qr {
                            complete = app.accept_seed_qr(payload.0).is_ok();
                        } else {
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
                }

                #[cfg(feature = "debug")]
                let extract_us = timer
                    .get_counter_low()
                    .wrapping_sub(extract_started)
                    .wrapping_sub(decode_us);

                #[cfg(feature = "debug")]
                let downscale_started = timer.get_counter_low();
                let display_frame = unsafe { frame_mut() };
                prepare_camera_preview(display_frame);
                #[cfg(feature = "debug")]
                let downscale_us = timer.get_counter_low().wrapping_sub(downscale_started);
                let progress = match scan_format {
                    1 => collector.progress(),
                    2 => ur_collector.progress(),
                    _ => (0, 0),
                };
                draw_scan_overlay(
                    display_frame,
                    &fonts,
                    sensor_zoom,
                    progress,
                    seed_qr || camera_entropy,
                );
                #[cfg(feature = "debug")]
                let lcd_started = timer.get_counter_low();
                send_frame_dma(&mut display, &mut lcd_dma);
                #[cfg(feature = "debug")]
                let lcd_us = timer.get_counter_low().wrapping_sub(lcd_started);
                #[cfg(feature = "debug")]
                {
                    let now = timer.get_counter_low();
                    scan_timing.capture_us = capture_us;
                    scan_timing.extract_us = extract_us;
                    scan_timing.decode_us = decode_us;
                    scan_timing.downscale_us = downscale_us;
                    scan_timing.lcd_us = lcd_us;
                    scan_timing.frame_us = now.wrapping_sub(frame_started);
                    if now.wrapping_sub(last_timing_report) >= 1_000_000 {
                        last_timing_report = now;
                        serial_debug::log_scan_timing(&mut usb_dev, &mut serial, &scan_timing);
                    }
                }
                #[cfg(feature = "debug")]
                serial_debug::poll(&mut usb_dev, &mut serial);

                match touch.point() {
                    Ok(None) if scan_touch_active => {
                        scan_touch_active = false;
                        if let Some(event) = scan_tracker.event(UiEvent::TouchUp {
                            x: scan_last_touch.0,
                            y: scan_last_touch.1,
                        }) {
                            match event.gesture {
                                Gesture::SwipeRight => cancelled = true,
                                Gesture::Tap
                                    if point_in_u16_button(
                                        event.x,
                                        event.y,
                                        SCAN_CANCEL_BUTTON,
                                    ) =>
                                {
                                    cancelled = true
                                }
                                Gesture::Tap
                                    if point_in_u16_button(event.x, event.y, SCAN_ZOOM_BUTTON) =>
                                {
                                    let next_zoom = sensor_zoom.toggle();
                                    if camera.set_zoom(next_zoom).is_ok() {
                                        sensor_zoom = next_zoom;
                                    }
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

            // The decoder owns most of the QR heap. Release it before the app
            // copies the completed PSBT into its pending-review slot.
            drop(decoder);
            drop(grayscale);
            #[cfg(feature = "debug")]
            serial_debug::log_heap(&mut usb_dev, &mut serial, "scan-exit");
            if camera_entropy {
                if !complete {
                    app.navigate(Screen::EntropySources);
                }
            } else if seed_qr {
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
            last_activity_us = timer.get_counter_low();
            render_shared_to_lcd(&mut display, &mut lcd_dma, &fonts, &app, None, 0);
            continue;
        }

        let point = touch.point().ok().flatten();
        if let Some(point) = point {
            last_activity_us = timer.get_counter_low();
            if was_touched {
                last_touch = (point.x, point.y);
                let _ = tracker.event(UiEvent::Tick);
                let (progress, _) = tracker.hold_progress();
                if progress > rendered_hold_ticks {
                    if let Some(action) = hold_action {
                        if let Some(bounds) = hold_bar_bounds(action) {
                            draw_shared_progress_to_lcd(
                                &mut display,
                                &mut lcd_dma,
                                rendered_hold_ticks,
                                progress,
                                bounds,
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
                        render_shared_to_lcd(
                            &mut display,
                            &mut lcd_dma,
                            &fonts,
                            &app,
                            hold_action,
                            0,
                        );
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
                // A hold that drifted off the sign button must not sign.
                let signing_off_button = matches!(action, Some(HoldAction::Sign))
                    && gesture.gesture == Gesture::Hold
                    && !in_button(last_touch.0, last_touch.1, REVIEW_SIGN_BUTTON);

                let effect = if signing_off_button {
                    Effect::None
                } else {
                    dispatch(&mut app, &mut entropy, &mut store, action, gesture.gesture)
                };
                redraw_screen = effect.needs_redraw();

                match effect {
                    // Paint the "Deriving" screen before the blocking
                    // derivation so the wait is not a frozen frame.
                    Effect::RenderThenDerive => {
                        render_shared_to_lcd(&mut display, &mut lcd_dma, &fonts, &app, None, 0);
                        finish_derivation(&mut app, &mut entropy, &mut store);
                    }
                    Effect::RenderThenSign => {
                        render_shared_to_lcd(&mut display, &mut lcd_dma, &fonts, &app, None, 0);
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
                    }
                    // The scan screen owns the camera; the loop below picks it
                    // up from the app state on the next pass.
                    Effect::BeginScan | Effect::BeginSeedQrScan => {}
                    Effect::CycleBrightness => {
                        backlight_level = (backlight_level + 1) % BACKLIGHT_DUTY.len();
                        backlight
                            .set_duty_cycle(BACKLIGHT_DUTY[backlight_level])
                            .unwrap();
                    }
                    // Leaving a QR screen returns the backlight to its floor.
                    Effect::DimBacklight => {
                        if backlight_level != 0 {
                            backlight_level = 0;
                            backlight
                                .set_duty_cycle(BACKLIGHT_DUTY[backlight_level])
                                .unwrap();
                        }
                    }
                    Effect::Redraw | Effect::None => {}
                }
            }
            if redraw_screen {
                render_shared_to_lcd(&mut display, &mut lcd_dma, &fonts, &app, None, 0);
            } else if let Some(action) = action.and_then(hold_bar_bounds) {
                // TouchUp consumes the gesture state. Any release short of the
                // hold threshold clears only the progress-bar rectangle.
                reset_shared_hold_to_lcd(&mut display, &mut lcd_dma, action);
            }
            hold_action = None;
            rendered_hold_ticks = 0;
        }
        timer.delay_ms(20);
    }
}

fn render_shared_to_lcd<CH: hal::dma::SingleChannel>(
    display: &mut Lcd,
    lcd_dma: &mut Option<CH>,
    fonts: &Fonts,
    app: &AppState,
    hold_action: Option<HoldAction>,
    hold_ticks: u16,
) {
    let display_frame = unsafe { frame_mut() };
    let mut target = PortraitFrame(display_frame);
    draw_shared_app(&mut target, fonts, app, hold_action, hold_ticks);
    drop(target);
    send_frame_dma(display, lcd_dma);
}

fn draw_shared_progress_to_lcd<CH: hal::dma::SingleChannel>(
    display: &mut Lcd,
    lcd_dma: &mut Option<CH>,
    previous: u16,
    current: u16,
    bounds: (i32, i32, u32, u32),
) {
    let display_frame = unsafe { frame_mut() };
    let mut target = PortraitFrame(display_frame);
    shared_draw_hold_bar_progress(&mut target, previous, current, bounds);
    drop(target);
    send_frame_dma(display, lcd_dma);
}

fn reset_shared_hold_to_lcd<CH: hal::dma::SingleChannel>(
    display: &mut Lcd,
    lcd_dma: &mut Option<CH>,
    bounds: (i32, i32, u32, u32),
) {
    let display_frame = unsafe { frame_mut() };
    let mut target = PortraitFrame(display_frame);
    shared_reset_hold_bar(&mut target, bounds);
    drop(target);
    send_frame_dma(display, lcd_dma);
}
