//! Shared PicoSigner presentation and touch geometry.
//!
//! The RP2350 firmware and desktop emulator both draw through these routines,
//! so the simulator is a view of the real application rather than a mock UI.

extern crate alloc;

use core::convert::Infallible;

use embedded_graphics::{
    geometry::Size,
    pixelcolor::Rgb565,
    prelude::{DrawTarget, Drawable, Point, Primitive},
    primitives::{Circle, Line, Polyline, PrimitiveStyle, Rectangle},
    text::{Alignment, Text},
};
use embedded_ttf::{FontTextStyle, FontTextStyleBuilder};
use picosigner_qr::{
    encode_compact_seed_qr, encode_descriptor_qr, encode_export_qr, DESCRIPTOR_QR_MODULES,
    DESCRIPTOR_QR_SIDE, EXPORT_QR_MODULES, EXPORT_QR_SIDE,
};
use picosigner_ui::Screen;
use picosigner_wallet::Network;
use rusttype::Font;
use zeroize::Zeroize;

use crate::{app::AppState, palette};

pub const DISPLAY_WIDTH: u32 = 240;
pub const DISPLAY_HEIGHT: u32 = 320;

pub const HOME_SCAN_BUTTON: (i32, i32, u32, u32) = (16, 60, 208, 40);
pub const HOME_BACKUP_BUTTON: (i32, i32, u32, u32) = (16, 112, 208, 40);
pub const HOME_STORAGE_BUTTON: (i32, i32, u32, u32) = (16, 164, 208, 40);
pub const HOME_LOGOUT_BUTTON: (i32, i32, u32, u32) = (16, 216, 208, 40);
pub const HOME_NEW_WALLET_BUTTON: (i32, i32, u32, u32) = HOME_LOGOUT_BUTTON;
pub const HOME_NEW_WALLET_BAR_Y: i32 = 270;
pub const DESCRIPTOR_BACK_BUTTON: (i32, i32, u32, u32) = (0, 296, 240, 24);
pub const REVIEW_BACK_BUTTON: (i32, i32, u32, u32) = (180, 0, 60, 32);
pub const REVIEW_SIGN_BUTTON: (i32, i32, u32, u32) = (16, 288, 208, 32);
pub const REVIEW_SIGN_BAR_Y: i32 = 304;
pub const SCAN_CANCEL_BUTTON: (u16, u16, u16, u16) = (0, 280, 60, 40);
pub const SCAN_ZOOM_BUTTON: (u16, u16, u16, u16) = (120, 280, 120, 40);
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
    Load,
    Wallet(usize),
    Import,
    Create,
    ScanSeedQr,
    EnterMnemonic,
    MnemonicWords(u8),
    Network(Network),
    Derivation,
    BackupMenu,
    MnemonicBackup,
    SeedQrBackup,
    Logout,
    SaveWallet,
    DeleteWallet,
    Suggestion(usize),
    Key(u8),
    Backspace,
    Cancel,
    BackupPage,
}

pub fn action_at(screen: Screen, x: u16, y: u16) -> Option<HoldAction> {
    match screen {
        Screen::LoadWallet if in_button(x, y, (16, 60, 208, 40)) => Some(HoldAction::Load),
        Screen::LoadWallet if in_button(x, y, (16, 164, 208, 40)) => Some(HoldAction::Import),
        Screen::LoadWallet if in_button(x, y, (16, 216, 208, 40)) => Some(HoldAction::Create),
        Screen::WalletList if y >= 32 && y < 272 => {
            Some(HoldAction::Wallet(((y - 32) / 30) as usize))
        }
        Screen::WalletList => Some(HoldAction::Back),
        Screen::ImportWallet if in_button(x, y, (16, 70, 208, 44)) => Some(HoldAction::ScanSeedQr),
        Screen::ImportWallet if in_button(x, y, (16, 130, 208, 44)) => {
            Some(HoldAction::EnterMnemonic)
        }
        Screen::ImportWallet => Some(HoldAction::Cancel),
        Screen::MnemonicLength if in_button(x, y, (16, 80, 208, 44)) => {
            Some(HoldAction::MnemonicWords(12))
        }
        Screen::MnemonicLength if in_button(x, y, (16, 140, 208, 44)) => {
            Some(HoldAction::MnemonicWords(24))
        }
        Screen::MnemonicLength => Some(HoldAction::Cancel),
        Screen::MnemonicEntry if x < 120 && y >= 50 && y < 154 => {
            let relative_y = y - 50;
            let row = relative_y / 36;
            (row < 3 && relative_y % 36 < 32).then_some(HoldAction::Suggestion(usize::from(row)))
        }
        Screen::MnemonicEntry if y >= 160 => {
            let column = usize::from(x / 40);
            let row = usize::from((y - 160) / 32);
            let index = row * 6 + column;
            if index < 26 {
                Some(HoldAction::Key(b'a' + index as u8))
            } else if index == 26 || index == 27 {
                Some(HoldAction::Backspace)
            } else {
                Some(HoldAction::Cancel)
            }
        }
        Screen::MnemonicEntry => None,
        Screen::Recovery => Some(HoldAction::Cancel),
        Screen::Network if in_button(x, y, (16, 60, 208, 40)) => {
            Some(HoldAction::Network(Network::Mainnet))
        }
        Screen::Network if in_button(x, y, (16, 112, 208, 40)) => {
            Some(HoldAction::Network(Network::Testnet4))
        }
        Screen::Network if in_button(x, y, (16, 164, 208, 40)) => {
            Some(HoldAction::Network(Network::Signet))
        }
        Screen::Network => Some(HoldAction::Cancel),
        Screen::Derivation if in_button(x, y, (16, 92, 208, 70)) => Some(HoldAction::Derivation),
        Screen::Derivation => Some(HoldAction::Cancel),
        Screen::Backup => Some(HoldAction::Backup),
        Screen::BackupMenu if in_button(x, y, (16, 80, 208, 44)) => {
            Some(HoldAction::MnemonicBackup)
        }
        Screen::BackupMenu if in_button(x, y, (16, 140, 208, 44)) => Some(HoldAction::SeedQrBackup),
        Screen::BackupMenu if in_button(x, y, (16, 200, 208, 44)) => Some(HoldAction::Descriptor),
        Screen::BackupMenu => Some(HoldAction::Back),
        Screen::BackupWords => Some(HoldAction::BackupPage),
        Screen::BackupSeedQr if in_button(x, y, DESCRIPTOR_BACK_BUTTON) => Some(HoldAction::Back),
        Screen::BackupSeedQr => Some(HoldAction::Brightness),
        Screen::Home if in_button(x, y, HOME_SCAN_BUTTON) => Some(HoldAction::Scan),
        Screen::Home if in_button(x, y, HOME_BACKUP_BUTTON) => Some(HoldAction::BackupMenu),
        Screen::Home if in_button(x, y, HOME_STORAGE_BUTTON) => Some(HoldAction::SaveWallet),
        Screen::Home if in_button(x, y, HOME_LOGOUT_BUTTON) => Some(HoldAction::Logout),
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

pub fn action_at_for_app(app: &AppState, x: u16, y: u16) -> Option<HoldAction> {
    match action_at(app.screen(), x, y) {
        Some(HoldAction::SaveWallet) if app.wallet_is_saved() => Some(HoldAction::DeleteWallet),
        action => action,
    }
}

pub fn hold_bar_y(action: HoldAction) -> Option<i32> {
    match action {
        HoldAction::Backup => Some(258),
        HoldAction::Regenerate => Some(HOME_NEW_WALLET_BAR_Y),
        HoldAction::DeleteWallet => Some(HOME_NEW_WALLET_BAR_Y),
        HoldAction::Sign => Some(REVIEW_SIGN_BAR_Y),
        HoldAction::Scan
        | HoldAction::ShowBackup
        | HoldAction::Descriptor
        | HoldAction::Brightness
        | HoldAction::Back
        | HoldAction::Load
        | HoldAction::Wallet(_)
        | HoldAction::Import
        | HoldAction::Create
        | HoldAction::ScanSeedQr
        | HoldAction::EnterMnemonic
        | HoldAction::MnemonicWords(_)
        | HoldAction::Network(_)
        | HoldAction::Derivation
        | HoldAction::BackupMenu
        | HoldAction::MnemonicBackup
        | HoldAction::SeedQrBackup
        | HoldAction::Logout
        | HoldAction::SaveWallet
        | HoldAction::Suggestion(_)
        | HoldAction::Key(_)
        | HoldAction::Backspace
        | HoldAction::Cancel
        | HoldAction::BackupPage => None,
    }
}

const FIRA_CODE_REGULAR: &[u8] = include_bytes!("../assets/FiraCode-Medium.ttf");
const FONT_SIZE: u32 = 20;
const MNEMONIC_FONT_SIZE: u32 = 20;

pub fn text_point(x: i32, baseline_y: i32) -> Point {
    Point::new(x, baseline_y - FONT_SIZE as i32)
}

fn small_text_point(x: i32, baseline_y: i32) -> Point {
    Point::new(x, baseline_y - 14)
}

/// Top edge of a `FONT_SIZE`-tall line of text centred inside `bounds`.
///
/// `Text` is positioned by the top of its line box, so vertical centring is
/// the only correct way to place a button label: the old per-call baseline
/// constants drifted with every change to a button's height.
fn centered_text_top(bounds: (i32, i32, u32, u32)) -> i32 {
    bounds.1 + (bounds.3 as i32 - FONT_SIZE as i32) / 2
}

/// Outline artwork adapted from the 24px SVGs at bitcoinicons.com.
#[derive(Clone, Copy)]
pub enum BitcoinIcon {
    Wallet,
    Scan,
    Receive,
    Plus,
    Mnemonic,
    QrCode,
    Export,
    Globe,
    SdCard,
    Trash,
    Exit,
    Cross,
    Search,
}

/// Draw a Bitcoin Icons outline glyph at its native 24px size.
pub fn draw_bitcoin_icon<D>(target: &mut D, icon: BitcoinIcon, origin: Point, color: Rgb565)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    let p = |x, y| origin + Point::new(x, y);
    let stroke = PrimitiveStyle::with_stroke(color, 2);
    let line = |target: &mut D, a: (i32, i32), b: (i32, i32)| {
        Line::new(p(a.0, a.1), p(b.0, b.1))
            .into_styled(stroke)
            .draw(target)
            .unwrap();
    };
    let rect = |target: &mut D, x, y, w, h| {
        Rectangle::new(p(x, y), Size::new(w, h))
            .into_styled(stroke)
            .draw(target)
            .unwrap();
    };
    match icon {
        BitcoinIcon::Wallet => {
            rect(target, 4, 6, 13, 12);
            line(target, (16, 7), (20, 7));
            line(target, (20, 7), (20, 17));
            line(target, (20, 17), (16, 17));
            Circle::new(p(7, 10), 4)
                .into_styled(stroke)
                .draw(target)
                .unwrap();
        }
        BitcoinIcon::Scan => {
            for (a, b) in [
                ((5, 9), (5, 5)),
                ((5, 5), (9, 5)),
                ((15, 5), (19, 5)),
                ((19, 5), (19, 9)),
                ((5, 15), (5, 19)),
                ((5, 19), (9, 19)),
                ((15, 19), (19, 19)),
                ((19, 19), (19, 15)),
            ] {
                line(target, a, b);
            }
        }
        BitcoinIcon::Receive => {
            line(target, (5, 16), (5, 19));
            line(target, (5, 19), (19, 19));
            line(target, (19, 19), (19, 16));
            line(target, (12, 5), (12, 15));
            line(target, (8, 11), (12, 15));
            line(target, (12, 15), (16, 11));
        }
        BitcoinIcon::Plus => {
            line(target, (12, 4), (12, 20));
            line(target, (4, 12), (20, 12));
        }
        BitcoinIcon::Mnemonic => {
            rect(target, 5, 5, 14, 14);
            for y in [8, 11, 14, 17] {
                for x in [8, 12, 16] {
                    line(target, (x, y), (x + 1, y));
                }
            }
        }
        BitcoinIcon::QrCode => {
            rect(target, 5, 5, 5, 5);
            rect(target, 14, 5, 5, 5);
            rect(target, 5, 14, 5, 5);
            for (x, y) in [
                (12, 6),
                (12, 10),
                (12, 14),
                (12, 18),
                (15, 13),
                (18, 13),
                (15, 17),
                (18, 18),
            ] {
                Rectangle::new(p(x, y), Size::new(2, 2))
                    .into_styled(PrimitiveStyle::with_fill(color))
                    .draw(target)
                    .unwrap();
            }
        }
        BitcoinIcon::Export => {
            let points = [
                p(12, 7),
                p(7, 7),
                p(5, 9),
                p(5, 17),
                p(7, 19),
                p(15, 19),
                p(17, 17),
                p(17, 12),
            ];
            Polyline::new(&points)
                .into_styled(stroke)
                .draw(target)
                .unwrap();
            line(target, (12, 12), (19, 5));
            line(target, (14, 5), (19, 5));
            line(target, (19, 5), (19, 10));
        }
        BitcoinIcon::Globe => {
            Circle::new(p(4, 4), 16)
                .into_styled(stroke)
                .draw(target)
                .unwrap();
            line(target, (4, 12), (20, 12));
            rect(target, 9, 4, 7, 16);
        }
        BitcoinIcon::SdCard => {
            let points = [
                p(6, 9),
                p(10, 5),
                p(17, 5),
                p(18, 6),
                p(18, 19),
                p(7, 19),
                p(6, 18),
                p(6, 9),
            ];
            Polyline::new(&points)
                .into_styled(stroke)
                .draw(target)
                .unwrap();
            for x in [11, 14, 17] {
                line(target, (x, 7), (x, 10));
            }
        }
        BitcoinIcon::Trash => {
            line(target, (6, 7), (18, 7));
            line(target, (8, 7), (9, 20));
            line(target, (9, 20), (16, 20));
            line(target, (16, 20), (17, 7));
            line(target, (9, 7), (9, 5));
            line(target, (9, 5), (15, 5));
            line(target, (15, 5), (15, 7));
            line(target, (11, 10), (11, 17));
            line(target, (14, 10), (14, 17));
        }
        BitcoinIcon::Exit => {
            line(target, (13, 7), (13, 4));
            line(target, (13, 4), (4, 4));
            line(target, (4, 4), (4, 20));
            line(target, (4, 20), (13, 20));
            line(target, (13, 20), (13, 17));
            line(target, (11, 12), (20, 12));
            line(target, (16, 8), (20, 12));
            line(target, (20, 12), (16, 16));
        }
        BitcoinIcon::Cross => {
            line(target, (6, 6), (18, 18));
            line(target, (18, 6), (6, 18));
        }
        BitcoinIcon::Search => {
            Circle::new(p(5, 5), 12)
                .into_styled(stroke)
                .draw(target)
                .unwrap();
            line(target, (15, 15), (20, 20));
        }
    }
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
        self.regular_on(color, palette::BACKGROUND)
    }

    pub fn regular_on(&self, color: Rgb565, background: Rgb565) -> FontTextStyle<Rgb565> {
        FontTextStyleBuilder::new(self.fira_code_regular.clone())
            .font_size(FONT_SIZE)
            .text_color(color)
            .anti_aliasing_color(background)
            .build()
    }

    pub fn mnemonic(&self, color: Rgb565) -> FontTextStyle<Rgb565> {
        FontTextStyleBuilder::new(self.fira_code_regular.clone())
            .font_size(MNEMONIC_FONT_SIZE)
            .text_color(color)
            .anti_aliasing_color(palette::BACKGROUND)
            .build()
    }

    pub fn small(&self, color: Rgb565) -> FontTextStyle<Rgb565> {
        self.small_on(color, palette::BACKGROUND)
    }

    pub fn small_on(&self, color: Rgb565, background: Rgb565) -> FontTextStyle<Rgb565> {
        FontTextStyleBuilder::new(self.fira_code_regular.clone())
            .font_size(14)
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
    target.clear(palette::BACKGROUND).unwrap();
    let title = || fonts.regular(palette::TEXT);
    let accent = || fonts.regular(palette::TEXT);
    let body = || fonts.regular(palette::TEXT_MUTED);
    let heading = match app.screen() {
        Screen::LoadWallet => "PicoSigner",
        Screen::WalletList => "Load wallet",
        Screen::ImportWallet => "Import wallet",
        Screen::SeedQrScan => "Scan SeedQR",
        Screen::MnemonicLength => "Mnemonic length",
        Screen::MnemonicEntry => "",
        Screen::Network => "Choose network",
        Screen::Derivation => "Derivation",
        Screen::Backup => "Backup required",
        Screen::BackupMenu => "Show backup",
        Screen::BackupWords => "Wallet backup",
        Screen::BackupSeedQr => "Compact SeedQR",
        Screen::Home => "",
        Screen::Scan => "Scan PSBT QR",
        Screen::Descriptor => "Descriptor export",
        Screen::Review => "Review transaction",
        Screen::Sign => "Hold to sign",
        Screen::Signed => "Signed PSBT",
        Screen::Recovery => "Wallet error",
        Screen::Boot | Screen::Seed => "Initializing",
    };
    if !matches!(app.screen(), Screen::Descriptor | Screen::MnemonicEntry) {
        if app.screen() == Screen::Home {
            if let Some(wallet) = app.wallet() {
                if let Some(fingerprint) = wallet.master_fingerprint() {
                    draw_wallet_identity(target, fonts, 8, 1, fingerprint, wallet.network());
                }
            }
        } else {
            Text::new(heading, text_point(16, 24), title())
                .draw(target)
                .unwrap();
        }
    }

    match app.screen() {
        Screen::LoadWallet => draw_load_wallet(target, fonts, app),
        Screen::WalletList => draw_wallet_list(target, fonts, app),
        Screen::ImportWallet => {
            draw_menu_item(
                target,
                fonts,
                (16, 70, 208, 44),
                BitcoinIcon::Scan,
                "Scan SeedQR",
            );
            draw_menu_item(
                target,
                fonts,
                (16, 130, 208, 44),
                BitcoinIcon::Mnemonic,
                "Enter mnemonic",
            );
            Text::with_alignment(
                "Tap below to cancel",
                text_point(120, 306),
                title(),
                Alignment::Center,
            )
            .draw(target)
            .unwrap();
        }
        Screen::MnemonicLength => {
            draw_menu_item(
                target,
                fonts,
                (16, 80, 208, 44),
                BitcoinIcon::Mnemonic,
                "12 words",
            );
            draw_menu_item(
                target,
                fonts,
                (16, 140, 208, 44),
                BitcoinIcon::Mnemonic,
                "24 words",
            );
            Text::with_alignment("Cancel", text_point(120, 306), title(), Alignment::Center)
                .draw(target)
                .unwrap();
        }
        Screen::MnemonicEntry => draw_mnemonic_keyboard(target, fonts, app),
        Screen::Network => {
            draw_menu_item(
                target,
                fonts,
                (16, 60, 208, 40),
                BitcoinIcon::Globe,
                "Mainnet",
            );
            draw_menu_item(
                target,
                fonts,
                (16, 112, 208, 40),
                BitcoinIcon::Globe,
                "Testnet",
            );
            draw_menu_item(
                target,
                fonts,
                (16, 164, 208, 40),
                BitcoinIcon::Globe,
                "Signet",
            );
            Text::with_alignment("Cancel", text_point(120, 306), title(), Alignment::Center)
                .draw(target)
                .unwrap();
        }
        Screen::Derivation => {
            // Two stacked lines, so centre the pair rather than each line.
            const DERIVATION_BUTTON: (i32, i32, u32, u32) = (16, 92, 208, 70);
            draw_menu_item(
                target,
                fonts,
                DERIVATION_BUTTON,
                BitcoinIcon::Wallet,
                "BIP86 Taproot",
            );
            Text::new(
                if matches!(app.selected_network(), Network::Mainnet) {
                    "m/86'/0'/0'"
                } else {
                    "m/86'/1'/0'"
                },
                text_point(34, 200),
                body(),
            )
            .draw(target)
            .unwrap();
            Text::with_alignment("Cancel", text_point(120, 306), title(), Alignment::Center)
                .draw(target)
                .unwrap();
        }
        Screen::Backup => {
            Text::new("Write down these 12 words", text_point(16, 48), accent())
                .draw(target)
                .unwrap();
            draw_mnemonic(target, fonts, app, 0);
            Text::new("Hold to confirm backup", text_point(16, 238), accent())
                .draw(target)
                .unwrap();
            draw_hold_bar(target, hold_ticks, 258);
            // Keep the security context above the mnemonic visually explicit.
            Text::new("Backup required", text_point(16, 24), title())
                .draw(target)
                .unwrap();
        }
        Screen::BackupMenu => {
            draw_menu_item(
                target,
                fonts,
                (16, 80, 208, 44),
                BitcoinIcon::Mnemonic,
                "Show mnemonic",
            );
            draw_menu_item(
                target,
                fonts,
                (16, 140, 208, 44),
                BitcoinIcon::QrCode,
                "Compact SeedQR",
            );
            draw_menu_item(
                target,
                fonts,
                (16, 200, 208, 44),
                BitcoinIcon::Export,
                "Export descriptor",
            );
            Text::with_alignment("Back", text_point(120, 306), title(), Alignment::Center)
                .draw(target)
                .unwrap();
        }
        Screen::BackupWords => {
            Text::new("Keep these words private", text_point(16, 48), accent())
                .draw(target)
                .unwrap();
            draw_mnemonic(target, fonts, app, app.backup_page());
            Text::with_alignment(
                "Tap to return",
                text_point(120, 313),
                title(),
                Alignment::Center,
            )
            .draw(target)
            .unwrap();
            Text::new("Wallet backup", text_point(16, 24), title())
                .draw(target)
                .unwrap();
        }
        Screen::BackupSeedQr => draw_compact_seed_qr(target, fonts, app),
        Screen::Recovery => {
            Text::with_alignment(
                app.notice().unwrap_or("Wallet operation failed"),
                text_point(120, 130),
                fonts.regular(palette::DANGER_TEXT),
                Alignment::Center,
            )
            .draw(target)
            .unwrap();
            Text::with_alignment(
                "Tap to return",
                text_point(120, 313),
                title(),
                Alignment::Center,
            )
            .draw(target)
            .unwrap();
        }
        Screen::Home => draw_home(target, fonts, app, hold_action, hold_ticks),
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
                    "Descriptor unavailable",
                    text_point(120, 144),
                    fonts.regular(palette::DANGER_TEXT),
                    Alignment::Center,
                )
                .draw(target)
                .unwrap();
            }
            Text::with_alignment(
                "Tap to return",
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

/// Draw a menu action as a Bitcoin icon followed by white text on black.
fn draw_menu_item<D>(
    target: &mut D,
    fonts: &Fonts,
    bounds: (i32, i32, u32, u32),
    icon: BitcoinIcon,
    label: &str,
) where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    let icon_top = bounds.1 + (bounds.3 as i32 - 24) / 2;
    draw_bitcoin_icon(
        target,
        icon,
        Point::new(bounds.0 + 4, icon_top),
        palette::TEXT,
    );
    Text::new(
        label,
        Point::new(bounds.0 + 40, centered_text_top(bounds)),
        fonts.regular(palette::TEXT),
    )
    .draw(target)
    .unwrap();
}

/// Draw a filled keyboard key with its label centred inside `bounds`.
fn draw_button<D>(
    target: &mut D,
    fonts: &Fonts,
    bounds: (i32, i32, u32, u32),
    color: Rgb565,
    label: &str,
) where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    Rectangle::new(
        Point::new(bounds.0, bounds.1),
        Size::new(bounds.2, bounds.3),
    )
    .into_styled(PrimitiveStyle::with_fill(color))
    .draw(target)
    .unwrap();
    Text::with_alignment(
        label,
        Point::new(bounds.0 + bounds.2 as i32 / 2, centered_text_top(bounds)),
        fonts.regular_on(palette::TEXT, color),
        Alignment::Center,
    )
    .draw(target)
    .unwrap();
}

fn fingerprint_text(fingerprint: [u8; 4]) -> alloc::string::String {
    alloc::format!(
        "{:02x}{:02x}{:02x}{:02x}",
        fingerprint[0],
        fingerprint[1],
        fingerprint[2],
        fingerprint[3]
    )
}

fn draw_wallet_identity<D>(
    target: &mut D,
    fonts: &Fonts,
    left: i32,
    top: i32,
    fingerprint: [u8; 4],
    network: Network,
) where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    draw_bitcoin_icon(
        target,
        BitcoinIcon::Wallet,
        Point::new(left, top),
        palette::TEXT,
    );
    let fingerprint = fingerprint_text(fingerprint);
    Text::new(
        &fingerprint,
        Point::new(left + 32, top + 2),
        fonts.regular(palette::TEXT),
    )
    .draw(target)
    .unwrap();

    let (chip_color, chip_text, text_color) = match network {
        Network::Mainnet => (palette::MAINNET, "Mainnet", palette::TEXT),
        Network::Signet => (palette::SIGNET, "Signet", palette::TEXT),
        Network::Testnet4 => (palette::TESTNET, "Testnet", palette::TEXT),
    };
    let chip = (left + 150, top + 3, 74, 18);
    Rectangle::new(Point::new(chip.0, chip.1), Size::new(chip.2, chip.3))
        .into_styled(PrimitiveStyle::with_fill(chip_color))
        .draw(target)
        .unwrap();
    Text::with_alignment(
        chip_text,
        Point::new(chip.0 + chip.2 as i32 / 2, chip.1 + 2),
        fonts.small_on(text_color, chip_color),
        Alignment::Center,
    )
    .draw(target)
    .unwrap();
}

fn draw_load_wallet<D>(target: &mut D, fonts: &Fonts, app: &AppState)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    if !app.wallets().is_empty() {
        draw_menu_item(
            target,
            fonts,
            (16, 60, 208, 40),
            BitcoinIcon::Wallet,
            "Load wallet",
        );
    }
    draw_menu_item(
        target,
        fonts,
        (16, 164, 208, 40),
        BitcoinIcon::Receive,
        "Import wallet",
    );
    draw_menu_item(
        target,
        fonts,
        (16, 216, 208, 40),
        BitcoinIcon::Plus,
        "Create wallet",
    );
}

fn draw_wallet_list<D>(target: &mut D, fonts: &Fonts, app: &AppState)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    for (index, wallet) in app.wallets().iter().enumerate() {
        let top = 32 + index as i32 * 30;
        let network = match wallet.network {
            picosigner_board::Network::Mainnet => Network::Mainnet,
            picosigner_board::Network::Testnet4 => Network::Testnet4,
            picosigner_board::Network::Signet => Network::Signet,
        };
        draw_wallet_identity(
            target,
            fonts,
            8,
            top + 2,
            wallet.fingerprint.unwrap_or([0; 4]),
            network,
        );
    }
    Text::with_alignment(
        "Back",
        text_point(120, 313),
        fonts.regular(palette::TEXT),
        Alignment::Center,
    )
    .draw(target)
    .unwrap();
}

fn draw_mnemonic_keyboard<D>(target: &mut D, fonts: &Fonts, app: &AppState)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    let status = alloc::format!(
        "WORD {}/{}",
        app.mnemonic_entered() + 1,
        app.mnemonic_target()
    );
    Text::new(
        &status,
        text_point(8, 24),
        fonts.regular(if app.mnemonic_error() {
            palette::DANGER_TEXT
        } else {
            palette::TEXT
        }),
    )
    .draw(target)
    .unwrap();
    let prefix = alloc::format!("> {}", app.mnemonic_prefix());
    Text::new(
        &prefix,
        small_text_point(8, 44),
        fonts.small(palette::TEXT_MUTED),
    )
    .draw(target)
    .unwrap();
    for (index, word) in app.mnemonic_suggestions().iter().take(3).enumerate() {
        let top = 50 + index as i32 * 36;
        draw_button(target, fonts, (4, top, 116, 32), palette::PRIMARY, word);
    }
    Text::new(
        "Recent",
        small_text_point(148, 44),
        fonts.small(palette::TEXT_MUTED),
    )
    .draw(target)
    .unwrap();
    for (row, entry) in app
        .recent_mnemonic_words()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let label = alloc::format!("{:>2}. {}", entry.0, entry.1);
        Text::new(
            &label,
            small_text_point(126, 66 + row as i32 * 22),
            fonts.small(palette::TEXT),
        )
        .draw(target)
        .unwrap();
    }
    for index in 0..26usize {
        let column = index % 6;
        let row = index / 6;
        let left = column as i32 * 40;
        let top = 160 + row as i32 * 32;
        let label = alloc::format!("{}", char::from(b'A' + index as u8));
        draw_button(
            target,
            fonts,
            (left, top, 39, 31),
            palette::SURFACE_ALT,
            &label,
        );
    }
    for (left, label) in [(80, "Erase"), (160, "Cancel")] {
        draw_button(
            target,
            fonts,
            (left, 288, 79, 31),
            palette::SECONDARY,
            label,
        );
    }
}

fn draw_compact_seed_qr<D>(target: &mut D, fonts: &Fonts, app: &AppState)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    if let Some(wallet) = app.wallet() {
        let secret = wallet.entropy();
        if let Ok(qr) = encode_compact_seed_qr(secret.as_bytes()) {
            let scale = if qr.side == 21 { 8 } else { 7 };
            let symbol = (qr.side as u32 + 8) * scale;
            draw_qr_modules_on_white(
                target,
                &qr.modules,
                qr.side,
                scale,
                4,
                (DISPLAY_WIDTH - symbol) / 2,
                32,
            );
        }
    }
    Text::with_alignment(
        "Private backup - keep safe",
        text_point(120, 282),
        fonts.regular(palette::DANGER_TEXT),
        Alignment::Center,
    )
    .draw(target)
    .unwrap();
    Text::with_alignment(
        "Tap to return",
        text_point(120, 313),
        fonts.regular(palette::TEXT),
        Alignment::Center,
    )
    .draw(target)
    .unwrap();
}

fn draw_home<D>(
    target: &mut D,
    fonts: &Fonts,
    app: &AppState,
    hold_action: Option<HoldAction>,
    hold_ticks: u16,
) where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    let (storage_icon, storage_label) = if app.wallet_is_saved() {
        (BitcoinIcon::Trash, "Delete wallet")
    } else {
        (BitcoinIcon::SdCard, "Save wallet")
    };
    let buttons = [
        (HOME_SCAN_BUTTON, BitcoinIcon::Scan, "Scan PSBT"),
        (HOME_BACKUP_BUTTON, BitcoinIcon::Mnemonic, "Show backup"),
        (HOME_STORAGE_BUTTON, storage_icon, storage_label),
        (HOME_LOGOUT_BUTTON, BitcoinIcon::Exit, "Log out"),
    ];
    for (bounds, icon, label) in buttons {
        draw_menu_item(target, fonts, bounds, icon, label);
    }
    if matches!(hold_action, Some(HoldAction::DeleteWallet)) {
        draw_hold_bar(target, hold_ticks, HOME_NEW_WALLET_BAR_Y);
    }
}

fn draw_mnemonic<D>(target: &mut D, fonts: &Fonts, app: &AppState, page: usize)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    if let Ok(mut mnemonic) = app.mnemonic() {
        for (visible, word) in mnemonic
            .split_whitespace()
            .skip(page * 12)
            .take(12)
            .enumerate()
        {
            let index = page * 12 + visible;
            let column = visible / 6;
            let row = visible % 6;
            let label = alloc::format!("{:>2}. {word}", index + 1);
            Text::new(
                &label,
                Point::new(12 + column as i32 * 116, 62 + row as i32 * 25),
                fonts.mnemonic(palette::TEXT),
            )
            .draw(target)
            .unwrap();
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
    Text::new("Back", text_point(196, 20), fonts.regular(palette::TEXT))
        .draw(target)
        .unwrap();
    let mut y = 48;
    let mut recipients = 0usize;
    for output in review.outputs.iter().filter(|output| !output.is_change) {
        recipients += 1;
        let amount = alloc::format!("SEND {} sats", output.value_sat);
        Text::new(&amount, text_point(12, y), fonts.regular(palette::TEXT))
            .draw(target)
            .unwrap();
        y += 14;
        if let Some(address) = &output.address {
            for chunk in address.as_bytes().chunks(29) {
                if y > 242 {
                    break;
                }
                if let Ok(line) = core::str::from_utf8(chunk) {
                    Text::new(line, text_point(12, y), fonts.regular(palette::TEXT))
                        .draw(target)
                        .unwrap();
                }
                y += 12;
            }
        } else {
            Text::new(
                "Unknown output script",
                text_point(12, y),
                fonts.regular(palette::TEXT),
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
    Text::new(
        &summary,
        text_point(12, 266),
        fonts.regular(palette::TEXT_MUTED),
    )
    .draw(target)
    .unwrap();
    let fee = match review.fee_sat {
        Some(value) => alloc::format!("Fee: {value} sats"),
        None => alloc::format!("Fee: unavailable"),
    };
    Text::new(
        &fee,
        text_point(12, 282),
        fonts.regular(palette::TEXT_MUTED),
    )
    .draw(target)
    .unwrap();
    Text::with_alignment(
        "Hold to confirm & sign",
        text_point(120, 301),
        fonts.regular(palette::TEXT),
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
        .into_styled(PrimitiveStyle::with_stroke(palette::PROGRESS_OUTLINE, 1))
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
        .into_styled(PrimitiveStyle::with_fill(palette::PROGRESS))
        .draw(display)
        .unwrap();
    }
}

pub fn reset_hold_bar<D>(display: &mut D, y: i32)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    Rectangle::new(Point::new(16, y), Size::new(208, 14))
        .into_styled(PrimitiveStyle::with_fill(palette::BACKGROUND))
        .draw(display)
        .unwrap();
    draw_hold_bar_outline(display, y);
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
    target.clear(palette::BACKGROUND).unwrap();
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
        fonts.regular(palette::TEXT),
        Alignment::Center,
    )
    .draw(target)
    .unwrap();
    let part = alloc::format!("PART {}/{}", frame_index + 1, frame_count);
    Text::with_alignment(
        &part,
        text_point(120, 270),
        fonts.regular(palette::TEXT),
        Alignment::Center,
    )
    .draw(target)
    .unwrap();
    Text::with_alignment(
        "Tap to return",
        text_point(120, 313),
        fonts.regular(palette::TEXT),
        Alignment::Center,
    )
    .draw(target)
    .unwrap();
}

pub fn render_message<D>(target: &mut D, fonts: &Fonts, title: &str, lines: &[&str])
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    target.clear(palette::BACKGROUND).unwrap();
    Text::new(
        title,
        text_point(16, 24),
        fonts.regular(palette::DANGER_TEXT),
    )
    .draw(target)
    .unwrap();
    for (index, line) in lines.iter().enumerate() {
        Text::new(
            line,
            text_point(16, 72 + index as i32 * 22),
            fonts.regular(palette::TEXT),
        )
        .draw(target)
        .unwrap();
    }
    Text::with_alignment(
        "Tap to return",
        text_point(120, 313),
        fonts.regular(palette::TEXT),
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
    .into_styled(PrimitiveStyle::with_fill(palette::QR_LIGHT))
    .draw(display)
    .unwrap();
    let black = PrimitiveStyle::with_fill(palette::QR_DARK);
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
        assert_eq!(action_at(Screen::Home, 120, 80), Some(HoldAction::Scan));
        assert_eq!(
            action_at(Screen::Home, 120, 132),
            Some(HoldAction::BackupMenu)
        );
        assert_eq!(
            action_at(Screen::Home, 120, 184),
            Some(HoldAction::SaveWallet)
        );
        assert_eq!(action_at(Screen::Home, 120, 236), Some(HoldAction::Logout));
        assert_eq!(
            action_at(Screen::BackupMenu, 120, 220),
            Some(HoldAction::Descriptor)
        );
        assert_eq!(hold_bar_y(HoldAction::DeleteWallet), Some(270));
        assert_eq!(action_at(Screen::Review, 200, 16), Some(HoldAction::Back));
        assert_eq!(action_at(Screen::Review, 120, 300), Some(HoldAction::Sign));
        assert!(point_in_u16_button(10, 300, SCAN_CANCEL_BUTTON));
        assert!(point_in_u16_button(200, 300, SCAN_ZOOM_BUTTON));
        assert!(!point_in_u16_button(200, 10, SCAN_CANCEL_BUTTON));
        assert_eq!(
            action_at(Screen::BackupWords, 120, 308),
            Some(HoldAction::BackupPage)
        );
        assert_eq!(
            action_at(Screen::MnemonicEntry, 85, 304),
            Some(HoldAction::Backspace)
        );
        assert_eq!(
            action_at(Screen::MnemonicEntry, 155, 304),
            Some(HoldAction::Backspace)
        );
        assert_eq!(
            action_at(Screen::MnemonicEntry, 165, 304),
            Some(HoldAction::Cancel)
        );
        assert_eq!(
            action_at(Screen::MnemonicEntry, 235, 304),
            Some(HoldAction::Cancel)
        );
        assert_eq!(action_at(Screen::MnemonicEntry, 180, 88), None);
        assert_eq!(
            action_at(Screen::MnemonicEntry, 60, 60),
            Some(HoldAction::Suggestion(0))
        );
        assert_eq!(action_at(Screen::MnemonicEntry, 60, 84), None);
        assert_eq!(
            action_at(Screen::MnemonicEntry, 60, 96),
            Some(HoldAction::Suggestion(1))
        );
    }
}
