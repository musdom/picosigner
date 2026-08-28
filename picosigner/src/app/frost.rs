//! Recovering a Frostsnap wallet from its 25-word backup shares.
//!
//! The user enters `threshold` shares, each a 25-word backup carrying its own
//! index and checksum. Once enough have been collected the secret is
//! interpolated from them; the individual shares are cleared as soon as that
//! succeeds, since each is as sensitive as the seed it reconstructs.

use picosigner_ui::Screen;
use zeroize::Zeroize;

use super::{AppError, AppState};
use picosigner_wallet::{
    recover_frost_secret, FrostShareBackup, WalletError, WalletKind, WalletSecret,
};

impl AppState {
    pub fn choose_frost_import(&mut self) {
        self.clear_mnemonic_input();
        self.frost_shares.clear();
        self.frost_threshold = 2;
        self.frost_share_index = 1;
        self.pending_secret = None;
        self.pending_stored = None;
        self.zero_passphrase();
        self.pending_kind = WalletKind::Frost;
        self.screen = Screen::FrostThreshold;
    }
    pub fn frost_threshold(&self) -> u8 {
        self.frost_threshold
    }
    pub fn frost_share_index(&self) -> u32 {
        self.frost_share_index
    }
    pub fn frost_shares_entered(&self) -> usize {
        self.frost_shares.len()
    }
    pub fn decrease_frost_threshold(&mut self) {
        self.frost_threshold = self.frost_threshold.saturating_sub(1).max(1);
    }
    pub fn increase_frost_threshold(&mut self) {
        self.frost_threshold = self.frost_threshold.saturating_add(1).min(8);
    }
    pub fn confirm_frost_threshold(&mut self) {
        self.frost_shares.clear();
        self.frost_share_index = 1;
        self.screen = Screen::FrostShareIndex;
    }
    pub fn decrease_frost_share_index(&mut self) {
        self.notice = None;
        self.frost_share_index = self.frost_share_index.saturating_sub(1).max(1);
    }
    pub fn increase_frost_share_index(&mut self) {
        self.notice = None;
        self.frost_share_index = self.frost_share_index.saturating_add(1);
    }
    pub fn start_frost_share_entry(&mut self) -> Result<(), AppError> {
        if self
            .frost_shares
            .iter()
            .any(|share| share.index() == self.frost_share_index)
        {
            self.notice = Some("KEY NUMBER ALREADY USED");
            return Err(AppError::Wallet(WalletError::InvalidFrostBackup));
        }
        self.notice = None;
        self.clear_mnemonic_input();
        self.mnemonic_word_count = 25;
        self.screen = Screen::FrostMnemonicEntry;
        Ok(())
    }
    pub fn edit_frost_share(&mut self) -> Result<(), AppError> {
        if self.screen != Screen::FrostMnemonicConfirm || self.mnemonic_words.len() != 25 {
            return Err(AppError::InvalidTransition);
        }
        self.mnemonic_error = false;
        self.screen = Screen::FrostMnemonicEntry;
        self.mnemonic_backspace();
        Ok(())
    }
    pub fn confirm_frost_share(&mut self) -> Result<(), AppError> {
        if self.screen != Screen::FrostMnemonicConfirm || self.mnemonic_words.len() != 25 {
            return Err(AppError::InvalidTransition);
        }
        let mut words = [0u16; 25];
        words.copy_from_slice(&self.mnemonic_words);
        let share = match FrostShareBackup::from_word_indices(self.frost_share_index, words) {
            Ok(share) => share,
            Err(_) => {
                self.screen = Screen::FrostMnemonicEntry;
                self.mnemonic_backspace();
                self.mnemonic_error = true;
                return Err(AppError::Wallet(WalletError::InvalidFrostBackup));
            }
        };
        self.clear_mnemonic_input();
        self.frost_shares
            .push(share)
            .map_err(|_| AppError::InvalidTransition)?;
        if self.frost_shares.len() == usize::from(self.frost_threshold) {
            match recover_frost_secret(&self.frost_shares) {
                Ok(mut bytes) => {
                    let secret = WalletSecret::from(bytes);
                    bytes.zeroize();
                    self.frost_shares.clear();
                    self.pending_kind = WalletKind::Frost;
                    self.accept_import_secret(secret);
                }
                Err(_) => {
                    let _ = self.frost_shares.pop();
                    self.notice = Some("BACKUPS DO NOT MATCH");
                    self.screen = Screen::FrostShareIndex;
                    return Err(AppError::Wallet(WalletError::InvalidFrostBackup));
                }
            }
        } else {
            while self
                .frost_shares
                .iter()
                .any(|share| share.index() == self.frost_share_index)
            {
                self.frost_share_index = self.frost_share_index.saturating_add(1);
            }
            self.screen = Screen::FrostShareIndex;
        }
        Ok(())
    }
}
