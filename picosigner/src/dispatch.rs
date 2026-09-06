//! Touch dispatch shared by the firmware and the emulator.
//!
//! [`dispatch`] is the single mapping from a released gesture to an
//! [`AppState`] transition. Both the device binary and the desktop emulator
//! call it, so there is exactly one place to change when UI behaviour changes
//! and no way for the two to drift apart.
//!
//! Anything a host must do for itself — repaint, drive the camera, write a
//! file, dim the backlight — is returned as an [`Effect`] rather than performed
//! here. That keeps this function free of hardware and testable on the host.

use picosigner_ui::{Gesture, Navigator, Screen};

use crate::{
    app::{AppError, AppState},
    ui::{supports_swipe_back, HoldAction},
};
use picosigner_board::{CryptoEntropy, SettingsStore, WalletStore};

/// Something the caller must do after a dispatch, because only the caller
/// knows how (or whether) it can be done on its platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Nothing changed; the caller may skip repainting.
    None,
    /// State changed; repaint the screen.
    Redraw,
    /// Paint the current screen, then finish deriving the wallet.
    ///
    /// Derivation is a slow synchronous operation, so the caller shows the
    /// "Deriving" screen first. Call [`finish_derivation`] once painted.
    RenderThenDerive,
    /// Begin a PSBT scan: camera on device, a file in the emulator.
    BeginScan,
    /// Import a SeedQR: camera on device, a file in the emulator.
    BeginSeedQrScan,
    /// Paint the current screen, then sign and export the reviewed PSBT.
    RenderThenSign,
    /// Advance the backlight to its next level.
    CycleBrightness,
    /// Dim the backlight before showing a QR the user must scan.
    DimBacklight,
    /// Settings were persisted; restart to apply hardware policy.
    RestartDevice,
}

impl Effect {
    /// Whether the caller still needs to repaint after handling the effect.
    pub fn needs_redraw(self) -> bool {
        !matches!(self, Effect::None)
    }
}

/// Runs the derivation that [`Effect::RenderThenDerive`] deferred.
///
/// Kept separate so the caller can paint the "Deriving" screen first. Errors
/// are reported through `AppState` unless the state machine already routed the
/// user somewhere that explains the failure (a wrong passphrase returns to the
/// passphrase prompt, which is not an error state).
pub fn finish_derivation<E, S>(app: &mut AppState, entropy: &mut E, store: &mut S)
where
    E: CryptoEntropy,
    S: WalletStore,
{
    if let Err(error) = app.confirm_derivation(entropy, store) {
        if app.screen() != Screen::PassphraseChoice {
            app.show_error(error);
        }
    }
}

/// Applies a released gesture to the application state.
///
/// `action` is the hit-test result captured when the touch began, and
/// `gesture` is what the touch turned out to be. Returns the [`Effect`] the
/// caller must perform.
///
/// Errors from storage-backed operations are surfaced through
/// [`AppState::show_error`] so both hosts report them identically; hosts that
/// want richer reporting can inspect the state afterwards.
pub fn dispatch<E, S>(
    app: &mut AppState,
    entropy: &mut E,
    store: &mut S,
    action: Option<HoldAction>,
    gesture: Gesture,
) -> Effect
where
    E: CryptoEntropy,
    S: WalletStore + SettingsStore,
{
    /// Reports a failed operation and repaints. `Ok` results simply repaint.
    fn report(app: &mut AppState, result: Result<(), AppError>) -> Effect {
        if let Err(error) = result {
            app.show_error(error);
        }
        Effect::Redraw
    }

    /// `report!(app.method(..))` — evaluates the call, then reports any error.
    macro_rules! report {
        ($call:expr) => {{
            let result = $call;
            report(app, result)
        }};
    }

    match (action, gesture) {
        (Some(HoldAction::Settings), Gesture::Tap) if app.screen() == Screen::LoadWallet => {
            app.navigate(Screen::Settings);
            Effect::Redraw
        }
        (Some(HoldAction::ToggleUsb), Gesture::Tap) if app.screen() == Screen::Settings => {
            app.toggle_usb_pending();
            Effect::Redraw
        }
        (Some(HoldAction::SaveSettings), Gesture::Tap) if app.screen() == Screen::Settings => {
            match app.save_settings_and_reboot(store) {
                Ok(()) => Effect::RestartDevice,
                Err(error) => report(app, Err(error)),
            }
        }
        (Some(HoldAction::DiscardSettings), Gesture::Tap) if app.screen() == Screen::Settings => {
            app.discard_settings();
            Effect::Redraw
        }
        (Some(HoldAction::Backup), Gesture::Hold) => {
            report!(app.acknowledge_backup_and_store(store))
        }
        (Some(HoldAction::DeleteWallet), Gesture::Hold) => report!(app.delete_wallet(store)),
        (Some(HoldAction::Regenerate), Gesture::Hold) => {
            report!(app.generate_seed(entropy, store))
        }
        (Some(HoldAction::OpenDeleteWallet), Gesture::Tap) => {
            app.navigate(Screen::DeleteWallet);
            Effect::Redraw
        }

        // The caller checks the sign button's bounds itself: a hold that drifts
        // off the button must not sign.
        (Some(HoldAction::Sign), Gesture::Hold) => match app.mark_all_outputs_reviewed() {
            Ok(()) => Effect::RenderThenSign,
            Err(error) => report(app, Err(error)),
        },

        (Some(HoldAction::Load), Gesture::Tap) => {
            app.navigate(Screen::WalletList);
            Effect::Redraw
        }
        (Some(HoldAction::Wallet(_)), Gesture::SwipeUp) => {
            app.next_wallet_list_page();
            Effect::Redraw
        }
        (Some(HoldAction::Wallet(_)), Gesture::SwipeDown) => {
            app.previous_wallet_list_page();
            Effect::Redraw
        }
        (Some(HoldAction::Wallet(index)), Gesture::Tap) => {
            match app.wallets().get(index).map(|wallet| wallet.id) {
                Some(id) => report!(app.load_wallet_id(id, store)),
                None => Effect::None,
            }
        }

        (Some(HoldAction::Import), Gesture::Tap) => {
            app.begin_import();
            Effect::Redraw
        }
        (Some(HoldAction::Create), Gesture::Tap) => {
            app.begin_create();
            Effect::Redraw
        }
        (Some(HoldAction::ScanSeedQr), Gesture::Tap) => {
            app.choose_seed_qr_import();
            Effect::BeginSeedQrScan
        }
        (Some(HoldAction::EnterMnemonic), Gesture::Tap) => {
            app.choose_mnemonic_import();
            Effect::Redraw
        }
        (Some(HoldAction::EnterFrost), Gesture::Tap) => {
            app.choose_frost_import();
            Effect::Redraw
        }

        (Some(HoldAction::FrostDecrease), Gesture::Tap) => {
            match app.screen() {
                Screen::FrostThreshold => app.decrease_frost_threshold(),
                Screen::FrostShareIndex => app.decrease_frost_share_index(),
                _ => {}
            }
            Effect::Redraw
        }
        (Some(HoldAction::FrostIncrease), Gesture::Tap) => {
            match app.screen() {
                Screen::FrostThreshold => app.increase_frost_threshold(),
                Screen::FrostShareIndex => app.increase_frost_share_index(),
                _ => {}
            }
            Effect::Redraw
        }
        (Some(HoldAction::FrostConfirm), Gesture::Tap) => {
            app.confirm_frost_threshold();
            Effect::Redraw
        }
        (Some(HoldAction::FrostEnterShare), Gesture::Tap) => {
            let _ = app.start_frost_share_entry();
            Effect::Redraw
        }
        (Some(HoldAction::FrostEditShare), Gesture::Tap) => {
            let _ = app.edit_frost_share();
            Effect::Redraw
        }
        (Some(HoldAction::FrostAcceptShare), Gesture::Tap) => {
            let _ = app.confirm_frost_share();
            Effect::Redraw
        }
        (Some(HoldAction::MnemonicWords(words)), Gesture::Tap) => {
            let _ = app.start_mnemonic_entry(words);
            Effect::Redraw
        }

        (Some(HoldAction::Network(network)), Gesture::Tap) => {
            app.select_network(network);
            Effect::Redraw
        }

        // Selecting a derivation, declining a passphrase, or confirming one can
        // each complete the setup flow and land on `Deriving`.
        (Some(HoldAction::Derivation(account)), Gesture::Tap) => {
            app.select_account_type(account);
            derive_if_ready(app)
        }
        (Some(HoldAction::NoPassphrase), Gesture::Tap) => {
            app.choose_no_passphrase();
            derive_if_ready(app)
        }
        (Some(HoldAction::ConfirmPassphrase), Gesture::Tap) => {
            app.confirm_passphrase();
            derive_if_ready(app)
        }
        (Some(HoldAction::EnterPassphrase), Gesture::Tap) => {
            app.begin_passphrase_entry();
            Effect::Redraw
        }

        (Some(HoldAction::GenerationWords(words)), Gesture::Tap) => {
            let _ = app.select_generation_words(words);
            Effect::Redraw
        }
        (Some(HoldAction::CameraEntropy), Gesture::Tap) => {
            app.begin_camera_entropy();
            Effect::Redraw
        }
        (Some(HoldAction::DiceEntropy), Gesture::Tap) => {
            app.begin_dice_entropy();
            Effect::Redraw
        }
        (Some(HoldAction::DiceRoll(roll)), Gesture::Tap) => {
            let _ = app.add_dice_roll(roll);
            Effect::Redraw
        }
        (Some(HoldAction::EntropyDone), Gesture::Tap) => {
            if app.screen() == Screen::DiceEntropy {
                app.finish_dice_entropy();
                Effect::Redraw
            } else {
                app.navigate(Screen::Deriving);
                Effect::RenderThenDerive
            }
        }

        (Some(HoldAction::Suggestion(index)), Gesture::Tap) => {
            let _ = app.choose_mnemonic_suggestion(index);
            Effect::Redraw
        }
        (Some(HoldAction::Key(letter)), Gesture::Tap) => {
            if app.screen() == Screen::PassphraseEntry {
                app.passphrase_letter(letter);
            } else {
                app.mnemonic_letter(letter);
            }
            Effect::Redraw
        }
        (Some(HoldAction::Backspace), Gesture::Tap) => {
            if app.screen() == Screen::PassphraseEntry {
                app.passphrase_backspace();
            } else {
                app.mnemonic_backspace();
            }
            Effect::Redraw
        }
        (Some(HoldAction::PassphraseSpace), Gesture::Tap) => {
            app.passphrase_space();
            Effect::Redraw
        }
        (Some(HoldAction::PassphraseKey(index)), Gesture::Tap) => {
            app.passphrase_key(index);
            Effect::Redraw
        }

        (Some(HoldAction::Cancel), Gesture::Tap) => {
            match app.screen() {
                Screen::Backup => app.cancel_backup(),
                _ => app.cancel_setup(),
            }
            Effect::Redraw
        }

        // Mnemonic and passphrase entry ask for confirmation before
        // discarding what was typed, rather than cancelling immediately.
        (Some(HoldAction::OpenCancelEntry), Gesture::Tap) => {
            app.open_cancel_entry_confirm();
            Effect::Redraw
        }
        (Some(HoldAction::ConfirmCancelEntry), Gesture::Tap) => {
            app.confirm_cancel_entry();
            Effect::Redraw
        }

        (Some(HoldAction::Scan), Gesture::Tap) => match app.begin_scan() {
            Ok(()) => Effect::BeginScan,
            Err(error) => report(app, Err(error)),
        },
        (Some(HoldAction::BackupMenu), Gesture::Tap) => report!(app.open_backup_menu()),
        (Some(HoldAction::SaveWallet), Gesture::Tap) => report!(app.save_wallet(store)),
        (Some(HoldAction::MnemonicBackup), Gesture::Tap) => report!(app.open_backup()),
        (Some(HoldAction::SeedQrBackup), Gesture::Tap) => report!(app.open_backup_seed_qr()),
        (Some(HoldAction::Descriptor), Gesture::Tap) => report!(app.open_descriptor()),
        (Some(HoldAction::Logout), Gesture::Tap) => report!(app.logout(store)),

        (Some(HoldAction::BackupPage), Gesture::SwipeLeft) => {
            app.next_backup_page();
            Effect::Redraw
        }
        (Some(HoldAction::BackupPage), Gesture::SwipeRight) => {
            app.previous_backup_page();
            Effect::Redraw
        }

        // A swipe from any action goes back, when the screen allows it.
        (Some(_), Gesture::SwipeRight) if supports_swipe_back(app.screen()) => {
            let dim = shows_scannable_qr(app.screen());
            app.go_back();
            if dim {
                Effect::DimBacklight
            } else {
                Effect::Redraw
            }
        }

        (Some(HoldAction::BackupPage), Gesture::Tap) => {
            app.navigate(Screen::BackupMenu);
            Effect::Redraw
        }
        (Some(HoldAction::Brightness), Gesture::Tap) => Effect::CycleBrightness,

        (Some(HoldAction::Back), Gesture::Tap) => {
            let dim = shows_scannable_qr(app.screen());
            match app.screen() {
                screen if supports_swipe_back(screen) => app.go_back(),
                Screen::WalletList => app.navigate(Screen::LoadWallet),
                Screen::BackupSeedQr | Screen::BackupWords | Screen::Descriptor => {
                    app.navigate(Screen::BackupMenu)
                }
                _ => app.navigate(Screen::Home),
            }
            if dim {
                Effect::DimBacklight
            } else {
                Effect::Redraw
            }
        }

        _ => Effect::None,
    }
}

/// Setup steps land on `Deriving` once every choice has been made.
fn derive_if_ready(app: &AppState) -> Effect {
    if app.screen() == Screen::Deriving {
        Effect::RenderThenDerive
    } else {
        Effect::Redraw
    }
}

/// Screens showing a QR the user points another device at. Leaving one dims the
/// backlight, which was raised to make the code scannable.
fn shows_scannable_qr(screen: Screen) -> bool {
    matches!(screen, Screen::BackupSeedQr | Screen::Descriptor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use picosigner_board::{
        StoredSeed as BoardSeed, WalletCatalog, WalletId, WalletMetadata, WalletStoreErrorKind,
    };
    use picosigner_wallet::Network;

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

    #[derive(Default)]
    struct Store(Option<BoardSeed>);
    impl picosigner_board::SettingsStore for Store {
        type SettingsError = ();
        fn load_settings(&mut self) -> Result<picosigner_board::DeviceSettings, ()> {
            Ok(Default::default())
        }
        fn save_settings(&mut self, _: picosigner_board::DeviceSettings) -> Result<(), ()> {
            Ok(())
        }
    }
    impl WalletStore for Store {
        type Error = ();
        fn catalog(&mut self) -> Result<WalletCatalog, Self::Error> {
            let mut catalog = WalletCatalog::empty();
            if let Some(seed) = self.0 {
                catalog.push(WalletMetadata {
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
        fn error_kind(_: &Self::Error) -> WalletStoreErrorKind {
            WalletStoreErrorKind::Storage
        }
    }

    /// Drives `dispatch` the way a host does.
    fn tap(app: &mut AppState, action: HoldAction) -> Effect {
        let mut entropy = Entropy;
        let mut store = Store::default();
        dispatch(app, &mut entropy, &mut store, Some(action), Gesture::Tap)
    }

    fn booted() -> AppState {
        let mut app = AppState::new(Network::Signet);
        let mut entropy = Entropy;
        let mut store = Store::default();
        app.boot(&mut entropy, &mut store).expect("boot");
        app
    }

    #[test]
    fn unhandled_pairs_do_nothing() {
        let mut app = booted();
        let before = app.screen();
        // `Backup` only responds to a hold, never a tap.
        assert_eq!(tap(&mut app, HoldAction::Backup), Effect::None);
        assert_eq!(app.screen(), before);
    }

    #[test]
    fn navigation_actions_request_a_redraw() {
        let mut app = booted();
        assert_eq!(tap(&mut app, HoldAction::Import), Effect::Redraw);
        assert_eq!(app.screen(), Screen::ImportWallet);
    }

    /// Cancelling FROST word entry steps back one share instead of discarding
    /// the whole import. The firmware and the emulator disagreed about this
    /// before dispatch was shared; the firmware's behaviour is the correct one.
    #[test]
    fn cancelling_frost_entry_steps_back_instead_of_discarding_setup() {
        let mut app = booted();
        tap(&mut app, HoldAction::Import);
        tap(&mut app, HoldAction::EnterFrost);
        tap(&mut app, HoldAction::FrostConfirm);
        tap(&mut app, HoldAction::FrostEnterShare);
        assert_eq!(app.screen(), Screen::FrostMnemonicEntry);

        assert_eq!(tap(&mut app, HoldAction::OpenCancelEntry), Effect::Redraw);
        assert_eq!(app.screen(), Screen::CancelEntryConfirm);
        assert_eq!(
            tap(&mut app, HoldAction::ConfirmCancelEntry),
            Effect::Redraw
        );
        assert_ne!(
            app.screen(),
            Screen::LoadWallet,
            "cancelling one share must not discard the whole FROST import"
        );
    }

    /// Declining the cancel-entry prompt returns to entry exactly as it was
    /// left, rather than clearing it the way the entry screen's own back
    /// handling would.
    #[test]
    fn declining_cancel_entry_keeps_what_was_typed() {
        let mut app = booted();
        tap(&mut app, HoldAction::Import);
        tap(&mut app, HoldAction::EnterMnemonic);
        tap(&mut app, HoldAction::MnemonicWords(12));
        tap(&mut app, HoldAction::Key(b'a'));
        assert_eq!(app.mnemonic_prefix(), "a");

        assert_eq!(tap(&mut app, HoldAction::OpenCancelEntry), Effect::Redraw);
        assert_eq!(app.screen(), Screen::CancelEntryConfirm);
        assert_eq!(tap(&mut app, HoldAction::Back), Effect::Redraw);

        assert_eq!(app.screen(), Screen::MnemonicEntry);
        assert_eq!(app.mnemonic_prefix(), "a");
    }

    /// Scanning is the host's job: dispatch only opens the screen and says so.
    #[test]
    fn scan_actions_delegate_to_the_host() {
        let mut app = booted();
        tap(&mut app, HoldAction::Import);
        assert_eq!(
            tap(&mut app, HoldAction::ScanSeedQr),
            Effect::BeginSeedQrScan
        );
        assert_eq!(app.screen(), Screen::SeedQrScan);
    }

    /// Derivation is slow, so the host paints first and derives after.
    #[test]
    fn completing_setup_defers_derivation_to_the_host() {
        let mut app = booted();
        tap(&mut app, HoldAction::Create);
        tap(&mut app, HoldAction::Network(Network::Signet));
        let effect = tap(
            &mut app,
            HoldAction::Derivation(picosigner_wallet::AccountType::NativeSegwit),
        );
        // Either a further choice is needed, or derivation is deferred.
        assert!(matches!(effect, Effect::Redraw | Effect::RenderThenDerive));
        if effect == Effect::RenderThenDerive {
            assert_eq!(app.screen(), Screen::Deriving);
        }
    }

    /// Leaving a QR screen dims the backlight that was raised to show it.
    #[test]
    fn leaving_a_qr_screen_dims_the_backlight() {
        let mut app = booted();
        app.navigate(Screen::Descriptor);
        assert_eq!(tap(&mut app, HoldAction::Back), Effect::DimBacklight);
        assert_eq!(app.screen(), Screen::BackupMenu);

        app.navigate(Screen::Home);
        assert_eq!(tap(&mut app, HoldAction::Back), Effect::Redraw);
    }

    #[test]
    fn brightness_is_the_hosts_to_apply() {
        let mut app = booted();
        let before = app.screen();
        assert_eq!(
            tap(&mut app, HoldAction::Brightness),
            Effect::CycleBrightness
        );
        assert_eq!(app.screen(), before, "brightness must not navigate");
    }

    #[test]
    fn effects_other_than_none_need_a_redraw() {
        assert!(!Effect::None.needs_redraw());
        for effect in [
            Effect::Redraw,
            Effect::RenderThenDerive,
            Effect::BeginScan,
            Effect::BeginSeedQrScan,
            Effect::RenderThenSign,
            Effect::CycleBrightness,
            Effect::DimBacklight,
        ] {
            assert!(effect.needs_redraw(), "{effect:?}");
        }
    }
}
