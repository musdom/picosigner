#![no_std]
extern crate alloc;

use alloc::{
    borrow::Cow,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use bip39::{Language, Mnemonic};
use bitcoin::{
    bip32::{ChildNumber, DerivationPath, Xpriv, Xpub},
    hashes::{sha256, Hash, HashEngine},
    Network as BitcoinNetwork, Psbt,
};
use zeroize::{Zeroize, Zeroizing};

mod descriptor;
mod frost_backup;
mod review;
mod sign;

pub use descriptor::account_type_from_descriptor;
use descriptor::{derive_export, descriptor_cache_tag, parse_cached_descriptor};
pub use frost_backup::{
    recover_frost_secret, FrostBackupError, FrostShareBackup, FROST_BACKUP_WORDS,
};

pub const MAX_PSBT_BYTES: usize = 32 * 1024;
pub const MAX_INPUTS: usize = 64;
pub const MAX_OUTPUTS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntropySource {
    HardwareTrng,
    Camera,
    D6Dice,
    SeedJar,
}

/// Domain-separated transcript mixer for optional user entropy. Hardware
/// entropy is mandatory at finalization; optional transcripts can only add to
/// it, never replace it.
#[derive(Clone, Debug)]
pub struct EntropyMixer {
    state: [u8; 32],
    sources: u8,
    dice_rolls: u16,
}

impl Default for EntropyMixer {
    fn default() -> Self {
        Self::new()
    }
}

impl EntropyMixer {
    pub fn new() -> Self {
        Self {
            state: sha256::Hash::hash(b"PicoSigner entropy transcript v1\0").to_byte_array(),
            sources: 0,
            dice_rolls: 0,
        }
    }

    pub fn absorb(&mut self, source: EntropySource, transcript: &[u8]) {
        let mut engine = sha256::Hash::engine();
        engine.input(b"PicoSigner entropy source v1\0");
        engine.input(&self.state);
        engine.input(&[source as u8]);
        engine.input(&(transcript.len() as u32).to_le_bytes());
        engine.input(transcript);
        self.state = sha256::Hash::from_engine(engine).to_byte_array();
        self.sources |= 1 << source as u8;
    }

    pub fn add_d6_roll(&mut self, roll: u8) -> Result<(), WalletError> {
        if !(1..=6).contains(&roll) {
            return Err(WalletError::InvalidEntropy);
        }
        self.absorb(EntropySource::D6Dice, &[roll]);
        self.dice_rolls = self.dice_rolls.saturating_add(1);
        Ok(())
    }

    pub fn has_source(&self, source: EntropySource) -> bool {
        self.sources & (1 << source as u8) != 0
    }

    pub const fn dice_rolls(&self) -> u16 {
        self.dice_rolls
    }

    pub fn finish(
        mut self,
        hardware: &mut [u8; 32],
        entropy_len: usize,
    ) -> Result<WalletSecret, WalletError> {
        if entropy_len != 16 && entropy_len != 32 {
            hardware.zeroize();
            return Err(WalletError::InvalidEntropy);
        }
        self.absorb(EntropySource::HardwareTrng, hardware);
        let mut engine = sha256::Hash::engine();
        engine.input(b"PicoSigner BIP39 entropy v1\0");
        engine.input(&self.state);
        engine.input(&[entropy_len as u8]);
        let mut output = sha256::Hash::from_engine(engine).to_byte_array();
        hardware.zeroize();
        let secret = WalletSecret::from_slice(&output[..entropy_len]);
        output.zeroize();
        self.state.zeroize();
        secret
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Network {
    Mainnet,
    Testnet4,
    Signet,
}
impl Network {
    pub fn bitcoin(self) -> BitcoinNetwork {
        match self {
            Self::Mainnet => BitcoinNetwork::Bitcoin,
            Self::Testnet4 => BitcoinNetwork::Testnet4,
            Self::Signet => BitcoinNetwork::Signet,
        }
    }
    pub fn coin_type(self) -> u32 {
        if matches!(self, Self::Mainnet) {
            0
        } else {
            1
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WalletError {
    InvalidEntropy,
    InvalidPsbt,
    OversizedPsbt,
    TooManyInputs,
    TooManyOutputs,
    PsbtV2,
    MissingPrevout,
    UnsafeSighash,
    NetworkMismatch,
    ReviewChanged,
    PolicyRejected,
    InvalidTaprootTweak,
    InvalidSignature,
    InvalidSchnorrKey,
    SchnorrSelfCheck,
    InvalidSchnorrEncoding,
    Descriptor,
    InvalidMnemonic,
    InvalidSeedQr,
    InvalidFrostBackup,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WalletKind {
    Bip39,
    Frost,
}

/// Single-key account policy selected when a wallet is created or imported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountType {
    /// BIP49 P2SH-wrapped P2WPKH (`sh(wpkh(...))`).
    NestedSegwit,
    /// BIP84 native P2WPKH (`wpkh(...)`).
    NativeSegwit,
    /// BIP86 key-path Taproot (`tr(...)`).
    Taproot,
}

impl AccountType {
    pub const fn purpose(self) -> u32 {
        match self {
            Self::NestedSegwit => 49,
            Self::NativeSegwit => 84,
            Self::Taproot => 86,
        }
    }
}

/// Fixed-capacity BIP39 entropy that keeps secret material out of heap-backed
/// containers. PicoSigner currently accepts the two common SeedQR sizes only.
#[derive(Clone, PartialEq, Eq)]
pub struct WalletSecret {
    bytes: [u8; 32],
    len: u8,
}

impl WalletSecret {
    pub fn from_slice(bytes: &[u8]) -> Result<Self, WalletError> {
        if bytes.len() != 16 && bytes.len() != 32 {
            return Err(WalletError::InvalidEntropy);
        }
        let mut secret = Self {
            bytes: [0; 32],
            len: bytes.len() as u8,
        };
        secret.bytes[..bytes.len()].copy_from_slice(bytes);
        Ok(secret)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }

    pub const fn len(&self) -> usize {
        self.len as usize
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn padded_bytes(&self) -> [u8; 32] {
        self.bytes
    }

    pub fn from_mnemonic(words: &str) -> Result<Self, WalletError> {
        let mnemonic = Mnemonic::parse_in_normalized(Language::English, words)
            .map_err(|_| WalletError::InvalidMnemonic)?;
        if mnemonic.word_count() != 12 && mnemonic.word_count() != 24 {
            return Err(WalletError::InvalidMnemonic);
        }
        let (entropy, len) = mnemonic.to_entropy_array();
        Self::from_slice(&entropy[..len])
    }

    /// Parse SeedSigner's Standard SeedQR (48/96 ASCII digits) or
    /// CompactSeedQR (16/32 raw entropy bytes).
    pub fn from_seed_qr(payload: &[u8]) -> Result<Self, WalletError> {
        if payload.len() == 16 || payload.len() == 32 {
            return Self::from_slice(payload);
        }
        if (payload.len() != 48 && payload.len() != 96) || !payload.iter().all(u8::is_ascii_digit) {
            return Err(WalletError::InvalidSeedQr);
        }
        let mut phrase = String::new();
        for (position, digits) in payload.chunks_exact(4).enumerate() {
            let index = digits.iter().fold(0usize, |value, digit| {
                value * 10 + usize::from(digit - b'0')
            });
            let word = Language::English
                .word_list()
                .get(index)
                .ok_or(WalletError::InvalidSeedQr)?;
            if position != 0 {
                phrase.push(' ');
            }
            phrase.push_str(word);
        }
        Self::from_mnemonic(&phrase).map_err(|_| WalletError::InvalidSeedQr)
    }
}

pub fn bip39_words_by_prefix<'a>(prefix: &'a str) -> &'a [&'static str] {
    Language::English.words_by_prefix(prefix)
}

impl From<[u8; 16]> for WalletSecret {
    fn from(value: [u8; 16]) -> Self {
        let mut bytes = [0; 32];
        bytes[..16].copy_from_slice(&value);
        Self { bytes, len: 16 }
    }
}

impl From<[u8; 32]> for WalletSecret {
    fn from(bytes: [u8; 32]) -> Self {
        Self { bytes, len: 32 }
    }
}

impl core::fmt::Debug for WalletSecret {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("WalletSecret")
            .field("len", &self.len)
            .field("bytes", &"[REDACTED]")
            .finish()
    }
}

impl Drop for WalletSecret {
    fn drop(&mut self) {
        self.bytes.zeroize();
        self.len.zeroize();
    }
}

pub struct WalletSession {
    entropy: WalletSecret,
    network: Network,
    kind: WalletKind,
    account_type: AccountType,
    passphrase: Zeroizing<String>,
    // Descriptor exports are public data. Cache the expensive BIP39/BIP32
    // result so showing the QR never has to derive keys on the UI thread.
    cached_export: Result<WalletExport, WalletError>,
    account_xpub: Option<Xpub>,
}
impl Drop for WalletSession {
    fn drop(&mut self) {
        self.entropy.bytes.zeroize();
        self.entropy.len.zeroize();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletExport {
    pub descriptor: String,
    pub fingerprint: [u8; 4],
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletDescriptorCache {
    pub descriptor: String,
    pub tag: [u8; 32],
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivedAddress {
    pub address: String,
    pub path: String,
    pub branch: u32,
    pub index: u32,
    pub network: Network,
    pub descriptor_fingerprint: [u8; 4],
}
#[derive(Debug, PartialEq, Eq)]
pub struct PsbtReview {
    pub input_count: usize,
    pub output_count: usize,
    pub outgoing_sat: u64,
    pub fee_sat: Option<u64>,
    pub digest: [u8; 32],
    pub outputs: Vec<ReviewedOutput>,
    pub warnings: Vec<ReviewWarning>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewedOutput {
    pub value_sat: u64,
    pub address: Option<String>,
    pub is_change: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewWarning {
    UnknownInput,
    UnverifiedChange,
    UnsafeSighash,
    MissingPrevout,
    NetworkMismatch,
}
pub trait SigningPolicy {
    fn permits(&self, review: &PsbtReview) -> bool;
}
pub struct StrictSigningPolicy;
impl SigningPolicy for StrictSigningPolicy {
    fn permits(&self, r: &PsbtReview) -> bool {
        r.warnings.is_empty()
    }
}
pub struct SignedPsbt {
    pub bytes: Vec<u8>,
    pub reviewed_digest: [u8; 32],
    #[cfg(feature = "debug")]
    pub taproot_debug: Vec<TaprootSigningDebug>,
}
#[cfg(feature = "debug")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaprootSigningDebug {
    pub input_index: usize,
    pub sighash: [u8; 32],
    pub output_key: [u8; 32],
    pub self_verified: bool,
}

impl WalletSession {
    pub fn new<E: Into<WalletSecret>>(entropy: E, network: Network) -> Self {
        Self::new_with_options(entropy, network, AccountType::Taproot, "")
    }

    pub fn new_with_options<E: Into<WalletSecret>>(
        entropy: E,
        network: Network,
        account_type: AccountType,
        passphrase: &str,
    ) -> Self {
        Self::from_secret(
            entropy.into(),
            network,
            WalletKind::Bip39,
            account_type,
            passphrase,
        )
    }

    pub fn new_frost<E: Into<WalletSecret>>(secret: E, network: Network) -> Self {
        Self::from_secret(
            secret.into(),
            network,
            WalletKind::Frost,
            AccountType::Taproot,
            "",
        )
    }

    fn from_secret(
        entropy: WalletSecret,
        network: Network,
        kind: WalletKind,
        account_type: AccountType,
        passphrase: &str,
    ) -> Self {
        let passphrase = Zeroizing::new(normalized_passphrase(passphrase));
        let (cached_export, account_xpub) =
            match derive_export(&entropy, network, kind, account_type, &passphrase) {
                Ok((export, account_xpub)) => (Ok(export), Some(account_xpub)),
                Err(error) => (Err(error), None),
            };
        Self {
            entropy,
            network,
            kind,
            account_type,
            passphrase,
            cached_export,
            account_xpub,
        }
    }

    /// Restore public wallet metadata without repeating BIP39 and hardened
    /// BIP32 derivation. The cache is bound to the entropy and network by a
    /// domain-separated digest and its descriptor is parsed canonically.
    pub fn from_descriptor_cache<E: Into<WalletSecret>>(
        entropy: E,
        network: Network,
        descriptor: &str,
        tag: [u8; 32],
    ) -> Result<Self, WalletError> {
        Self::from_descriptor_cache_with_passphrase(entropy, network, descriptor, tag, "")
    }

    pub fn from_descriptor_cache_with_passphrase<E: Into<WalletSecret>>(
        entropy: E,
        network: Network,
        descriptor: &str,
        tag: [u8; 32],
        passphrase: &str,
    ) -> Result<Self, WalletError> {
        let mut entropy = entropy.into();
        let passphrase = Zeroizing::new(normalized_passphrase(passphrase));
        if descriptor_cache_tag(
            entropy.as_bytes(),
            network,
            passphrase.as_bytes(),
            descriptor.as_bytes(),
        ) != tag
        {
            entropy.bytes.zeroize();
            return Err(WalletError::Descriptor);
        }
        let (export, account_xpub, kind, account_type) =
            match parse_cached_descriptor(descriptor, network) {
                Ok(cache) => cache,
                Err(error) => {
                    entropy.bytes.zeroize();
                    return Err(error);
                }
            };
        Ok(Self {
            entropy,
            network,
            kind,
            account_type,
            passphrase,
            cached_export: Ok(export),
            account_xpub: Some(account_xpub),
        })
    }

    pub fn descriptor_cache(&self) -> Result<WalletDescriptorCache, WalletError> {
        let export = self.cached_export.as_ref().map_err(Clone::clone)?;
        Ok(WalletDescriptorCache {
            descriptor: export.descriptor.clone(),
            tag: descriptor_cache_tag(
                self.entropy.as_bytes(),
                self.network,
                self.passphrase.as_bytes(),
                export.descriptor.as_bytes(),
            ),
        })
    }
    pub fn network(&self) -> Network {
        self.network
    }
    pub fn kind(&self) -> WalletKind {
        self.kind
    }
    pub fn account_type(&self) -> AccountType {
        self.account_type
    }
    pub fn has_passphrase(&self) -> bool {
        !self.passphrase.is_empty()
    }
    pub fn master_fingerprint(&self) -> Option<[u8; 4]> {
        self.cached_export
            .as_ref()
            .ok()
            .map(|export| export.fingerprint)
    }
    /// Return the persisted entropy for the transactional backup flag update.
    /// The caller receives a copy; the session retains and zeroizes its own
    /// secret on drop.
    pub fn entropy(&self) -> WalletSecret {
        self.entropy.clone()
    }
    pub fn mnemonic(&self) -> Result<String, WalletError> {
        if self.kind == WalletKind::Frost {
            return Err(WalletError::InvalidEntropy);
        }
        Mnemonic::from_entropy_in(Language::English, self.entropy.as_bytes())
            .map(|m| m.to_string())
            .map_err(|_| WalletError::InvalidEntropy)
    }
    pub fn seed(&self) -> [u8; 64] {
        assert_eq!(
            self.kind,
            WalletKind::Bip39,
            "FROST roots are not BIP39 seeds"
        );
        Mnemonic::from_entropy_in(Language::English, self.entropy.as_bytes())
            .expect("validated entropy")
            .to_seed(self.passphrase.as_str())
    }
    pub fn export(&self) -> Result<WalletExport, WalletError> {
        self.cached_export.clone()
    }

    pub fn derive_address(&self, branch: u32, index: u32) -> Result<DerivedAddress, WalletError> {
        if branch > 1 || index >= (1 << 31) {
            return Err(WalletError::Descriptor);
        }
        let account_xpub = self.account_xpub.ok_or(WalletError::Descriptor)?;
        let export = self.cached_export.as_ref().map_err(Clone::clone)?;
        let secp = bitcoin::secp256k1::Secp256k1::verification_only();
        let relative = DerivationPath::from(vec![
            ChildNumber::from_normal_idx(branch).map_err(|_| WalletError::Descriptor)?,
            ChildNumber::from_normal_idx(index).map_err(|_| WalletError::Descriptor)?,
        ]);
        let child = account_xpub
            .derive_pub(&secp, &relative)
            .map_err(|_| WalletError::Descriptor)?;
        let address = match self.account_type {
            AccountType::NestedSegwit => bitcoin::Address::p2shwpkh(
                &bitcoin::key::CompressedPublicKey(child.public_key),
                self.network.bitcoin(),
            ),
            AccountType::NativeSegwit => bitcoin::Address::p2wpkh(
                &bitcoin::key::CompressedPublicKey(child.public_key),
                self.network.bitcoin(),
            ),
            AccountType::Taproot => {
                let xonly = child.public_key.x_only_public_key().0;
                bitcoin::Address::p2tr(&secp, xonly, None, self.network.bitcoin())
            }
        };
        let base = if self.kind == WalletKind::Frost {
            "m/0/0/0/0".to_string()
        } else {
            alloc::format!(
                "m/{}'/{}'/0'",
                self.account_type.purpose(),
                self.network.coin_type()
            )
        };
        Ok(DerivedAddress {
            address: address.to_string(),
            path: alloc::format!("{base}/{branch}/{index}"),
            branch,
            index,
            network: self.network,
            descriptor_fingerprint: export.fingerprint,
        })
    }

    fn master_xpriv(&self) -> Result<Xpriv, WalletError> {
        if self.kind == WalletKind::Frost {
            let private_key = bitcoin::secp256k1::SecretKey::from_slice(self.entropy.as_bytes())
                .map_err(|_| WalletError::InvalidFrostBackup)?;
            return Ok(Xpriv {
                network: self.network.bitcoin().into(),
                depth: 0,
                parent_fingerprint: [0; 4].into(),
                child_number: ChildNumber::from_normal_idx(0).unwrap(),
                private_key,
                chain_code: [0; 32].into(),
            });
        }
        let mut seed = self.seed();
        let master =
            Xpriv::new_master(self.network.bitcoin(), &seed).map_err(|_| WalletError::Descriptor);
        seed.zeroize();
        master
    }
}

fn normalized_passphrase(passphrase: &str) -> String {
    let mut normalized = Cow::Borrowed(passphrase);
    Mnemonic::normalize_utf8_cow(&mut normalized);
    normalized.into_owned()
}

fn parse_psbt(bytes: &[u8]) -> Result<Psbt, WalletError> {
    if bytes.len() > MAX_PSBT_BYTES {
        return Err(WalletError::OversizedPsbt);
    }
    let p = Psbt::deserialize(bytes).map_err(|_| WalletError::InvalidPsbt)?;
    if p.version != 0 {
        return Err(WalletError::PsbtV2);
    }
    if p.inputs.len() > MAX_INPUTS {
        return Err(WalletError::TooManyInputs);
    }
    if p.outputs.len() > MAX_OUTPUTS {
        return Err(WalletError::TooManyOutputs);
    }
    Ok(p)
}
fn digest(bytes: &[u8]) -> [u8; 32] {
    sha256::Hash::hash(bytes).to_byte_array()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::{
        bip32::{ChildNumber, DerivationPath},
        hashes::Hash,
        key::TapTweak,
        secp256k1::{Keypair, Secp256k1, XOnlyPublicKey},
        sighash::{Prevouts, SighashCache},
        Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid,
    };

    fn taproot_fixture() -> Vec<u8> {
        let session = WalletSession::new([0; 16], Network::Signet);
        let mut seed = session.seed();
        let secp = Secp256k1::new();
        let master = Xpriv::new_master(Network::Signet.bitcoin(), &seed).unwrap();
        seed.zeroize();
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
            input: vec![
                TxIn {
                    previous_output: OutPoint {
                        txid: Txid::from_byte_array([7; 32]),
                        vout: 0,
                    },
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    witness: bitcoin::Witness::default(),
                },
                TxIn {
                    previous_output: OutPoint {
                        txid: Txid::from_byte_array([8; 32]),
                        vout: 0,
                    },
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    witness: bitcoin::Witness::default(),
                },
            ],
            output: vec![TxOut {
                value: Amount::from_sat(900),
                script_pubkey: ScriptBuf::new_p2tr(&secp, xonly, None),
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
        psbt.inputs[0].unknown.insert(
            bitcoin::psbt::raw::Key {
                type_value: 0x55,
                key: vec![0x42],
            },
            vec![1, 2, 3],
        );
        psbt.inputs[1].witness_utxo = Some(TxOut {
            value: Amount::from_sat(2_000),
            script_pubkey: ScriptBuf::new(),
        });
        psbt.serialize()
    }

    fn frost_taproot_fixture() -> Vec<u8> {
        let session = WalletSession::new_frost([1u8; 32], Network::Signet);
        let secp = Secp256k1::new();
        let master = session.master_xpriv().unwrap();
        let path = DerivationPath::from(vec![ChildNumber::from_normal_idx(0).unwrap(); 6]);
        let child = master.derive_priv(&secp, &path).unwrap();
        let keypair = Keypair::from_secret_key(&secp, &child.private_key);
        let xonly = XOnlyPublicKey::from_keypair(&keypair).0;
        let tx = Transaction {
            version: bitcoin::transaction::Version(2),
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint {
                    txid: Txid::from_byte_array([42; 32]),
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

    fn multi_input_taproot_fixture() -> Vec<u8> {
        let session = WalletSession::new([0; 16], Network::Signet);
        let mut seed = session.seed();
        let secp = Secp256k1::new();
        let master = Xpriv::new_master(Network::Signet.bitcoin(), &seed).unwrap();
        seed.zeroize();
        let paths = [
            DerivationPath::from(vec![
                ChildNumber::from_hardened_idx(86).unwrap(),
                ChildNumber::from_hardened_idx(1).unwrap(),
                ChildNumber::from_hardened_idx(0).unwrap(),
                ChildNumber::from_normal_idx(0).unwrap(),
                ChildNumber::from_normal_idx(1).unwrap(),
            ]),
            DerivationPath::from(vec![
                ChildNumber::from_hardened_idx(86).unwrap(),
                ChildNumber::from_hardened_idx(1).unwrap(),
                ChildNumber::from_hardened_idx(0).unwrap(),
                ChildNumber::from_normal_idx(1).unwrap(),
                ChildNumber::from_normal_idx(1).unwrap(),
            ]),
        ];
        let keys = paths
            .iter()
            .map(|path| {
                let child = master.derive_priv(&secp, path).unwrap();
                XOnlyPublicKey::from_keypair(&Keypair::from_secret_key(&secp, &child.private_key)).0
            })
            .collect::<Vec<_>>();
        let tx = Transaction {
            version: bitcoin::transaction::Version(2),
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: (0..2)
                .map(|index| TxIn {
                    previous_output: OutPoint {
                        txid: Txid::from_byte_array([20 + index as u8; 32]),
                        vout: 0,
                    },
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    witness: bitcoin::Witness::default(),
                })
                .collect(),
            output: vec![TxOut {
                value: Amount::from_sat(2_900),
                script_pubkey: ScriptBuf::new(),
            }],
        };
        let mut psbt = Psbt::from_unsigned_tx(tx).unwrap();
        for index in 0..2 {
            psbt.inputs[index].witness_utxo = Some(TxOut {
                value: Amount::from_sat(1_500),
                script_pubkey: ScriptBuf::new_p2tr(&secp, keys[index], None),
            });
            psbt.inputs[index].tap_internal_key = Some(keys[index]);
            psbt.inputs[index].tap_key_origins.insert(
                keys[index],
                (
                    Vec::new(),
                    (master.fingerprint(&secp), paths[index].clone()),
                ),
            );
        }
        psbt.serialize()
    }

    fn segwit_fixture(account_type: AccountType) -> Vec<u8> {
        let session = WalletSession::new_with_options([0u8; 16], Network::Signet, account_type, "");
        let secp = Secp256k1::new();
        let master = session.master_xpriv().unwrap();
        let path = DerivationPath::from(vec![
            ChildNumber::from_hardened_idx(account_type.purpose()).unwrap(),
            ChildNumber::from_hardened_idx(1).unwrap(),
            ChildNumber::from_hardened_idx(0).unwrap(),
            ChildNumber::from_normal_idx(0).unwrap(),
            ChildNumber::from_normal_idx(7).unwrap(),
        ]);
        let child = master.derive_priv(&secp, &path).unwrap();
        let public_key = bitcoin::secp256k1::PublicKey::from_secret_key(&secp, &child.private_key);
        let key = bitcoin::PublicKey::new(public_key);
        let witness_script = ScriptBuf::new_p2wpkh(&key.wpubkey_hash().unwrap());
        let previous_script = match account_type {
            AccountType::NestedSegwit => ScriptBuf::new_p2sh(&witness_script.script_hash()),
            AccountType::NativeSegwit => witness_script.clone(),
            AccountType::Taproot => unreachable!(),
        };
        let change_path = DerivationPath::from(vec![
            ChildNumber::from_hardened_idx(account_type.purpose()).unwrap(),
            ChildNumber::from_hardened_idx(1).unwrap(),
            ChildNumber::from_hardened_idx(0).unwrap(),
            ChildNumber::from_normal_idx(1).unwrap(),
            ChildNumber::from_normal_idx(3).unwrap(),
        ]);
        let change_child = master.derive_priv(&secp, &change_path).unwrap();
        let change_public =
            bitcoin::secp256k1::PublicKey::from_secret_key(&secp, &change_child.private_key);
        let change_key = bitcoin::PublicKey::new(change_public);
        let change_witness = ScriptBuf::new_p2wpkh(&change_key.wpubkey_hash().unwrap());
        let change_script = match account_type {
            AccountType::NestedSegwit => ScriptBuf::new_p2sh(&change_witness.script_hash()),
            AccountType::NativeSegwit => change_witness,
            AccountType::Taproot => unreachable!(),
        };
        let tx = Transaction {
            version: bitcoin::transaction::Version(2),
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint {
                    txid: Txid::from_byte_array([17; 32]),
                    vout: 0,
                },
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: bitcoin::Witness::default(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(900),
                script_pubkey: change_script,
            }],
        };
        let mut psbt = Psbt::from_unsigned_tx(tx).unwrap();
        psbt.inputs[0].witness_utxo = Some(TxOut {
            value: Amount::from_sat(1_000),
            script_pubkey: previous_script,
        });
        if account_type == AccountType::NestedSegwit {
            psbt.inputs[0].redeem_script = Some(witness_script);
        }
        psbt.inputs[0]
            .bip32_derivation
            .insert(public_key, (master.fingerprint(&secp), path));
        psbt.outputs[0]
            .bip32_derivation
            .insert(change_public, (master.fingerprint(&secp), change_path));
        psbt.serialize()
    }
    #[test]
    fn bip39_vector() {
        let w = WalletSession::new([0; 16], Network::Mainnet);
        assert_eq!(w.mnemonic().unwrap(), "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about");
    }

    #[test]
    fn bip49_bip84_and_bip86_exports_are_canonical_and_passphrase_sensitive() {
        for (account, prefix, purpose) in [
            (AccountType::NestedSegwit, "sh(wpkh([", "49h"),
            (AccountType::NativeSegwit, "wpkh([", "84h"),
            (AccountType::Taproot, "tr([", "86h"),
        ] {
            let wallet =
                WalletSession::new_with_options([0u8; 16], Network::Mainnet, account, "TREZOR");
            let export = wallet.export().unwrap();
            assert!(export.descriptor.starts_with(prefix));
            assert!(export.descriptor.contains(purpose));
            assert!(wallet.has_passphrase());
            assert_ne!(
                export.fingerprint,
                WalletSession::new_with_options([0u8; 16], Network::Mainnet, account, "")
                    .export()
                    .unwrap()
                    .fingerprint
            );
            let cache = wallet.descriptor_cache().unwrap();
            assert!(WalletSession::from_descriptor_cache_with_passphrase(
                [0u8; 16],
                Network::Mainnet,
                &cache.descriptor,
                cache.tag,
                "wrong"
            )
            .is_err());
            assert_eq!(
                WalletSession::from_descriptor_cache_with_passphrase(
                    [0u8; 16],
                    Network::Mainnet,
                    &cache.descriptor,
                    cache.tag,
                    "TREZOR"
                )
                .unwrap()
                .account_type(),
                account
            );
        }
    }

    #[test]
    fn receive_and_change_addresses_report_policy_path_and_network() {
        for account in [
            AccountType::NestedSegwit,
            AccountType::NativeSegwit,
            AccountType::Taproot,
        ] {
            let wallet = WalletSession::new_with_options([4u8; 16], Network::Testnet4, account, "");
            let receive = wallet.derive_address(0, 5).unwrap();
            let change = wallet.derive_address(1, 5).unwrap();
            assert_ne!(receive.address, change.address);
            assert_eq!(receive.network, Network::Testnet4);
            assert_eq!(
                receive.path,
                alloc::format!("m/{}'/1'/0'/0/5", account.purpose())
            );
            assert_eq!(
                change.path,
                alloc::format!("m/{}'/1'/0'/1/5", account.purpose())
            );
            assert_eq!(
                receive.descriptor_fingerprint,
                wallet.master_fingerprint().unwrap()
            );
        }
    }

    #[test]
    fn bip49_and_bip84_inputs_are_signed() {
        for account in [AccountType::NestedSegwit, AccountType::NativeSegwit] {
            let wallet = WalletSession::new_with_options([0u8; 16], Network::Signet, account, "");
            let bytes = segwit_fixture(account);
            let review = wallet.review(&bytes).unwrap();
            assert!(review.warnings.is_empty());
            assert!(review.outputs[0].is_change);
            let signed = wallet.sign(&bytes, &review, &StrictSigningPolicy).unwrap();
            let psbt = Psbt::deserialize(&signed.bytes).unwrap();
            assert_eq!(psbt.inputs[0].partial_sigs.len(), 1);
        }
    }

    #[test]
    fn optional_entropy_is_domain_separated_and_never_replaces_trng() {
        let mut hardware_a = [7u8; 32];
        let base = EntropyMixer::new().finish(&mut hardware_a, 16).unwrap();
        let mut camera = EntropyMixer::new();
        camera.absorb(EntropySource::Camera, b"same transcript");
        let mut hardware_b = [7u8; 32];
        let camera_secret = camera.finish(&mut hardware_b, 16).unwrap();
        let mut dice = EntropyMixer::new();
        for roll in [1, 2, 3, 4, 5, 6] {
            dice.add_d6_roll(roll).unwrap();
        }
        let mut hardware_c = [7u8; 32];
        let dice_secret = dice.finish(&mut hardware_c, 16).unwrap();
        assert_ne!(base, camera_secret);
        assert_ne!(camera_secret, dice_secret);
        assert_eq!(hardware_a, [0u8; 32]);
        assert_eq!(hardware_b, [0u8; 32]);
        assert_eq!(hardware_c, [0u8; 32]);
    }

    #[test]
    fn standard_and_compact_seedqr_vectors_roundtrip() {
        let words = "forum undo fragile fade shy sign arrest garment culture tube off merit";
        let standard = b"073318950739065415961602009907670428187212261116";
        let compact = [
            0x5b, 0xbd, 0x9d, 0x71, 0xa8, 0xec, 0x79, 0x90, 0x83, 0x1a, 0xff, 0x35, 0x9d, 0x42,
            0x65, 0x45,
        ];
        for payload in [&standard[..], &compact[..]] {
            let secret = WalletSecret::from_seed_qr(payload).unwrap();
            assert_eq!(
                WalletSession::new(secret, Network::Signet)
                    .mnemonic()
                    .unwrap(),
                words
            );
        }
        assert_eq!(
            WalletSecret::from_seed_qr(b"999999999999999999999999999999999999999999999999"),
            Err(WalletError::InvalidSeedQr)
        );
        assert_eq!(
            WalletSecret::from_seed_qr(b"short"),
            Err(WalletError::InvalidSeedQr)
        );
    }

    #[test]
    fn twenty_four_word_entropy_is_supported() {
        let secret = WalletSecret::from_slice(&[7; 32]).unwrap();
        let wallet = WalletSession::new(secret.clone(), Network::Mainnet);
        assert_eq!(wallet.mnemonic().unwrap().split_whitespace().count(), 24);
        assert_eq!(wallet.entropy(), secret);
    }
    #[test]
    fn frost_root_uses_zero_chain_code_and_frostsnap_path() {
        let wallet = WalletSession::new_frost([1u8; 32], Network::Mainnet);
        assert_eq!(wallet.kind(), WalletKind::Frost);
        let export = wallet.export().unwrap();
        let secp = Secp256k1::new();
        let root = wallet.master_xpriv().unwrap();
        assert_eq!(root.chain_code.to_bytes(), [0u8; 32]);
        let path = DerivationPath::from(vec![ChildNumber::from_normal_idx(0).unwrap(); 4]);
        let account = root.derive_priv(&secp, &path).unwrap();
        let expected_xpub = Xpub::from_priv(&secp, &account).to_string();
        assert!(export
            .descriptor
            .contains(&alloc::format!("/0/0/0/0]{expected_xpub}/<0;1>/*)")));

        let cache = wallet.descriptor_cache().unwrap();
        let restored = WalletSession::from_descriptor_cache(
            [1u8; 32],
            Network::Mainnet,
            &cache.descriptor,
            cache.tag,
        )
        .unwrap();
        assert_eq!(restored.kind(), WalletKind::Frost);
        assert_eq!(restored.export().unwrap(), export);
    }
    #[test]
    fn frost_wallet_signs_its_six_component_derivation_path() {
        let wallet = WalletSession::new_frost([1u8; 32], Network::Signet);
        let bytes = frost_taproot_fixture();
        let review = wallet.review(&bytes).unwrap();
        assert!(review.warnings.is_empty());
        let signed = wallet.sign(&bytes, &review, &StrictSigningPolicy).unwrap();
        let psbt = Psbt::deserialize(&signed.bytes).unwrap();
        assert!(psbt.inputs[0].tap_key_sig.is_some());
    }
    #[test]
    fn bip86_export_is_checksummed() {
        let w = WalletSession::new([0; 16], Network::Mainnet);
        let d = w.export().unwrap().descriptor;
        assert_eq!(d.matches('#').count(), 1);
        assert_eq!(d.len(), d.rfind('#').unwrap() + 9);
    }

    #[test]
    fn descriptor_cache_restores_export_and_change_xpub() {
        let wallet = WalletSession::new([3; 16], Network::Signet);
        let cache = wallet.descriptor_cache().unwrap();
        let restored = WalletSession::from_descriptor_cache(
            [3; 16],
            Network::Signet,
            &cache.descriptor,
            cache.tag,
        )
        .unwrap();
        assert_eq!(restored.export().unwrap(), wallet.export().unwrap());
        assert_eq!(restored.account_xpub, wallet.account_xpub);
    }

    #[test]
    fn descriptor_cache_rejects_wrong_seed_or_modified_descriptor() {
        let wallet = WalletSession::new([3; 16], Network::Signet);
        let cache = wallet.descriptor_cache().unwrap();
        assert!(WalletSession::from_descriptor_cache(
            [4; 16],
            Network::Signet,
            &cache.descriptor,
            cache.tag,
        )
        .is_err());
        let mut modified = cache.descriptor.clone();
        let replacement = if modified.as_bytes()[4] == b'0' {
            "1"
        } else {
            "0"
        };
        modified.replace_range(4..5, replacement);
        assert!(WalletSession::from_descriptor_cache(
            [3; 16],
            Network::Signet,
            &modified,
            cache.tag,
        )
        .is_err());
    }

    #[cfg(feature = "host")]
    #[test]
    fn bip86_export_is_bdk_parseable_on_all_supported_network_kinds() {
        use bdk_wallet::bitcoin::{secp256k1::Secp256k1, NetworkKind};
        use bdk_wallet::descriptor::{calc_checksum, IntoWalletDescriptor};

        let secp = Secp256k1::new();
        for network in [Network::Mainnet, Network::Testnet4, Network::Signet] {
            let descriptor = WalletSession::new([0; 16], network)
                .export()
                .unwrap()
                .descriptor;
            let kind = if network == Network::Mainnet {
                NetworkKind::Main
            } else {
                NetworkKind::Test
            };
            let (raw, actual) = descriptor.split_once('#').unwrap();
            assert_eq!(
                actual,
                calc_checksum(raw).unwrap(),
                "checksum mismatch for {network:?}: {raw}"
            );
            descriptor
                .as_str()
                .into_wallet_descriptor(&secp, kind)
                .unwrap_or_else(|error| panic!("{network:?} descriptor rejected: {error:?}"));
        }
    }
    #[test]
    fn digest_changes() {
        assert_ne!(digest(b"a"), digest(b"b"));
    }
    #[test]
    fn rejects_large() {
        let w = WalletSession::new([0; 16], Network::Mainnet);
        assert_eq!(
            w.review(&vec![0; MAX_PSBT_BYTES + 1]),
            Err(WalletError::OversizedPsbt)
        );
    }

    #[test]
    fn signs_owned_taproot_input_and_preserves_review_digest() {
        let bytes = taproot_fixture();
        let wallet = WalletSession::new([0; 16], Network::Signet);
        let review = wallet.review(&bytes).unwrap();
        assert!(review.warnings.is_empty());
        assert_eq!(review.outputs.len(), 1);
        assert!(review.outputs[0]
            .address
            .as_ref()
            .is_some_and(|address| address.starts_with("tb1p")));
        assert!(!review.outputs[0].is_change);
        let signed = wallet.sign(&bytes, &review, &StrictSigningPolicy).unwrap();
        let psbt = Psbt::deserialize(&signed.bytes).unwrap();
        let secp = Secp256k1::verification_only();
        assert!(psbt.inputs[0].tap_key_sig.is_some());
        assert!(psbt.inputs[1].tap_key_sig.is_none());
        let signature = psbt.inputs[0].tap_key_sig.unwrap();
        let prevouts = psbt
            .inputs
            .iter()
            .map(|input| input.witness_utxo.as_ref().unwrap().clone())
            .collect::<Vec<_>>();
        let sighash = SighashCache::new(&psbt.unsigned_tx)
            .taproot_key_spend_signature_hash(0, &Prevouts::All(&prevouts), signature.sighash_type)
            .unwrap();
        let internal_key = psbt.inputs[0].tap_internal_key.unwrap();
        let output_key = internal_key
            .tap_tweak(&secp, psbt.inputs[0].tap_merkle_root)
            .0
            .to_x_only_public_key();
        secp.verify_schnorr(
            &signature.signature,
            &bitcoin::secp256k1::Message::from(sighash),
            &output_key,
        )
        .expect("taproot key-path signature must verify against the tweaked internal key");
        assert_eq!(
            psbt.inputs[0].unknown.get(&bitcoin::psbt::raw::Key {
                type_value: 0x55,
                key: vec![0x42],
            }),
            Some(&vec![1, 2, 3])
        );
        assert_eq!(signed.reviewed_digest, review.digest);
    }

    #[test]
    fn signs_multiple_external_and_change_inputs() {
        let bytes = multi_input_taproot_fixture();
        let wallet = WalletSession::new([0; 16], Network::Signet);
        let review = wallet.review(&bytes).unwrap();
        let signed = wallet.sign(&bytes, &review, &StrictSigningPolicy).unwrap();
        let psbt = Psbt::deserialize(&signed.bytes).unwrap();
        let secp = Secp256k1::verification_only();
        let prevouts = psbt
            .inputs
            .iter()
            .map(|input| input.witness_utxo.as_ref().unwrap().clone())
            .collect::<Vec<_>>();
        for (index, input) in psbt.inputs.iter().enumerate() {
            let signature = input.tap_key_sig.expect("owned input must be signed");
            let sighash = SighashCache::new(&psbt.unsigned_tx)
                .taproot_key_spend_signature_hash(
                    index,
                    &Prevouts::All(&prevouts),
                    signature.sighash_type,
                )
                .unwrap();
            let output_key = input
                .tap_internal_key
                .unwrap()
                .tap_tweak(&secp, input.tap_merkle_root)
                .0
                .to_x_only_public_key();
            secp.verify_schnorr(
                &signature.signature,
                &bitcoin::secp256k1::Message::from(sighash),
                &output_key,
            )
            .expect("every serialized key-path signature must verify");
        }
    }

    #[test]
    fn psbt_v2_is_rejected_before_review() {
        let mut psbt = Psbt::deserialize(&taproot_fixture()).unwrap();
        psbt.version = 2;
        let bytes = psbt.serialize();
        assert!(matches!(
            WalletSession::new([0; 16], Network::Signet).review(&bytes),
            Err(WalletError::PsbtV2) | Err(WalletError::InvalidPsbt)
        ));
    }
}
