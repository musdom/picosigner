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
    ui::{action_at, HoldAction, REVIEW_SIGN_BUTTON},
};
use picosigner_board::{
    CryptoEntropy, Network as BoardNetwork, SeedStore, StoredSeed, MAX_DESCRIPTOR_BYTES,
};
use picosigner_ui::{Gesture, GestureConfig, Navigator, Screen, TouchTracker, UiEvent};
use picosigner_wallet::Network;

const STATE_MAGIC_V1: &[u8; 7] = b"PSEMU\0\x01";
const STATE_MAGIC: &[u8; 7] = b"PSEMU\0\x02";
const V1_STATE_LEN: usize = STATE_MAGIC_V1.len() + 2 + 16;
const STATE_LEN: usize = STATE_MAGIC.len() + 2 + 16 + 2 + MAX_DESCRIPTOR_BYTES + 32;

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
    pub scale: u32,
}

impl Default for Config {
    fn default() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        Self {
            input: root.join("input.psbt"),
            output: root.join("signed.psbt"),
            state: root.join("wallet-state.bin"),
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
    "Usage: picosigner-emulator [--input PATH] [--output PATH] [--state PATH] [--scale N]"
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

impl SeedStore for FileSeedStore {
    type Error = EmulatorError;

    fn load(&mut self) -> Result<Option<StoredSeed>, Self::Error> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let is_v1 = bytes.len() == V1_STATE_LEN
            && bytes.get(..STATE_MAGIC_V1.len()) == Some(STATE_MAGIC_V1);
        let is_v2 = bytes.len() == STATE_LEN && bytes.get(..STATE_MAGIC.len()) == Some(STATE_MAGIC);
        if !is_v1 && !is_v2 {
            return Err(EmulatorError::State(
                "invalid or unsupported emulator wallet state",
            ));
        }
        let network = match bytes[STATE_MAGIC.len()] {
            0 => BoardNetwork::Mainnet,
            1 => BoardNetwork::Testnet4,
            2 => BoardNetwork::Signet,
            _ => {
                return Err(EmulatorError::State(
                    "invalid network in emulator wallet state",
                ))
            }
        };
        let backup_pending = match bytes[STATE_MAGIC.len() + 1] {
            0 => false,
            1 => true,
            _ => {
                return Err(EmulatorError::State(
                    "invalid backup flag in emulator wallet state",
                ))
            }
        };
        let entropy_offset = STATE_MAGIC.len() + 2;
        let mut entropy = [0u8; 16];
        entropy.copy_from_slice(&bytes[entropy_offset..entropy_offset + 16]);
        let mut seed = StoredSeed::new(entropy, network, backup_pending);
        if is_v2 {
            let len_offset = entropy_offset + 16;
            seed.descriptor_len =
                u16::from_le_bytes(bytes[len_offset..len_offset + 2].try_into().unwrap());
            if usize::from(seed.descriptor_len) > MAX_DESCRIPTOR_BYTES {
                return Err(EmulatorError::State(
                    "invalid descriptor length in emulator wallet state",
                ));
            }
            let descriptor_offset = len_offset + 2;
            seed.descriptor.copy_from_slice(
                &bytes[descriptor_offset..descriptor_offset + MAX_DESCRIPTOR_BYTES],
            );
            seed.descriptor_tag
                .copy_from_slice(&bytes[descriptor_offset + MAX_DESCRIPTOR_BYTES..STATE_LEN]);
        }
        Ok(Some(seed))
    }

    fn store(&mut self, seed: StoredSeed) -> Result<(), Self::Error> {
        if usize::from(seed.descriptor_len) > MAX_DESCRIPTOR_BYTES {
            return Err(EmulatorError::State(
                "descriptor is too large for emulator wallet state",
            ));
        }
        let mut bytes = [0u8; STATE_LEN];
        bytes[..STATE_MAGIC.len()].copy_from_slice(STATE_MAGIC);
        bytes[STATE_MAGIC.len()] = seed.network as u8;
        bytes[STATE_MAGIC.len() + 1] = u8::from(seed.backup_pending);
        let entropy_offset = STATE_MAGIC.len() + 2;
        bytes[entropy_offset..entropy_offset + 16].copy_from_slice(&seed.entropy);
        let len_offset = entropy_offset + 16;
        bytes[len_offset..len_offset + 2].copy_from_slice(&seed.descriptor_len.to_le_bytes());
        let descriptor_offset = len_offset + 2;
        bytes[descriptor_offset..descriptor_offset + MAX_DESCRIPTOR_BYTES]
            .copy_from_slice(&seed.descriptor);
        bytes[descriptor_offset + MAX_DESCRIPTOR_BYTES..].copy_from_slice(&seed.descriptor_tag);
        atomic_write(&self.path, &bytes).map_err(Into::into)
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
            action_at(self.app.screen(), x, y)
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
            }
            return;
        }
        match (action, gesture.gesture) {
            (Some(HoldAction::Backup), Gesture::Hold) => {
                if let Err(error) = self.app.acknowledge_backup_and_store(&mut self.store) {
                    self.fail(format!("could not save backup state: {error:?}"));
                }
            }
            (Some(HoldAction::Regenerate), Gesture::Hold) => {
                if let Err(error) = self.app.generate_seed(&mut self.entropy, &mut self.store) {
                    self.fail(format!("could not create wallet: {error:?}"));
                }
            }
            (Some(HoldAction::Scan), Gesture::Tap) => self.load_psbt(),
            (Some(HoldAction::ShowBackup), Gesture::Tap) => {
                if let Err(error) = self.app.open_backup() {
                    self.fail(format!("backup unavailable: {error:?}"));
                }
            }
            (Some(HoldAction::Descriptor), Gesture::Tap) => {
                if let Err(error) = self.app.open_descriptor() {
                    self.fail(format!("descriptor unavailable: {error:?}"));
                }
            }
            (Some(HoldAction::Back), Gesture::Tap) => self.app.navigate(Screen::Home),
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
        pixelcolor::{Rgb565, RgbColor},
    };
    use embedded_graphics_simulator::SimulatorDisplay;
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
        assert_eq!(loaded.entropy, [8; 16]);
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
            scale: 2,
        };
        let controller = Controller::new(config).unwrap();
        assert_eq!(controller.app.screen(), Screen::Home);
        let upgraded = fs::read(state).unwrap();
        assert_eq!(&upgraded[..STATE_MAGIC.len()], STATE_MAGIC);
        assert_eq!(upgraded.len(), STATE_LEN);
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
            scale: 2,
        };
        let mut controller = Controller::new(config.clone()).unwrap();
        assert_eq!(controller.app.screen(), Screen::Backup);
        controller.mouse_down(120, 260);
        for _ in 0..30 {
            controller.tick();
        }
        controller.mouse_up(120, 260);
        assert_eq!(controller.app.screen(), Screen::Home);
        let fingerprint = controller.app.wallet().unwrap().master_fingerprint();
        drop(controller);
        let restarted = Controller::new(config).unwrap();
        assert_eq!(restarted.app.screen(), Screen::Home);
        assert_eq!(
            restarted.app.wallet().unwrap().master_fingerprint(),
            fingerprint
        );
    }

    #[test]
    fn controller_reviews_holds_and_writes_a_signed_psbt() {
        let dir = tempdir().unwrap();
        let config = Config {
            input: dir.path().join("input.psbt"),
            output: dir.path().join("signed.psbt"),
            state: dir.path().join("state"),
            scale: 2,
        };
        let mut controller = Controller::new(config.clone()).unwrap();
        controller.mouse_down(120, 260);
        for _ in 0..30 {
            controller.tick();
        }
        controller.mouse_up(120, 260);

        let fixture = owned_psbt(controller.app.wallet().unwrap());
        fs::write(&config.input, fixture).unwrap();
        controller.mouse_down(120, 132);
        controller.mouse_up(120, 132);
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
    fn shared_home_renderer_draws_the_real_button_colors() {
        let mut app = AppState::new(Network::Signet);
        app.load_seed([0; 16], false);
        let fonts = picosigner::ui::Fonts::load();
        let mut display = SimulatorDisplay::<Rgb565>::new(Size::new(240, 320));
        picosigner::ui::render_app(&mut display, &fonts, &app, None, 0);
        assert_eq!(
            display.get_pixel(Point::new(20, 64)),
            Rgb565::new(24, 40, 0)
        );
        assert_eq!(display.get_pixel(Point::new(20, 115)), Rgb565::BLUE);
        assert_eq!(display.get_pixel(Point::new(20, 220)), Rgb565::RED);
    }

    #[test]
    fn controller_opens_and_closes_wallet_backup() {
        let dir = tempdir().unwrap();
        let config = Config {
            input: dir.path().join("input.psbt"),
            output: dir.path().join("signed.psbt"),
            state: dir.path().join("state"),
            scale: 2,
        };
        let mut controller = Controller::new(config).unwrap();
        controller.mouse_down(120, 260);
        for _ in 0..30 {
            controller.tick();
        }
        controller.mouse_up(120, 260);

        controller.mouse_down(120, 80);
        controller.mouse_up(120, 80);
        assert_eq!(controller.app.screen(), Screen::BackupWords);
        assert!(controller.app.mnemonic().is_ok());

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
            scale: 2,
        };
        let mut controller = Controller::new(config).unwrap();
        controller.mouse_down(120, 260);
        for _ in 0..30 {
            controller.tick();
        }
        controller.mouse_up(120, 260);
        controller.mouse_down(120, 132);
        controller.mouse_up(120, 132);
        assert_eq!(controller.app.screen(), Screen::Home);
        assert!(controller.status().is_some());
        controller.mouse_down(120, 310);
        controller.mouse_up(120, 310);
        assert!(controller.status().is_none());
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
