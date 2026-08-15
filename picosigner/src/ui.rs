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
    encode_compact_seed_qr, encode_descriptor_qr, encode_export_qr, DESCRIPTOR_QR_MODULES,
    DESCRIPTOR_QR_SIDE, EXPORT_QR_MODULES, EXPORT_QR_SIDE,
};
use picosigner_ui::Screen;
use picosigner_wallet::Network;
use rusttype::Font;
use zeroize::Zeroize;

use crate::app::AppState;

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
        Screen::BackupSeedQr => Some(HoldAction::Back),
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

    pub fn mnemonic(&self, color: Rgb565) -> FontTextStyle<Rgb565> {
        FontTextStyleBuilder::new(self.fira_code_regular.clone())
            .font_size(MNEMONIC_FONT_SIZE)
            .text_color(color)
            .anti_aliasing_color(Rgb565::BLACK)
            .build()
    }

    pub fn small(&self, color: Rgb565) -> FontTextStyle<Rgb565> {
        FontTextStyleBuilder::new(self.fira_code_regular.clone())
            .font_size(14)
            .text_color(color)
            .anti_aliasing_color(Rgb565::BLACK)
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
        Screen::LoadWallet => "PICOSIGNER",
        Screen::WalletList => "LOAD WALLET",
        Screen::ImportWallet => "IMPORT WALLET",
        Screen::SeedQrScan => "SCAN SEEDQR",
        Screen::MnemonicLength => "MNEMONIC LENGTH",
        Screen::MnemonicEntry => "",
        Screen::Network => "CHOOSE NETWORK",
        Screen::Derivation => "DERIVATION",
        Screen::Backup => "BACKUP REQUIRED",
        Screen::BackupMenu => "SHOW BACKUP",
        Screen::BackupWords => "WALLET BACKUP",
        Screen::BackupSeedQr => "COMPACT SEEDQR",
        Screen::Home => "",
        Screen::Scan => "SCAN PSBT QR",
        Screen::Descriptor => "DESCRIPTOR EXPORT",
        Screen::Review => "REVIEW TRANSACTION",
        Screen::Sign => "HOLD TO SIGN",
        Screen::Signed => "SIGNED PSBT",
        Screen::Recovery => "WALLET ERROR",
        Screen::Boot | Screen::Seed => "INITIALIZING",
    };
    if !matches!(app.screen(), Screen::Descriptor | Screen::MnemonicEntry) {
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
        Screen::LoadWallet => draw_load_wallet(target, fonts, app),
        Screen::WalletList => draw_wallet_list(target, fonts, app),
        Screen::ImportWallet => {
            draw_button(
                target,
                fonts,
                (16, 70, 208, 44),
                Rgb565::BLUE,
                "SCAN SEEDQR",
                99,
            );
            draw_button(
                target,
                fonts,
                (16, 130, 208, 44),
                Rgb565::new(0, 32, 0),
                "ENTER MNEMONIC",
                159,
            );
            Text::with_alignment(
                "TAP BELOW TO CANCEL",
                text_point(120, 306),
                title(),
                Alignment::Center,
            )
            .draw(target)
            .unwrap();
        }
        Screen::MnemonicLength => {
            draw_button(
                target,
                fonts,
                (16, 80, 208, 44),
                Rgb565::BLUE,
                "12 WORDS",
                109,
            );
            draw_button(
                target,
                fonts,
                (16, 140, 208, 44),
                Rgb565::new(0, 32, 0),
                "24 WORDS",
                169,
            );
            Text::with_alignment("CANCEL", text_point(120, 306), title(), Alignment::Center)
                .draw(target)
                .unwrap();
        }
        Screen::MnemonicEntry => draw_mnemonic_keyboard(target, fonts, app),
        Screen::Network => {
            draw_button(
                target,
                fonts,
                (16, 60, 208, 40),
                Rgb565::new(24, 40, 0),
                "MAINNET",
                85,
            );
            draw_button(
                target,
                fonts,
                (16, 112, 208, 40),
                Rgb565::BLUE,
                "TESTNET4",
                137,
            );
            draw_button(
                target,
                fonts,
                (16, 164, 208, 40),
                Rgb565::new(0, 32, 0),
                "SIGNET",
                189,
            );
            Text::with_alignment("CANCEL", text_point(120, 306), title(), Alignment::Center)
                .draw(target)
                .unwrap();
        }
        Screen::Derivation => {
            draw_button(
                target,
                fonts,
                (16, 92, 208, 70),
                Rgb565::new(0, 32, 0),
                "BIP86 TAPROOT",
                122,
            );
            Text::with_alignment(
                "ACCOUNT 0",
                text_point(120, 150),
                fonts.regular_on(Rgb565::CYAN, Rgb565::new(0, 32, 0)),
                Alignment::Center,
            )
            .draw(target)
            .unwrap();
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
            Text::with_alignment("CANCEL", text_point(120, 306), title(), Alignment::Center)
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
            Text::new("BACKUP REQUIRED", text_point(16, 24), title())
                .draw(target)
                .unwrap();
        }
        Screen::BackupMenu => {
            draw_button(
                target,
                fonts,
                (16, 80, 208, 44),
                Rgb565::new(24, 40, 0),
                "SHOW MNEMONIC",
                109,
            );
            draw_button(
                target,
                fonts,
                (16, 140, 208, 44),
                Rgb565::BLUE,
                "COMPACT SEEDQR",
                169,
            );
            draw_button(
                target,
                fonts,
                (16, 200, 208, 44),
                Rgb565::new(0, 32, 0),
                "EXPORT DESCRIPTOR",
                229,
            );
            Text::with_alignment("BACK", text_point(120, 306), title(), Alignment::Center)
                .draw(target)
                .unwrap();
        }
        Screen::BackupWords => {
            Text::new("Keep these words private", text_point(16, 48), accent())
                .draw(target)
                .unwrap();
            draw_mnemonic(target, fonts, app, app.backup_page());
            Text::with_alignment(
                "TAP TO RETURN",
                text_point(120, 313),
                title(),
                Alignment::Center,
            )
            .draw(target)
            .unwrap();
            Text::new("WALLET BACKUP", text_point(16, 24), title())
                .draw(target)
                .unwrap();
        }
        Screen::BackupSeedQr => draw_compact_seed_qr(target, fonts, app),
        Screen::Recovery => {
            Text::with_alignment(
                app.notice().unwrap_or("WALLET OPERATION FAILED"),
                text_point(120, 130),
                fonts.regular(Rgb565::RED),
                Alignment::Center,
            )
            .draw(target)
            .unwrap();
            Text::with_alignment(
                "TAP TO RETURN",
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

fn draw_button<D>(
    target: &mut D,
    fonts: &Fonts,
    bounds: (i32, i32, u32, u32),
    color: Rgb565,
    label: &str,
    baseline: i32,
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
        text_point(120, baseline),
        fonts.regular_on(Rgb565::WHITE, color),
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

fn draw_load_wallet<D>(target: &mut D, fonts: &Fonts, app: &AppState)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    if !app.wallets().is_empty() {
        let label = if app.wallets().len() == 1 {
            let fp = app
                .wallets()
                .get(0)
                .and_then(|wallet| wallet.fingerprint)
                .unwrap_or([0; 4]);
            alloc::format!("LOAD WALLET {}", fingerprint_text(fp))
        } else {
            "LOAD WALLET".into()
        };
        draw_button(
            target,
            fonts,
            (16, 60, 208, 40),
            Rgb565::new(24, 40, 0),
            &label,
            85,
        );
    }
    draw_button(
        target,
        fonts,
        (16, 164, 208, 40),
        Rgb565::BLUE,
        "IMPORT WALLET",
        189,
    );
    draw_button(
        target,
        fonts,
        (16, 216, 208, 40),
        Rgb565::new(0, 32, 0),
        "CREATE WALLET",
        241,
    );
}

fn draw_wallet_list<D>(target: &mut D, fonts: &Fonts, app: &AppState)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    for (index, wallet) in app.wallets().iter().enumerate() {
        let top = 32 + index as i32 * 30;
        let background = if index % 2 == 0 {
            Rgb565::new(0, 24, 0)
        } else {
            Rgb565::new(0, 0, 16)
        };
        Rectangle::new(Point::new(8, top), Size::new(224, 28))
            .into_styled(PrimitiveStyle::with_fill(background))
            .draw(target)
            .unwrap();
        let label = alloc::format!(
            "{} [{}]",
            fingerprint_text(wallet.fingerprint.unwrap_or([0; 4])),
            match wallet.network {
                picosigner_board::Network::Mainnet => "mainnet",
                picosigner_board::Network::Testnet4 => "testnet4",
                picosigner_board::Network::Signet => "signet",
            }
        );
        Text::new(
            &label,
            text_point(12, top + 23),
            fonts.regular_on(Rgb565::WHITE, background),
        )
        .draw(target)
        .unwrap();
    }
    Text::with_alignment(
        "BACK",
        text_point(120, 313),
        fonts.regular(Rgb565::WHITE),
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
            Rgb565::RED
        } else {
            Rgb565::CYAN
        }),
    )
    .draw(target)
    .unwrap();
    let prefix = alloc::format!("> {}", app.mnemonic_prefix());
    Text::new(&prefix, small_text_point(8, 44), fonts.small(Rgb565::CYAN))
        .draw(target)
        .unwrap();
    for (index, word) in app.mnemonic_suggestions().iter().take(3).enumerate() {
        let top = 50 + index as i32 * 36;
        let color = Rgb565::new(0, 24, 0);
        Rectangle::new(Point::new(4, top), Size::new(116, 32))
            .into_styled(PrimitiveStyle::with_fill(color))
            .draw(target)
            .unwrap();
        Text::with_alignment(
            word,
            text_point(62, top + 26),
            fonts.regular_on(Rgb565::GREEN, color),
            Alignment::Center,
        )
        .draw(target)
        .unwrap();
    }
    Text::new(
        "RECENT",
        small_text_point(148, 44),
        fonts.small(Rgb565::CYAN),
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
            fonts.small(Rgb565::WHITE),
        )
        .draw(target)
        .unwrap();
    }
    for index in 0..26usize {
        let column = index % 6;
        let row = index / 6;
        let left = column as i32 * 40;
        let top = 160 + row as i32 * 32;
        let color = Rgb565::new(0, 0, 24);
        Rectangle::new(Point::new(left, top), Size::new(39, 31))
            .into_styled(PrimitiveStyle::with_fill(color))
            .draw(target)
            .unwrap();
        let label = alloc::format!("{}", char::from(b'A' + index as u8));
        Text::with_alignment(
            &label,
            text_point(left + 20, top + 24),
            fonts.regular_on(Rgb565::WHITE, color),
            Alignment::Center,
        )
        .draw(target)
        .unwrap();
    }
    let action_color = Rgb565::new(24, 0, 0);
    for (left, label) in [(80, "ERASE"), (160, "CANCEL")] {
        Rectangle::new(Point::new(left, 288), Size::new(79, 31))
            .into_styled(PrimitiveStyle::with_fill(action_color))
            .draw(target)
            .unwrap();
        Text::with_alignment(
            label,
            text_point(left + 40, 312),
            fonts.regular_on(Rgb565::WHITE, action_color),
            Alignment::Center,
        )
        .draw(target)
        .unwrap();
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
        "PRIVATE BACKUP - KEEP SAFE",
        text_point(120, 282),
        fonts.regular(Rgb565::RED),
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

fn draw_home<D>(
    target: &mut D,
    fonts: &Fonts,
    app: &AppState,
    hold_action: Option<HoldAction>,
    hold_ticks: u16,
) where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    let buttons = [
        (HOME_SCAN_BUTTON, Rgb565::BLUE, "SCAN PSBT", 85),
        (
            HOME_BACKUP_BUTTON,
            Rgb565::new(24, 40, 0),
            "SHOW BACKUP",
            137,
        ),
        (
            HOME_STORAGE_BUTTON,
            if app.wallet_is_saved() {
                Rgb565::RED
            } else {
                Rgb565::new(0, 32, 0)
            },
            if app.wallet_is_saved() {
                "DELETE WALLET"
            } else {
                "SAVE WALLET"
            },
            189,
        ),
        (HOME_LOGOUT_BUTTON, Rgb565::RED, "LOG OUT", 241),
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
                fonts.mnemonic(Rgb565::GREEN),
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
