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
    Descriptor,
}

pub struct WalletSession {
    entropy: [u8; 16],
    network: Network,
    // Descriptor exports are public data. Cache the expensive BIP39/BIP32
    // result so showing the QR never has to derive keys on the UI thread.
    cached_export: Result<WalletExport, WalletError>,
    account_xpub: Option<Xpub>,
}
impl Drop for WalletSession {
    fn drop(&mut self) {
        self.entropy.zeroize();
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
    pub fn new(entropy: [u8; 16], network: Network) -> Self {
        let (cached_export, account_xpub) = match derive_export(&entropy, network) {
            Ok((export, account_xpub)) => (Ok(export), Some(account_xpub)),
            Err(error) => (Err(error), None),
        };
        Self {
            entropy,
            network,
            cached_export,
            account_xpub,
        }
    }

    /// Restore public wallet metadata without repeating BIP39 and hardened
    /// BIP32 derivation. The cache is bound to the entropy and network by a
    /// domain-separated digest and its descriptor is parsed canonically.
    pub fn from_descriptor_cache(
        mut entropy: [u8; 16],
        network: Network,
        descriptor: &str,
        tag: [u8; 32],
    ) -> Result<Self, WalletError> {
        if descriptor_cache_tag(&entropy, network, descriptor.as_bytes()) != tag {
            entropy.zeroize();
            return Err(WalletError::Descriptor);
        }
        let (export, account_xpub) = match parse_cached_descriptor(descriptor, network) {
            Ok(cache) => cache,
            Err(error) => {
                entropy.zeroize();
                return Err(error);
            }
        };
        Ok(Self {
            entropy,
            network,
            cached_export: Ok(export),
            account_xpub: Some(account_xpub),
        })
    }

    pub fn descriptor_cache(&self) -> Result<WalletDescriptorCache, WalletError> {
        let export = self.cached_export.as_ref().map_err(Clone::clone)?;
        Ok(WalletDescriptorCache {
            descriptor: export.descriptor.clone(),
            tag: descriptor_cache_tag(&self.entropy, self.network, export.descriptor.as_bytes()),
        })
    }
    pub fn network(&self) -> Network {
        self.network
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
    pub fn entropy(&self) -> [u8; 16] {
        self.entropy
    }
    pub fn mnemonic(&self) -> Result<String, WalletError> {
        Mnemonic::from_entropy_in(Language::English, &self.entropy)
            .map(|m| m.to_string())
            .map_err(|_| WalletError::InvalidEntropy)
    }
    pub fn seed(&self) -> [u8; 64] {
        Mnemonic::from_entropy_in(Language::English, &self.entropy)
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
                if !leaf_hashes.is_empty()
                    || fingerprint.to_bytes() != export.fingerprint
                    || components.len() != 5
                    || components[0] != ChildNumber::from_hardened_idx(86).unwrap()
                    || components[1]
                        != ChildNumber::from_hardened_idx(self.network.coin_type()).unwrap()
                    || components[2] != ChildNumber::from_hardened_idx(0).unwrap()
                    || components[3] != ChildNumber::from_normal_idx(1).unwrap()
                    || !components[4].is_normal()
                {
                    return false;
                }
                let relative = DerivationPath::from(vec![components[3], components[4]]);
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

        let mut seed = self.seed();
        let secp = Secp256k1::new();
        let master = Xpriv::new_master(self.network.bitcoin(), &seed)
            .map_err(|_| WalletError::Descriptor)?;
        seed.zeroize();
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
                if *origin_fp != fingerprint
                    || components.len() != 5
                    || components[0] != ChildNumber::from_hardened_idx(86).unwrap()
                    || components[1]
                        != ChildNumber::from_hardened_idx(self.network.coin_type()).unwrap()
                    || components[2] != ChildNumber::from_hardened_idx(0).unwrap()
                    || !components[3].is_normal()
                    || !components[4].is_normal()
                {
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
                let message = bitcoin::secp256k1::Message::from(sighash);
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
                // Generate BIP340 signatures with an independent pure-Rust
                // implementation. The C libsecp256k1 signer returns invalid
                // signatures for some inputs on the RP2350 target, despite
                // producing the correct tweaked public key.
                use schnorr_fun::fun::{
                    marker::{NonZero, Secret},
                    Scalar,
                };
                let signing_scalar =
                    Scalar::<Secret, NonZero>::from_bytes(tweaked_secret.secret_bytes())
                        .ok_or(WalletError::InvalidSignature)?;
                let schnorr = schnorr_fun::new_with_deterministic_nonces::<sha2::Sha256>();
                let signing_key = schnorr.new_keypair(signing_scalar);
                let fun_signature =
                    schnorr.sign(&signing_key, schnorr_fun::Message::raw(sighash.as_ref()));
                let signature =
                    bitcoin::secp256k1::schnorr::Signature::from_slice(&fun_signature.to_bytes())
                        .map_err(|_| WalletError::InvalidSignature)?;
                let self_verified = secp
                    .verify_schnorr(&signature, &message, &output_key)
                    .is_ok();
                #[cfg(feature = "debug")]
                taproot_debug.push(TaprootSigningDebug {
                    input_index: index,
                    sighash: sighash.to_byte_array(),
                    output_key: output_key.serialize(),
                    self_verified,
                });
                if !self_verified {
                    return Err(WalletError::InvalidSignature);
                }
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
}

fn derive_export(
    entropy: &[u8; 16],
    network: Network,
) -> Result<(WalletExport, Xpub), WalletError> {
    let mnemonic = Mnemonic::from_entropy_in(Language::English, entropy)
        .map_err(|_| WalletError::InvalidEntropy)?;
    let mut seed = mnemonic.to_seed_normalized("");
    let master =
        Xpriv::new_master(network.bitcoin(), &seed).map_err(|_| WalletError::Descriptor)?;
    seed.zeroize();
    let secp = bitcoin::secp256k1::Secp256k1::new();
    let path = DerivationPath::from(vec![
        ChildNumber::from_hardened_idx(86).unwrap(),
        ChildNumber::from_hardened_idx(network.coin_type()).unwrap(),
        ChildNumber::from_hardened_idx(0).unwrap(),
    ]);
    let account = master
        .derive_priv(&secp, &path)
        .map_err(|_| WalletError::Descriptor)?;
    let xpub = Xpub::from_priv(&secp, &account);
    let fp = master.fingerprint(&secp);
    let raw = alloc::format!(
        "tr([{}/86h/{}h/0h]{}/<0;1>/*)",
        fp,
        network.coin_type(),
        xpub
    );
    let checksum = descriptor_checksum(&raw)?;
    Ok((
        WalletExport {
            descriptor: alloc::format!("{}#{}", raw, checksum),
            fingerprint: fp.to_bytes(),
        },
        xpub,
    ))
}

fn descriptor_cache_tag(entropy: &[u8; 16], network: Network, descriptor: &[u8]) -> [u8; 32] {
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
) -> Result<(WalletExport, Xpub), WalletError> {
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
    let expected_coin = alloc::format!("{}h", network.coin_type());
    if origin_parts.next() != Some("86h")
        || origin_parts.next() != Some(expected_coin.as_str())
        || origin_parts.next() != Some("0h")
        || origin_parts.next().is_some()
        || fingerprint_text.len() != 8
    {
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
    let canonical_raw = alloc::format!(
        "tr([{}/86h/{}h/0h]{}/<0;1>/*)",
        fingerprint_text,
        network.coin_type(),
        xpub
    );
    if canonical_raw != raw {
        return Err(WalletError::Descriptor);
    }
    Ok((
        WalletExport {
            descriptor: descriptor.to_string(),
            fingerprint,
        },
        xpub,
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
