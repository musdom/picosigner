//! Shared PicoSigner presentation and touch geometry.
//!
//! The RP2350 firmware and desktop emulator both draw through these routines,
//! so the simulator is a view of the real application rather than a mock UI.

extern crate alloc;

use core::convert::Infallible;

use embedded_graphics::{
    geometry::Size,
    pixelcolor::{Rgb565, RgbColor},
    prelude::{DrawTarget, Drawable, Point, Primitive},
    primitives::{PrimitiveStyle, Rectangle},
    text::{Alignment, Text},
};
use embedded_ttf::{FontTextStyle, FontTextStyleBuilder};
use picosigner_qr::{
    encode_descriptor_qr, encode_export_qr, DESCRIPTOR_QR_MODULES, DESCRIPTOR_QR_SIDE,
    EXPORT_QR_MODULES, EXPORT_QR_SIDE,
};
use picosigner_ui::Screen;
use picosigner_wallet::Network;
use rusttype::Font;
use zeroize::Zeroize;

use crate::app::AppState;

pub const DISPLAY_WIDTH: u32 = 240;
pub const DISPLAY_HEIGHT: u32 = 320;

pub const HOME_BACKUP_BUTTON: (i32, i32, u32, u32) = (16, 60, 208, 40);
pub const HOME_SCAN_BUTTON: (i32, i32, u32, u32) = (16, 112, 208, 40);
pub const HOME_DESCRIPTOR_BUTTON: (i32, i32, u32, u32) = (16, 164, 208, 40);
pub const HOME_NEW_WALLET_BUTTON: (i32, i32, u32, u32) = (16, 216, 208, 40);
pub const HOME_NEW_WALLET_BAR_Y: i32 = 270;
pub const DESCRIPTOR_BACK_BUTTON: (i32, i32, u32, u32) = (0, 296, 240, 24);
pub const REVIEW_BACK_BUTTON: (i32, i32, u32, u32) = (180, 0, 60, 32);
pub const REVIEW_SIGN_BUTTON: (i32, i32, u32, u32) = (16, 288, 208, 32);
pub const REVIEW_SIGN_BAR_Y: i32 = 304;
pub const SCAN_CANCEL_BUTTON: (u16, u16, u16, u16) = (174, 0, 66, 24);
pub const SCAN_ZOOM_BUTTON: (u16, u16, u16, u16) = (70, 280, 100, 40);
pub const SCAN_PROGRESS_BAR: (i32, i32, u32, u32) = (12, 263, 216, 11);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldAction {
    Backup,
    Regenerate,
    Sign,
    Scan,
    ShowBackup,
    Descriptor,
    Brightness,
    Back,
}

pub fn action_at(screen: Screen, x: u16, y: u16) -> Option<HoldAction> {
    match screen {
        Screen::Backup => Some(HoldAction::Backup),
        Screen::BackupWords if in_button(x, y, DESCRIPTOR_BACK_BUTTON) => Some(HoldAction::Back),
        Screen::Home if in_button(x, y, HOME_BACKUP_BUTTON) => Some(HoldAction::ShowBackup),
        Screen::Home if in_button(x, y, HOME_SCAN_BUTTON) => Some(HoldAction::Scan),
        Screen::Home if in_button(x, y, HOME_DESCRIPTOR_BUTTON) => Some(HoldAction::Descriptor),
        Screen::Home if in_button(x, y, HOME_NEW_WALLET_BUTTON) => Some(HoldAction::Regenerate),
        Screen::Descriptor if in_button(x, y, DESCRIPTOR_BACK_BUTTON) => Some(HoldAction::Back),
        Screen::Descriptor => Some(HoldAction::Brightness),
        Screen::Review if in_button(x, y, REVIEW_BACK_BUTTON) => Some(HoldAction::Back),
        Screen::Review if in_button(x, y, REVIEW_SIGN_BUTTON) => Some(HoldAction::Sign),
        Screen::Signed if y >= DESCRIPTOR_BACK_BUTTON.1 as u16 => Some(HoldAction::Back),
        _ => None,
    }
}

pub fn in_button(x: u16, y: u16, button: (i32, i32, u32, u32)) -> bool {
    let (left, top, width, height) = button;
    let right = left + width as i32;
    let bottom = top + height as i32;
    (x as i32) >= left && (x as i32) < right && (y as i32) >= top && (y as i32) < bottom
}

pub fn point_in_u16_button(x: u16, y: u16, button: (u16, u16, u16, u16)) -> bool {
    let (left, top, width, height) = button;
    x >= left && x < left.saturating_add(width) && y >= top && y < top.saturating_add(height)
}

pub fn hold_bar_y(action: HoldAction) -> Option<i32> {
    match action {
        HoldAction::Backup => Some(258),
        HoldAction::Regenerate => Some(HOME_NEW_WALLET_BAR_Y),
        HoldAction::Sign => Some(REVIEW_SIGN_BAR_Y),
        HoldAction::Scan
        | HoldAction::ShowBackup
        | HoldAction::Descriptor
        | HoldAction::Brightness
        | HoldAction::Back => None,
    }
}

const FIRA_CODE_REGULAR: &[u8] = include_bytes!("bin/picosigner/FiraCode-Medium.ttf");
const FONT_SIZE: u32 = 20;

pub fn text_point(x: i32, baseline_y: i32) -> Point {
    Point::new(x, baseline_y - FONT_SIZE as i32)
}

pub struct Fonts {
    fira_code_regular: Font<'static>,
}

impl Fonts {
    pub fn load() -> Self {
        Self {
            fira_code_regular: Font::try_from_bytes(FIRA_CODE_REGULAR)
                .expect("invalid Fira Code Regular font"),
        }
    }

    pub fn regular(&self, color: Rgb565) -> FontTextStyle<Rgb565> {
        self.regular_on(color, Rgb565::BLACK)
    }

    pub fn regular_on(&self, color: Rgb565, background: Rgb565) -> FontTextStyle<Rgb565> {
        FontTextStyleBuilder::new(self.fira_code_regular.clone())
            .font_size(FONT_SIZE)
            .text_color(color)
            .anti_aliasing_color(background)
            .build()
    }
}

pub fn render_app<D>(
    target: &mut D,
    fonts: &Fonts,
    app: &AppState,
    hold_action: Option<HoldAction>,
    hold_ticks: u16,
) where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    target.clear(Rgb565::BLACK).unwrap();
    let title = || fonts.regular(Rgb565::WHITE);
    let accent = || fonts.regular(Rgb565::CYAN);
    let body = || fonts.regular(Rgb565::GREEN);
    let heading = match app.screen() {
        Screen::Backup => "BACKUP REQUIRED",
        Screen::BackupWords => "WALLET BACKUP",
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
                        .draw(target)
                        .unwrap();
                }
            }
        } else {
            Text::new(heading, text_point(16, 24), title())
                .draw(target)
                .unwrap();
        }
    }

    match app.screen() {
        Screen::Backup => {
            Text::new("Write down these 12 words", text_point(16, 48), accent())
                .draw(target)
                .unwrap();
            draw_mnemonic(target, app, &body);
            Text::new("Hold to confirm backup", text_point(16, 238), accent())
                .draw(target)
                .unwrap();
            draw_hold_bar(target, hold_ticks, 258);
        }
        Screen::BackupWords => {
            Text::new("Keep these words private", text_point(16, 48), accent())
                .draw(target)
                .unwrap();
            draw_mnemonic(target, app, &body);
            Text::with_alignment(
                "TAP TO RETURN",
                text_point(120, 313),
                title(),
                Alignment::Center,
            )
            .draw(target)
            .unwrap();
        }
        Screen::Home => draw_home(target, fonts, hold_action, hold_ticks),
        Screen::Scan => {
            Text::new("Camera/BBQr receive next", text_point(16, 76), accent())
                .draw(target)
                .unwrap();
            Text::new("No signing without review", text_point(16, 102), body())
                .draw(target)
                .unwrap();
        }
        Screen::Review => draw_review(target, fonts, app, hold_action, hold_ticks),
        Screen::Sign => {
            Text::new("Signing reviewed PSBT...", text_point(16, 76), accent())
                .draw(target)
                .unwrap();
            Text::new(
                "Verifying transaction \ndigest",
                text_point(16, 102),
                body(),
            )
            .draw(target)
            .unwrap();
        }
        Screen::Signed => {
            Text::new("PSBT signed successfully", text_point(16, 76), accent())
                .draw(target)
                .unwrap();
            Text::new("Preparing animated BBQr...", text_point(16, 102), body())
                .draw(target)
                .unwrap();
        }
        Screen::Descriptor => {
            if let Ok(export) = app.export_descriptor() {
                draw_descriptor_qr(target, export.descriptor.as_bytes());
            } else {
                Text::with_alignment(
                    "DESCRIPTOR UNAVAILABLE",
                    text_point(120, 144),
                    fonts.regular(Rgb565::RED),
                    Alignment::Center,
                )
                .draw(target)
                .unwrap();
            }
            Text::with_alignment(
                "TAP TO RETURN",
                text_point(120, 313),
                title(),
                Alignment::Center,
            )
            .draw(target)
            .unwrap();
        }
        _ => {
            Text::new("State ready", text_point(16, 76), accent())
                .draw(target)
                .unwrap();
        }
    }
}

fn draw_home<D>(target: &mut D, fonts: &Fonts, hold_action: Option<HoldAction>, hold_ticks: u16)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    let descriptor_color = Rgb565::new(0, 32, 0);
    let buttons = [
        (
            HOME_BACKUP_BUTTON,
            Rgb565::new(24, 40, 0),
            "SHOW BACKUP",
            85,
        ),
        (HOME_SCAN_BUTTON, Rgb565::BLUE, "SCAN PSBT", 137),
        (
            HOME_DESCRIPTOR_BUTTON,
            descriptor_color,
            "EXPORT DESCRIPTOR",
            189,
        ),
        (
            HOME_NEW_WALLET_BUTTON,
            Rgb565::RED,
            "HOLD FOR NEW WALLET",
            241,
        ),
    ];
    for (bounds, color, label, baseline) in buttons {
        Rectangle::new(
            Point::new(bounds.0, bounds.1),
            Size::new(bounds.2, bounds.3),
        )
        .into_styled(PrimitiveStyle::with_fill(color))
        .draw(target)
        .unwrap();
        Text::with_alignment(
            label,
            text_point(120, baseline),
            fonts.regular_on(Rgb565::WHITE, color),
            Alignment::Center,
        )
        .draw(target)
        .unwrap();
    }
    if matches!(hold_action, Some(HoldAction::Regenerate)) {
        draw_hold_bar(target, hold_ticks, HOME_NEW_WALLET_BAR_Y);
    }
}

fn draw_mnemonic<D, F>(target: &mut D, app: &AppState, body: &F)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
    F: Fn() -> FontTextStyle<Rgb565>,
{
    if let Ok(mut mnemonic) = app.mnemonic() {
        for (line, chunk) in mnemonic.as_bytes().chunks(34).enumerate() {
            if let Ok(text) = core::str::from_utf8(chunk) {
                Text::new(text, text_point(16, 76 + line as i32 * 14), body())
                    .draw(target)
                    .unwrap();
            }
        }
        mnemonic.zeroize();
    }
}

fn draw_review<D>(
    target: &mut D,
    fonts: &Fonts,
    app: &AppState,
    hold_action: Option<HoldAction>,
    hold_ticks: u16,
) where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    let Some(review) = app.review() else { return };
    Text::new("BACK", text_point(196, 20), fonts.regular(Rgb565::WHITE))
        .draw(target)
        .unwrap();
    let mut y = 48;
    let mut recipients = 0usize;
    for output in review.outputs.iter().filter(|output| !output.is_change) {
        recipients += 1;
        let amount = alloc::format!("SEND {} sats", output.value_sat);
        Text::new(&amount, text_point(12, y), fonts.regular(Rgb565::CYAN))
            .draw(target)
            .unwrap();
        y += 14;
        if let Some(address) = &output.address {
            for chunk in address.as_bytes().chunks(29) {
                if y > 242 {
                    break;
                }
                if let Ok(line) = core::str::from_utf8(chunk) {
                    Text::new(line, text_point(12, y), fonts.regular(Rgb565::WHITE))
                        .draw(target)
                        .unwrap();
                }
                y += 12;
            }
        } else {
            Text::new(
                "UNKNOWN OUTPUT SCRIPT",
                text_point(12, y),
                fonts.regular(Rgb565::WHITE),
            )
            .draw(target)
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
    Text::new(&summary, text_point(12, 266), fonts.regular(Rgb565::GREEN))
        .draw(target)
        .unwrap();
    let fee = match review.fee_sat {
        Some(value) => alloc::format!("Fee: {value} sats"),
        None => alloc::format!("Fee: unavailable"),
    };
    Text::new(&fee, text_point(12, 282), fonts.regular(Rgb565::GREEN))
        .draw(target)
        .unwrap();
    Text::with_alignment(
        "HOLD TO CONFIRM & SIGN",
        text_point(120, 301),
        fonts.regular(Rgb565::WHITE),
        Alignment::Center,
    )
    .draw(target)
    .unwrap();
    draw_hold_bar(
        target,
        if matches!(hold_action, Some(HoldAction::Sign)) {
            hold_ticks
        } else {
            0
        },
        REVIEW_SIGN_BAR_Y,
    );
}

pub fn draw_hold_bar<D>(display: &mut D, hold_ticks: u16, y: i32)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    draw_hold_bar_outline(display, y);
    draw_hold_bar_progress(display, 0, hold_ticks, y);
}

fn draw_hold_bar_outline<D>(display: &mut D, y: i32)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    Rectangle::new(Point::new(16, y), Size::new(208, 14))
        .into_styled(PrimitiveStyle::with_stroke(Rgb565::CYAN, 1))
        .draw(display)
        .unwrap();
}

pub fn draw_hold_bar_progress<D>(display: &mut D, previous_ticks: u16, current_ticks: u16, y: i32)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    let previous = 206 * u32::from(previous_ticks.min(30)) / 30;
    let current = 206 * u32::from(current_ticks.min(30)) / 30;
    if current > previous {
        Rectangle::new(
            Point::new(17 + previous as i32, y + 1),
            Size::new(current - previous, 12),
        )
        .into_styled(PrimitiveStyle::with_fill(Rgb565::GREEN))
        .draw(display)
        .unwrap();
    }
}

pub fn reset_hold_bar<D>(display: &mut D, y: i32)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    Rectangle::new(Point::new(16, y), Size::new(208, 14))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
        .draw(display)
        .unwrap();
    draw_hold_bar_outline(display, y);
}

fn network_name(network: Network) -> &'static str {
    match network {
        Network::Mainnet => "mainnet",
        Network::Testnet4 => "testnet4",
        Network::Signet => "signet",
    }
}

pub fn draw_descriptor_qr<D>(display: &mut D, descriptor: &[u8])
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    let mut modules = [false; DESCRIPTOR_QR_MODULES];
    if encode_descriptor_qr(descriptor, &mut modules).is_ok() {
        draw_qr_modules_on_white(display, &modules, DESCRIPTOR_QR_SIDE, 4, 4, 6, 28);
    }
}

pub fn draw_signed_psbt_qr<D>(
    target: &mut D,
    fonts: &Fonts,
    frame: &[u8],
    frame_index: usize,
    frame_count: usize,
) where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    target.clear(Rgb565::BLACK).unwrap();
    let mut modules = [false; EXPORT_QR_MODULES];
    if encode_export_qr(frame, &mut modules).is_err() {
        return;
    }
    let symbol = (EXPORT_QR_SIDE as u32 + 8) * 5;
    let left = (DISPLAY_WIDTH - symbol) / 2;
    draw_qr_modules_on_white(target, &modules, EXPORT_QR_SIDE, 5, 4, left, 4);
    Text::with_alignment(
        "SIGNED PSBT - BBQr",
        text_point(120, 250),
        fonts.regular(Rgb565::CYAN),
        Alignment::Center,
    )
    .draw(target)
    .unwrap();
    let part = alloc::format!("PART {}/{}", frame_index + 1, frame_count);
    Text::with_alignment(
        &part,
        text_point(120, 270),
        fonts.regular(Rgb565::WHITE),
        Alignment::Center,
    )
    .draw(target)
    .unwrap();
    Text::with_alignment(
        "TAP TO RETURN",
        text_point(120, 313),
        fonts.regular(Rgb565::WHITE),
        Alignment::Center,
    )
    .draw(target)
    .unwrap();
}

pub fn render_message<D>(target: &mut D, fonts: &Fonts, title: &str, lines: &[&str])
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    target.clear(Rgb565::BLACK).unwrap();
    Text::new(title, text_point(16, 24), fonts.regular(Rgb565::RED))
        .draw(target)
        .unwrap();
    for (index, line) in lines.iter().enumerate() {
        Text::new(
            line,
            text_point(16, 72 + index as i32 * 22),
            fonts.regular(Rgb565::WHITE),
        )
        .draw(target)
        .unwrap();
    }
    Text::with_alignment(
        "TAP TO RETURN",
        text_point(120, 313),
        fonts.regular(Rgb565::CYAN),
        Alignment::Center,
    )
    .draw(target)
    .unwrap();
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_use_shared_display_coordinates() {
        assert_eq!(
            action_at(Screen::Home, 120, 80),
            Some(HoldAction::ShowBackup)
        );
        assert_eq!(action_at(Screen::Home, 120, 132), Some(HoldAction::Scan));
        assert_eq!(
            action_at(Screen::Home, 120, 236),
            Some(HoldAction::Regenerate)
        );
        assert_eq!(action_at(Screen::Review, 200, 16), Some(HoldAction::Back));
        assert_eq!(action_at(Screen::Review, 120, 300), Some(HoldAction::Sign));
        assert_eq!(
            action_at(Screen::BackupWords, 120, 308),
            Some(HoldAction::Back)
        );
    }
}
