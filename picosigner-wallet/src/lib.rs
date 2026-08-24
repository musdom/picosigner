#![no_std]
extern crate alloc;

use alloc::{
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
use core::str::FromStr;
use zeroize::Zeroize;

mod frost_backup;
pub use frost_backup::{
    recover_frost_secret, FrostBackupError, FrostShareBackup, FROST_BACKUP_WORDS,
};

pub const MAX_PSBT_BYTES: usize = 32 * 1024;
pub const MAX_INPUTS: usize = 64;
pub const MAX_OUTPUTS: usize = 64;

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
        let entropy = entropy.into();
        Self::from_secret(entropy, network, WalletKind::Bip39)
    }

    pub fn new_frost<E: Into<WalletSecret>>(secret: E, network: Network) -> Self {
        Self::from_secret(secret.into(), network, WalletKind::Frost)
    }

    fn from_secret(entropy: WalletSecret, network: Network, kind: WalletKind) -> Self {
        let (cached_export, account_xpub) = match derive_export(&entropy, network, kind) {
            Ok((export, account_xpub)) => (Ok(export), Some(account_xpub)),
            Err(error) => (Err(error), None),
        };
        Self {
            entropy,
            network,
            kind,
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
        let mut entropy = entropy.into();
        if descriptor_cache_tag(entropy.as_bytes(), network, descriptor.as_bytes()) != tag {
            entropy.bytes.zeroize();
            return Err(WalletError::Descriptor);
        }
        let (export, account_xpub, kind) = match parse_cached_descriptor(descriptor, network) {
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
            .to_seed_normalized("")
    }
    pub fn export(&self) -> Result<WalletExport, WalletError> {
        self.cached_export.clone()
    }

    pub fn review(&self, bytes: &[u8]) -> Result<PsbtReview, WalletError> {
        let psbt = parse_psbt(bytes)?;
        let fee = psbt.fee().ok().map(|a| a.to_sat());
        let outgoing = psbt
            .unsigned_tx
            .output
            .iter()
            .map(|o| o.value.to_sat())
            .try_fold(0u64, u64::checked_add)
            .ok_or(WalletError::InvalidPsbt)?;
        let digest = digest(bytes);
        let secp = bitcoin::secp256k1::Secp256k1::verification_only();
        let outputs = psbt
            .unsigned_tx
            .output
            .iter()
            .zip(&psbt.outputs)
            .map(|(txout, metadata)| ReviewedOutput {
                value_sat: txout.value.to_sat(),
                address: bitcoin::Address::from_script(
                    &txout.script_pubkey,
                    self.network.bitcoin(),
                )
                .ok()
                .map(|address| address.to_string()),
                is_change: self.is_verified_change(&secp, txout, metadata),
            })
            .collect();
        let mut warnings = Vec::new();
        for input in &psbt.inputs {
            if input.witness_utxo.is_none() && input.non_witness_utxo.is_none() {
                warnings.push(ReviewWarning::MissingPrevout);
            }
            if let Some(sighash) = input.sighash_type {
                match sighash.taproot_hash_ty() {
                    Ok(bitcoin::sighash::TapSighashType::Default)
                    | Ok(bitcoin::sighash::TapSighashType::All) => {}
                    _ => warnings.push(ReviewWarning::UnsafeSighash),
                }
            }
        }
        Ok(PsbtReview {
            input_count: psbt.inputs.len(),
            output_count: psbt.outputs.len(),
            outgoing_sat: outgoing,
            fee_sat: fee,
            digest,
            outputs,
            warnings,
        })
    }

    fn is_verified_change<C: bitcoin::secp256k1::Verification>(
        &self,
        secp: &bitcoin::secp256k1::Secp256k1<C>,
        txout: &bitcoin::TxOut,
        metadata: &bitcoin::psbt::Output,
    ) -> bool {
        let (Some(account_xpub), Ok(export)) = (self.account_xpub, &self.cached_export) else {
            return false;
        };
        metadata
            .tap_key_origins
            .iter()
            .any(|(&xonly, (leaf_hashes, (fingerprint, path)))| {
                let components = path.as_ref();
                if !leaf_hashes.is_empty() || fingerprint.to_bytes() != export.fingerprint {
                    return false;
                }
                let branch_position = match self.kind {
                    WalletKind::Bip39
                        if components.len() == 5
                            && components[0] == ChildNumber::from_hardened_idx(86).unwrap()
                            && components[1]
                                == ChildNumber::from_hardened_idx(self.network.coin_type())
                                    .unwrap()
                            && components[2] == ChildNumber::from_hardened_idx(0).unwrap() =>
                    {
                        3
                    }
                    WalletKind::Frost
                        if components.len() == 6
                            && components[..4].iter().all(|child| {
                                *child == ChildNumber::from_normal_idx(0).unwrap()
                            }) =>
                    {
                        4
                    }
                    _ => return false,
                };
                if components[branch_position] != ChildNumber::from_normal_idx(1).unwrap()
                    || !components[branch_position + 1].is_normal()
                {
                    return false;
                }
                let relative = DerivationPath::from(vec![
                    components[branch_position],
                    components[branch_position + 1],
                ]);
                let Ok(child) = account_xpub.derive_pub(secp, &relative) else {
                    return false;
                };
                let derived = child.public_key.x_only_public_key().0;
                derived == xonly
                    && metadata.tap_internal_key == Some(derived)
                    && txout.script_pubkey == bitcoin::ScriptBuf::new_p2tr(secp, derived, None)
            })
    }

    pub fn sign<P: SigningPolicy>(
        &self,
        bytes: &[u8],
        reviewed: &PsbtReview,
        policy: &P,
    ) -> Result<SignedPsbt, WalletError> {
        if digest(bytes) != reviewed.digest {
            return Err(WalletError::ReviewChanged);
        }
        if !policy.permits(reviewed) {
            return Err(WalletError::PolicyRejected);
        }
        let psbt = parse_psbt(bytes)?;
        self.sign_with_rust_bitcoin(psbt, reviewed.digest)
    }

    #[cfg(feature = "host")]
    #[allow(dead_code)]
    fn sign_with_bdk(
        &self,
        mut psbt: Psbt,
        reviewed_digest: [u8; 32],
    ) -> Result<SignedPsbt, WalletError> {
        use bdk_wallet::{SignOptions, Wallet};
        let mut seed = self.seed();
        let secp = bitcoin::secp256k1::Secp256k1::new();
        let master = Xpriv::new_master(self.network.bitcoin(), &seed)
            .map_err(|_| WalletError::Descriptor)?;
        seed.zeroize();
        let path = DerivationPath::from(vec![
            ChildNumber::from_hardened_idx(86).unwrap(),
            ChildNumber::from_hardened_idx(self.network.coin_type()).unwrap(),
            ChildNumber::from_hardened_idx(0).unwrap(),
        ]);
        let account = master
            .derive_priv(&secp, &path)
            .map_err(|_| WalletError::Descriptor)?;
        let fp = master.fingerprint(&secp);
        let ext = alloc::format!(
            "tr([{}/86h/{}h/0h]{}/0/*)",
            fp,
            self.network.coin_type(),
            account
        );
        let int = alloc::format!(
            "tr([{}/86h/{}h/0h]{}/1/*)",
            fp,
            self.network.coin_type(),
            account
        );
        let wallet = Wallet::create(ext, int)
            .network(self.network.bitcoin())
            .create_wallet_no_persist()
            .map_err(|_| WalletError::Descriptor)?;
        let mut options = SignOptions::default();
        options.trust_witness_utxo = false;
        options.allow_all_sighashes = false;
        if digest(&psbt.serialize()) != reviewed_digest {
            return Err(WalletError::ReviewChanged);
        }
        wallet
            .sign(&mut psbt, options)
            .map_err(|_| WalletError::PolicyRejected)?;
        // BDK may legitimately report success while leaving a PSBT input
        // untouched when its descriptor metadata is incomplete. The embedded
        // rust-bitcoin adapter below is the authoritative bounded signer and
        // fills only keys proven owned by our BIP86 origin metadata.
        if !psbt.inputs.iter().any(|input| input.tap_key_sig.is_some()) {
            return self.sign_with_rust_bitcoin(psbt, reviewed_digest);
        }
        Ok(SignedPsbt {
            bytes: psbt.serialize(),
            reviewed_digest,
            #[cfg(feature = "debug")]
            taproot_debug: Vec::new(),
        })
    }

    /// Embedded signer. BDK 3.1's public dependency graph enables std-only
    /// serde_json/base64 defaults even when its own `std` feature is off, so
    /// the embedded adapter uses the same rust-bitcoin PSBT and Taproot
    /// primitives directly. BDK remains the host interoperability oracle.
    fn sign_with_rust_bitcoin(
        &self,
        mut psbt: Psbt,
        reviewed_digest: [u8; 32],
    ) -> Result<SignedPsbt, WalletError> {
        use bitcoin::{
            bip32::ChildNumber,
            key::TapTweak,
            secp256k1::{Keypair, Parity, PublicKey, Secp256k1, XOnlyPublicKey},
            sighash::{Prevouts, SighashCache},
            taproot::{self, TapTweakHash},
        };

        let secp = Secp256k1::new();
        let master = self.master_xpriv()?;
        let fingerprint = master.fingerprint(&secp);
        let prevouts = psbt
            .inputs
            .iter()
            .enumerate()
            .map(|(index, input)| {
                if let Some(utxo) = &input.witness_utxo {
                    Some(utxo.clone())
                } else {
                    let outpoint = psbt.unsigned_tx.input[index].previous_output;
                    input
                        .non_witness_utxo
                        .as_ref()
                        .and_then(|tx| tx.output.get(outpoint.vout as usize).cloned())
                }
            })
            .collect::<Option<Vec<_>>>()
            .ok_or(WalletError::MissingPrevout)?;
        let all_prevouts = Prevouts::All(&prevouts);
        let mut cache = SighashCache::new(&psbt.unsigned_tx);
        #[cfg(feature = "debug")]
        let mut taproot_debug = Vec::new();

        for index in 0..psbt.inputs.len() {
            let input = psbt.inputs[index].clone();
            let mut owned = None;
            for (&xonly, (leaves, key_source)) in &input.tap_key_origins {
                let (origin_fp, path) = key_source;
                let components = path.as_ref();
                let path_matches = match self.kind {
                    WalletKind::Bip39 => {
                        components.len() == 5
                            && components[0] == ChildNumber::from_hardened_idx(86).unwrap()
                            && components[1]
                                == ChildNumber::from_hardened_idx(self.network.coin_type()).unwrap()
                            && components[2] == ChildNumber::from_hardened_idx(0).unwrap()
                            && components[3].is_normal()
                            && components[4].is_normal()
                    }
                    WalletKind::Frost => {
                        components.len() == 6
                            && components[..4]
                                .iter()
                                .all(|child| *child == ChildNumber::from_normal_idx(0).unwrap())
                            && components[4].is_normal()
                            && components[5].is_normal()
                    }
                };
                if *origin_fp != fingerprint || !path_matches {
                    continue;
                }
                let child = master
                    .derive_priv(&secp, path)
                    .map_err(|_| WalletError::PolicyRejected)?;
                let keypair = Keypair::from_secret_key(&secp, &child.private_key);
                let derived = XOnlyPublicKey::from_keypair(&keypair).0;
                if derived == xonly {
                    owned = Some((xonly, leaves.clone(), child.private_key));
                    break;
                }
            }
            let Some((xonly, leaves, secret_key)) = owned else {
                continue;
            };
            let keypair = Keypair::from_secret_key(&secp, &secret_key);
            let mut updated = input;

            if updated.tap_key_sig.is_none()
                && updated.tap_internal_key == Some(xonly)
                && leaves.is_empty()
            {
                let sighash_type = updated
                    .sighash_type
                    .and_then(|ty| ty.taproot_hash_ty().ok())
                    .unwrap_or(bitcoin::sighash::TapSighashType::Default);
                let sighash = cache
                    .taproot_key_spend_signature_hash(index, &all_prevouts, sighash_type)
                    .map_err(|_| WalletError::MissingPrevout)?;
                // Apply the BIP340 even-Y normalization explicitly before the
                // BIP341 tweak. On the embedded target, libsecp's in-place
                // keypair x-only tweak produced a correct public half but an
                // invalid secret half for odd-Y internal keys.
                let (_, parity) =
                    PublicKey::from_secret_key(&secp, &secret_key).x_only_public_key();
                let normalized_secret = if parity == Parity::Odd {
                    secret_key.negate()
                } else {
                    secret_key
                };
                let tweak =
                    TapTweakHash::from_key_and_tweak(xonly, updated.tap_merkle_root).to_scalar();
                let tweaked_secret = normalized_secret
                    .add_tweak(&tweak)
                    .map_err(|_| WalletError::InvalidTaprootTweak)?;
                let tweaked = Keypair::from_secret_key(&secp, &tweaked_secret);
                let output_key = XOnlyPublicKey::from_keypair(&tweaked).0;
                let expected_output_key = xonly
                    .tap_tweak(&secp, updated.tap_merkle_root)
                    .0
                    .to_x_only_public_key();
                if output_key != expected_output_key {
                    return Err(WalletError::InvalidTaprootTweak);
                }
                // Keep BIP340 signing and verification within schnorr_fun.
                // Crossing the RP2350's 32-bit point representation with
                // libsecp256k1 caused valid candidates to be rejected before
                // they could be serialized.
                use schnorr_fun::fun::{
                    marker::{NonZero, Secret},
                    Scalar,
                };
                let schnorr = schnorr_fun::new_with_deterministic_nonces::<sha2::Sha256>();
                let signing_scalar =
                    Scalar::<Secret, NonZero>::from_bytes(tweaked_secret.secret_bytes())
                        .ok_or(WalletError::InvalidSchnorrKey)?;
                let signing_key = schnorr.new_keypair(signing_scalar);
                if signing_key.public_key().to_xonly_bytes() != output_key.serialize() {
                    return Err(WalletError::InvalidTaprootTweak);
                }
                let fun_message = schnorr_fun::Message::raw(sighash.as_ref());
                let fun_signature = schnorr.sign(&signing_key, fun_message);
                let self_verified =
                    schnorr.verify(&signing_key.public_key(), fun_message, &fun_signature);
                #[cfg(feature = "debug")]
                taproot_debug.push(TaprootSigningDebug {
                    input_index: index,
                    sighash: sighash.to_byte_array(),
                    output_key: output_key.serialize(),
                    self_verified,
                });
                if !self_verified {
                    return Err(WalletError::SchnorrSelfCheck);
                }
                let signature =
                    bitcoin::secp256k1::schnorr::Signature::from_slice(&fun_signature.to_bytes())
                        .map_err(|_| WalletError::InvalidSchnorrEncoding)?;
                updated.tap_key_sig = Some(taproot::Signature {
                    signature,
                    sighash_type,
                });
            }

            for leaf_hash in leaves {
                if updated.tap_script_sigs.contains_key(&(xonly, leaf_hash)) {
                    continue;
                }
                let sighash_type = updated
                    .sighash_type
                    .and_then(|ty| ty.taproot_hash_ty().ok())
                    .unwrap_or(bitcoin::sighash::TapSighashType::Default);
                let sighash = cache
                    .taproot_script_spend_signature_hash(
                        index,
                        &all_prevouts,
                        leaf_hash,
                        sighash_type,
                    )
                    .map_err(|_| WalletError::MissingPrevout)?;
                let message = bitcoin::secp256k1::Message::from(sighash);
                let signature = secp.sign_schnorr_no_aux_rand(&message, &keypair);
                updated.tap_script_sigs.insert(
                    (xonly, leaf_hash),
                    taproot::Signature {
                        signature,
                        sighash_type,
                    },
                );
            }
            psbt.inputs[index] = updated;
        }

        Ok(SignedPsbt {
            bytes: psbt.serialize(),
            reviewed_digest,
            #[cfg(feature = "debug")]
            taproot_debug,
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

fn derive_export(
    secret: &WalletSecret,
    network: Network,
    kind: WalletKind,
) -> Result<(WalletExport, Xpub), WalletError> {
    let secp = bitcoin::secp256k1::Secp256k1::new();
    let master = if kind == WalletKind::Frost {
        let private_key = bitcoin::secp256k1::SecretKey::from_slice(secret.as_bytes())
            .map_err(|_| WalletError::InvalidFrostBackup)?;
        Xpriv {
            network: network.bitcoin().into(),
            depth: 0,
            parent_fingerprint: [0; 4].into(),
            child_number: ChildNumber::from_normal_idx(0).unwrap(),
            private_key,
            chain_code: [0; 32].into(),
        }
    } else {
        let mnemonic = Mnemonic::from_entropy_in(Language::English, secret.as_bytes())
            .map_err(|_| WalletError::InvalidEntropy)?;
        let mut seed = mnemonic.to_seed_normalized("");
        let master =
            Xpriv::new_master(network.bitcoin(), &seed).map_err(|_| WalletError::Descriptor)?;
        seed.zeroize();
        master
    };
    let path = match kind {
        WalletKind::Bip39 => DerivationPath::from(vec![
            ChildNumber::from_hardened_idx(86).unwrap(),
            ChildNumber::from_hardened_idx(network.coin_type()).unwrap(),
            ChildNumber::from_hardened_idx(0).unwrap(),
        ]),
        WalletKind::Frost => {
            DerivationPath::from(vec![ChildNumber::from_normal_idx(0).unwrap(); 4])
        }
    };
    let account = master
        .derive_priv(&secp, &path)
        .map_err(|_| WalletError::Descriptor)?;
    let xpub = Xpub::from_priv(&secp, &account);
    let fp = master.fingerprint(&secp);
    let raw = match kind {
        WalletKind::Bip39 => alloc::format!(
            "tr([{}/86h/{}h/0h]{}/<0;1>/*)",
            fp,
            network.coin_type(),
            xpub
        ),
        WalletKind::Frost => alloc::format!("tr([{}/0/0/0/0]{}/<0;1>/*)", fp, xpub),
    };
    let checksum = descriptor_checksum(&raw)?;
    Ok((
        WalletExport {
            descriptor: alloc::format!("{}#{}", raw, checksum),
            fingerprint: fp.to_bytes(),
        },
        xpub,
    ))
}

fn descriptor_cache_tag(entropy: &[u8], network: Network, descriptor: &[u8]) -> [u8; 32] {
    let mut engine = sha256::Hash::engine();
    engine.input(b"picosigner descriptor cache v1\0");
    engine.input(entropy);
    engine.input(&[match network {
        Network::Mainnet => 0,
        Network::Testnet4 => 1,
        Network::Signet => 2,
    }]);
    engine.input(descriptor);
    sha256::Hash::from_engine(engine).to_byte_array()
}

fn parse_cached_descriptor(
    descriptor: &str,
    network: Network,
) -> Result<(WalletExport, Xpub, WalletKind), WalletError> {
    let (raw, supplied_checksum) = descriptor.rsplit_once('#').ok_or(WalletError::Descriptor)?;
    if descriptor_checksum(raw)? != supplied_checksum {
        return Err(WalletError::Descriptor);
    }
    let body = raw
        .strip_prefix("tr([")
        .and_then(|value| value.strip_suffix("/<0;1>/*)"))
        .ok_or(WalletError::Descriptor)?;
    let (origin, xpub_text) = body.split_once(']').ok_or(WalletError::Descriptor)?;
    let mut origin_parts = origin.split('/');
    let fingerprint_text = origin_parts.next().ok_or(WalletError::Descriptor)?;
    let origin_path = origin_parts.collect::<Vec<_>>();
    let expected_coin = alloc::format!("{}h", network.coin_type());
    let kind = if origin_path == ["86h", expected_coin.as_str(), "0h"] {
        WalletKind::Bip39
    } else if origin_path == ["0", "0", "0", "0"] {
        WalletKind::Frost
    } else {
        return Err(WalletError::Descriptor);
    };
    if fingerprint_text.len() != 8 {
        return Err(WalletError::Descriptor);
    }
    let mut fingerprint = [0u8; 4];
    for (index, pair) in fingerprint_text.as_bytes().chunks_exact(2).enumerate() {
        fingerprint[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    let xpub = Xpub::from_str(xpub_text).map_err(|_| WalletError::Descriptor)?;
    if xpub.network != network.bitcoin().into() {
        return Err(WalletError::Descriptor);
    }
    let canonical_raw = match kind {
        WalletKind::Bip39 => alloc::format!(
            "tr([{}/86h/{}h/0h]{}/<0;1>/*)",
            fingerprint_text,
            network.coin_type(),
            xpub
        ),
        WalletKind::Frost => {
            alloc::format!("tr([{}/0/0/0/0]{}/<0;1>/*)", fingerprint_text, xpub)
        }
    };
    if canonical_raw != raw {
        return Err(WalletError::Descriptor);
    }
    Ok((
        WalletExport {
            descriptor: descriptor.to_string(),
            fingerprint,
        },
        xpub,
        kind,
    ))
}

fn hex_nibble(byte: u8) -> Result<u8, WalletError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(WalletError::Descriptor),
    }
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
fn descriptor_checksum(raw: &str) -> Result<String, WalletError> {
    miniscript::descriptor::checksum::desc_checksum(raw).map_err(|_| WalletError::Descriptor)
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
    #[test]
    fn bip39_vector() {
        let w = WalletSession::new([0; 16], Network::Mainnet);
        assert_eq!(w.mnemonic().unwrap(), "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about");
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
