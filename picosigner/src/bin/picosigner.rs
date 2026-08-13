#![no_std]
#![no_main]

extern crate alloc;

use embedded_graphics::{
    geometry::Size,
    mono_font::{ascii::FONT_6X10, MonoTextStyle},
    pixelcolor::{Rgb565, RgbColor},
    prelude::{DrawTarget, Drawable, Point, Primitive},
    primitives::{PrimitiveStyle, Rectangle},
    text::Text,
};
use embedded_hal::{delay::DelayNs, pwm::SetDutyCycle};
use fugit::RateExtU32;
use hal::Clock;
use panic_halt as _;
use picosigner::{app::AppState, heap};
use picosigner_board::{
    CoordinatedSeedStore, Cst816d, Orientation, Rp2350FlashCoordinator, Rp2350NorFlash, Rp2350Trng,
    TouchPoint, TransactionalSeedStore, WaveshareLcd,
};
use picosigner_qr::{encode_descriptor_qr, DESCRIPTOR_QR_MODULES, DESCRIPTOR_QR_SIDE};
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
    Scan,
    Descriptor,
    Brightness,
    Back,
}

const HOME_SCAN_BUTTON: (i32, i32, u32, u32) = (16, 100, 288, 36);
const HOME_DESCRIPTOR_BUTTON: (i32, i32, u32, u32) = (16, 144, 288, 36);
const HOME_NEW_WALLET_BUTTON: (i32, i32, u32, u32) = (16, 188, 288, 32);
const HOME_NEW_WALLET_BAR_Y: i32 = 224;
// The descriptor QR occupies the display above this bottom return strip.
const DESCRIPTOR_BACK_BUTTON: (i32, i32, u32, u32) = (0, 228, 320, 12);
const BACKLIGHT_DUTY: [u16; 3] = [255, 100, 25];

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

    let mut display = WaveshareLcd::new(spi, dc, cs, rst);
    display.init(&mut timer, Orientation::Landscape);

    display.clear(Rgb565::BLACK).unwrap();
    let title = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);
    let accent = MonoTextStyle::new(&FONT_6X10, Rgb565::CYAN);
    Text::new("PicoSigner", Point::new(24, 88), title)
        .draw(&mut display)
        .unwrap();
    Text::new("Initializing secure wallet...", Point::new(24, 112), accent)
        .draw(&mut display)
        .unwrap();

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

    render_app(&mut display, &app, None, 0);
    let mut tracker = TouchTracker::new(GestureConfig::default());
    let mut was_touched = false;
    let mut last_touch = (0, 0);
    let mut hold_action = None;
    let mut rendered_hold_ticks = 0;
    let mut backlight_level = 0usize;
    loop {
        let point = touch.point().ok().flatten();
        if let Some(point) = point {
            if was_touched {
                last_touch = landscape_touch(point);
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
                    _ => None,
                };
                if hold_action.is_some() {
                    last_touch = landscape_touch(point);
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
    let background = if app.screen() == Screen::Descriptor {
        Rgb565::WHITE
    } else {
        Rgb565::BLACK
    };
    display.clear(background).unwrap();
    let text_color = if app.screen() == Screen::Descriptor {
        Rgb565::BLACK
    } else {
        Rgb565::WHITE
    };
    let title = MonoTextStyle::new(&FONT_6X10, text_color);
    let accent = MonoTextStyle::new(
        &FONT_6X10,
        if app.screen() == Screen::Descriptor {
            Rgb565::BLACK
        } else {
            Rgb565::CYAN
        },
    );
    let body = MonoTextStyle::new(
        &FONT_6X10,
        if app.screen() == Screen::Descriptor {
            Rgb565::BLACK
        } else {
            Rgb565::GREEN
        },
    );
    let heading = match app.screen() {
        Screen::Backup => "BACKUP REQUIRED",
        Screen::Home => "WALLET READY",
        Screen::Scan => "SCAN PSBT QR",
        Screen::Descriptor => "DESCRIPTOR EXPORT",
        Screen::Review => "REVIEW TRANSACTION",
        Screen::Sign => "HOLD TO SIGN",
        Screen::Signed => "SIGNED PSBT",
        Screen::Boot | Screen::Network | Screen::Seed | Screen::Recovery => "INITIALIZING",
    };
    if app.screen() != Screen::Descriptor {
        Text::new(heading, Point::new(16, 24), title)
            .draw(display)
            .unwrap();
    }

    match app.screen() {
        Screen::Backup => {
            Text::new("Write down these 12 words", Point::new(16, 48), accent)
                .draw(display)
                .unwrap();
            if let Ok(mut mnemonic) = app.mnemonic() {
                for (line, chunk) in mnemonic.as_bytes().chunks(42).enumerate() {
                    if let Ok(text) = core::str::from_utf8(chunk) {
                        Text::new(text, Point::new(16, 76 + line as i32 * 14), body)
                            .draw(display)
                            .unwrap();
                    }
                }
                mnemonic.zeroize();
            }
            Text::new("Hold to confirm backup", Point::new(16, 188), accent)
                .draw(display)
                .unwrap();
            draw_hold_bar(display, hold_ticks, 208);
        }
        Screen::Home => {
            Text::new("Wallet ready", Point::new(16, 76), accent)
                .draw(display)
                .unwrap();
            Text::new("Offline signing enabled", Point::new(16, 96), body)
                .draw(display)
                .unwrap();
            Rectangle::new(
                Point::new(HOME_SCAN_BUTTON.0, HOME_SCAN_BUTTON.1),
                Size::new(HOME_SCAN_BUTTON.2, HOME_SCAN_BUTTON.3),
            )
            .into_styled(PrimitiveStyle::with_fill(Rgb565::BLUE))
            .draw(display)
            .unwrap();
            Text::new("SCAN PSBT", Point::new(122, 124), title)
                .draw(display)
                .unwrap();
            Rectangle::new(
                Point::new(HOME_DESCRIPTOR_BUTTON.0, HOME_DESCRIPTOR_BUTTON.1),
                Size::new(HOME_DESCRIPTOR_BUTTON.2, HOME_DESCRIPTOR_BUTTON.3),
            )
            .into_styled(PrimitiveStyle::with_fill(Rgb565::new(0, 32, 0)))
            .draw(display)
            .unwrap();
            Text::new("EXPORT DESCRIPTOR", Point::new(92, 168), title)
                .draw(display)
                .unwrap();
            Rectangle::new(
                Point::new(HOME_NEW_WALLET_BUTTON.0, HOME_NEW_WALLET_BUTTON.1),
                Size::new(HOME_NEW_WALLET_BUTTON.2, HOME_NEW_WALLET_BUTTON.3),
            )
            .into_styled(PrimitiveStyle::with_fill(Rgb565::RED))
            .draw(display)
            .unwrap();
            Text::new("HOLD FOR NEW WALLET", Point::new(72, 210), title)
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
        Screen::Descriptor => {
            if let Ok(export) = app.export_descriptor() {
                draw_descriptor_qr(display, export.descriptor.as_bytes());
            } else {
                Text::new(
                    "DESCRIPTOR UNAVAILABLE",
                    Point::new(72, 120),
                    MonoTextStyle::new(&FONT_6X10, Rgb565::RED),
                )
                .draw(display)
                .unwrap();
            }
            Text::new(
                "TAP TO RETURN",
                Point::new(112, 238),
                MonoTextStyle::new(&FONT_6X10, Rgb565::BLACK),
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

fn landscape_touch(point: TouchPoint) -> (u16, u16) {
    // CST816D reports the panel's native 240x320 portrait axes. The LCD is
    // configured with MADCTL landscape mode: it swaps the axes and reverses
    // the resulting landscape Y direction.
    (point.y, 239u16.saturating_sub(point.x.min(239)))
}

fn home_action(point: TouchPoint) -> Option<HoldAction> {
    let (x, y) = landscape_touch(point);
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
    let (x, y) = landscape_touch(point);
    if in_button(x, y, DESCRIPTOR_BACK_BUTTON) {
        Some(HoldAction::Back)
    } else if y < DESCRIPTOR_BACK_BUTTON.1 as u16 {
        Some(HoldAction::Brightness)
    } else {
        None
    }
}

fn in_button(x: u16, y: u16, button: (i32, i32, u32, u32)) -> bool {
    let (left, top, width, height) = button;
    let right = left + width as i32;
    let bottom = top + height as i32;
    (x as i32) >= left && (x as i32) < right && (y as i32) >= top && (y as i32) < bottom
}

fn hold_bar_y(action: HoldAction) -> Option<i32> {
    match action {
        HoldAction::Backup => Some(208),
        HoldAction::Regenerate => Some(HOME_NEW_WALLET_BAR_Y),
        HoldAction::Scan | HoldAction::Descriptor | HoldAction::Brightness | HoldAction::Back => {
            None
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
    // the required four-module quiet zone, is 228 pixels high and fits the
    // 240-pixel display. This is the largest practical symbol for phone
    // cameras while retaining a clean quiet zone on every side.
    const SCALE: u32 = 4;
    const QUIET: u32 = 4;
    let symbol = (DESCRIPTOR_QR_SIDE as u32 + QUIET * 2) * SCALE;
    let left = (320 - symbol) / 2;
    let top = 0;
    let black = PrimitiveStyle::with_fill(Rgb565::BLACK);
    for y in 0..DESCRIPTOR_QR_SIDE {
        for x in 0..DESCRIPTOR_QR_SIDE {
            if modules[y * DESCRIPTOR_QR_SIDE + x] {
                Rectangle::new(
                    Point::new(
                        (left + (QUIET + x as u32) * SCALE) as i32,
                        (top + (QUIET + y as u32) * SCALE) as i32,
                    ),
                    Size::new(SCALE, SCALE),
                )
                .into_styled(black)
                .draw(display)
                .unwrap();
            }
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
    const WIDTH: u32 = 288;
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
    const WIDTH: u32 = 288;
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
    const WIDTH: u32 = 288;
    const HEIGHT: u32 = 14;
    Rectangle::new(Point::new(16, y), Size::new(WIDTH, HEIGHT))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
        .draw(display)
        .unwrap();
    draw_hold_bar_outline(display, y);
}
