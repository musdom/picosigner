//! Touch input: what a tap or hold at a point means on each screen.
//!
//! [`HoldAction`] is the vocabulary of everything the UI can be asked to do;
//! [`action_at`] maps a touch point to one. Both the renderer and this module
//! read their geometry from [`crate::layout`], so a control is touchable
//! exactly where it is drawn — the two used to be written separately and could
//! disagree.

use picosigner_ui::{Screen, Widget};

use crate::{
    app::{AppState, WALLETS_PER_PAGE},
    layout,
};
use picosigner_wallet::{AccountType, Network, WalletKind};

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
pub(crate) const WALLET_LIST_TOP: i32 = 44;
pub(crate) const WALLET_LIST_ROW_HEIGHT: i32 = 54;
pub(crate) const WALLET_LIST_BOTTOM: i32 =
    WALLET_LIST_TOP + WALLET_LIST_ROW_HEIGHT * WALLETS_PER_PAGE as i32;

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
    Derivation(AccountType),
    NoPassphrase,
    EnterPassphrase,
    ConfirmPassphrase,
    PassphraseSpace,
    PassphraseKey(usize),
    GenerationWords(u8),
    CameraEntropy,
    DiceEntropy,
    DiceRoll(u8),
    EntropyDone,
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
    OpenCancelEntry,
    ConfirmCancelEntry,
    SwipeBack,
    Cancel,
    BackupPage,
}

pub fn action_at(screen: Screen, x: u16, y: u16) -> Option<HoldAction> {
    match screen {
        screen if shows_nav_back_button(screen) && in_button(x, y, NAV_BACK_BUTTON) => {
            Some(HoldAction::Back)
        }
        Screen::LoadWallet if menu_hit(layout::menus::LOAD_WALLET, x, y, 0) => {
            Some(HoldAction::Load)
        }
        Screen::LoadWallet if menu_hit(layout::menus::LOAD_WALLET, x, y, 2) => {
            Some(HoldAction::Import)
        }
        Screen::LoadWallet if menu_hit(layout::menus::LOAD_WALLET, x, y, 3) => {
            Some(HoldAction::Create)
        }
        Screen::WalletList if (y as i32) >= WALLET_LIST_TOP && (y as i32) < WALLET_LIST_BOTTOM => {
            Some(HoldAction::Wallet(
                ((y as i32 - WALLET_LIST_TOP) / WALLET_LIST_ROW_HEIGHT) as usize,
            ))
        }
        Screen::WalletList => Some(HoldAction::SwipeBack),
        Screen::ImportWallet if menu_hit(layout::menus::IMPORT_WALLET, x, y, 0) => {
            Some(HoldAction::ScanSeedQr)
        }
        Screen::ImportWallet if menu_hit(layout::menus::IMPORT_WALLET, x, y, 1) => {
            Some(HoldAction::EnterMnemonic)
        }
        Screen::ImportWallet if menu_hit(layout::menus::IMPORT_WALLET, x, y, 2) => {
            Some(HoldAction::EnterFrost)
        }
        Screen::ImportWallet => Some(HoldAction::SwipeBack),
        Screen::MnemonicLength if menu_hit(layout::menus::MNEMONIC_LENGTH, x, y, 0) => {
            Some(HoldAction::MnemonicWords(12))
        }
        Screen::MnemonicLength if menu_hit(layout::menus::MNEMONIC_LENGTH, x, y, 1) => {
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
            // Letters come from the same grid the renderer draws. The last
            // row is shared: Y and Z sit left of the Erase and Cancel buttons,
            // which start at MNEMONIC_ERASE_LEFT and MNEMONIC_CANCEL_LEFT.
            match mnemonic_key_at(x, y) {
                Some(index) => Some(HoldAction::Key(b'a' + index as u8)),
                None if x < layout::mnemonic::CANCEL_LEFT => Some(HoldAction::Backspace),
                None => Some(HoldAction::OpenCancelEntry),
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
        Screen::Network if menu_hit(layout::menus::NETWORK, x, y, 0) => {
            Some(HoldAction::Network(Network::Mainnet))
        }
        Screen::Network if menu_hit(layout::menus::NETWORK, x, y, 1) => {
            Some(HoldAction::Network(Network::Testnet4))
        }
        Screen::Network if menu_hit(layout::menus::NETWORK, x, y, 2) => {
            Some(HoldAction::Network(Network::Signet))
        }
        Screen::Network => Some(HoldAction::SwipeBack),
        Screen::Derivation if menu_hit(layout::menus::DERIVATION, x, y, 0) => {
            Some(HoldAction::Derivation(AccountType::NestedSegwit))
        }
        Screen::Derivation if menu_hit(layout::menus::DERIVATION, x, y, 1) => {
            Some(HoldAction::Derivation(AccountType::NativeSegwit))
        }
        Screen::Derivation if menu_hit(layout::menus::DERIVATION, x, y, 2) => {
            Some(HoldAction::Derivation(AccountType::Taproot))
        }
        Screen::Derivation => Some(HoldAction::SwipeBack),
        Screen::PassphraseChoice if in_button(x, y, (16, 80, 208, 44)) => {
            Some(HoldAction::NoPassphrase)
        }
        Screen::PassphraseChoice if in_button(x, y, (16, 140, 208, 44)) => {
            Some(HoldAction::EnterPassphrase)
        }
        Screen::PassphraseChoice => Some(HoldAction::SwipeBack),
        Screen::PassphraseEntry if in_button(x, y, BACKUP_CANCEL_BUTTON) => {
            Some(HoldAction::OpenCancelEntry)
        }
        Screen::PassphraseEntry if in_button(x, y, BACKUP_CONFIRM_BUTTON) => {
            Some(HoldAction::ConfirmPassphrase)
        }
        Screen::PassphraseEntry if passphrase_key_at(x, y).is_some() => {
            passphrase_key_at(x, y).map(HoldAction::PassphraseKey)
        }
        Screen::PassphraseEntry => None,
        Screen::GenerationWords if in_button(x, y, (16, 80, 208, 44)) => {
            Some(HoldAction::GenerationWords(12))
        }
        Screen::GenerationWords if in_button(x, y, (16, 140, 208, 44)) => {
            Some(HoldAction::GenerationWords(24))
        }
        Screen::GenerationWords => Some(HoldAction::SwipeBack),
        Screen::EntropySources if in_button(x, y, (16, 70, 208, 44)) => {
            Some(HoldAction::CameraEntropy)
        }
        Screen::EntropySources if in_button(x, y, (16, 130, 208, 44)) => {
            Some(HoldAction::DiceEntropy)
        }
        Screen::EntropySources if in_button(x, y, BACKUP_CANCEL_BUTTON) => Some(HoldAction::Cancel),
        Screen::EntropySources if in_button(x, y, BACKUP_CONFIRM_BUTTON) => {
            Some(HoldAction::EntropyDone)
        }
        Screen::EntropySources => None,
        Screen::DiceEntropy if dice_key_at(x, y).is_some() => {
            // Same grid the renderer draws from, so a face is touchable
            // exactly where it appears.
            dice_key_at(x, y).map(HoldAction::DiceRoll)
        }
        Screen::DiceEntropy if in_button(x, y, BACKUP_CONFIRM_BUTTON) => {
            Some(HoldAction::EntropyDone)
        }
        Screen::DiceEntropy if in_button(x, y, BACKUP_CANCEL_BUTTON) => Some(HoldAction::Cancel),
        Screen::DiceEntropy => None,
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
        Screen::CancelEntryConfirm if in_button(x, y, BACKUP_CANCEL_BUTTON) => {
            Some(HoldAction::Back)
        }
        Screen::CancelEntryConfirm if in_button(x, y, BACKUP_CONFIRM_BUTTON) => {
            Some(HoldAction::ConfirmCancelEntry)
        }
        Screen::CancelEntryConfirm => None,
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
        Some(HoldAction::Derivation(_)) if app.pending_wallet_kind() == WalletKind::Frost => {
            Some(HoldAction::Derivation(AccountType::Taproot))
        }
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
        | HoldAction::Derivation(_)
        | HoldAction::NoPassphrase
        | HoldAction::EnterPassphrase
        | HoldAction::ConfirmPassphrase
        | HoldAction::PassphraseSpace
        | HoldAction::PassphraseKey(_)
        | HoldAction::GenerationWords(_)
        | HoldAction::CameraEntropy
        | HoldAction::DiceEntropy
        | HoldAction::DiceRoll(_)
        | HoldAction::EntropyDone
        | HoldAction::BackupMenu
        | HoldAction::MnemonicBackup
        | HoldAction::SeedQrBackup
        | HoldAction::Logout
        | HoldAction::SaveWallet
        | HoldAction::OpenDeleteWallet
        | HoldAction::Suggestion(_)
        | HoldAction::Key(_)
        | HoldAction::Backspace
        | HoldAction::OpenCancelEntry
        | HoldAction::ConfirmCancelEntry
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
            | Screen::PassphraseChoice
            | Screen::GenerationWords
            | Screen::BackupMenu
            | Screen::BackupSeedQr
            | Screen::Descriptor
            | Screen::Review
            | Screen::DeleteWallet
            | Screen::CancelEntryConfirm
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
            | Screen::PassphraseChoice
            | Screen::GenerationWords
            | Screen::BackupMenu
            | Screen::BackupWords
            | Screen::BackupSeedQr
            | Screen::Descriptor
    )
}

/// Whether a touch falls on one menu row.
fn menu_hit(menu: picosigner_ui::MenuList, x: u16, y: u16, slot: u16) -> bool {
    menu.row(slot).contains(x, y)
}

/// The key index at a touch point, from the shared passphrase grid.
fn passphrase_key_at(x: u16, y: u16) -> Option<usize> {
    let mut grid = layout::passphrase::grid();
    grid.set_bounds(layout::passphrase::BOUNDS);
    grid.hit(x, y)
}

/// The letter index at a touch point, from the shared mnemonic grid.
fn mnemonic_key_at(x: u16, y: u16) -> Option<usize> {
    let mut grid = layout::mnemonic::grid();
    grid.set_bounds(layout::mnemonic::BOUNDS);
    grid.hit(x, y)
}

/// The die face at a touch point, from the shared dice grid.
fn dice_key_at(x: u16, y: u16) -> Option<u8> {
    let mut grid = layout::dice::grid();
    grid.set_bounds(layout::dice::BOUNDS);
    grid.hit(x, y).map(layout::dice::face)
}
