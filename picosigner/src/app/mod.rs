//! Application state orchestration. Hardware adapters stay outside this state machine.
//!
//! The state machine owns the security-critical ordering: generated wallets
//! must be backed up before use, Flash persistence is always explicit, every
//! PSBT is reviewed before signing, and the reviewed digest is checked again
//! by the wallet immediately before signatures are produced.
extern crate alloc;

mod frost;
mod mnemonic;

use alloc::vec::Vec;
use picosigner_board::{
    CryptoEntropy, StoredSeed as BoardSeed, WalletCatalog, WalletId, WalletStore,
    WalletStoreErrorKind, MAX_DESCRIPTOR_BYTES,
};
use picosigner_ui::{Navigator, Screen};
use picosigner_wallet::{
    account_type_from_descriptor, AccountType, EntropyMixer, EntropySource, FrostShareBackup,
    Network, PsbtReview, SignedPsbt, StrictSigningPolicy, WalletError, WalletKind, WalletSecret,
    WalletSession,
};
use zeroize::Zeroize;

#[derive(Debug, PartialEq, Eq)]
pub enum AppError {
    NoSeed,
    BackupRequired,
    InvalidTransition,
    Entropy,
    Storage,
    Wallet(WalletError),
    DuplicateWallet,
    StoreFull,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupFlow {
    Create,
    Import,
}

pub const WALLETS_PER_PAGE: usize = 4;

pub struct AppState {
    screen: Screen,
    network: Network,
    wallet: Option<WalletSession>,
    wallets: WalletCatalog,
    wallet_list_page: usize,
    active_wallet: Option<WalletId>,
    setup_flow: Option<SetupFlow>,
    pending_secret: Option<WalletSecret>,
    pending_kind: WalletKind,
    account_type: AccountType,
    passphrase: heapless::String<64>,
    passphrase_mode: u8,
    generation_words: u8,
    entropy_mixer: EntropyMixer,
    last_entropy_sources: u8,
    pending_stored: Option<(WalletId, BoardSeed)>,
    mnemonic_words: heapless::Vec<u16, 25>,
    mnemonic_prefix: heapless::String<10>,
    mnemonic_word_count: u8,
    mnemonic_error: bool,
    frost_threshold: u8,
    frost_share_index: u32,
    frost_shares: heapless::Vec<FrostShareBackup, 8>,
    backup_page: u8,
    notice: Option<&'static str>,
    backup_acknowledged: bool,
    review: Option<PsbtReview>,
    pending_psbt: Option<Vec<u8>>,
    signed: Option<SignedPsbt>,
    /// Where [`Screen::CancelEntryConfirm`] returns to on "No", so declining
    /// the prompt keeps whatever mnemonic or passphrase entry was in
    /// progress instead of routing through a screen's usual back handling
    /// (which clears that entry).
    cancel_entry_return: Option<Screen>,
}

impl AppState {
    pub fn new(network: Network) -> Self {
        Self {
            screen: Screen::Boot,
            network,
            wallet: None,
            wallets: WalletCatalog::empty(),
            wallet_list_page: 0,
            active_wallet: None,
            setup_flow: None,
            pending_secret: None,
            pending_kind: WalletKind::Bip39,
            account_type: AccountType::Taproot,
            passphrase: heapless::String::new(),
            passphrase_mode: 0,
            generation_words: 12,
            entropy_mixer: EntropyMixer::new(),
            last_entropy_sources: 0,
            pending_stored: None,
            mnemonic_words: heapless::Vec::new(),
            mnemonic_prefix: heapless::String::new(),
            mnemonic_word_count: 12,
            mnemonic_error: false,
            frost_threshold: 2,
            frost_share_index: 1,
            frost_shares: heapless::Vec::new(),
            backup_page: 0,
            notice: None,
            backup_acknowledged: false,
            review: None,
            pending_psbt: None,
            signed: None,
            cancel_entry_return: None,
        }
    }

    pub fn screen(&self) -> Screen {
        self.screen
    }
    pub fn wallet(&self) -> Option<&WalletSession> {
        self.wallet.as_ref()
    }
    pub fn wallets(&self) -> &WalletCatalog {
        &self.wallets
    }
    pub fn wallet_list_page(&self) -> usize {
        self.wallet_list_page
    }
    pub fn wallet_list_page_count(&self) -> usize {
        self.wallets.len().max(1).div_ceil(WALLETS_PER_PAGE)
    }
    pub fn next_wallet_list_page(&mut self) {
        if self.wallet_list_page + 1 < self.wallet_list_page_count() {
            self.wallet_list_page += 1;
        }
    }
    pub fn previous_wallet_list_page(&mut self) {
        self.wallet_list_page = self.wallet_list_page.saturating_sub(1);
    }
    pub fn selected_network(&self) -> Network {
        self.network
    }
    pub fn setup_flow(&self) -> Option<SetupFlow> {
        self.setup_flow
    }
    pub fn pending_wallet_kind(&self) -> WalletKind {
        self.pending_kind
    }
    pub fn account_type(&self) -> AccountType {
        self.account_type
    }
    pub fn generation_words(&self) -> u8 {
        self.generation_words
    }
    pub fn passphrase_len(&self) -> usize {
        self.passphrase.len()
    }
    pub fn passphrase_display(&self) -> &str {
        &self.passphrase
    }
    pub fn passphrase_mode(&self) -> u8 {
        self.passphrase_mode
    }
    pub fn dice_rolls(&self) -> u16 {
        self.entropy_mixer.dice_rolls()
    }
    pub fn has_entropy_source(&self, source: EntropySource) -> bool {
        self.entropy_mixer.has_source(source)
    }
    pub fn used_entropy_source(&self, source: EntropySource) -> bool {
        self.last_entropy_sources & (1 << source as u8) != 0
    }
    pub fn notice(&self) -> Option<&'static str> {
        self.notice
    }
    pub fn show_error(&mut self, error: AppError) {
        self.notice = Some(match error {
            AppError::DuplicateWallet => "WALLET ALREADY STORED",
            AppError::StoreFull => "WALLET STORE FULL (8)",
            AppError::Wallet(WalletError::InvalidMnemonic) => "INVALID BIP39 CHECKSUM",
            AppError::Wallet(WalletError::InvalidFrostBackup) => "INVALID FROST BACKUP",
            _ => "WALLET OPERATION FAILED",
        });
        self.screen = Screen::Recovery;
    }
    pub fn active_wallet_id(&self) -> Option<WalletId> {
        self.active_wallet
    }
    pub fn wallet_is_saved(&self) -> bool {
        self.active_wallet.is_some()
    }
    pub fn begin_create(&mut self) {
        self.setup_flow = Some(SetupFlow::Create);
        self.account_type = AccountType::Taproot;
        self.generation_words = 12;
        self.entropy_mixer = EntropyMixer::new();
        self.last_entropy_sources = 0;
        self.zero_passphrase();
        self.screen = Screen::Network;
    }
    pub fn begin_import(&mut self) {
        self.setup_flow = Some(SetupFlow::Import);
        self.account_type = AccountType::Taproot;
        self.zero_passphrase();
        self.screen = Screen::ImportWallet;
    }
    pub fn choose_mnemonic_import(&mut self) {
        self.pending_kind = WalletKind::Bip39;
        self.screen = Screen::MnemonicLength;
    }
    pub fn choose_seed_qr_import(&mut self) {
        self.pending_kind = WalletKind::Bip39;
        self.screen = Screen::SeedQrScan;
    }

    fn zero_passphrase(&mut self) {
        // SAFETY: the buffer contains ASCII entered by the on-screen keyboard.
        unsafe { self.passphrase.as_mut_vec().as_mut_slice().zeroize() };
        self.passphrase.clear();
        self.passphrase_mode = 0;
    }
    pub fn passphrase_letter(&mut self, letter: u8) {
        if letter.is_ascii_lowercase() {
            let _ = self.passphrase.push(letter as char);
        }
    }
    pub fn passphrase_space(&mut self) {
        let _ = self.passphrase.push(' ');
    }
    pub fn passphrase_key(&mut self, index: usize) {
        use picosigner_ui::passphrase_keyboard::{key, Key, MODES};
        match key(index, self.passphrase_mode) {
            Key::Char(byte) => {
                let _ = self.passphrase.push(byte as char);
            }
            Key::Backspace => self.passphrase_backspace(),
            Key::Shift => self.passphrase_mode = (self.passphrase_mode + 1) % MODES,
            Key::Space => self.passphrase_space(),
        }
    }
    pub fn passphrase_backspace(&mut self) {
        let _ = self.passphrase.pop();
    }
    pub fn select_account_type(&mut self, account_type: AccountType) {
        if self.pending_kind == WalletKind::Frost && account_type != AccountType::Taproot {
            return;
        }
        self.account_type = account_type;
        self.screen = if self.pending_kind == WalletKind::Frost {
            Screen::Deriving
        } else {
            Screen::PassphraseChoice
        };
    }
    pub fn choose_no_passphrase(&mut self) {
        self.zero_passphrase();
        self.after_passphrase();
    }
    pub fn begin_passphrase_entry(&mut self) {
        self.zero_passphrase();
        self.screen = Screen::PassphraseEntry;
    }
    pub fn confirm_passphrase(&mut self) {
        self.after_passphrase();
    }
    fn after_passphrase(&mut self) {
        if self.pending_stored.is_some() || matches!(self.setup_flow, Some(SetupFlow::Import)) {
            self.screen = Screen::Deriving;
        } else {
            self.screen = Screen::GenerationWords;
        }
    }
    pub fn select_generation_words(&mut self, words: u8) -> Result<(), AppError> {
        if words != 12 && words != 24 {
            return Err(AppError::InvalidTransition);
        }
        self.generation_words = words;
        self.screen = Screen::EntropySources;
        Ok(())
    }
    pub fn begin_camera_entropy(&mut self) {
        self.screen = Screen::CameraEntropy;
    }
    pub fn accept_camera_entropy(&mut self, frame: &[u8]) {
        self.entropy_mixer.absorb(EntropySource::Camera, frame);
        self.screen = Screen::EntropySources;
    }
    pub fn begin_dice_entropy(&mut self) {
        self.screen = Screen::DiceEntropy;
    }
    pub fn add_dice_roll(&mut self, roll: u8) -> Result<(), AppError> {
        self.entropy_mixer
            .add_d6_roll(roll)
            .map_err(AppError::Wallet)
    }
    pub fn finish_dice_entropy(&mut self) {
        self.screen = Screen::EntropySources;
    }
    /// Clears every secret gathered while setting up or importing a wallet.
    ///
    /// `pending_stored` holds raw BIP39 entropy, so it is zeroized rather than
    /// dropped. Every path that abandons a session must go through here; a
    /// caller that clears these fields itself will eventually miss one.
    fn zero_setup_secrets(&mut self) {
        self.pending_secret = None;
        if let Some((_, mut seed)) = self.pending_stored.take() {
            seed.entropy.zeroize();
        }
        self.zero_passphrase();
        self.clear_mnemonic_input();
        self.frost_shares.clear();
        self.pending_kind = WalletKind::Bip39;
    }

    /// Clears the PSBT under review and any signature produced from it.
    ///
    /// The pending PSBT is zeroized: it is attacker-supplied but describes the
    /// user's own transaction.
    fn zero_signing_state(&mut self) {
        self.review = None;
        if let Some(bytes) = self.pending_psbt.as_mut() {
            bytes.zeroize();
        }
        self.pending_psbt = None;
        self.signed = None;
    }

    pub fn cancel_setup(&mut self) {
        self.zero_setup_secrets();
        // Cancel is the fallback for any screen, so a session abandoned here
        // may still hold a PSBT. Scrub it rather than reason about which
        // screens can reach this.
        self.zero_signing_state();
        self.setup_flow = None;
        self.notice = None;
        self.cancel_entry_return = None;
        self.screen = Screen::LoadWallet;
    }
    /// Opens the "are you sure?" prompt for abandoning mnemonic or passphrase
    /// entry, remembering the screen to return to if the user says no.
    pub fn open_cancel_entry_confirm(&mut self) {
        self.cancel_entry_return = Some(self.screen);
        self.screen = Screen::CancelEntryConfirm;
    }
    /// "Yes": abandons the entry the confirmation prompt was opened from.
    ///
    /// Leaving FROST mnemonic entry steps back to the share flow rather than
    /// discarding the whole setup, so already-entered shares survive.
    pub fn confirm_cancel_entry(&mut self) {
        match self.cancel_entry_return {
            Some(Screen::FrostMnemonicEntry) => {
                self.screen = Screen::FrostMnemonicEntry;
                self.go_back();
                self.cancel_entry_return = None;
            }
            _ => self.cancel_setup(),
        }
    }
    pub fn go_back(&mut self) {
        match self.screen {
            Screen::WalletList => self.screen = Screen::LoadWallet,
            Screen::ImportWallet => self.cancel_setup(),
            Screen::MnemonicLength => self.screen = Screen::ImportWallet,
            Screen::MnemonicEntry => {
                self.clear_mnemonic_input();
                self.screen = Screen::MnemonicLength;
            }
            Screen::FrostThreshold => self.screen = Screen::ImportWallet,
            Screen::FrostShareIndex => {
                self.frost_shares.clear();
                self.screen = Screen::FrostThreshold;
            }
            Screen::FrostMnemonicEntry => {
                self.clear_mnemonic_input();
                self.screen = Screen::FrostShareIndex;
            }
            Screen::FrostMnemonicConfirm => {
                let _ = self.edit_frost_share();
            }
            Screen::Network => {
                self.pending_secret = None;
                if matches!(self.setup_flow, Some(SetupFlow::Import)) {
                    self.screen = Screen::ImportWallet;
                } else {
                    self.setup_flow = None;
                    self.screen = Screen::LoadWallet;
                }
            }
            Screen::Derivation => self.screen = Screen::Network,
            Screen::PassphraseChoice => {
                self.screen = if self.pending_stored.is_some() {
                    Screen::WalletList
                } else {
                    Screen::Derivation
                }
            }
            Screen::PassphraseEntry => {
                self.zero_passphrase();
                self.screen = Screen::PassphraseChoice;
            }
            Screen::GenerationWords => self.screen = Screen::PassphraseChoice,
            Screen::EntropySources => self.screen = Screen::GenerationWords,
            Screen::DiceEntropy | Screen::CameraEntropy => self.screen = Screen::EntropySources,
            Screen::BackupMenu => self.screen = Screen::Home,
            Screen::BackupWords | Screen::BackupSeedQr | Screen::Descriptor => {
                self.screen = Screen::BackupMenu
            }
            Screen::Review | Screen::Signed => self.screen = Screen::Home,
            Screen::DeleteWallet => self.screen = Screen::Home,
            // "No": return to entry exactly as it was left, bypassing the
            // origin screen's own back handling (which would clear it).
            Screen::CancelEntryConfirm => {
                self.screen = self.cancel_entry_return.take().unwrap_or(Screen::LoadWallet);
            }
            _ => {}
        }
    }
    pub fn cancel_backup(&mut self) {
        self.wallet = None;
        self.active_wallet = None;
        self.zero_setup_secrets();
        self.zero_signing_state();
        self.setup_flow = None;
        self.backup_acknowledged = false;
        self.backup_page = 0;
        self.notice = None;
        self.screen = Screen::LoadWallet;
    }
    pub fn select_network(&mut self, network: Network) {
        self.network = network;
        self.screen = Screen::Derivation;
    }

    pub fn accept_import_secret(&mut self, secret: WalletSecret) {
        self.pending_secret = Some(secret);
        self.setup_flow = Some(SetupFlow::Import);
        self.screen = Screen::Network;
    }

    pub fn accept_seed_qr(&mut self, payload: &[u8]) -> Result<(), AppError> {
        let secret = WalletSecret::from_seed_qr(payload).map_err(AppError::Wallet)?;
        self.pending_kind = WalletKind::Bip39;
        self.accept_import_secret(secret);
        Ok(())
    }

    pub fn accept_mnemonic(&mut self, words: &str) -> Result<(), AppError> {
        let secret = WalletSecret::from_mnemonic(words).map_err(AppError::Wallet)?;
        self.pending_kind = WalletKind::Bip39;
        self.accept_import_secret(secret);
        Ok(())
    }

    /// Generate a new session-only wallet. Flash is not touched until the
    /// user explicitly selects Save wallet from the home screen.
    pub fn generate_seed<E, S>(&mut self, entropy: &mut E, _store: &mut S) -> Result<(), AppError>
    where
        E: CryptoEntropy,
        S: WalletStore,
    {
        let mut hardware = [0u8; 32];
        if entropy.fill(&mut hardware).is_err() {
            hardware.zeroize();
            return Err(AppError::Entropy);
        }
        self.last_entropy_sources = (1 << EntropySource::HardwareTrng as u8)
            | if self.entropy_mixer.has_source(EntropySource::Camera) {
                1 << EntropySource::Camera as u8
            } else {
                0
            }
            | if self.entropy_mixer.has_source(EntropySource::D6Dice) {
                1 << EntropySource::D6Dice as u8
            } else {
                0
            };
        let mixer = core::mem::take(&mut self.entropy_mixer);
        let secret = mixer
            .finish(
                &mut hardware,
                if self.generation_words == 24 { 32 } else { 16 },
            )
            .map_err(AppError::Wallet)?;
        let wallet = WalletSession::new_with_options(
            secret,
            self.network,
            self.account_type,
            &self.passphrase,
        );
        self.active_wallet = None;
        self.wallet = Some(wallet);
        self.zero_passphrase();
        self.backup_acknowledged = false;
        self.review = None;
        self.pending_psbt = None;
        self.screen = Screen::Backup;
        Ok(())
    }

    pub fn confirm_derivation<E, S>(
        &mut self,
        entropy: &mut E,
        store: &mut S,
    ) -> Result<(), AppError>
    where
        E: CryptoEntropy,
        S: WalletStore,
    {
        match self.setup_flow {
            Some(SetupFlow::Create) => self.generate_seed(entropy, store),
            Some(SetupFlow::Import) => {
                let secret = self
                    .pending_secret
                    .as_ref()
                    .ok_or(AppError::InvalidTransition)?;
                let wallet = match self.pending_kind {
                    WalletKind::Bip39 => WalletSession::new_with_options(
                        secret.clone(),
                        self.network,
                        self.account_type,
                        &self.passphrase,
                    ),
                    WalletKind::Frost => WalletSession::new_frost(secret.clone(), self.network),
                };
                self.pending_secret = None;
                self.zero_passphrase();
                self.pending_kind = WalletKind::Bip39;
                self.active_wallet = None;
                self.load_wallet(wallet, false);
                self.setup_flow = None;
                Ok(())
            }
            None if self.pending_stored.is_some() => self.unlock_pending_wallet(store),
            None => Err(AppError::InvalidTransition),
        }
    }

    /// Boot from the newest committed seed, or generate and commit one when
    /// storage is empty. Callers provide the board's validated entropy and
    /// transactional store.
    pub fn boot<E, S>(&mut self, _entropy: &mut E, store: &mut S) -> Result<(), AppError>
    where
        E: CryptoEntropy,
        S: WalletStore,
    {
        self.wallets = store.catalog().map_err(|_| AppError::Storage)?;
        // Old records did not persist a fingerprint. Upgrade the sole legacy
        // record lazily before exposing the load list.
        for index in 0..self.wallets.len() {
            let Some(metadata) = self.wallets.get(index).copied() else {
                continue;
            };
            if metadata.fingerprint.is_none() {
                let seed = store
                    .load_wallet(metadata.id)
                    .map_err(|_| AppError::Storage)?
                    .ok_or(AppError::NoSeed)?;
                let secret = WalletSecret::from_slice(seed.entropy().ok_or(AppError::Storage)?)
                    .map_err(AppError::Wallet)?;
                let wallet = WalletSession::new(secret, wallet_network(seed.network));
                let upgraded =
                    record_with_wallet_cache(&wallet, wallet.network(), seed.backup_pending)
                        .ok_or(AppError::Storage)?;
                store
                    .update_wallet(metadata.id, upgraded)
                    .map_err(|_| AppError::Storage)?;
            }
        }
        self.wallets = store.catalog().map_err(|_| AppError::Storage)?;
        self.screen = Screen::LoadWallet;
        Ok(())
    }

    /// Load the newest committed Flash record and select the network encoded
    /// in that record. Corrupt or interrupted records are rejected by the
    /// board store before this method is called.
    pub fn load_from_store<S>(&mut self, store: &mut S) -> Result<(), AppError>
    where
        S: WalletStore,
    {
        self.wallets = store.catalog().map_err(|_| AppError::Storage)?;
        let id = self
            .wallets
            .get(0)
            .map(|entry| entry.id)
            .ok_or(AppError::NoSeed)?;
        let seed = store
            .load_wallet(id)
            .map_err(|_| AppError::Storage)?
            .ok_or(AppError::NoSeed)?;
        self.load_stored_seed(id, seed, store)
    }

    pub fn load_wallet_id<S: WalletStore>(
        &mut self,
        id: WalletId,
        store: &mut S,
    ) -> Result<(), AppError> {
        let seed = store
            .load_wallet(id)
            .map_err(|_| AppError::Storage)?
            .ok_or(AppError::NoSeed)?;
        self.load_stored_seed(id, seed, store)
    }

    fn load_stored_seed<S>(
        &mut self,
        id: WalletId,
        mut seed: BoardSeed,
        store: &mut S,
    ) -> Result<(), AppError>
    where
        S: WalletStore,
    {
        self.network = wallet_network(seed.network);
        let secret = WalletSecret::from_slice(seed.entropy().ok_or(AppError::Storage)?)
            .map_err(AppError::Wallet)?;
        let descriptor = seed
            .descriptor()
            .and_then(|bytes| core::str::from_utf8(bytes).ok());
        let account_type = descriptor
            .and_then(|descriptor| account_type_from_descriptor(descriptor).ok())
            .unwrap_or(AccountType::Taproot);
        if seed.passphrase_required {
            self.account_type = account_type;
            self.pending_stored = Some((id, seed));
            self.active_wallet = None;
            self.zero_passphrase();
            self.screen = Screen::PassphraseChoice;
            return Ok(());
        }
        let cached_wallet = descriptor.and_then(|descriptor| {
            WalletSession::from_descriptor_cache(
                secret.clone(),
                self.network,
                descriptor,
                seed.descriptor_tag,
            )
            .ok()
        });
        if let Some(wallet) = cached_wallet {
            self.active_wallet = Some(id);
            self.load_wallet(wallet, seed.backup_pending);
            seed.entropy.zeroize();
            return Ok(());
        }

        // Cache-less/corrupt legacy records pay the derivation cost once and
        // are repaired transactionally below. Valid current records never
        // reach this path.
        let empty_wallet =
            WalletSession::new_with_options(secret.clone(), self.network, account_type, "");
        let legacy_empty_match = descriptor.is_some_and(|descriptor| {
            empty_wallet
                .export()
                .ok()
                .is_some_and(|export| export.descriptor == descriptor)
        });
        if descriptor.is_some() && !legacy_empty_match {
            self.account_type = account_type;
            self.pending_stored = Some((id, seed));
            self.active_wallet = None;
            self.zero_passphrase();
            self.screen = Screen::PassphraseChoice;
            return Ok(());
        }
        self.active_wallet = Some(id);
        self.load_wallet(empty_wallet, seed.backup_pending);
        if let Some(record) = self
            .wallet
            .as_ref()
            .and_then(|wallet| record_with_wallet_cache(wallet, self.network, seed.backup_pending))
        {
            // Invalid-cache records are repaired lazily. A failed
            // performance-only upgrade doesn't prevent loading the wallet.
            let _ = store.update_wallet(id, record);
        }
        seed.entropy.zeroize();
        Ok(())
    }

    fn unlock_pending_wallet<S: WalletStore>(&mut self, store: &mut S) -> Result<(), AppError> {
        let (id, mut seed) = self.pending_stored.ok_or(AppError::InvalidTransition)?;
        let secret = WalletSecret::from_slice(seed.entropy().ok_or(AppError::Storage)?)
            .map_err(AppError::Wallet)?;
        let descriptor = seed
            .descriptor()
            .and_then(|bytes| core::str::from_utf8(bytes).ok())
            .ok_or(AppError::Storage)?;
        let wallet = match WalletSession::from_descriptor_cache_with_passphrase(
            secret,
            self.network,
            descriptor,
            seed.descriptor_tag,
            &self.passphrase,
        ) {
            Ok(wallet) => wallet,
            Err(error) => {
                seed.entropy.zeroize();
                self.zero_passphrase();
                self.notice = Some("PASSPHRASE DID NOT MATCH");
                self.screen = Screen::PassphraseChoice;
                return Err(AppError::Wallet(error));
            }
        };
        self.pending_stored = None;
        self.zero_passphrase();
        self.notice = None;
        self.account_type = wallet.account_type();
        self.active_wallet = Some(id);
        self.load_wallet(wallet, seed.backup_pending);
        seed.entropy.zeroize();
        self.wallets = store.catalog().map_err(|_| AppError::Storage)?;
        Ok(())
    }

    pub fn mnemonic(&self) -> Result<alloc::string::String, AppError> {
        self.wallet
            .as_ref()
            .ok_or(AppError::NoSeed)?
            .mnemonic()
            .map_err(AppError::Wallet)
    }

    pub fn export_descriptor(&self) -> Result<picosigner_wallet::WalletExport, AppError> {
        if !self.backup_acknowledged {
            return Err(AppError::BackupRequired);
        }
        self.wallet
            .as_ref()
            .ok_or(AppError::NoSeed)?
            .export()
            .map_err(AppError::Wallet)
    }
    pub fn load_seed<E: Into<WalletSecret>>(&mut self, entropy: E, backup_pending: bool) {
        self.load_wallet(WalletSession::new(entropy, self.network), backup_pending);
    }

    fn load_wallet(&mut self, wallet: WalletSession, backup_pending: bool) {
        self.wallet = Some(wallet);
        self.backup_acknowledged = !backup_pending;
        self.review = None;
        self.pending_psbt = None;
        self.signed = None;
        self.screen = if backup_pending {
            Screen::Backup
        } else {
            Screen::Home
        };
    }

    /// Acknowledge backup. For a legacy Flash-backed wallet whose pending flag
    /// is still set, atomically clear that flag; new wallets remain unsaved.
    pub fn acknowledge_backup_and_store<S>(&mut self, store: &mut S) -> Result<(), AppError>
    where
        S: WalletStore,
    {
        let wallet = self.wallet.as_ref().ok_or(AppError::NoSeed)?;
        if let Some(id) = self.active_wallet {
            let record =
                record_with_wallet_cache(wallet, self.network, false).ok_or(AppError::Storage)?;
            store
                .update_wallet(id, record)
                .map_err(|_| AppError::Storage)?;
            self.wallets = store.catalog().map_err(|_| AppError::Storage)?;
        }
        self.acknowledge_backup()
    }

    pub fn save_wallet<S: WalletStore>(&mut self, store: &mut S) -> Result<(), AppError> {
        if self.active_wallet.is_some() {
            return Err(AppError::InvalidTransition);
        }
        let wallet = self.wallet.as_ref().ok_or(AppError::NoSeed)?;
        if !self.backup_acknowledged {
            return Err(AppError::BackupRequired);
        }
        let record =
            record_with_wallet_cache(wallet, self.network, false).ok_or(AppError::Storage)?;
        let id = store
            .insert_wallet(record)
            .map_err(|error| map_store_error::<S>(&error))?;
        self.active_wallet = Some(id);
        self.wallets = store.catalog().map_err(|_| AppError::Storage)?;
        Ok(())
    }

    pub fn delete_wallet<S: WalletStore>(&mut self, store: &mut S) -> Result<(), AppError> {
        let id = self.active_wallet.ok_or(AppError::InvalidTransition)?;
        store.delete_wallet(id).map_err(|_| AppError::Storage)?;
        self.active_wallet = None;
        self.wallets = store.catalog().map_err(|_| AppError::Storage)?;
        self.screen = Screen::Home;
        Ok(())
    }
    pub fn acknowledge_backup(&mut self) -> Result<(), AppError> {
        if self.wallet.is_none() {
            return Err(AppError::NoSeed);
        }
        self.backup_acknowledged = true;
        self.screen = Screen::Home;
        Ok(())
    }
    pub fn open_descriptor(&mut self) -> Result<(), AppError> {
        if self.wallet.is_none() {
            return Err(AppError::NoSeed);
        }
        if !self.backup_acknowledged {
            return Err(AppError::BackupRequired);
        }
        self.screen = Screen::Descriptor;
        Ok(())
    }
    pub fn open_backup(&mut self) -> Result<(), AppError> {
        if self.wallet.is_none() {
            return Err(AppError::NoSeed);
        }
        if !self.backup_acknowledged {
            return Err(AppError::BackupRequired);
        }
        if self
            .wallet
            .as_ref()
            .is_some_and(|wallet| wallet.kind() == WalletKind::Frost)
        {
            return Err(AppError::InvalidTransition);
        }
        self.backup_page = 0;
        self.screen = Screen::BackupWords;
        Ok(())
    }
    pub fn backup_page(&self) -> usize {
        usize::from(self.backup_page)
    }
    pub fn next_backup_page(&mut self) {
        if self
            .wallet
            .as_ref()
            .is_some_and(|wallet| wallet.entropy().len() == 32)
        {
            self.backup_page = 1;
        }
    }
    pub fn previous_backup_page(&mut self) {
        self.backup_page = 0;
    }
    pub fn begin_scan(&mut self) -> Result<(), AppError> {
        if self.wallet.is_none() {
            return Err(AppError::NoSeed);
        }
        if !self.backup_acknowledged {
            return Err(AppError::BackupRequired);
        }
        self.screen = Screen::Scan;
        Ok(())
    }

    /// Parse and validate the exact PSBT that will be presented for review.
    /// The bytes are retained only for the subsequent hold-to-sign action.
    pub fn receive_psbt(&mut self, bytes: &[u8]) -> Result<&PsbtReview, AppError> {
        if self.wallet.is_none() {
            return Err(AppError::NoSeed);
        }
        if !self.backup_acknowledged {
            return Err(AppError::BackupRequired);
        }
        let review = self
            .wallet
            .as_ref()
            .ok_or(AppError::NoSeed)?
            .review(bytes)
            .map_err(AppError::Wallet)?;
        self.pending_psbt = Some(bytes.to_vec());
        self.review = Some(review);
        self.signed = None;
        self.screen = Screen::Review;
        Ok(self.review.as_ref().expect("review was just stored"))
    }

    /// Feed one bounded BBQr fragment to the receive session. Fragments may
    /// arrive in any order and duplicates are handled by the collector.
    pub fn receive_qr_fragment<const N: usize>(
        &mut self,
        collector: &mut picosigner_qr::BbqrCollector<N>,
        fragment: &[u8],
    ) -> Result<bool, AppError> {
        let complete = collector
            .push(fragment)
            .map_err(|_| AppError::InvalidTransition)?;
        if complete {
            let bytes = collector
                .finish()
                .map_err(|_| AppError::InvalidTransition)?
                .to_vec();
            self.receive_psbt(&bytes)?;
        }
        Ok(complete)
    }

    pub fn review(&self) -> Option<&PsbtReview> {
        self.review.as_ref()
    }

    pub fn mark_all_outputs_reviewed(&mut self) -> Result<(), AppError> {
        if self.review.is_none() || self.pending_psbt.is_none() {
            return Err(AppError::InvalidTransition);
        }
        self.screen = Screen::Sign;
        Ok(())
    }

    /// Sign only after the UI has completed its output/change/fee review.
    /// `WalletSession::sign` repeats digest validation immediately before
    /// invoking the signer, so a mutated receive buffer cannot be signed.
    pub fn sign_after_hold(&mut self, hold_confirmed: bool) -> Result<&SignedPsbt, AppError> {
        if !hold_confirmed || self.screen != Screen::Sign {
            return Err(AppError::InvalidTransition);
        }
        let wallet = self.wallet.as_ref().ok_or(AppError::NoSeed)?;
        let review = self.review.as_ref().ok_or(AppError::InvalidTransition)?;
        let bytes = self
            .pending_psbt
            .as_deref()
            .ok_or(AppError::InvalidTransition)?;
        let signed = wallet
            .sign(bytes, review, &StrictSigningPolicy)
            .map_err(AppError::Wallet)?;
        self.pending_psbt = None;
        self.review = None;
        self.screen = Screen::Signed;
        // Store the result in a separate slot so the QR encoder can stream it
        // after the screen transition without exposing the seed session.
        self.signed = Some(signed);
        Ok(self.signed.as_ref().expect("signed was just stored"))
    }

    pub fn take_signed_psbt(&mut self) -> Option<SignedPsbt> {
        self.signed.take()
    }

    pub fn signed_psbt(&self) -> Option<&SignedPsbt> {
        self.signed.as_ref()
    }

    pub fn open_backup_menu(&mut self) -> Result<(), AppError> {
        if self.wallet.is_none() {
            return Err(AppError::NoSeed);
        }
        self.screen = Screen::BackupMenu;
        Ok(())
    }

    pub fn open_backup_seed_qr(&mut self) -> Result<(), AppError> {
        if self.wallet.is_none() {
            return Err(AppError::NoSeed);
        }
        if self
            .wallet
            .as_ref()
            .is_some_and(|wallet| wallet.kind() == WalletKind::Frost)
        {
            return Err(AppError::InvalidTransition);
        }
        self.screen = Screen::BackupSeedQr;
        Ok(())
    }

    pub fn logout<S: WalletStore>(&mut self, store: &mut S) -> Result<(), AppError> {
        self.expire_session();
        self.wallets = store.catalog().map_err(|_| AppError::Storage)?;
        Ok(())
    }

    /// Zeroize all session-only secrets and return to the wallet loader. This
    /// does not need Flash access and is safe to call from an inactivity timer
    /// or reset path.
    pub fn expire_session(&mut self) {
        self.wallet = None;
        self.zero_setup_secrets();
        self.zero_signing_state();
        self.active_wallet = None;
        self.setup_flow = None;
        self.notice = None;
        self.backup_acknowledged = false;
        self.screen = Screen::LoadWallet;
    }
}

fn wallet_network(value: picosigner_board::Network) -> Network {
    match value {
        picosigner_board::Network::Mainnet => Network::Mainnet,
        picosigner_board::Network::Testnet4 => Network::Testnet4,
        picosigner_board::Network::Signet => Network::Signet,
    }
}

fn board_network(value: Network) -> picosigner_board::Network {
    match value {
        Network::Mainnet => picosigner_board::Network::Mainnet,
        Network::Testnet4 => picosigner_board::Network::Testnet4,
        Network::Signet => picosigner_board::Network::Signet,
    }
}

fn map_store_error<S: WalletStore>(error: &S::Error) -> AppError {
    match S::error_kind(error) {
        WalletStoreErrorKind::Duplicate => AppError::DuplicateWallet,
        WalletStoreErrorKind::Full => AppError::StoreFull,
        WalletStoreErrorKind::Storage => AppError::Storage,
    }
}

fn record_with_wallet_cache(
    wallet: &WalletSession,
    network: Network,
    backup_pending: bool,
) -> Option<BoardSeed> {
    let cache = wallet.descriptor_cache().ok()?;
    if cache.descriptor.len() > MAX_DESCRIPTOR_BYTES {
        return None;
    }
    let secret = wallet.entropy();
    let mut record =
        BoardSeed::from_entropy(secret.as_bytes(), board_network(network), backup_pending)?;
    record.fingerprint = wallet.master_fingerprint()?;
    record.passphrase_required = wallet.has_passphrase();
    record.descriptor_len = cache.descriptor.len() as u16;
    record.descriptor[..cache.descriptor.len()].copy_from_slice(cache.descriptor.as_bytes());
    record.descriptor_tag = cache.tag;
    Some(record)
}

impl Navigator for AppState {
    fn current(&self) -> Screen {
        self.screen
    }
    fn navigate(&mut self, screen: Screen) {
        if screen == Screen::WalletList && self.screen != Screen::WalletList {
            self.wallet_list_page = 0;
        }
        self.screen = screen;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Entropy;
    impl CryptoEntropy for Entropy {
        type Error = ();
        fn fill(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
            for (i, byte) in dst.iter_mut().enumerate() {
                *byte = i as u8;
            }
            Ok(())
        }
    }

    struct Store(Option<BoardSeed>);
    impl WalletStore for Store {
        type Error = ();
        fn catalog(&mut self) -> Result<WalletCatalog, Self::Error> {
            let mut catalog = WalletCatalog::empty();
            if let Some(seed) = self.0 {
                catalog.push(picosigner_board::WalletMetadata {
                    id: WalletId(0),
                    fingerprint: (seed.fingerprint != [0; 4]).then_some(seed.fingerprint),
                    network: seed.network,
                    backup_pending: seed.backup_pending,
                });
            }
            Ok(catalog)
        }
        fn load_wallet(&mut self, id: WalletId) -> Result<Option<BoardSeed>, Self::Error> {
            Ok((id == WalletId(0)).then_some(self.0).flatten())
        }
        fn insert_wallet(&mut self, seed: BoardSeed) -> Result<WalletId, Self::Error> {
            if self.0.is_some() {
                return Err(());
            }
            self.0 = Some(seed);
            Ok(WalletId(0))
        }
        fn update_wallet(&mut self, id: WalletId, seed: BoardSeed) -> Result<(), Self::Error> {
            if id != WalletId(0) || self.0.is_none() {
                return Err(());
            }
            self.0 = Some(seed);
            Ok(())
        }
        fn delete_wallet(&mut self, id: WalletId) -> Result<(), Self::Error> {
            if id != WalletId(0) || self.0.is_none() {
                return Err(());
            }
            self.0 = None;
            Ok(())
        }
    }

    fn enter_frost_phrase(app: &mut AppState, phrase: &str) {
        for word in phrase.split_whitespace() {
            for letter in word.bytes() {
                app.mnemonic_letter(letter);
            }
            app.choose_mnemonic_suggestion(0).unwrap();
        }
    }

    #[test]
    fn frost_import_recovers_at_threshold_and_uses_frost_derivation() {
        const SHARE_1: &str = "mutual jeans snap sting bless journey moral bread room limit dose gravity sort deliver outdoor ripple donkey blouse play cart century maximum make local mobile";
        const SHARE_2: &str = "cash trash foil prefer butter idea brave bitter item wink drift smile tomato lunch option hero three engine bless manage horse jar advice sheriff business";
        let mut app = AppState::new(Network::Signet);
        app.begin_import();
        app.choose_frost_import();
        assert_eq!(app.screen(), Screen::FrostThreshold);
        app.confirm_frost_threshold();
        app.start_frost_share_entry().unwrap();
        enter_frost_phrase(&mut app, SHARE_1);
        assert_eq!(app.screen(), Screen::FrostMnemonicConfirm);
        app.edit_frost_share().unwrap();
        assert_eq!(app.screen(), Screen::FrostMnemonicEntry);
        assert_eq!(app.mnemonic_entered(), 24);
        assert_eq!(app.mnemonic_prefix(), "mobile");
        app.choose_mnemonic_suggestion(0).unwrap();
        assert_eq!(app.screen(), Screen::FrostMnemonicConfirm);
        app.confirm_frost_share().unwrap();
        assert_eq!(app.screen(), Screen::FrostShareIndex);
        assert_eq!(app.frost_shares_entered(), 1);
        app.start_frost_share_entry().unwrap();
        enter_frost_phrase(&mut app, SHARE_2);
        assert_eq!(app.screen(), Screen::FrostMnemonicConfirm);
        app.confirm_frost_share().unwrap();
        assert_eq!(app.screen(), Screen::Network);
        assert_eq!(app.pending_wallet_kind(), WalletKind::Frost);
        app.select_network(Network::Mainnet);
        let mut store = Store(None);
        app.confirm_derivation(&mut Entropy, &mut store).unwrap();
        assert_eq!(app.screen(), Screen::Home);
        assert_eq!(app.wallet().unwrap().kind(), WalletKind::Frost);
        assert!(app
            .export_descriptor()
            .unwrap()
            .descriptor
            .contains("/0/0/0/0]"));
        app.save_wallet(&mut store).unwrap();
        let saved_id = app.active_wallet_id().unwrap();
        let mut reloaded = AppState::new(Network::Signet);
        reloaded.load_wallet_id(saved_id, &mut store).unwrap();
        assert_eq!(reloaded.selected_network(), Network::Mainnet);
        assert_eq!(reloaded.wallet().unwrap().kind(), WalletKind::Frost);
        assert_eq!(
            reloaded.export_descriptor().unwrap(),
            app.export_descriptor().unwrap()
        );
    }

    #[test]
    fn invalid_frost_share_returns_to_an_editable_last_word() {
        const INVALID: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon";
        let mut app = AppState::new(Network::Signet);
        app.begin_import();
        app.choose_frost_import();
        app.confirm_frost_threshold();
        app.start_frost_share_entry().unwrap();
        enter_frost_phrase(&mut app, INVALID);

        assert_eq!(app.screen(), Screen::FrostMnemonicConfirm);
        assert!(app.confirm_frost_share().is_err());
        assert_eq!(app.screen(), Screen::FrostMnemonicEntry);
        assert_eq!(app.mnemonic_entered(), 24);
        assert_eq!(app.mnemonic_prefix(), "abandon");
        assert!(app.mnemonic_error());
    }

    #[test]
    fn backup_is_mandatory() {
        let mut app = AppState::new(Network::Signet);
        app.load_seed([0; 16], true);
        assert_eq!(app.open_descriptor(), Err(AppError::BackupRequired));
        app.acknowledge_backup().unwrap();
        assert!(app.open_descriptor().is_ok());
        app.navigate(Screen::Home);
        assert!(app.open_backup().is_ok());
        assert_eq!(app.screen(), Screen::BackupWords);
    }

    #[test]
    fn generation_requires_backup_then_explicit_save() {
        let mut app = AppState::new(Network::Signet);
        let mut entropy = Entropy;
        let mut store = Store(None);
        app.generate_seed(&mut entropy, &mut store).unwrap();
        assert_eq!(app.screen(), Screen::Backup);
        assert!(store.0.is_none());
        app.acknowledge_backup_and_store(&mut store).unwrap();
        assert_eq!(app.screen(), Screen::Home);
        assert!(store.0.is_none());
        assert!(!app.wallet_is_saved());
        app.save_wallet(&mut store).unwrap();
        assert!(!store.0.unwrap().backup_pending);
        assert!(store.0.unwrap().descriptor().is_some());
        assert!(app.wallet_is_saved());
        assert!(app.export_descriptor().is_ok());
        app.delete_wallet(&mut store).unwrap();
        assert!(store.0.is_none());
        assert!(!app.wallet_is_saved());
        assert!(app.wallet().is_some());
    }

    #[test]
    fn cancelling_a_new_wallet_backup_discards_the_session() {
        let mut app = AppState::new(Network::Signet);
        app.load_seed([0; 16], true);
        assert_eq!(app.screen(), Screen::Backup);

        app.cancel_backup();

        assert_eq!(app.screen(), Screen::LoadWallet);
        assert!(app.wallet().is_none());
    }

    #[test]
    fn setup_back_navigation_returns_one_screen_at_a_time() {
        let mut app = AppState::new(Network::Signet);
        app.navigate(Screen::LoadWallet);

        app.begin_import();
        app.choose_mnemonic_import();
        app.start_mnemonic_entry(12).unwrap();
        app.mnemonic_letter(b'a');
        app.go_back();
        assert_eq!(app.screen(), Screen::MnemonicLength);
        assert!(app.mnemonic_prefix().is_empty());
        app.go_back();
        assert_eq!(app.screen(), Screen::ImportWallet);

        app.accept_seed_qr(b"073318950739065415961602009907670428187212261116")
            .unwrap();
        app.select_network(Network::Mainnet);
        app.go_back();
        assert_eq!(app.screen(), Screen::Network);
        app.go_back();
        assert_eq!(app.screen(), Screen::ImportWallet);

        app.go_back();
        assert_eq!(app.screen(), Screen::LoadWallet);
        app.begin_create();
        app.go_back();
        assert_eq!(app.screen(), Screen::LoadWallet);
    }

    #[test]
    fn persisted_seed_loads_into_home_only_after_backup() {
        let mut store = Store(Some(BoardSeed::new(
            [0; 16],
            picosigner_board::Network::Mainnet,
            false,
        )));
        let mut app = AppState::new(Network::Signet);
        app.load_from_store(&mut store).unwrap();
        assert_eq!(app.screen(), Screen::Home);
        assert_eq!(app.wallet().unwrap().network(), Network::Mainnet);
        assert!(store.0.unwrap().descriptor().is_some());
    }

    #[test]
    fn empty_boot_import_and_logout_follow_stage_two_flow() {
        let mut app = AppState::new(Network::Signet);
        let mut entropy = Entropy;
        let mut store = Store(None);
        app.boot(&mut entropy, &mut store).unwrap();
        assert_eq!(app.screen(), Screen::LoadWallet);
        assert!(app.wallets().is_empty());
        app.begin_import();
        app.accept_seed_qr(b"073318950739065415961602009907670428187212261116")
            .unwrap();
        app.select_network(Network::Mainnet);
        assert_eq!(app.screen(), Screen::Derivation);
        app.confirm_derivation(&mut entropy, &mut store).unwrap();
        assert_eq!(app.screen(), Screen::Home);
        assert!(!app.wallet_is_saved());
        assert!(store.0.is_none());
        assert_eq!(
            app.mnemonic().unwrap(),
            "forum undo fragile fade shy sign arrest garment culture tube off merit"
        );
        app.logout(&mut store).unwrap();
        assert_eq!(app.screen(), Screen::LoadWallet);
        assert!(app.wallet().is_none());
        assert!(app.wallets().is_empty());
    }

    #[test]
    fn mnemonic_entry_reports_the_last_three_accepted_words() {
        let mut app = AppState::new(Network::Signet);
        app.start_mnemonic_entry(12).unwrap();
        for word in ["abandon", "ability", "able", "about"] {
            for letter in word.bytes() {
                app.mnemonic_letter(letter);
            }
            app.choose_mnemonic_suggestion(0).unwrap();
        }
        assert_eq!(
            app.recent_mnemonic_words(),
            [Some((2, "ability")), Some((3, "able")), Some((4, "about"))]
        );
    }

    #[test]
    fn passphrase_wallet_requires_verified_reentry_after_logout() {
        let mut app = AppState::new(Network::Signet);
        let mut entropy = Entropy;
        let mut store = Store(None);
        app.begin_import();
        app.accept_import_secret(WalletSecret::from_slice(&[9u8; 16]).unwrap());
        app.select_network(Network::Mainnet);
        app.select_account_type(AccountType::NativeSegwit);
        app.begin_passphrase_entry();
        for letter in b"secret" {
            app.passphrase_letter(*letter);
        }
        assert_eq!(app.passphrase_display(), "secret");
        app.confirm_passphrase();
        app.confirm_derivation(&mut entropy, &mut store).unwrap();
        let expected = app.wallet().unwrap().master_fingerprint().unwrap();
        app.save_wallet(&mut store).unwrap();
        assert!(store.0.unwrap().passphrase_required);
        let id = app.active_wallet_id().unwrap();
        app.logout(&mut store).unwrap();

        app.load_wallet_id(id, &mut store).unwrap();
        assert_eq!(app.screen(), Screen::PassphraseChoice);
        app.choose_no_passphrase();
        assert!(app.confirm_derivation(&mut entropy, &mut store).is_err());
        assert_eq!(app.screen(), Screen::PassphraseChoice);
        assert!(app.wallet().is_none());
        app.begin_passphrase_entry();
        for letter in b"secret" {
            app.passphrase_letter(*letter);
        }
        app.confirm_passphrase();
        app.confirm_derivation(&mut entropy, &mut store).unwrap();
        assert_eq!(app.screen(), Screen::Home);
        assert_eq!(app.wallet().unwrap().master_fingerprint(), Some(expected));
        assert_eq!(
            app.wallet().unwrap().account_type(),
            AccountType::NativeSegwit
        );
    }

    #[test]
    fn creation_mixes_optional_camera_and_dice_with_trng_for_24_words() {
        let mut app = AppState::new(Network::Signet);
        let mut entropy = Entropy;
        let mut store = Store(None);
        app.begin_create();
        app.select_network(Network::Signet);
        app.select_account_type(AccountType::NestedSegwit);
        app.choose_no_passphrase();
        app.select_generation_words(24).unwrap();
        app.accept_camera_entropy(b"camera frame transcript");
        for roll in [1, 6, 2, 5, 3, 4] {
            app.add_dice_roll(roll).unwrap();
        }
        app.confirm_derivation(&mut entropy, &mut store).unwrap();
        assert_eq!(app.screen(), Screen::Backup);
        assert_eq!(app.wallet().unwrap().entropy().len(), 32);
        assert_eq!(
            app.wallet().unwrap().account_type(),
            AccountType::NestedSegwit
        );
        assert!(app.used_entropy_source(EntropySource::HardwareTrng));
        assert!(app.used_entropy_source(EntropySource::Camera));
        assert!(app.used_entropy_source(EntropySource::D6Dice));
    }

    #[test]
    fn inactivity_expiry_zeroizes_and_returns_to_loader_without_flash() {
        let mut app = AppState::new(Network::Signet);
        app.load_seed([3u8; 16], false);
        assert!(app.wallet().is_some());
        app.expire_session();
        assert_eq!(app.screen(), Screen::LoadWallet);
        assert!(app.wallet().is_none());
        assert_eq!(app.passphrase_len(), 0);
        assert!(app.pending_psbt.is_none());
    }

    /// Every path that abandons a session must scrub the same secrets.
    /// `cancel_backup` once dropped the pending PSBT without zeroizing it while
    /// `expire_session` zeroized it; both now share one teardown.
    #[test]
    fn every_abandon_path_clears_the_same_secrets() {
        for (name, abandon) in [
            (
                "expire_session",
                AppState::expire_session as fn(&mut AppState),
            ),
            ("cancel_backup", AppState::cancel_backup),
            ("cancel_setup", AppState::cancel_setup),
        ] {
            let mut app = AppState::new(Network::Signet);
            app.load_seed([3u8; 16], false);
            app.begin_passphrase_entry();
            app.passphrase_letter(b'x');
            app.mnemonic_letter(b'a');
            app.pending_psbt = Some(alloc::vec![0xab; 64]);
            app.pending_stored = Some((
                WalletId(0),
                BoardSeed::from_entropy(&[7u8; 16], picosigner_board::Network::Signet, false)
                    .expect("valid entropy"),
            ));

            abandon(&mut app);

            assert_eq!(app.screen(), Screen::LoadWallet, "{name}");
            assert_eq!(app.passphrase_len(), 0, "{name}: passphrase retained");
            assert!(app.pending_psbt.is_none(), "{name}: PSBT retained");
            assert!(app.pending_secret.is_none(), "{name}: secret retained");
            assert!(app.pending_stored.is_none(), "{name}: stored seed retained");
            assert!(
                app.mnemonic_prefix().is_empty(),
                "{name}: mnemonic retained"
            );
        }
    }
}
