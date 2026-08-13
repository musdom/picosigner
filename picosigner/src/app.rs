//! Application state orchestration. Hardware adapters stay outside this state machine.
//!
//! The state machine owns the security-critical ordering: a seed is persisted
//! before it is used, backup acknowledgement is persisted before leaving the
//! backup screen, every PSBT is reviewed before signing, and the reviewed
//! digest is checked again by the wallet immediately before signatures are
//! produced.
extern crate alloc;

use alloc::vec::Vec;
use picosigner_board::{CryptoEntropy, SeedStore, StoredSeed as BoardSeed};
use picosigner_ui::{Navigator, Screen};
use picosigner_wallet::{
    Network, PsbtReview, SignedPsbt, StrictSigningPolicy, WalletError, WalletSession,
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
}

pub struct AppState {
    screen: Screen,
    network: Network,
    wallet: Option<WalletSession>,
    backup_acknowledged: bool,
    review: Option<PsbtReview>,
    pending_psbt: Option<Vec<u8>>,
    signed: Option<SignedPsbt>,
}

impl AppState {
    pub const fn new(network: Network) -> Self {
        Self {
            screen: Screen::Boot,
            network,
            wallet: None,
            backup_acknowledged: false,
            review: None,
            pending_psbt: None,
            signed: None,
        }
    }

    pub fn screen(&self) -> Screen {
        self.screen
    }
    pub fn wallet(&self) -> Option<&WalletSession> {
        self.wallet.as_ref()
    }
    pub fn select_network(&mut self, network: Network) {
        self.network = network;
        self.screen = Screen::Seed;
    }

    /// Generate and persist a new wallet. The record is written with
    /// `backup_pending` set before the wallet is exposed to the UI.
    pub fn generate_seed<E, S>(&mut self, entropy: &mut E, store: &mut S) -> Result<(), AppError>
    where
        E: CryptoEntropy,
        S: SeedStore,
    {
        let mut bytes = [0u8; 16];
        if entropy.fill(&mut bytes).is_err() {
            bytes.zeroize();
            return Err(AppError::Entropy);
        }
        let network = BoardSeed {
            entropy: bytes,
            network: board_network(self.network),
            backup_pending: true,
        };
        if store.store(network).is_err() {
            bytes.zeroize();
            return Err(AppError::Storage);
        }
        self.wallet = Some(WalletSession::new(bytes, self.network));
        bytes.zeroize();
        self.backup_acknowledged = false;
        self.review = None;
        self.pending_psbt = None;
        self.screen = Screen::Backup;
        Ok(())
    }

    /// Boot from the newest committed seed, or generate and commit one when
    /// storage is empty. Callers provide the board's validated entropy and
    /// transactional store.
    pub fn boot<E, S>(&mut self, entropy: &mut E, store: &mut S) -> Result<(), AppError>
    where
        E: CryptoEntropy,
        S: SeedStore,
    {
        if store.load().map_err(|_| AppError::Storage)?.is_some() {
            self.load_from_store(store)
        } else {
            self.generate_seed(entropy, store)
        }
    }

    /// Load the newest committed Flash record and select the network encoded
    /// in that record. Corrupt or interrupted records are rejected by the
    /// board store before this method is called.
    pub fn load_from_store<S>(&mut self, store: &mut S) -> Result<(), AppError>
    where
        S: SeedStore,
    {
        let mut seed = store
            .load()
            .map_err(|_| AppError::Storage)?
            .ok_or(AppError::NoSeed)?;
        self.network = wallet_network(seed.network);
        self.load_seed(seed.entropy, seed.backup_pending);
        seed.entropy.zeroize();
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
    pub fn load_seed(&mut self, entropy: [u8; 16], backup_pending: bool) {
        self.wallet = Some(WalletSession::new(entropy, self.network));
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

    /// Acknowledge backup and atomically clear the pending flag in storage.
    pub fn acknowledge_backup_and_store<S>(&mut self, store: &mut S) -> Result<(), AppError>
    where
        S: SeedStore,
    {
        let wallet = self.wallet.as_ref().ok_or(AppError::NoSeed)?;
        let mut entropy = wallet.entropy();
        let result = store
            .store(BoardSeed {
                entropy,
                network: board_network(self.network),
                backup_pending: false,
            })
            .map_err(|_| AppError::Storage);
        entropy.zeroize();
        result?;
        self.acknowledge_backup()
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

impl Navigator for AppState {
    fn current(&self) -> Screen {
        self.screen
    }
    fn navigate(&mut self, screen: Screen) {
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
    impl SeedStore for Store {
        type Error = ();
        fn load(&mut self) -> Result<Option<BoardSeed>, Self::Error> {
            Ok(self.0)
        }
        fn store(&mut self, seed: BoardSeed) -> Result<(), Self::Error> {
            self.0 = Some(seed);
            Ok(())
        }
    }

    #[test]
    fn backup_is_mandatory() {
        let mut app = AppState::new(Network::Signet);
        app.load_seed([0; 16], true);
        assert_eq!(app.open_descriptor(), Err(AppError::BackupRequired));
        app.acknowledge_backup().unwrap();
        assert!(app.open_descriptor().is_ok());
    }

    #[test]
    fn generation_and_persistent_backup_acknowledgement_are_real_operations() {
        let mut app = AppState::new(Network::Signet);
        let mut entropy = Entropy;
        let mut store = Store(None);
        app.generate_seed(&mut entropy, &mut store).unwrap();
        assert_eq!(app.screen(), Screen::Backup);
        assert!(store.0.unwrap().backup_pending);
        app.acknowledge_backup_and_store(&mut store).unwrap();
        assert_eq!(app.screen(), Screen::Home);
        assert!(!store.0.unwrap().backup_pending);
        assert!(app.export_descriptor().is_ok());
    }

    #[test]
    fn persisted_seed_loads_into_home_only_after_backup() {
        let mut store = Store(Some(BoardSeed {
            entropy: [0; 16],
            network: picosigner_board::Network::Mainnet,
            backup_pending: false,
        }));
        let mut app = AppState::new(Network::Signet);
        app.load_from_store(&mut store).unwrap();
        assert_eq!(app.screen(), Screen::Home);
        assert_eq!(app.wallet().unwrap().network(), Network::Mainnet);
    }
}
