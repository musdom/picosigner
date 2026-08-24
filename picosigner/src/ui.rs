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
use heapless::String as FixedString;
use picosigner_qr::{
    encode_compact_seed_qr, encode_descriptor_qr, encode_export_qr, DESCRIPTOR_QR_MODULES,
    DESCRIPTOR_QR_SIDE, EXPORT_QR_MODULES, EXPORT_QR_SIDE,
};
use picosigner_ui::Screen;
use picosigner_wallet::{Network, WalletKind};
use rusttype::Font;
use zeroize::Zeroize;

use crate::{
    app::{AppState, WALLETS_PER_PAGE},
    palette,
};

pub const DISPLAY_WIDTH: u32 = 240;
pub const DISPLAY_HEIGHT: u32 = 320;

pub const HOME_SCAN_BUTTON: (i32, i32, u32, u32) = (16, 60, 208, 40);
pub const HOME_BACKUP_BUTTON: (i32, i32, u32, u32) = (16, 112, 208, 40);
pub const HOME_STORAGE_BUTTON: (i32, i32, u32, u32) = (16, 164, 208, 40);
pub const HOME_LOGOUT_BUTTON: (i32, i32, u32, u32) = (16, 216, 208, 40);
pub const HOME_NEW_WALLET_BUTTON: (i32, i32, u32, u32) = HOME_LOGOUT_BUTTON;
pub const HOME_NEW_WALLET_BAR: (i32, i32, u32, u32) = (16, HOME_NEW_WALLET_BUTTON.1, 208, 6);
pub const HOME_STORAGE_BAR: (i32, i32, u32, u32) = (16, HOME_STORAGE_BUTTON.1, 208, 6);
pub const BACKUP_CANCEL_BUTTON: (i32, i32, u32, u32) = (0, 270, 60, 50);
pub const BACKUP_CONFIRM_BUTTON: (i32, i32, u32, u32) = (60, 270, 180, 50);
pub const BACKUP_CONFIRM_BAR: (i32, i32, u32, u32) = (0, 259, 240, 12);
pub const NAV_BACK_BUTTON: (i32, i32, u32, u32) = (90, 270, 60, 50);
pub const MNEMONIC_CANCEL_BAR: (i32, i32, u32, u32) = (160, 312, 80, 8);
pub const FROST_EDIT_BUTTON: (i32, i32, u32, u32) = (0, 270, 100, 50);
pub const FROST_CONFIRM_BUTTON: (i32, i32, u32, u32) = (100, 270, 140, 50);
pub const DESCRIPTOR_BACK_BUTTON: (i32, i32, u32, u32) = (0, 296, 240, 24);
pub const REVIEW_BACK_BUTTON: (i32, i32, u32, u32) = BACKUP_CANCEL_BUTTON;
pub const REVIEW_SIGN_BUTTON: (i32, i32, u32, u32) = BACKUP_CONFIRM_BUTTON;
pub const REVIEW_SIGN_BAR: (i32, i32, u32, u32) = BACKUP_CONFIRM_BAR;
pub const SCAN_CANCEL_BUTTON: (u16, u16, u16, u16) = (0, 280, 60, 40);
pub const SCAN_ZOOM_BUTTON: (u16, u16, u16, u16) = (120, 280, 120, 40);
pub const SCAN_PROGRESS_BAR: (i32, i32, u32, u32) = (12, 263, 216, 11);
pub const QR_RESULT_CANCEL_BUTTON: (i32, i32, u32, u32) = BACKUP_CANCEL_BUTTON;
pub const QR_RESULT_CONFIRM_BUTTON: (i32, i32, u32, u32) = BACKUP_CONFIRM_BUTTON;
pub const DERIVATION_PROGRESS_BAR: (i32, i32, u32, u32) = (16, 132, 208, 12);
const WALLET_LIST_TOP: i32 = 44;
const WALLET_LIST_ROW_HEIGHT: i32 = 54;
const WALLET_LIST_BOTTOM: i32 = WALLET_LIST_TOP + WALLET_LIST_ROW_HEIGHT * WALLETS_PER_PAGE as i32;

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
    EnterFrost,
    FrostDecrease,
    FrostIncrease,
    FrostConfirm,
    FrostEnterShare,
    FrostEditShare,
    FrostAcceptShare,
    MnemonicWords(u8),
    Network(Network),
    Derivation,
    BackupMenu,
    MnemonicBackup,
    SeedQrBackup,
    Logout,
    SaveWallet,
    OpenDeleteWallet,
    DeleteWallet,
    Suggestion(usize),
    Key(u8),
    Backspace,
    CancelEntry,
    SwipeBack,
    Cancel,
    BackupPage,
}

pub fn action_at(screen: Screen, x: u16, y: u16) -> Option<HoldAction> {
    match screen {
        screen if shows_nav_back_button(screen) && in_button(x, y, NAV_BACK_BUTTON) => {
            Some(HoldAction::Back)
        }
        Screen::LoadWallet if in_button(x, y, (16, 60, 208, 40)) => Some(HoldAction::Load),
        Screen::LoadWallet if in_button(x, y, (16, 164, 208, 40)) => Some(HoldAction::Import),
        Screen::LoadWallet if in_button(x, y, (16, 216, 208, 40)) => Some(HoldAction::Create),
        Screen::WalletList if (y as i32) >= WALLET_LIST_TOP && (y as i32) < WALLET_LIST_BOTTOM => {
            Some(HoldAction::Wallet(
                ((y as i32 - WALLET_LIST_TOP) / WALLET_LIST_ROW_HEIGHT) as usize,
            ))
        }
        Screen::WalletList => Some(HoldAction::SwipeBack),
        Screen::ImportWallet if in_button(x, y, (16, 52, 208, 44)) => Some(HoldAction::ScanSeedQr),
        Screen::ImportWallet if in_button(x, y, (16, 108, 208, 44)) => {
            Some(HoldAction::EnterMnemonic)
        }
        Screen::ImportWallet if in_button(x, y, (16, 164, 208, 44)) => Some(HoldAction::EnterFrost),
        Screen::ImportWallet => Some(HoldAction::SwipeBack),
        Screen::MnemonicLength if in_button(x, y, (16, 80, 208, 44)) => {
            Some(HoldAction::MnemonicWords(12))
        }
        Screen::MnemonicLength if in_button(x, y, (16, 140, 208, 44)) => {
            Some(HoldAction::MnemonicWords(24))
        }
        Screen::MnemonicLength => Some(HoldAction::SwipeBack),
        Screen::FrostThreshold | Screen::FrostShareIndex if in_button(x, y, (16, 80, 60, 44)) => {
            Some(HoldAction::FrostDecrease)
        }
        Screen::FrostThreshold | Screen::FrostShareIndex if in_button(x, y, (164, 80, 60, 44)) => {
            Some(HoldAction::FrostIncrease)
        }
        Screen::FrostThreshold if in_button(x, y, (16, 150, 208, 44)) => {
            Some(HoldAction::FrostConfirm)
        }
        Screen::FrostShareIndex if in_button(x, y, (16, 150, 208, 44)) => {
            Some(HoldAction::FrostEnterShare)
        }
        Screen::FrostThreshold | Screen::FrostShareIndex => Some(HoldAction::SwipeBack),
        Screen::MnemonicEntry | Screen::FrostMnemonicEntry if x < 120 && y >= 50 && y < 154 => {
            let relative_y = y - 50;
            let row = relative_y / 36;
            if row < 3 && relative_y % 36 < 32 {
                Some(HoldAction::Suggestion(usize::from(row)))
            } else {
                Some(HoldAction::SwipeBack)
            }
        }
        Screen::MnemonicEntry | Screen::FrostMnemonicEntry if y >= 160 => {
            let column = usize::from(x / 40);
            let row = usize::from((y - 160) / 32);
            let index = row * 6 + column;
            if index < 26 {
                Some(HoldAction::Key(b'a' + index as u8))
            } else if index == 26 || index == 27 {
                Some(HoldAction::Backspace)
            } else {
                Some(HoldAction::CancelEntry)
            }
        }
        Screen::MnemonicEntry | Screen::FrostMnemonicEntry => Some(HoldAction::SwipeBack),
        Screen::FrostMnemonicConfirm if in_button(x, y, FROST_EDIT_BUTTON) => {
            Some(HoldAction::FrostEditShare)
        }
        Screen::FrostMnemonicConfirm if in_button(x, y, FROST_CONFIRM_BUTTON) => {
            Some(HoldAction::FrostAcceptShare)
        }
        Screen::FrostMnemonicConfirm => Some(HoldAction::SwipeBack),
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
        Screen::Network => Some(HoldAction::SwipeBack),
        Screen::Derivation if in_button(x, y, (16, 92, 208, 70)) => Some(HoldAction::Derivation),
        Screen::Derivation => Some(HoldAction::SwipeBack),
        Screen::Backup if in_button(x, y, BACKUP_CANCEL_BUTTON) => Some(HoldAction::Cancel),
        Screen::Backup if in_button(x, y, BACKUP_CONFIRM_BUTTON) => Some(HoldAction::Backup),
        Screen::Backup => None,
        Screen::BackupMenu if in_button(x, y, (16, 80, 208, 44)) => {
            Some(HoldAction::MnemonicBackup)
        }
        Screen::BackupMenu if in_button(x, y, (16, 140, 208, 44)) => Some(HoldAction::SeedQrBackup),
        Screen::BackupMenu if in_button(x, y, (16, 200, 208, 44)) => Some(HoldAction::Descriptor),
        Screen::BackupMenu => Some(HoldAction::SwipeBack),
        Screen::BackupWords => Some(HoldAction::BackupPage),
        Screen::BackupSeedQr if in_button(x, y, DESCRIPTOR_BACK_BUTTON) => Some(HoldAction::Back),
        Screen::BackupSeedQr => Some(HoldAction::Brightness),
        Screen::Home if in_button(x, y, HOME_SCAN_BUTTON) => Some(HoldAction::Scan),
        Screen::Home if in_button(x, y, HOME_BACKUP_BUTTON) => Some(HoldAction::BackupMenu),
        Screen::Home if in_button(x, y, HOME_STORAGE_BUTTON) => Some(HoldAction::SaveWallet),
        Screen::Home if in_button(x, y, HOME_LOGOUT_BUTTON) => Some(HoldAction::Logout),
        Screen::DeleteWallet if in_button(x, y, BACKUP_CANCEL_BUTTON) => Some(HoldAction::Back),
        Screen::DeleteWallet if in_button(x, y, BACKUP_CONFIRM_BUTTON) => {
            Some(HoldAction::DeleteWallet)
        }
        Screen::DeleteWallet => None,
        Screen::Descriptor if in_button(x, y, DESCRIPTOR_BACK_BUTTON) => Some(HoldAction::Back),
        Screen::Descriptor => Some(HoldAction::Brightness),
        Screen::Review if in_button(x, y, REVIEW_BACK_BUTTON) => Some(HoldAction::Back),
        Screen::Review if in_button(x, y, REVIEW_SIGN_BUTTON) => Some(HoldAction::Sign),
        Screen::Review => Some(HoldAction::SwipeBack),
        Screen::Signed if y >= DESCRIPTOR_BACK_BUTTON.1 as u16 => Some(HoldAction::Back),
        Screen::Signed => Some(HoldAction::SwipeBack),
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
        Some(HoldAction::MnemonicBackup)
            if app
                .wallet()
                .is_some_and(|wallet| wallet.kind() == WalletKind::Frost) =>
        {
            Some(HoldAction::Descriptor)
        }
        Some(HoldAction::SeedQrBackup)
            if app
                .wallet()
                .is_some_and(|wallet| wallet.kind() == WalletKind::Frost) =>
        {
            Some(HoldAction::SwipeBack)
        }
        Some(HoldAction::SaveWallet) if app.wallet_is_saved() => Some(HoldAction::OpenDeleteWallet),
        Some(HoldAction::Wallet(index)) => Some(HoldAction::Wallet(
            app.wallet_list_page() * WALLETS_PER_PAGE + index,
        )),
        action => action,
    }
}

pub fn hold_bar_bounds(action: HoldAction) -> Option<(i32, i32, u32, u32)> {
    match action {
        HoldAction::Backup => Some(BACKUP_CONFIRM_BAR),
        HoldAction::Regenerate => Some(HOME_NEW_WALLET_BAR),
        HoldAction::DeleteWallet => Some(BACKUP_CONFIRM_BAR),
        HoldAction::Sign => Some(REVIEW_SIGN_BAR),
        HoldAction::CancelEntry => Some(MNEMONIC_CANCEL_BAR),
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
        | HoldAction::EnterFrost
        | HoldAction::FrostDecrease
        | HoldAction::FrostIncrease
        | HoldAction::FrostConfirm
        | HoldAction::FrostEnterShare
        | HoldAction::FrostEditShare
        | HoldAction::FrostAcceptShare
        | HoldAction::MnemonicWords(_)
        | HoldAction::Network(_)
        | HoldAction::Derivation
        | HoldAction::BackupMenu
        | HoldAction::MnemonicBackup
        | HoldAction::SeedQrBackup
        | HoldAction::Logout
        | HoldAction::SaveWallet
        | HoldAction::OpenDeleteWallet
        | HoldAction::Suggestion(_)
        | HoldAction::Key(_)
        | HoldAction::Backspace
        | HoldAction::SwipeBack
        | HoldAction::Cancel
        | HoldAction::BackupPage => None,
    }
}

pub fn supports_swipe_back(screen: Screen) -> bool {
    matches!(
        screen,
        Screen::WalletList
            | Screen::ImportWallet
            | Screen::MnemonicLength
            | Screen::FrostThreshold
            | Screen::FrostShareIndex
            | Screen::FrostMnemonicConfirm
            | Screen::Network
            | Screen::Derivation
            | Screen::BackupMenu
            | Screen::BackupSeedQr
            | Screen::Descriptor
            | Screen::Review
            | Screen::DeleteWallet
            | Screen::Signed
    )
}

fn shows_nav_back_button(screen: Screen) -> bool {
    matches!(
        screen,
        Screen::WalletList
            | Screen::ImportWallet
            | Screen::MnemonicLength
            | Screen::FrostThreshold
            | Screen::FrostShareIndex
            | Screen::Network
            | Screen::Derivation
            | Screen::BackupMenu
            | Screen::BackupWords
            | Screen::BackupSeedQr
            | Screen::Descriptor
    )
}

const UI_FONT: &[u8] = include_bytes!("../assets/subset-font.ttf");
const FONT_SIZE: u32 = 20;
const MENU_FONT_SIZE: u32 = 26;
const SUBHEADING_FONT_SIZE: u32 = 18;
const BODY_FONT_SIZE: u32 = 16;
const MNEMONIC_FONT_SIZE: u32 = 20;

pub fn text_point(x: i32, baseline_y: i32) -> Point {
    Point::new(x, baseline_y - FONT_SIZE as i32)
}

fn small_text_point(x: i32, baseline_y: i32) -> Point {
    Point::new(x, baseline_y - 14)
}

fn subheading_text_point(x: i32, baseline_y: i32) -> Point {
    Point::new(x, baseline_y - SUBHEADING_FONT_SIZE as i32)
}

fn body_text_point(x: i32, baseline_y: i32) -> Point {
    Point::new(x, baseline_y - BODY_FONT_SIZE as i32)
}

/// Top edge of a `FONT_SIZE`-tall line of text centred inside `bounds`.
///
/// `Text` is positioned by the top of its line box, so vertical centring is
/// the only correct way to place a button label: the old per-call baseline
/// constants drifted with every change to a button's height.
fn centered_text_top(bounds: (i32, i32, u32, u32), font_size: u32) -> i32 {
    bounds.1 + (bounds.3 as i32 - font_size as i32) / 2
}

/// Outline artwork adapted from the 24px SVGs at bitcoinicons.com.
#[derive(Clone, Copy)]
pub enum BitcoinIcon {
    Wallet,
    Scan,
    Receive,
    Plus,
    Mnemonic,
    Snowflake,
    QrCode,
    Export,
    Globe,
    Fingerprint,
    SdCard,
    Trash,
    Exit,
    Edit,
    Cross,
    Check,
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
        BitcoinIcon::Snowflake => {
            // 24px outline snowflake supplied for the FROST import action.
            // Its half-pixel SVG coordinates are rounded onto the display's
            // integer pixel grid.
            for (a, b) in [
                ((12, 4), (12, 20)),
                ((20, 12), (4, 12)),
                ((19, 8), (15, 12)),
                ((15, 12), (19, 15)),
                ((5, 15), (9, 12)),
                ((9, 12), (5, 8)),
                ((16, 18), (12, 15)),
                ((12, 15), (9, 18)),
                ((9, 5), (12, 9)),
                ((12, 9), (16, 5)),
            ] {
                line(target, a, b);
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
        BitcoinIcon::Fingerprint => {
            let outer = [
                p(4, 15),
                p(5, 10),
                p(8, 6),
                p(12, 4),
                p(16, 6),
                p(19, 10),
                p(19, 15),
                p(17, 19),
            ];
            Polyline::new(&outer)
                .into_styled(stroke)
                .draw(target)
                .unwrap();
            let middle = [
                p(7, 18),
                p(7, 13),
                p(9, 9),
                p(12, 8),
                p(15, 9),
                p(17, 12),
                p(16, 16),
            ];
            Polyline::new(&middle)
                .into_styled(stroke)
                .draw(target)
                .unwrap();
            let inner = [
                p(10, 20),
                p(9, 16),
                p(10, 13),
                p(12, 11),
                p(14, 13),
                p(13, 17),
                p(15, 20),
            ];
            Polyline::new(&inner)
                .into_styled(stroke)
                .draw(target)
                .unwrap();
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
        BitcoinIcon::Edit => {
            // Adapted from Bitcoin Icons' 24px outline edit.svg.
            let body = [p(5, 17), p(5, 19), p(7, 19), p(16, 11), p(13, 8), p(5, 17)];
            Polyline::new(&body)
                .into_styled(stroke)
                .draw(target)
                .unwrap();
            let cap = [
                p(15, 6),
                p(18, 9),
                p(19, 8),
                p(19, 6),
                p(18, 5),
                p(16, 5),
                p(15, 6),
            ];
            Polyline::new(&cap)
                .into_styled(stroke)
                .draw(target)
                .unwrap();
        }
        BitcoinIcon::Cross => {
            line(target, (6, 6), (18, 18));
            line(target, (18, 6), (6, 18));
        }
        BitcoinIcon::Check => {
            line(target, (6, 13), (11, 18));
            line(target, (11, 18), (18, 6));
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
    ui_regular: Font<'static>,
}

impl Fonts {
    pub fn load() -> Self {
        Self {
            ui_regular: Font::try_from_bytes(UI_FONT).expect("invalid UI font"),
        }
    }

    pub fn regular(&self, color: Rgb565) -> FontTextStyle<Rgb565> {
        self.regular_on(color, palette::BACKGROUND)
    }

    pub fn regular_on(&self, color: Rgb565, background: Rgb565) -> FontTextStyle<Rgb565> {
        FontTextStyleBuilder::new(self.ui_regular.clone())
            .font_size(FONT_SIZE)
            .text_color(color)
            .anti_aliasing_color(background)
            .build()
    }

    pub fn menu(&self, color: Rgb565) -> FontTextStyle<Rgb565> {
        FontTextStyleBuilder::new(self.ui_regular.clone())
            .font_size(MENU_FONT_SIZE)
            .text_color(color)
            .anti_aliasing_color(palette::BACKGROUND)
            .build()
    }

    pub fn subheading(&self, color: Rgb565) -> FontTextStyle<Rgb565> {
        FontTextStyleBuilder::new(self.ui_regular.clone())
            .font_size(SUBHEADING_FONT_SIZE)
            .text_color(color)
            .anti_aliasing_color(palette::BACKGROUND)
            .build()
    }

    pub fn body(&self, color: Rgb565) -> FontTextStyle<Rgb565> {
        FontTextStyleBuilder::new(self.ui_regular.clone())
            .font_size(BODY_FONT_SIZE)
            .text_color(color)
            .anti_aliasing_color(palette::BACKGROUND)
            .build()
    }

    pub fn mnemonic(&self, color: Rgb565) -> FontTextStyle<Rgb565> {
        FontTextStyleBuilder::new(self.ui_regular.clone())
            .font_size(MNEMONIC_FONT_SIZE)
            .text_color(color)
            .anti_aliasing_color(palette::BACKGROUND)
            .build()
    }

    pub fn small(&self, color: Rgb565) -> FontTextStyle<Rgb565> {
        self.small_on(color, palette::BACKGROUND)
    }

    pub fn small_on(&self, color: Rgb565, background: Rgb565) -> FontTextStyle<Rgb565> {
        FontTextStyleBuilder::new(self.ui_regular.clone())
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
    let accent = || fonts.subheading(palette::TEXT);
    let body = || fonts.body(palette::TEXT_MUTED);
    let heading = match app.screen() {
        Screen::LoadWallet => "PicoSigner",
        Screen::WalletList => "Load wallet",
        Screen::ImportWallet => "Import wallet",
        Screen::SeedQrScan => "Scan SeedQR",
        Screen::MnemonicLength => "Mnemonic length",
        Screen::MnemonicEntry => "",
        Screen::FrostThreshold => "FROST recovery",
        Screen::FrostShareIndex => "Backup key number",
        Screen::FrostMnemonicEntry => "",
        Screen::FrostMnemonicConfirm => "Confirm 25 words",
        Screen::Network => "Choose network",
        Screen::Derivation => "Derivation",
        Screen::Deriving => "Deriving wallet",
        Screen::Backup => "Backup required",
        Screen::BackupMenu => "Show backup",
        Screen::BackupWords => "Wallet backup",
        Screen::BackupSeedQr => "Compact SeedQR",
        Screen::Home => "",
        Screen::DeleteWallet => "Delete wallet",
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
                    draw_wallet_identity(target, fonts, 8, 1, fingerprint, wallet.network(), false);
                }
            }
        } else {
            Text::new(heading, text_point(16, 24), title())
                .draw(target)
                .unwrap();
        }
    }

    match app.screen() {
        Screen::LoadWallet => draw_load_wallet(target, fonts),
        Screen::WalletList => draw_wallet_list(target, fonts, app),
        Screen::ImportWallet => {
            draw_menu_item(
                target,
                fonts,
                (16, 52, 208, 44),
                BitcoinIcon::Scan,
                "Scan SeedQR",
            );
            draw_menu_item(
                target,
                fonts,
                (16, 108, 208, 44),
                BitcoinIcon::Mnemonic,
                "Enter mnemonic",
            );
            draw_menu_item(
                target,
                fonts,
                (16, 164, 208, 44),
                BitcoinIcon::Snowflake,
                "FROST",
            );
            draw_nav_back_button(target);
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
            draw_nav_back_button(target);
        }
        Screen::FrostThreshold => {
            draw_number_picker(
                target,
                fonts,
                app.frost_threshold() as u32,
                "backups required",
            );
            draw_menu_item(
                target,
                fonts,
                (16, 150, 208, 44),
                BitcoinIcon::Check,
                "Continue",
            );
            Text::with_alignment(
                "EMERGENCY RECOVERY ONLY",
                small_text_point(120, 216),
                fonts.small(palette::DANGER_TEXT),
                Alignment::Center,
            )
            .draw(target)
            .unwrap();
            for (line, baseline) in [
                ("Your multisig becomes singlesig.", 232),
                ("Use only without new Frostsnap", 248),
                ("devices or the Frostsnap app.", 264),
            ] {
                Text::with_alignment(
                    line,
                    small_text_point(120, baseline),
                    fonts.small(palette::TEXT_MUTED),
                    Alignment::Center,
                )
                .draw(target)
                .unwrap();
            }
            draw_nav_back_button(target);
        }
        Screen::FrostShareIndex => {
            let progress = alloc::format!(
                "Backup {} of {}",
                app.frost_shares_entered() + 1,
                app.frost_threshold()
            );
            draw_number_picker(target, fonts, app.frost_share_index(), &progress);
            draw_menu_item(
                target,
                fonts,
                (16, 150, 208, 44),
                BitcoinIcon::Mnemonic,
                "Enter 25 words",
            );
            if let Some(notice) = app.notice() {
                Text::with_alignment(
                    notice,
                    small_text_point(120, 220),
                    fonts.small(palette::DANGER_TEXT),
                    Alignment::Center,
                )
                .draw(target)
                .unwrap();
            }
            draw_nav_back_button(target);
        }
        Screen::MnemonicEntry | Screen::FrostMnemonicEntry => {
            draw_mnemonic_keyboard(target, fonts, app, hold_action, hold_ticks)
        }
        Screen::FrostMnemonicConfirm => draw_frost_mnemonic_confirmation(target, fonts, app),
        Screen::Network => {
            draw_menu_item_with_icon_color(
                target,
                fonts,
                (16, 60, 208, 40),
                BitcoinIcon::Globe,
                "Mainnet",
                palette::MAINNET,
            );
            draw_menu_item_with_icon_color(
                target,
                fonts,
                (16, 112, 208, 40),
                BitcoinIcon::Globe,
                "Testnet",
                palette::TESTNET,
            );
            draw_menu_item_with_icon_color(
                target,
                fonts,
                (16, 164, 208, 40),
                BitcoinIcon::Globe,
                "Signet",
                palette::SIGNET,
            );
            draw_nav_back_button(target);
        }
        Screen::Derivation => {
            // Two stacked lines, so centre the pair rather than each line.
            const DERIVATION_BUTTON: (i32, i32, u32, u32) = (16, 92, 208, 70);
            draw_menu_item(
                target,
                fonts,
                DERIVATION_BUTTON,
                BitcoinIcon::Wallet,
                if app.pending_wallet_kind() == WalletKind::Frost {
                    "Frostsnap Taproot"
                } else {
                    "BIP86 Taproot"
                },
            );
            Text::new(
                if app.pending_wallet_kind() == WalletKind::Frost {
                    "m/0/0/0/0"
                } else if matches!(app.selected_network(), Network::Mainnet) {
                    "m/86'/0'/0'"
                } else {
                    "m/86'/1'/0'"
                },
                body_text_point(34, 200),
                body(),
            )
            .draw(target)
            .unwrap();
            draw_nav_back_button(target);
        }
        Screen::Deriving => {
            Text::new(
                if app.pending_wallet_kind() == WalletKind::Frost {
                    "Deriving Frostsnap wallet"
                } else {
                    "Deriving BIP86 Taproot"
                },
                subheading_text_point(16, 82),
                accent(),
            )
            .draw(target)
            .unwrap();
            Text::new("Please wait...", body_text_point(16, 108), body())
                .draw(target)
                .unwrap();
            draw_indeterminate_progress(target, DERIVATION_PROGRESS_BAR);
        }
        Screen::Backup => {
            Text::new(
                "Write down these 12 words",
                subheading_text_point(16, 48),
                accent(),
            )
            .draw(target)
            .unwrap();
            draw_mnemonic(target, fonts, app, 0);
            draw_confirmation_footer(
                target,
                fonts,
                if matches!(hold_action, Some(HoldAction::Backup)) {
                    hold_ticks
                } else {
                    0
                },
            );
            // Keep the security context above the mnemonic visually explicit.
            Text::new("Backup required", text_point(16, 24), title())
                .draw(target)
                .unwrap();
        }
        Screen::BackupMenu => {
            if app
                .wallet()
                .is_some_and(|wallet| wallet.kind() == WalletKind::Frost)
            {
                draw_menu_item(
                    target,
                    fonts,
                    (16, 80, 208, 44),
                    BitcoinIcon::Export,
                    "Descriptor",
                );
                Text::new(
                    "Restore again from your FROST cards",
                    small_text_point(16, 154),
                    fonts.small(palette::TEXT_MUTED),
                )
                .draw(target)
                .unwrap();
            } else {
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
                    "SeedQR",
                );
                draw_menu_item(
                    target,
                    fonts,
                    (16, 200, 208, 44),
                    BitcoinIcon::Export,
                    "Descriptor",
                );
            }
            draw_nav_back_button(target);
        }
        Screen::BackupWords => {
            Text::new(
                "Keep these words private",
                subheading_text_point(16, 48),
                accent(),
            )
            .draw(target)
            .unwrap();
            draw_mnemonic(target, fonts, app, app.backup_page());
            draw_nav_back_button(target);
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
            draw_nav_back_button(target);
        }
        Screen::Home => draw_home(target, fonts, app),
        Screen::DeleteWallet => {
            let fingerprint = app
                .wallet()
                .and_then(|wallet| wallet.master_fingerprint())
                .map(fingerprint_text)
                .unwrap_or_else(|| "????????".into());
            Text::new(
                "Are you sure you want to",
                subheading_text_point(16, 92),
                fonts.subheading(palette::TEXT),
            )
            .draw(target)
            .unwrap();
            let question = alloc::format!("delete wallet {fingerprint}?");
            Text::new(
                &question,
                subheading_text_point(16, 120),
                fonts.subheading(palette::TEXT),
            )
            .draw(target)
            .unwrap();
            draw_confirmation_footer(
                target,
                fonts,
                if matches!(hold_action, Some(HoldAction::DeleteWallet)) {
                    hold_ticks
                } else {
                    0
                },
            );
        }
        Screen::Scan => {
            Text::new(
                "Camera/BBQr receive next",
                subheading_text_point(16, 76),
                accent(),
            )
            .draw(target)
            .unwrap();
            Text::new(
                "No signing without review",
                body_text_point(16, 102),
                body(),
            )
            .draw(target)
            .unwrap();
        }
        Screen::Review => draw_review(target, fonts, app, hold_action, hold_ticks),
        Screen::Sign => {
            Text::new(
                "Signing reviewed PSBT...",
                subheading_text_point(16, 76),
                accent(),
            )
            .draw(target)
            .unwrap();
            Text::new(
                "Verifying transaction \ndigest",
                body_text_point(16, 102),
                body(),
            )
            .draw(target)
            .unwrap();
        }
        Screen::Signed => {
            Text::new(
                "PSBT signed successfully",
                subheading_text_point(16, 76),
                accent(),
            )
            .draw(target)
            .unwrap();
            Text::new(
                "Preparing animated BBQr...",
                body_text_point(16, 102),
                body(),
            )
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
            draw_nav_back_button(target);
        }
        _ => {
            Text::new("State ready", subheading_text_point(16, 76), accent())
                .draw(target)
                .unwrap();
        }
    }
}

fn draw_indeterminate_progress<D>(target: &mut D, bounds: (i32, i32, u32, u32))
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    Rectangle::new(
        Point::new(bounds.0, bounds.1),
        Size::new(bounds.2, bounds.3),
    )
    .into_styled(PrimitiveStyle::with_stroke(palette::PROGRESS_OUTLINE, 1))
    .draw(target)
    .unwrap();

    let inner_height = bounds.3.saturating_sub(2);
    let mut left = bounds.0 + 1;
    let right = bounds.0 + bounds.2 as i32 - 1;
    while left < right {
        let width = 8u32.min((right - left) as u32);
        Rectangle::new(
            Point::new(left, bounds.1 + 1),
            Size::new(width, inner_height),
        )
        .into_styled(PrimitiveStyle::with_fill(palette::PROGRESS))
        .draw(target)
        .unwrap();
        left += 16;
    }
}

fn draw_confirmation_footer<D>(target: &mut D, fonts: &Fonts, hold_ticks: u16)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    draw_hold_bar(target, hold_ticks, BACKUP_CONFIRM_BAR);

    draw_confirmation_footer_icons(target, fonts, "Long press");
}

pub fn draw_confirmation_footer_icons<D>(target: &mut D, fonts: &Fonts, confirm_label: &str)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    draw_bitcoin_icon(
        target,
        BitcoinIcon::Cross,
        Point::new(18, 283),
        palette::TEXT,
    );
    draw_bitcoin_icon(
        target,
        BitcoinIcon::Check,
        Point::new(96, 283),
        palette::TEXT,
    );
    Text::new(
        confirm_label,
        Point::new(128, centered_text_top(BACKUP_CONFIRM_BUTTON, FONT_SIZE)),
        fonts.regular(palette::TEXT),
    )
    .draw(target)
    .unwrap();
}

/// Draws the live camera controls shared by the firmware scanner and the QR
/// example. The camera image is already present in the target; only the black
/// control band and its status are painted here.
pub fn draw_scan_overlay<D>(
    target: &mut D,
    fonts: &Fonts,
    zoom_factor: u8,
    progress: (usize, usize),
) where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    Rectangle::new(Point::new(0, 256), Size::new(DISPLAY_WIDTH, 64))
        .into_styled(PrimitiveStyle::with_fill(palette::BACKGROUND))
        .draw(target)
        .unwrap();
    Rectangle::new(
        Point::new(SCAN_PROGRESS_BAR.0, SCAN_PROGRESS_BAR.1),
        Size::new(SCAN_PROGRESS_BAR.2, SCAN_PROGRESS_BAR.3),
    )
    .into_styled(PrimitiveStyle::with_stroke(palette::PROGRESS_OUTLINE, 1))
    .draw(target)
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
        .into_styled(PrimitiveStyle::with_fill(palette::PROGRESS))
        .draw(target)
        .unwrap();
    }
    draw_bitcoin_icon(
        target,
        BitcoinIcon::Cross,
        Point::new(8, 288),
        palette::TEXT,
    );
    draw_bitcoin_icon(
        target,
        BitcoinIcon::Search,
        Point::new(172, 288),
        palette::TEXT,
    );
    Text::with_alignment(
        match zoom_factor {
            1 => "1x",
            _ => "2x",
        },
        text_point(224, 309),
        fonts.regular(palette::TEXT),
        Alignment::Right,
    )
    .draw(target)
    .unwrap();
}

/// Draws a bounded, printable representation of a decoded QR payload.
pub fn draw_qr_result<D>(target: &mut D, fonts: &Fonts, payload: &[u8])
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    Rectangle::new(Point::zero(), Size::new(DISPLAY_WIDTH, DISPLAY_HEIGHT))
        .into_styled(PrimitiveStyle::with_fill(palette::BACKGROUND))
        .draw(target)
        .unwrap();
    Text::new(
        "QR decoded",
        text_point(16, 24),
        fonts.regular(palette::TEXT),
    )
    .draw(target)
    .unwrap();

    let mut line = FixedString::<24>::new();
    let mut baseline = 52;
    for &byte in payload {
        if byte == b'\n' || line.len() == line.capacity() {
            Text::new(&line, text_point(16, baseline), fonts.body(palette::TEXT))
                .draw(target)
                .unwrap();
            line.clear();
            baseline += 17;
            if baseline > 250 {
                break;
            }
            if byte == b'\n' {
                continue;
            }
        }
        let printable = if byte.is_ascii_graphic() || byte == b' ' {
            byte as char
        } else {
            '.'
        };
        let _ = line.push(printable);
    }
    if !line.is_empty() && baseline <= 250 {
        Text::new(&line, text_point(16, baseline), fonts.body(palette::TEXT))
            .draw(target)
            .unwrap();
    }
    draw_confirmation_footer_icons(target, fonts, "Confirm");
}

fn draw_frost_confirmation_footer<D>(target: &mut D, fonts: &Fonts)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    draw_bitcoin_icon(target, BitcoinIcon::Edit, Point::new(8, 283), palette::TEXT);
    Text::new(
        "Edit",
        Point::new(38, centered_text_top(FROST_EDIT_BUTTON, FONT_SIZE)),
        fonts.regular(palette::TEXT),
    )
    .draw(target)
    .unwrap();
    draw_bitcoin_icon(
        target,
        BitcoinIcon::Check,
        Point::new(108, 283),
        palette::TEXT,
    );
    Text::new(
        "Confirm",
        Point::new(140, centered_text_top(FROST_CONFIRM_BUTTON, FONT_SIZE)),
        fonts.regular(palette::TEXT),
    )
    .draw(target)
    .unwrap();
}

fn draw_nav_back_button<D>(target: &mut D)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    draw_bitcoin_icon(
        target,
        BitcoinIcon::Cross,
        Point::new(108, 283),
        palette::TEXT,
    );
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
    draw_menu_item_with_icon_color(target, fonts, bounds, icon, label, palette::TEXT);
}

fn draw_menu_item_with_icon_color<D>(
    target: &mut D,
    fonts: &Fonts,
    bounds: (i32, i32, u32, u32),
    icon: BitcoinIcon,
    label: &str,
    icon_color: Rgb565,
) where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    let icon_top = bounds.1 + (bounds.3 as i32 - 24) / 2;
    draw_bitcoin_icon(target, icon, Point::new(bounds.0 + 4, icon_top), icon_color);
    Text::new(
        label,
        Point::new(bounds.0 + 36, centered_text_top(bounds, MENU_FONT_SIZE)),
        fonts.menu(palette::TEXT),
    )
    .draw(target)
    .unwrap();
}

fn draw_number_picker<D>(target: &mut D, fonts: &Fonts, value: u32, caption: &str)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    draw_button(target, fonts, (16, 80, 60, 44), palette::PRIMARY, "-");
    draw_button(target, fonts, (164, 80, 60, 44), palette::PRIMARY, "+");
    let number = alloc::format!("{}", value);
    Text::with_alignment(
        &number,
        text_point(120, 112),
        fonts.regular(palette::TEXT),
        Alignment::Center,
    )
    .draw(target)
    .unwrap();
    Text::with_alignment(
        caption,
        small_text_point(120, 140),
        fonts.small(palette::TEXT_MUTED),
        Alignment::Center,
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
        Point::new(
            bounds.0 + bounds.2 as i32 / 2,
            centered_text_top(bounds, FONT_SIZE),
        ),
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
    large: bool,
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
    let fingerprint_top = if large { top - 1 } else { top + 2 };
    Text::new(
        &fingerprint,
        Point::new(left + 32, fingerprint_top),
        if large {
            fonts.menu(palette::TEXT)
        } else {
            fonts.regular(palette::TEXT)
        },
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

fn draw_wallet_scrollbar<D>(target: &mut D, app: &AppState)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    let page_count = app.wallet_list_page_count();
    if page_count <= 1 {
        return;
    }
    const LEFT: i32 = 232;
    let height = WALLET_LIST_BOTTOM - WALLET_LIST_TOP;
    Rectangle::new(
        Point::new(LEFT, WALLET_LIST_TOP),
        Size::new(4, height as u32),
    )
    .into_styled(PrimitiveStyle::with_stroke(palette::PROGRESS_OUTLINE, 1))
    .draw(target)
    .unwrap();

    let thumb_height = height / page_count as i32;
    let thumb_top = WALLET_LIST_TOP + app.wallet_list_page() as i32 * thumb_height;
    Rectangle::new(
        Point::new(LEFT, thumb_top),
        Size::new(4, thumb_height as u32),
    )
    .into_styled(PrimitiveStyle::with_fill(palette::TEXT_MUTED))
    .draw(target)
    .unwrap();
}

fn draw_load_wallet<D>(target: &mut D, fonts: &Fonts)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    draw_menu_item(
        target,
        fonts,
        (16, 60, 208, 40),
        BitcoinIcon::Wallet,
        "Load wallet",
    );
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
    if app.wallets().is_empty() {
        Text::with_alignment(
            "No wallets saved",
            subheading_text_point(120, 112),
            fonts.subheading(palette::TEXT_MUTED),
            Alignment::Center,
        )
        .draw(target)
        .unwrap();
    }
    for (visible_index, wallet) in app
        .wallets()
        .iter()
        .skip(app.wallet_list_page() * WALLETS_PER_PAGE)
        .take(WALLETS_PER_PAGE)
        .enumerate()
    {
        let row_top = WALLET_LIST_TOP + visible_index as i32 * WALLET_LIST_ROW_HEIGHT;
        let identity_top = row_top + (WALLET_LIST_ROW_HEIGHT - 24) / 2;
        let network = match wallet.network {
            picosigner_board::Network::Mainnet => Network::Mainnet,
            picosigner_board::Network::Testnet4 => Network::Testnet4,
            picosigner_board::Network::Signet => Network::Signet,
        };
        draw_wallet_identity(
            target,
            fonts,
            8,
            identity_top,
            wallet.fingerprint.unwrap_or([0; 4]),
            network,
            true,
        );
    }
    draw_wallet_scrollbar(target, app);
    draw_nav_back_button(target);
}

fn draw_mnemonic_keyboard<D>(
    target: &mut D,
    fonts: &Fonts,
    app: &AppState,
    hold_action: Option<HoldAction>,
    hold_ticks: u16,
) where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    let word_number = (app.mnemonic_entered() + 1).min(app.mnemonic_target());
    let status = if app.screen() == Screen::FrostMnemonicEntry {
        alloc::format!("KEY #{}  WORD {}/25", app.frost_share_index(), word_number)
    } else {
        alloc::format!("WORD {}/{}", word_number, app.mnemonic_target())
    };
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
    let prefix = alloc::format!("> {}", app.mnemonic_prefix().to_ascii_uppercase());
    Text::new(
        &prefix,
        small_text_point(8, 44),
        fonts.small(palette::TEXT_MUTED),
    )
    .draw(target)
    .unwrap();
    for (index, word) in app.mnemonic_suggestions().iter().take(3).enumerate() {
        let top = 50 + index as i32 * 36;
        let word = word.to_ascii_uppercase();
        draw_button(target, fonts, (4, top, 116, 32), palette::PRIMARY, &word);
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
        let label = alloc::format!("{:>2}. {}", entry.0, entry.1.to_ascii_uppercase());
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
    if matches!(hold_action, Some(HoldAction::CancelEntry)) {
        draw_hold_bar(target, hold_ticks, MNEMONIC_CANCEL_BAR);
    }
}

fn draw_frost_mnemonic_confirmation<D>(target: &mut D, fonts: &Fonts, app: &AppState)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    Line::new(Point::new(119, 34), Point::new(119, 255))
        .into_styled(PrimitiveStyle::with_stroke(palette::SURFACE_ALT, 1))
        .draw(target)
        .unwrap();

    for position in 0..25 {
        let (column, row) = if position < 13 {
            (0, position)
        } else {
            (1, position - 13)
        };
        if let Some(word) = app.mnemonic_word(position) {
            let label = alloc::format!("{:>2}. {}", position + 1, word);
            Text::new(
                &label,
                small_text_point(4 + column as i32 * 120, 47 + row as i32 * 17),
                fonts.small(palette::TEXT),
            )
            .draw(target)
            .unwrap();
        }
    }

    draw_frost_confirmation_footer(target, fonts);
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
    draw_nav_back_button(target);
}

fn draw_home<D>(target: &mut D, fonts: &Fonts, app: &AppState)
where
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
            let label = alloc::format!("{:>2}. {}", index + 1, word.to_ascii_uppercase());
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
    let mut y = 48;
    let mut recipients = 0usize;
    for output in review.outputs.iter().filter(|output| !output.is_change) {
        recipients += 1;
        let amount = alloc::format!("SEND {} sats", output.value_sat);
        Text::new(
            &amount,
            subheading_text_point(12, y),
            fonts.subheading(palette::TEXT),
        )
        .draw(target)
        .unwrap();
        y += 20;
        if let Some(address) = &output.address {
            for chunk in address.as_bytes().chunks(29) {
                if y > 204 {
                    break;
                }
                if let Ok(line) = core::str::from_utf8(chunk) {
                    Text::new(line, body_text_point(12, y), fonts.body(palette::TEXT))
                        .draw(target)
                        .unwrap();
                }
                y += 16;
            }
        } else {
            Text::new(
                "Unknown output script",
                body_text_point(12, y),
                fonts.body(palette::TEXT),
            )
            .draw(target)
            .unwrap();
            y += 16;
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
        body_text_point(12, 236),
        fonts.body(palette::TEXT_MUTED),
    )
    .draw(target)
    .unwrap();
    let fee = match review.fee_sat {
        Some(value) => alloc::format!("Fee: {value} sats"),
        None => alloc::format!("Fee: unavailable"),
    };
    Text::new(
        &fee,
        body_text_point(12, 256),
        fonts.body(palette::TEXT_MUTED),
    )
    .draw(target)
    .unwrap();
    draw_confirmation_footer(
        target,
        fonts,
        if matches!(hold_action, Some(HoldAction::Sign)) {
            hold_ticks
        } else {
            0
        },
    );
}

pub fn draw_hold_bar<D>(display: &mut D, hold_ticks: u16, bounds: (i32, i32, u32, u32))
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    if hold_ticks > 0 {
        draw_hold_bar_progress(display, 0, hold_ticks, bounds);
    }
}

fn draw_hold_bar_outline<D>(display: &mut D, bounds: (i32, i32, u32, u32))
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    Rectangle::new(
        Point::new(bounds.0, bounds.1),
        Size::new(bounds.2, bounds.3),
    )
    .into_styled(PrimitiveStyle::with_stroke(palette::PROGRESS_OUTLINE, 1))
    .draw(display)
    .unwrap();
}

pub fn draw_hold_bar_progress<D>(
    display: &mut D,
    previous_ticks: u16,
    current_ticks: u16,
    bounds: (i32, i32, u32, u32),
) where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    let inner_width = bounds.2.saturating_sub(2);
    let previous = inner_width * u32::from(previous_ticks.min(30)) / 30;
    let current = inner_width * u32::from(current_ticks.min(30)) / 30;
    if current > previous {
        if previous_ticks == 0 {
            draw_hold_bar_outline(display, bounds);
        }
        Rectangle::new(
            Point::new(bounds.0 + 1 + previous as i32, bounds.1 + 1),
            Size::new(current - previous, bounds.3.saturating_sub(2)),
        )
        .into_styled(PrimitiveStyle::with_fill(palette::PROGRESS))
        .draw(display)
        .unwrap();
    }
}

pub fn reset_hold_bar<D>(display: &mut D, bounds: (i32, i32, u32, u32))
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    Rectangle::new(
        Point::new(bounds.0, bounds.1),
        Size::new(bounds.2, bounds.3),
    )
    .into_styled(PrimitiveStyle::with_fill(palette::BACKGROUND))
    .draw(display)
    .unwrap();
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
        assert_eq!(
            hold_bar_bounds(HoldAction::DeleteWallet),
            Some(BACKUP_CONFIRM_BAR)
        );
        assert_eq!(
            action_at(Screen::Review, 200, 16),
            Some(HoldAction::SwipeBack)
        );
        assert!(supports_swipe_back(Screen::Review));
        assert!(!supports_swipe_back(Screen::MnemonicEntry));
        assert!(supports_swipe_back(Screen::FrostMnemonicConfirm));
        assert_eq!(
            action_at(Screen::FrostMnemonicConfirm, 40, 290),
            Some(HoldAction::FrostEditShare)
        );
        assert_eq!(
            action_at(Screen::FrostMnemonicConfirm, 180, 290),
            Some(HoldAction::FrostAcceptShare)
        );
        assert_eq!(
            action_at(Screen::Review, 120, 250),
            Some(HoldAction::SwipeBack)
        );
        assert_eq!(action_at(Screen::Network, 120, 290), Some(HoldAction::Back));
        assert_eq!(action_at(Screen::Review, 24, 285), Some(HoldAction::Back));
        assert_eq!(action_at(Screen::Review, 120, 300), Some(HoldAction::Sign));
        assert_eq!(action_at(Screen::Backup, 24, 285), Some(HoldAction::Cancel));
        assert_eq!(
            action_at(Screen::Backup, 120, 285),
            Some(HoldAction::Backup)
        );
        assert_eq!(action_at(Screen::Backup, 120, 250), None);
        assert!(point_in_u16_button(10, 300, SCAN_CANCEL_BUTTON));
        assert!(point_in_u16_button(200, 300, SCAN_ZOOM_BUTTON));
        assert!(!point_in_u16_button(200, 10, SCAN_CANCEL_BUTTON));
        assert!(in_button(24, 300, QR_RESULT_CANCEL_BUTTON));
        assert!(in_button(180, 300, QR_RESULT_CONFIRM_BUTTON));
        assert!(!in_button(120, 250, QR_RESULT_CONFIRM_BUTTON));
        assert_eq!(
            action_at(Screen::BackupWords, 120, 308),
            Some(HoldAction::Back)
        );
        assert_eq!(
            action_at(Screen::BackupWords, 20, 200),
            Some(HoldAction::BackupPage)
        );
        assert_eq!(
            action_at(Screen::MnemonicEntry, 85, 304),
            Some(HoldAction::Backspace)
        );
        assert_eq!(
            action_at(Screen::MnemonicEntry, 120, 304),
            Some(HoldAction::Backspace)
        );
        assert_eq!(
            action_at(Screen::MnemonicEntry, 155, 304),
            Some(HoldAction::Backspace)
        );
        assert_eq!(
            action_at(Screen::MnemonicEntry, 165, 304),
            Some(HoldAction::CancelEntry)
        );
        assert_eq!(
            action_at(Screen::MnemonicEntry, 235, 304),
            Some(HoldAction::CancelEntry)
        );
        assert_eq!(
            action_at(Screen::MnemonicEntry, 180, 88),
            Some(HoldAction::SwipeBack)
        );
        assert_eq!(
            action_at(Screen::MnemonicEntry, 60, 60),
            Some(HoldAction::Suggestion(0))
        );
        assert_eq!(
            action_at(Screen::MnemonicEntry, 60, 84),
            Some(HoldAction::SwipeBack)
        );
        assert_eq!(
            action_at(Screen::MnemonicEntry, 60, 96),
            Some(HoldAction::Suggestion(1))
        );
    }
}
