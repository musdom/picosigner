//! BIP39 word entry.
//!
//! The user types a prefix and picks from the matching wordlist entries, so the
//! device never renders a full keyboard's worth of validation. Entered words
//! are held as wordlist indices rather than strings, and both the indices and
//! the in-progress prefix are zeroized when entry is abandoned — see
//! `zero_setup_secrets` in the parent module.

use alloc::string::String;
use picosigner_ui::Screen;
use zeroize::Zeroize;

use super::{AppError, AppState};
use picosigner_wallet::{bip39_words_by_prefix, WalletError, WalletSecret};

impl AppState {
    pub fn start_mnemonic_entry(&mut self, words: u8) -> Result<(), AppError> {
        if words != 12 && words != 24 {
            return Err(AppError::InvalidTransition);
        }
        self.clear_mnemonic_input();
        self.mnemonic_word_count = words;
        self.screen = Screen::MnemonicEntry;
        Ok(())
    }
    pub fn mnemonic_prefix(&self) -> &str {
        &self.mnemonic_prefix
    }
    pub fn mnemonic_entered(&self) -> usize {
        self.mnemonic_words.len()
    }
    pub fn mnemonic_target(&self) -> usize {
        usize::from(self.mnemonic_word_count)
    }
    pub fn mnemonic_error(&self) -> bool {
        self.mnemonic_error
    }
    pub fn mnemonic_suggestions(&self) -> &[&'static str] {
        if self.mnemonic_prefix.is_empty() {
            &[]
        } else {
            bip39_words_by_prefix(&self.mnemonic_prefix)
        }
    }
    pub fn recent_mnemonic_words(&self) -> [Option<(usize, &'static str)>; 3] {
        let mut recent = [None; 3];
        let start = self.mnemonic_words.len().saturating_sub(3);
        let list = picosigner_wallet::bip39_words_by_prefix("");
        for (output, position) in (start..self.mnemonic_words.len()).enumerate() {
            let index = usize::from(self.mnemonic_words[position]);
            recent[output] = Some((position + 1, list[index]));
        }
        recent
    }
    pub fn mnemonic_word(&self, position: usize) -> Option<&'static str> {
        let index = usize::from(*self.mnemonic_words.get(position)?);
        picosigner_wallet::bip39_words_by_prefix("")
            .get(index)
            .copied()
    }
    pub fn mnemonic_letter(&mut self, letter: u8) {
        if letter.is_ascii_lowercase() && self.mnemonic_prefix.len() < 9 {
            let _ = self.mnemonic_prefix.push(char::from(letter));
            self.mnemonic_error = false;
        }
    }
    pub fn mnemonic_backspace(&mut self) {
        let removed_prefix = if self.mnemonic_prefix.is_empty() {
            false
        } else {
            // SAFETY: prefixes contain ASCII only; replacing the final byte
            // with NUL keeps valid UTF-8 before `pop` shortens the string.
            unsafe {
                if let Some(last) = self.mnemonic_prefix.as_mut_vec().as_mut_slice().last_mut() {
                    *last = 0;
                }
            }
            let _ = self.mnemonic_prefix.pop();
            true
        };
        if !removed_prefix {
            if let Some(index) = self.mnemonic_words.pop() {
                if let Some(word) =
                    picosigner_wallet::bip39_words_by_prefix("").get(usize::from(index))
                {
                    let _ = self.mnemonic_prefix.push_str(word);
                }
            }
        }
        self.mnemonic_error = false;
    }
    pub fn choose_mnemonic_suggestion(&mut self, suggestion: usize) -> Result<(), AppError> {
        let word = *self
            .mnemonic_suggestions()
            .get(suggestion)
            .ok_or(AppError::InvalidTransition)?;
        let index = picosigner_wallet::bip39_words_by_prefix("")
            .binary_search(&word)
            .map_err(|_| AppError::InvalidTransition)? as u16;
        self.mnemonic_words
            .push(index)
            .map_err(|_| AppError::InvalidTransition)?;
        self.zero_mnemonic_prefix();
        if self.mnemonic_words.len() == self.mnemonic_target()
            && self.screen == Screen::FrostMnemonicEntry
        {
            self.screen = Screen::FrostMnemonicConfirm;
        } else if self.mnemonic_words.len() == self.mnemonic_target() {
            let mut phrase = String::new();
            let list = picosigner_wallet::bip39_words_by_prefix("");
            for (position, index) in self.mnemonic_words.iter().enumerate() {
                if position != 0 {
                    phrase.push(' ');
                }
                phrase.push_str(list[usize::from(*index)]);
            }
            match WalletSecret::from_mnemonic(&phrase) {
                Ok(secret) => {
                    phrase.zeroize();
                    self.clear_mnemonic_input();
                    self.accept_import_secret(secret);
                }
                Err(_) => {
                    phrase.zeroize();
                    self.mnemonic_error = true;
                    return Err(AppError::Wallet(WalletError::InvalidMnemonic));
                }
            }
        }
        Ok(())
    }
    pub(super) fn clear_mnemonic_input(&mut self) {
        for index in self.mnemonic_words.iter_mut() {
            index.zeroize();
        }
        self.mnemonic_words.clear();
        self.zero_mnemonic_prefix();
        self.mnemonic_error = false;
    }
    pub(super) fn zero_mnemonic_prefix(&mut self) {
        // SAFETY: zeroing ASCII bytes preserves UTF-8 validity until clear
        // resets the logical length, and no alias to the string is retained.
        unsafe { self.mnemonic_prefix.as_mut_vec().as_mut_slice().zeroize() };
        self.mnemonic_prefix.clear();
    }
}
