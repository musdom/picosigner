use std::{
    ffi::OsString,
    fmt,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use picosigner::{
    app::AppState,
    ui::{action_at_for_app, supports_swipe_back, HoldAction, REVIEW_SIGN_BUTTON},
};
use picosigner_board::{
    CryptoEntropy, Network as BoardNetwork, SeedStore, StoredSeed, WalletCatalog, WalletId,
    WalletMetadata, WalletStore, WalletStoreErrorKind, MAX_DESCRIPTOR_BYTES, MAX_WALLETS,
};
use picosigner_ui::{Gesture, GestureConfig, Navigator, Screen, TouchTracker, UiEvent};
use picosigner_wallet::Network;

const STATE_MAGIC_V1: &[u8; 7] = b"PSEMU\0\x01";
const STATE_MAGIC: &[u8; 7] = b"PSEMU\0\x02";
const STATE_MAGIC_V3: &[u8; 7] = b"PSEMU\0\x03";
const V1_STATE_LEN: usize = STATE_MAGIC_V1.len() + 2 + 16;
const STATE_LEN: usize = STATE_MAGIC.len() + 2 + 16 + 2 + MAX_DESCRIPTOR_BYTES + 32;
const V3_RECORD_LEN: usize = 1 + 1 + 1 + 1 + 4 + 32 + 2 + MAX_DESCRIPTOR_BYTES + 32;
const V3_STATE_LEN: usize = STATE_MAGIC_V3.len() + MAX_WALLETS * V3_RECORD_LEN;

#[derive(Debug)]
pub enum EmulatorError {
    Io(io::Error),
    State(&'static str),
    Arguments(String),
    App(String),
}

impl fmt::Display for EmulatorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::State(message) => f.write_str(message),
            Self::Arguments(message) | Self::App(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for EmulatorError {}

impl From<io::Error> for EmulatorError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub input: PathBuf,
    pub output: PathBuf,
    pub state: PathBuf,
    pub seedqr: PathBuf,
    pub scale: u32,
}

impl Default for Config {
    fn default() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        Self {
            input: root.join("input.psbt"),
            output: root.join("signed.psbt"),
            state: root.join("wallet-state.bin"),
            seedqr: root.join("seedqr.bin"),
            scale: 1,
        }
    }
}

impl Config {
    pub fn parse<I, S>(args: I) -> Result<Self, EmulatorError>
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let mut config = Self::default();
        let mut args = args.into_iter().map(Into::into);
        while let Some(flag) = args.next() {
            let flag = flag.to_string_lossy();
            let value = |args: &mut dyn Iterator<Item = OsString>| {
                args.next()
                    .ok_or_else(|| EmulatorError::Arguments(format!("missing value for {flag}")))
            };
            match flag.as_ref() {
                "--input" => config.input = PathBuf::from(value(&mut args)?),
                "--output" => config.output = PathBuf::from(value(&mut args)?),
                "--state" => config.state = PathBuf::from(value(&mut args)?),
                "--seedqr" => config.seedqr = PathBuf::from(value(&mut args)?),
                "--scale" => {
                    let raw = value(&mut args)?;
                    config.scale = raw.to_string_lossy().parse().map_err(|_| {
                        EmulatorError::Arguments("--scale must be a positive integer".into())
                    })?;
                    if config.scale == 0 || config.scale > 16 {
                        return Err(EmulatorError::Arguments(
                            "--scale must be between 1 and 16".into(),
                        ));
                    }
                }
                "--help" | "-h" => return Err(EmulatorError::Arguments(help().into())),
                _ => return Err(EmulatorError::Arguments(format!("unknown option: {flag}"))),
            }
        }
        Ok(config)
    }
}

pub fn help() -> &'static str {
    "Usage: picosigner-emulator [--input PATH] [--output PATH] [--state PATH] [--seedqr PATH] [--scale N]"
}

pub struct OsEntropy;

impl CryptoEntropy for OsEntropy {
    type Error = getrandom::Error;
    fn fill(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
        getrandom::getrandom(dst)
    }
}

pub struct FileSeedStore {
    path: PathBuf,
}

impl FileSeedStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl FileSeedStore {
    fn read_all(&self) -> Result<[Option<StoredSeed>; MAX_WALLETS], EmulatorError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok([None; MAX_WALLETS]),
            Err(error) => return Err(error.into()),
        };
        let mut wallets = [None; MAX_WALLETS];
        if bytes.len() == V3_STATE_LEN && bytes.get(..STATE_MAGIC_V3.len()) == Some(STATE_MAGIC_V3)
        {
            for (slot, destination) in wallets.iter_mut().enumerate() {
                let start = STATE_MAGIC_V3.len() + slot * V3_RECORD_LEN;
                let record = &bytes[start..start + V3_RECORD_LEN];
                if record[0] == 0 {
                    continue;
                }
                if record[0] != 1 {
                    return Err(EmulatorError::State("invalid wallet presence flag"));
                }
                let network = decode_network(record[1])?;
                let backup_pending = match record[2] {
                    0 => false,
                    1 => true,
                    _ => return Err(EmulatorError::State("invalid backup flag")),
                };
                let entropy_len = record[3];
                if entropy_len != 16 && entropy_len != 32 {
                    return Err(EmulatorError::State("invalid entropy length"));
                }
                let mut seed = StoredSeed::from_entropy(
                    &record[8..8 + usize::from(entropy_len)],
                    network,
                    backup_pending,
                )
                .ok_or(EmulatorError::State("invalid entropy"))?;
                seed.fingerprint.copy_from_slice(&record[4..8]);
                let len_offset = 40;
                seed.descriptor_len =
                    u16::from_le_bytes(record[len_offset..len_offset + 2].try_into().unwrap());
                if usize::from(seed.descriptor_len) > MAX_DESCRIPTOR_BYTES {
                    return Err(EmulatorError::State("invalid descriptor length"));
                }
                seed.descriptor
                    .copy_from_slice(&record[42..42 + MAX_DESCRIPTOR_BYTES]);
                seed.descriptor_tag
                    .copy_from_slice(&record[42 + MAX_DESCRIPTOR_BYTES..]);
                *destination = Some(seed);
            }
            return Ok(wallets);
        }
        let is_v1 = bytes.len() == V1_STATE_LEN
            && bytes.get(..STATE_MAGIC_V1.len()) == Some(STATE_MAGIC_V1);
        let is_v2 = bytes.len() == STATE_LEN && bytes.get(..STATE_MAGIC.len()) == Some(STATE_MAGIC);
        if !is_v1 && !is_v2 {
            return Err(EmulatorError::State(
                "invalid or unsupported emulator wallet state",
            ));
        }
        let network = decode_network(bytes[STATE_MAGIC.len()])?;
        let backup_pending = match bytes[STATE_MAGIC.len() + 1] {
            0 => false,
            1 => true,
            _ => return Err(EmulatorError::State("invalid backup flag")),
        };
        let entropy_offset = STATE_MAGIC.len() + 2;
        let mut entropy = [0; 16];
        entropy.copy_from_slice(&bytes[entropy_offset..entropy_offset + 16]);
        let mut seed = StoredSeed::new(entropy, network, backup_pending);
        if is_v2 {
            let len_offset = entropy_offset + 16;
            seed.descriptor_len =
                u16::from_le_bytes(bytes[len_offset..len_offset + 2].try_into().unwrap());
            if usize::from(seed.descriptor_len) > MAX_DESCRIPTOR_BYTES {
                return Err(EmulatorError::State("invalid descriptor length"));
            }
            seed.descriptor
                .copy_from_slice(&bytes[len_offset + 2..len_offset + 2 + MAX_DESCRIPTOR_BYTES]);
            seed.descriptor_tag
                .copy_from_slice(&bytes[len_offset + 2 + MAX_DESCRIPTOR_BYTES..]);
        }
        wallets[0] = Some(seed);
        Ok(wallets)
    }

    fn write_all(&self, wallets: &[Option<StoredSeed>; MAX_WALLETS]) -> Result<(), EmulatorError> {
        let mut bytes = vec![0u8; V3_STATE_LEN];
        bytes[..STATE_MAGIC_V3.len()].copy_from_slice(STATE_MAGIC_V3);
        for (slot, seed) in wallets.iter().enumerate() {
            let Some(seed) = seed else { continue };
            let start = STATE_MAGIC_V3.len() + slot * V3_RECORD_LEN;
            let record = &mut bytes[start..start + V3_RECORD_LEN];
            record[0] = 1;
            record[1] = seed.network as u8;
            record[2] = u8::from(seed.backup_pending);
            record[3] = seed.entropy_len;
            record[4..8].copy_from_slice(&seed.fingerprint);
            record[8..40].copy_from_slice(&seed.entropy);
            record[40..42].copy_from_slice(&seed.descriptor_len.to_le_bytes());
            record[42..42 + MAX_DESCRIPTOR_BYTES].copy_from_slice(&seed.descriptor);
            record[42 + MAX_DESCRIPTOR_BYTES..].copy_from_slice(&seed.descriptor_tag);
        }
        atomic_write(&self.path, &bytes).map_err(Into::into)
    }
}

fn decode_network(value: u8) -> Result<BoardNetwork, EmulatorError> {
    match value {
        0 => Ok(BoardNetwork::Mainnet),
        1 => Ok(BoardNetwork::Testnet4),
        2 => Ok(BoardNetwork::Signet),
        _ => Err(EmulatorError::State("invalid network")),
    }
}

impl SeedStore for FileSeedStore {
    type Error = EmulatorError;
    fn load(&mut self) -> Result<Option<StoredSeed>, Self::Error> {
        Ok(self.read_all()?[0])
    }
    fn store(&mut self, seed: StoredSeed) -> Result<(), Self::Error> {
        let mut wallets = self.read_all()?;
        wallets[0] = Some(seed);
        self.write_all(&wallets)
    }
}

impl WalletStore for FileSeedStore {
    type Error = EmulatorError;
    fn catalog(&mut self) -> Result<WalletCatalog, Self::Error> {
        let wallets = self.read_all()?;
        let mut catalog = WalletCatalog::empty();
        for (slot, seed) in wallets.iter().enumerate() {
            if let Some(seed) = seed {
                catalog.push(WalletMetadata {
                    id: WalletId(slot as u8),
                    fingerprint: (seed.fingerprint != [0; 4]).then_some(seed.fingerprint),
                    network: seed.network,
                    backup_pending: seed.backup_pending,
                });
            }
        }
        Ok(catalog)
    }
    fn load_wallet(&mut self, id: WalletId) -> Result<Option<StoredSeed>, Self::Error> {
        Ok(self.read_all()?.get(usize::from(id.0)).copied().flatten())
    }
    fn insert_wallet(&mut self, seed: StoredSeed) -> Result<WalletId, Self::Error> {
        let mut wallets = self.read_all()?;
        if wallets.iter().flatten().any(|existing| {
            existing.network == seed.network && existing.entropy() == seed.entropy()
        }) {
            return Err(EmulatorError::State("wallet already exists"));
        }
        let slot = wallets
            .iter()
            .position(Option::is_none)
            .ok_or(EmulatorError::State("wallet store is full"))?;
        wallets[slot] = Some(seed);
        self.write_all(&wallets)?;
        Ok(WalletId(slot as u8))
    }
    fn update_wallet(&mut self, id: WalletId, seed: StoredSeed) -> Result<(), Self::Error> {
        let mut wallets = self.read_all()?;
        let destination = wallets
            .get_mut(usize::from(id.0))
            .ok_or(EmulatorError::State("invalid wallet"))?;
        if destination.is_none() {
            return Err(EmulatorError::State("invalid wallet"));
        }
        *destination = Some(seed);
        self.write_all(&wallets)
    }
    fn delete_wallet(&mut self, id: WalletId) -> Result<(), Self::Error> {
        let mut wallets = self.read_all()?;
        let destination = wallets
            .get_mut(usize::from(id.0))
            .ok_or(EmulatorError::State("invalid wallet"))?;
        if destination.is_none() {
            return Err(EmulatorError::State("invalid wallet"));
        }
        *destination = None;
        self.write_all(&wallets)
    }
    fn error_kind(error: &Self::Error) -> WalletStoreErrorKind {
        match error {
            EmulatorError::State("wallet already exists") => WalletStoreErrorKind::Duplicate,
            EmulatorError::State("wallet store is full") => WalletStoreErrorKind::Full,
            _ => WalletStoreErrorKind::Storage,
        }
    }
}

pub fn read_psbt_file(path: &Path) -> Result<Vec<u8>, EmulatorError> {
    let bytes = fs::read(path)?;
    if bytes.starts_with(b"psbt\xff") {
        return Ok(bytes);
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| EmulatorError::App("input is neither raw PSBT nor Base64 text".into()))?;
    STANDARD
        .decode(text.trim())
        .map_err(|_| EmulatorError::App("invalid Base64 PSBT input".into()))
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("tmp");
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(temporary, path)
}

pub struct Controller {
    pub app: AppState,
    pub config: Config,
    store: FileSeedStore,
    entropy: OsEntropy,
    tracker: TouchTracker,
    action: Option<HoldAction>,
    last_touch: (u16, u16),
    status: Option<String>,
}

impl Controller {
    pub fn new(config: Config) -> Result<Self, EmulatorError> {
        let mut store = FileSeedStore::new(config.state.clone());
        let mut entropy = OsEntropy;
        let mut app = AppState::new(Network::Signet);
        app.boot(&mut entropy, &mut store)
            .map_err(|error| EmulatorError::App(format!("wallet boot failed: {error:?}")))?;
        Ok(Self {
            app,
            config,
            store,
            entropy,
            tracker: TouchTracker::new(GestureConfig::default()),
            action: None,
            last_touch: (0, 0),
            status: None,
        })
    }

    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }
    pub fn action(&self) -> Option<HoldAction> {
        self.action
    }
    pub fn hold_progress(&self) -> (u16, u16) {
        self.tracker.hold_progress()
    }

    pub fn mouse_down(&mut self, x: u16, y: u16) {
        self.last_touch = (x, y);
        self.action = if self.status.is_some() {
            (y >= picosigner::ui::DESCRIPTOR_BACK_BUTTON.1 as u16).then_some(HoldAction::Back)
        } else {
            action_at_for_app(&self.app, x, y)
        };
        if self.action.is_some() {
            self.tracker.event(UiEvent::TouchDown { x, y });
        }
    }

    pub fn mouse_move(&mut self, x: u16, y: u16) {
        self.last_touch = (x, y);
    }

    pub fn tick(&mut self) {
        if self.tracker.active() {
            self.tracker.event(UiEvent::Tick);
        }
    }

    pub fn mouse_up(&mut self, x: u16, y: u16) {
        self.last_touch = (x, y);
        let action = self.action.take();
        let Some(gesture) = self.tracker.event(UiEvent::TouchUp { x, y }) else {
            return;
        };
        if self.status.is_some() {
            self.status = None;
            if self.app.screen() == Screen::Signed {
                self.app.navigate(Screen::Home);
            } else if self.app.screen() == Screen::SeedQrScan {
                self.app.navigate(Screen::ImportWallet);
            }
            return;
        }
        match (action, gesture.gesture) {
            (Some(HoldAction::Backup), Gesture::Hold) => {
                if let Err(error) = self.app.acknowledge_backup_and_store(&mut self.store) {
                    self.fail(format!("could not save backup state: {error:?}"));
                }
            }
            (Some(HoldAction::DeleteWallet), Gesture::Hold) => {
                if let Err(error) = self.app.delete_wallet(&mut self.store) {
                    self.fail(format!("could not delete wallet: {error:?}"));
                }
            }
            (Some(HoldAction::Regenerate), Gesture::Hold) => {
                if let Err(error) = self.app.generate_seed(&mut self.entropy, &mut self.store) {
                    self.fail(format!("could not create wallet: {error:?}"));
                }
            }
            (Some(HoldAction::OpenDeleteWallet), Gesture::Tap) => {
                self.app.navigate(Screen::DeleteWallet)
            }
            (Some(HoldAction::Load), Gesture::Tap) => {
                self.app.navigate(Screen::WalletList);
            }
            (Some(HoldAction::Wallet(_)), Gesture::SwipeUp) => self.app.next_wallet_list_page(),
            (Some(HoldAction::Wallet(_)), Gesture::SwipeDown) => {
                self.app.previous_wallet_list_page()
            }
            (Some(HoldAction::Wallet(index)), Gesture::Tap) => {
                if let Some(id) = self.app.wallets().get(index).map(|wallet| wallet.id) {
                    if let Err(error) = self.app.load_wallet_id(id, &mut self.store) {
                        self.fail(format!("could not load wallet: {error:?}"));
                    }
                }
            }
            (Some(HoldAction::Import), Gesture::Tap) => self.app.begin_import(),
            (Some(HoldAction::Create), Gesture::Tap) => self.app.begin_create(),
            (Some(HoldAction::ScanSeedQr), Gesture::Tap) => self.load_seed_qr(),
            (Some(HoldAction::EnterMnemonic), Gesture::Tap) => self.app.choose_mnemonic_import(),
            (Some(HoldAction::EnterFrost), Gesture::Tap) => self.app.choose_frost_import(),
            (Some(HoldAction::FrostDecrease), Gesture::Tap) => match self.app.screen() {
                Screen::FrostThreshold => self.app.decrease_frost_threshold(),
                Screen::FrostShareIndex => self.app.decrease_frost_share_index(),
                _ => {}
            },
            (Some(HoldAction::FrostIncrease), Gesture::Tap) => match self.app.screen() {
                Screen::FrostThreshold => self.app.increase_frost_threshold(),
                Screen::FrostShareIndex => self.app.increase_frost_share_index(),
                _ => {}
            },
            (Some(HoldAction::FrostConfirm), Gesture::Tap) => self.app.confirm_frost_threshold(),
            (Some(HoldAction::FrostEnterShare), Gesture::Tap) => {
                let _ = self.app.start_frost_share_entry();
            }
            (Some(HoldAction::MnemonicWords(words)), Gesture::Tap) => {
                let _ = self.app.start_mnemonic_entry(words);
            }
            (Some(HoldAction::Network(network)), Gesture::Tap) => self.app.select_network(network),
            (Some(HoldAction::Derivation), Gesture::Tap) => {
                self.app.navigate(Screen::Deriving);
                if let Err(error) = self
                    .app
                    .confirm_derivation(&mut self.entropy, &mut self.store)
                {
                    self.fail(format!("could not save wallet: {error:?}"));
                }
            }
            (Some(HoldAction::Suggestion(index)), Gesture::Tap) => {
                let _ = self.app.choose_mnemonic_suggestion(index);
            }
            (Some(HoldAction::Key(letter)), Gesture::Tap) => self.app.mnemonic_letter(letter),
            (Some(HoldAction::Backspace), Gesture::Tap) => self.app.mnemonic_backspace(),
            (Some(HoldAction::CancelEntry), Gesture::Hold) => {
                if self.app.screen() == Screen::FrostMnemonicEntry {
                    self.app.go_back();
                } else {
                    self.app.cancel_setup();
                }
            }
            (Some(HoldAction::Cancel), Gesture::Tap) if self.app.screen() == Screen::Backup => {
                self.app.cancel_backup()
            }
            (Some(HoldAction::Cancel), Gesture::Tap) => self.app.cancel_setup(),
            (Some(HoldAction::Scan), Gesture::Tap) => self.load_psbt(),
            (Some(HoldAction::BackupMenu), Gesture::Tap) => {
                if let Err(error) = self.app.open_backup_menu() {
                    self.fail(format!("backup unavailable: {error:?}"));
                }
            }
            (Some(HoldAction::SaveWallet), Gesture::Tap) => {
                if let Err(error) = self.app.save_wallet(&mut self.store) {
                    self.fail(format!("could not save wallet: {error:?}"));
                }
            }
            (Some(HoldAction::MnemonicBackup), Gesture::Tap) => {
                let _ = self.app.open_backup();
            }
            (Some(HoldAction::SeedQrBackup), Gesture::Tap) => {
                let _ = self.app.open_backup_seed_qr();
            }
            (Some(HoldAction::Logout), Gesture::Tap) => {
                if let Err(error) = self.app.logout(&mut self.store) {
                    self.fail(format!("logout failed: {error:?}"));
                }
            }
            (Some(HoldAction::BackupPage), Gesture::SwipeLeft) => self.app.next_backup_page(),
            (Some(HoldAction::BackupPage), Gesture::SwipeRight) => self.app.previous_backup_page(),
            (Some(_), Gesture::SwipeRight) if supports_swipe_back(self.app.screen()) => {
                self.app.go_back()
            }
            (Some(HoldAction::BackupPage), Gesture::Tap) => self.app.navigate(Screen::BackupMenu),
            (Some(HoldAction::Descriptor), Gesture::Tap) => {
                if let Err(error) = self.app.open_descriptor() {
                    self.fail(format!("descriptor unavailable: {error:?}"));
                }
            }
            (Some(HoldAction::Back), Gesture::Tap) => match self.app.screen() {
                screen if supports_swipe_back(screen) => self.app.go_back(),
                Screen::WalletList => self.app.navigate(Screen::LoadWallet),
                Screen::BackupSeedQr | Screen::BackupWords => self.app.navigate(Screen::BackupMenu),
                Screen::Descriptor => self.app.navigate(Screen::BackupMenu),
                Screen::BackupMenu => self.app.navigate(Screen::Home),
                _ => self.app.navigate(Screen::Home),
            },
            (Some(HoldAction::Sign), Gesture::Hold)
                if picosigner::ui::in_button(x, y, REVIEW_SIGN_BUTTON) =>
            {
                self.sign()
            }
            _ => {}
        }
    }

    fn load_psbt(&mut self) {
        if let Err(error) = self.app.begin_scan() {
            self.fail(format!("scan unavailable: {error:?}"));
            return;
        }
        match read_psbt_file(&self.config.input).and_then(|bytes| {
            self.app
                .receive_psbt(&bytes)
                .map(|_| ())
                .map_err(|error| EmulatorError::App(format!("PSBT rejected: {error:?}")))
        }) {
            Ok(()) => {}
            Err(error) => {
                self.app.navigate(Screen::Home);
                self.fail(error.to_string());
            }
        }
    }

    fn load_seed_qr(&mut self) {
        self.app.choose_seed_qr_import();
        match fs::read(&self.config.seedqr) {
            Ok(payload) => {
                if let Err(error) = self.app.accept_seed_qr(&payload) {
                    self.fail(format!("SeedQR rejected: {error:?}"));
                }
            }
            Err(error) => self.fail(format!("could not read SeedQR: {error}")),
        }
    }

    fn sign(&mut self) {
        if let Err(error) = self.app.mark_all_outputs_reviewed() {
            self.fail(format!("review incomplete: {error:?}"));
            return;
        }
        match self.app.sign_after_hold(true) {
            Ok(signed) => {
                if let Err(error) = atomic_write(&self.config.output, &signed.bytes) {
                    self.fail(format!("could not write signed PSBT: {error}"));
                }
            }
            Err(error) => {
                self.app.navigate(Screen::Review);
                self.fail(format!("signing failed: {error:?}"));
            }
        }
    }

    fn fail(&mut self, message: String) {
        self.status = Some(message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::{
        bip32::{ChildNumber, DerivationPath, Xpriv},
        hashes::Hash,
        secp256k1::{Keypair, Secp256k1, XOnlyPublicKey},
        Amount, OutPoint, Psbt, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid,
    };
    use embedded_graphics::{
        geometry::{Point, Size},
        pixelcolor::Rgb565,
    };
    use embedded_graphics_simulator::SimulatorDisplay;
    use picosigner::palette;
    use tempfile::tempdir;

    #[test]
    fn config_defaults_and_overrides() {
        let defaults = Config::parse(Vec::<OsString>::new()).unwrap();
        assert!(defaults.input.ends_with("picosigner-emulator/input.psbt"));
        let parsed = Config::parse(["--input", "/tmp/a", "--scale", "3"]).unwrap();
        assert_eq!(parsed.input, PathBuf::from("/tmp/a"));
        assert_eq!(parsed.scale, 3);
    }

    #[test]
    fn state_round_trip_and_corruption_rejection() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state");
        let seed = StoredSeed::new([7; 16], BoardNetwork::Signet, true);
        let mut store = FileSeedStore::new(path.clone());
        store.store(seed).unwrap();
        assert_eq!(store.load().unwrap(), Some(seed));
        fs::write(path, b"broken").unwrap();
        assert!(store.load().is_err());
    }

    #[test]
    fn version_one_state_loads_without_a_descriptor_cache() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state");
        let mut bytes = [0u8; V1_STATE_LEN];
        bytes[..STATE_MAGIC_V1.len()].copy_from_slice(STATE_MAGIC_V1);
        bytes[STATE_MAGIC_V1.len()] = BoardNetwork::Signet as u8;
        bytes[STATE_MAGIC_V1.len() + 1] = 1;
        bytes[STATE_MAGIC_V1.len() + 2..].copy_from_slice(&[8; 16]);
        fs::write(&path, bytes).unwrap();
        let loaded = FileSeedStore::new(path).load().unwrap().unwrap();
        assert_eq!(loaded.entropy(), Some(&[8; 16][..]));
        assert!(loaded.descriptor().is_none());
    }

    #[test]
    fn controller_upgrades_version_one_state_with_a_descriptor_cache() {
        let dir = tempdir().unwrap();
        let state = dir.path().join("state");
        let mut bytes = [0u8; V1_STATE_LEN];
        bytes[..STATE_MAGIC_V1.len()].copy_from_slice(STATE_MAGIC_V1);
        bytes[STATE_MAGIC_V1.len()] = BoardNetwork::Signet as u8;
        bytes[STATE_MAGIC_V1.len() + 1] = 0;
        bytes[STATE_MAGIC_V1.len() + 2..].copy_from_slice(&[9; 16]);
        fs::write(&state, bytes).unwrap();
        let config = Config {
            input: dir.path().join("input"),
            output: dir.path().join("output"),
            state: state.clone(),
            seedqr: dir.path().join("seedqr"),
            scale: 2,
        };
        let mut controller = Controller::new(config).unwrap();
        assert_eq!(controller.app.screen(), Screen::LoadWallet);
        controller.mouse_down(120, 80);
        controller.mouse_up(120, 80);
        assert_eq!(controller.app.screen(), Screen::WalletList);
        controller.mouse_down(120, 70);
        controller.mouse_up(120, 70);
        assert_eq!(controller.app.screen(), Screen::Home);
        let upgraded = fs::read(state).unwrap();
        assert_eq!(&upgraded[..STATE_MAGIC_V3.len()], STATE_MAGIC_V3);
        assert_eq!(upgraded.len(), V3_STATE_LEN);
    }

    #[test]
    fn raw_and_base64_psbts_are_detected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("input");
        let raw = b"psbt\xffpayload";
        fs::write(&path, raw).unwrap();
        assert_eq!(read_psbt_file(&path).unwrap(), raw);
        fs::write(&path, STANDARD.encode(raw)).unwrap();
        assert_eq!(read_psbt_file(&path).unwrap(), raw);
    }

    #[test]
    fn controller_persists_backup_acknowledgement() {
        let dir = tempdir().unwrap();
        let config = Config {
            input: dir.path().join("input"),
            output: dir.path().join("output"),
            state: dir.path().join("state"),
            seedqr: dir.path().join("seedqr"),
            scale: 2,
        };
        let mut controller = Controller::new(config.clone()).unwrap();
        create_wallet(&mut controller);
        assert_eq!(controller.app.screen(), Screen::Backup);
        controller.mouse_down(120, 285);
        for _ in 0..30 {
            controller.tick();
        }
        controller.mouse_up(120, 285);
        assert_eq!(controller.app.screen(), Screen::Home);
        assert!(!controller.app.wallet_is_saved());
        controller.mouse_down(120, 184);
        controller.mouse_up(120, 184);
        assert!(controller.app.wallet_is_saved());
        let fingerprint = controller.app.wallet().unwrap().master_fingerprint();
        drop(controller);
        let restarted = Controller::new(config).unwrap();
        assert_eq!(restarted.app.screen(), Screen::LoadWallet);
        let mut restarted = restarted;
        restarted.mouse_down(120, 80);
        restarted.mouse_up(120, 80);
        assert_eq!(restarted.app.screen(), Screen::WalletList);
        restarted.mouse_down(120, 70);
        restarted.mouse_up(120, 70);
        assert_eq!(
            restarted.app.wallet().unwrap().master_fingerprint(),
            fingerprint
        );
    }

    #[test]
    fn deleting_a_saved_wallet_requires_a_hold() {
        let dir = tempdir().unwrap();
        let config = Config {
            input: dir.path().join("input"),
            output: dir.path().join("output"),
            state: dir.path().join("state"),
            seedqr: dir.path().join("seedqr"),
            scale: 2,
        };
        let mut controller = Controller::new(config).unwrap();
        create_wallet(&mut controller);
        controller.mouse_down(120, 285);
        for _ in 0..30 {
            controller.tick();
        }
        controller.mouse_up(120, 285);
        controller.mouse_down(120, 184);
        controller.mouse_up(120, 184);
        assert!(controller.app.wallet_is_saved());
        assert_eq!(controller.app.screen(), Screen::Home);

        controller.mouse_down(120, 184);
        controller.mouse_up(120, 184);
        assert_eq!(controller.app.screen(), Screen::DeleteWallet);

        controller.mouse_down(24, 285);
        controller.mouse_up(24, 285);
        assert_eq!(controller.app.screen(), Screen::Home);
        assert!(controller.app.wallet_is_saved());

        controller.mouse_down(120, 184);
        controller.mouse_up(120, 184);
        assert_eq!(controller.app.screen(), Screen::DeleteWallet);
        controller.mouse_down(120, 285);
        controller.tick();
        controller.mouse_up(120, 285);
        assert_eq!(controller.app.screen(), Screen::DeleteWallet);
        assert!(controller.app.wallet_is_saved());

        controller.mouse_down(120, 285);
        for _ in 0..30 {
            controller.tick();
        }
        controller.mouse_up(120, 285);
        assert_eq!(controller.app.screen(), Screen::Home);
        assert!(!controller.app.wallet_is_saved());
        assert!(controller.app.wallets().is_empty());
        assert!(controller.app.wallet().is_some());
    }

    #[test]
    fn controller_reviews_holds_and_writes_a_signed_psbt() {
        let dir = tempdir().unwrap();
        let config = Config {
            input: dir.path().join("input.psbt"),
            output: dir.path().join("signed.psbt"),
            state: dir.path().join("state"),
            seedqr: dir.path().join("seedqr"),
            scale: 2,
        };
        let mut controller = Controller::new(config.clone()).unwrap();
        create_wallet(&mut controller);
        controller.mouse_down(120, 285);
        for _ in 0..30 {
            controller.tick();
        }
        controller.mouse_up(120, 285);

        let fixture = owned_psbt(controller.app.wallet().unwrap());
        fs::write(&config.input, fixture).unwrap();
        controller.mouse_down(120, 80);
        controller.mouse_up(120, 80);
        assert_eq!(controller.app.screen(), Screen::Review);

        controller.mouse_down(20, 220);
        controller.mouse_up(100, 220);
        assert_eq!(controller.app.screen(), Screen::Home);
        controller.mouse_down(120, 80);
        controller.mouse_up(120, 80);
        assert_eq!(controller.app.screen(), Screen::Review);

        controller.mouse_down(120, 300);
        controller.tick();
        controller.mouse_up(120, 300);
        assert!(!config.output.exists());
        assert_eq!(controller.app.screen(), Screen::Review);

        controller.mouse_down(120, 300);
        for _ in 0..30 {
            controller.tick();
        }
        controller.mouse_up(120, 300);
        assert_eq!(controller.app.screen(), Screen::Signed);
        let signed = Psbt::deserialize(&fs::read(config.output).unwrap()).unwrap();
        assert!(signed.inputs[0].tap_key_sig.is_some());
    }

    #[test]
    fn shared_home_renderer_draws_black_menu_with_white_icons() {
        let mut app = AppState::new(Network::Signet);
        app.load_seed([0; 16], false);
        let fonts = picosigner::ui::Fonts::load();
        let mut display = SimulatorDisplay::<Rgb565>::new(Size::new(240, 320));
        picosigner::ui::render_app(&mut display, &fonts, &app, None, 0);
        // Former button interiors are black, including destructive actions.
        assert_eq!(display.get_pixel(Point::new(20, 64)), palette::BACKGROUND);
        assert_eq!(display.get_pixel(Point::new(20, 115)), palette::BACKGROUND);
        assert_eq!(display.get_pixel(Point::new(20, 168)), palette::BACKGROUND);
        assert_eq!(display.get_pixel(Point::new(20, 220)), palette::BACKGROUND);
        // The scan icon is rendered in white at the start of the first row.
        assert_eq!(display.get_pixel(Point::new(25, 73)), palette::TEXT);
    }

    #[test]
    fn holding_backup_cancel_does_not_fill_confirmation_progress() {
        let mut app = AppState::new(Network::Signet);
        app.load_seed([0; 16], true);
        let fonts = picosigner::ui::Fonts::load();
        let mut display = SimulatorDisplay::<Rgb565>::new(Size::new(240, 320));

        picosigner::ui::render_app(&mut display, &fonts, &app, Some(HoldAction::Cancel), 18);
        assert_eq!(display.get_pixel(Point::new(0, 264)), palette::BACKGROUND);
        assert_eq!(display.get_pixel(Point::new(100, 266)), palette::BACKGROUND);

        picosigner::ui::render_app(&mut display, &fonts, &app, Some(HoldAction::Backup), 18);
        assert_eq!(display.get_pixel(Point::new(100, 266)), palette::PROGRESS);
    }

    #[test]
    fn wallet_list_swipes_between_four_wallet_pages() {
        let dir = tempdir().unwrap();
        let config = Config {
            input: dir.path().join("input"),
            output: dir.path().join("output"),
            state: dir.path().join("state"),
            seedqr: dir.path().join("seedqr"),
            scale: 2,
        };
        let mut store = FileSeedStore::new(config.state.clone());
        for value in 0u8..8 {
            let seed = StoredSeed::from_entropy(&[value; 16], BoardNetwork::Signet, false)
                .expect("valid test seed");
            store.insert_wallet(seed).unwrap();
        }

        let mut controller = Controller::new(config).unwrap();
        controller.mouse_down(120, 80);
        controller.mouse_up(120, 80);
        assert_eq!(controller.app.screen(), Screen::WalletList);
        assert_eq!(controller.app.wallet_list_page(), 0);

        controller.mouse_down(120, 150);
        controller.mouse_up(120, 90);
        assert_eq!(controller.app.wallet_list_page(), 1);

        controller.mouse_down(120, 90);
        controller.mouse_up(120, 150);
        assert_eq!(controller.app.wallet_list_page(), 0);

        controller.mouse_down(20, 100);
        controller.mouse_up(100, 100);
        assert_eq!(controller.app.screen(), Screen::LoadWallet);
    }

    #[test]
    fn create_flow_swipes_right_back_to_welcome() {
        let dir = tempdir().unwrap();
        let config = Config {
            input: dir.path().join("input"),
            output: dir.path().join("output"),
            state: dir.path().join("state"),
            seedqr: dir.path().join("seedqr"),
            scale: 2,
        };
        let mut controller = Controller::new(config).unwrap();

        controller.mouse_down(120, 80);
        controller.mouse_up(120, 80);
        assert_eq!(controller.app.screen(), Screen::WalletList);
        assert!(controller.app.wallets().is_empty());
        controller.mouse_down(120, 290);
        controller.mouse_up(120, 290);
        assert_eq!(controller.app.screen(), Screen::LoadWallet);

        controller.mouse_down(120, 236);
        controller.mouse_up(120, 236);
        assert_eq!(controller.app.screen(), Screen::Network);

        controller.mouse_down(20, 40);
        controller.mouse_up(100, 40);
        assert_eq!(controller.app.screen(), Screen::LoadWallet);

        controller.mouse_down(120, 236);
        controller.mouse_up(120, 236);
        assert_eq!(controller.app.screen(), Screen::Network);
        controller.mouse_down(120, 290);
        controller.mouse_up(120, 290);
        assert_eq!(controller.app.screen(), Screen::LoadWallet);

        controller.app.navigate(Screen::BackupSeedQr);
        controller.mouse_down(20, 100);
        controller.mouse_up(100, 100);
        assert_eq!(controller.app.screen(), Screen::BackupMenu);
    }

    #[test]
    fn mnemonic_entry_ignores_right_swipe_navigation() {
        let dir = tempdir().unwrap();
        let config = Config {
            input: dir.path().join("input"),
            output: dir.path().join("output"),
            state: dir.path().join("state"),
            seedqr: dir.path().join("seedqr"),
            scale: 2,
        };
        let mut controller = Controller::new(config).unwrap();
        controller.app.begin_import();
        controller.app.choose_mnemonic_import();
        controller.app.start_mnemonic_entry(12).unwrap();

        controller.mouse_down(20, 200);
        controller.mouse_up(100, 200);

        assert_eq!(controller.app.screen(), Screen::MnemonicEntry);
    }

    #[test]
    fn mnemonic_erase_center_does_not_cancel_entry() {
        let dir = tempdir().unwrap();
        let config = Config {
            input: dir.path().join("input"),
            output: dir.path().join("output"),
            state: dir.path().join("state"),
            seedqr: dir.path().join("seedqr"),
            scale: 2,
        };
        let mut controller = Controller::new(config).unwrap();
        controller.app.begin_import();
        controller.app.choose_mnemonic_import();
        controller.app.start_mnemonic_entry(12).unwrap();
        controller.app.mnemonic_letter(b'a');

        controller.mouse_down(120, 304);
        controller.mouse_up(120, 304);

        assert_eq!(controller.app.screen(), Screen::MnemonicEntry);
        assert!(controller.app.mnemonic_prefix().is_empty());
    }

    #[test]
    fn frost_keyboard_cancel_requires_hold_and_returns_to_backup_key_number() {
        let dir = tempdir().unwrap();
        let config = Config {
            input: dir.path().join("input"),
            output: dir.path().join("output"),
            state: dir.path().join("state"),
            seedqr: dir.path().join("seedqr"),
            scale: 2,
        };
        let mut controller = Controller::new(config).unwrap();
        controller.app.begin_import();
        controller.app.choose_frost_import();
        controller.app.confirm_frost_threshold();
        controller.app.start_frost_share_entry().unwrap();
        controller.app.mnemonic_letter(b'a');

        controller.mouse_down(200, 304);
        controller.mouse_up(200, 304);

        assert_eq!(controller.app.screen(), Screen::FrostMnemonicEntry);
        assert_eq!(controller.app.mnemonic_prefix(), "a");

        controller.mouse_down(200, 304);
        for _ in 0..30 {
            controller.tick();
        }
        controller.mouse_up(200, 304);

        assert_eq!(controller.app.screen(), Screen::FrostShareIndex);
        assert!(controller.app.mnemonic_prefix().is_empty());
        assert_eq!(controller.app.frost_shares_entered(), 0);
    }

    #[test]
    fn controller_opens_and_closes_wallet_backup() {
        let dir = tempdir().unwrap();
        let config = Config {
            input: dir.path().join("input.psbt"),
            output: dir.path().join("signed.psbt"),
            state: dir.path().join("state"),
            seedqr: dir.path().join("seedqr"),
            scale: 2,
        };
        let mut controller = Controller::new(config).unwrap();
        create_wallet(&mut controller);
        controller.mouse_down(120, 285);
        for _ in 0..30 {
            controller.tick();
        }
        controller.mouse_up(120, 285);

        controller.mouse_down(120, 132);
        controller.mouse_up(120, 132);
        assert_eq!(controller.app.screen(), Screen::BackupMenu);
        controller.mouse_down(120, 100);
        controller.mouse_up(120, 100);
        assert_eq!(controller.app.screen(), Screen::BackupWords);
        assert!(controller.app.mnemonic().is_ok());

        controller.mouse_down(120, 308);
        controller.mouse_up(120, 308);
        assert_eq!(controller.app.screen(), Screen::BackupMenu);
        controller.mouse_down(120, 308);
        controller.mouse_up(120, 308);
        assert_eq!(controller.app.screen(), Screen::Home);
    }

    #[test]
    fn missing_input_is_a_recoverable_in_window_error() {
        let dir = tempdir().unwrap();
        let config = Config {
            input: dir.path().join("missing.psbt"),
            output: dir.path().join("signed.psbt"),
            state: dir.path().join("state"),
            seedqr: dir.path().join("seedqr"),
            scale: 2,
        };
        let mut controller = Controller::new(config).unwrap();
        create_wallet(&mut controller);
        controller.mouse_down(120, 285);
        for _ in 0..30 {
            controller.tick();
        }
        controller.mouse_up(120, 285);
        controller.mouse_down(120, 80);
        controller.mouse_up(120, 80);
        assert_eq!(controller.app.screen(), Screen::Home);
        assert!(controller.status().is_some());
        controller.mouse_down(120, 310);
        controller.mouse_up(120, 310);
        assert!(controller.status().is_none());
    }

    #[test]
    fn controller_imports_seedqr_and_logs_out_to_loader() {
        let dir = tempdir().unwrap();
        let config = Config {
            input: dir.path().join("input.psbt"),
            output: dir.path().join("signed.psbt"),
            state: dir.path().join("state"),
            seedqr: dir.path().join("seedqr"),
            scale: 2,
        };
        fs::write(
            &config.seedqr,
            b"073318950739065415961602009907670428187212261116",
        )
        .unwrap();
        let mut controller = Controller::new(config).unwrap();
        controller.mouse_down(120, 180);
        controller.mouse_up(120, 180);
        controller.mouse_down(120, 90);
        controller.mouse_up(120, 90);
        assert_eq!(controller.app.screen(), Screen::Network);
        controller.mouse_down(120, 80);
        controller.mouse_up(120, 80);
        controller.mouse_down(120, 120);
        controller.mouse_up(120, 120);
        assert_eq!(controller.app.screen(), Screen::Home);
        assert_eq!(controller.app.wallets().len(), 0);
        controller.mouse_down(120, 184);
        controller.mouse_up(120, 184);
        assert_eq!(controller.app.wallets().len(), 1);
        controller.mouse_down(120, 236);
        controller.mouse_up(120, 236);
        assert_eq!(controller.app.screen(), Screen::LoadWallet);
        assert!(controller.app.wallet().is_none());
    }

    fn create_wallet(controller: &mut Controller) {
        controller.app.begin_create();
        controller.app.select_network(Network::Signet);
        controller
            .app
            .confirm_derivation(&mut controller.entropy, &mut controller.store)
            .unwrap();
    }

    fn owned_psbt(wallet: &picosigner_wallet::WalletSession) -> Vec<u8> {
        let secp = Secp256k1::new();
        let master = Xpriv::new_master(Network::Signet.bitcoin(), &wallet.seed()).unwrap();
        let path = DerivationPath::from(vec![
            ChildNumber::from_hardened_idx(86).unwrap(),
            ChildNumber::from_hardened_idx(1).unwrap(),
            ChildNumber::from_hardened_idx(0).unwrap(),
            ChildNumber::from_normal_idx(0).unwrap(),
            ChildNumber::from_normal_idx(0).unwrap(),
        ]);
        let child = master.derive_priv(&secp, &path).unwrap();
        let keypair = Keypair::from_secret_key(&secp, &child.private_key);
        let xonly = XOnlyPublicKey::from_keypair(&keypair).0;
        let tx = Transaction {
            version: bitcoin::transaction::Version(2),
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint {
                    txid: Txid::from_byte_array([7; 32]),
                    vout: 0,
                },
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: bitcoin::Witness::default(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(900),
                script_pubkey: ScriptBuf::new(),
            }],
        };
        let mut psbt = Psbt::from_unsigned_tx(tx).unwrap();
        psbt.inputs[0].witness_utxo = Some(TxOut {
            value: Amount::from_sat(1_000),
            script_pubkey: ScriptBuf::new_p2tr(&secp, xonly, None),
        });
        psbt.inputs[0].tap_internal_key = Some(xonly);
        psbt.inputs[0]
            .tap_key_origins
            .insert(xonly, (Vec::new(), (master.fingerprint(&secp), path)));
        psbt.serialize()
    }
}
