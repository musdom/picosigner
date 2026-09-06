//! PSBT signing.
//!
//! This is the security-critical core of the wallet and is kept in its own
//! module so it can be read on its own. What a reviewer should check:
//!
//! - **The signature covers what the user approved.** [`WalletSession::sign`]
//!   re-hashes the exact bytes it is given and compares that digest against the
//!   one produced during review, before parsing anything. A PSBT mutated after
//!   approval is rejected rather than signed.
//! - **Only owned inputs are signed.** An input is signed only when its
//!   key-origin fingerprint, its derivation path shape, and the key re-derived
//!   from the seed all agree. Foreign inputs are left untouched.
//! - **Unsafe sighash types are refused**, both as a review warning and again
//!   here through [`SigningPolicy`].
//! - **Every Taproot key-path signature is verified before it is returned**, so
//!   a fault-injected signature is caught rather than exported.
//!
//! Known gaps are recorded in `docs/security.md`; the script-path branch in
//! particular signs coordinator-supplied leaf hashes without parsing the
//! tapscripts or checking merkle inclusion.

#[cfg(feature = "debug")]
use crate::TaprootSigningDebug;
use alloc::vec::Vec;
#[cfg(feature = "debug")]
use bitcoin::hashes::Hash;
use bitcoin::Psbt;

#[cfg(feature = "host")]
use alloc::vec;
#[cfg(feature = "host")]
use bitcoin::bip32::{ChildNumber, DerivationPath, Xpriv};
#[cfg(feature = "host")]
use zeroize::Zeroize;

use crate::{
    digest, parse_psbt, AccountType, PsbtReview, SignedPsbt, SigningPolicy, WalletError,
    WalletKind, WalletSession,
};

impl WalletSession {
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
            ChildNumber::from_hardened_idx(self.account_type.purpose()).unwrap(),
            ChildNumber::from_hardened_idx(self.network.coin_type()).unwrap(),
            ChildNumber::from_hardened_idx(0).unwrap(),
        ]);
        let account = master
            .derive_priv(&secp, &path)
            .map_err(|_| WalletError::Descriptor)?;
        let fp = master.fingerprint(&secp);
        let ext_key = alloc::format!(
            "[{}/{}h/{}h/0h]{}/0/*",
            fp,
            self.account_type.purpose(),
            self.network.coin_type(),
            account
        );
        let int_key = alloc::format!(
            "[{}/{}h/{}h/0h]{}/1/*",
            fp,
            self.account_type.purpose(),
            self.network.coin_type(),
            account
        );
        let wrap = |key: &str| match self.account_type {
            AccountType::NestedSegwit => alloc::format!("sh(wpkh({key}))"),
            AccountType::NativeSegwit => alloc::format!("wpkh({key})"),
            AccountType::Taproot => alloc::format!("tr({key})"),
        };
        let ext = wrap(&ext_key);
        let int = wrap(&int_key);
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
        if !psbt
            .inputs
            .iter()
            .any(|input| input.tap_key_sig.is_some() || !input.partial_sigs.is_empty())
        {
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
            if self.kind == WalletKind::Bip39 && self.account_type != AccountType::Taproot {
                let mut updated = input;
                for (public_key, (origin_fp, path)) in updated.bip32_derivation.clone() {
                    if origin_fp != fingerprint || !self.bip39_path_matches(path.as_ref()) {
                        continue;
                    }
                    let child = master
                        .derive_priv(&secp, &path)
                        .map_err(|_| WalletError::PolicyRejected)?;
                    if bitcoin::secp256k1::PublicKey::from_secret_key(&secp, &child.private_key)
                        != public_key
                    {
                        continue;
                    }
                    let previous = &prevouts[index];
                    let script_code = match self.account_type {
                        AccountType::NestedSegwit => updated
                            .redeem_script
                            .as_ref()
                            .ok_or(WalletError::MissingPrevout)?,
                        AccountType::NativeSegwit => &previous.script_pubkey,
                        AccountType::Taproot => unreachable!(),
                    };
                    let sighash_type = updated
                        .sighash_type
                        .and_then(|ty| ty.ecdsa_hash_ty().ok())
                        .unwrap_or(bitcoin::sighash::EcdsaSighashType::All);
                    if sighash_type != bitcoin::sighash::EcdsaSighashType::All {
                        return Err(WalletError::UnsafeSighash);
                    }
                    let sighash = cache
                        .p2wpkh_signature_hash(index, script_code, previous.value, sighash_type)
                        .map_err(|_| WalletError::MissingPrevout)?;
                    let message = bitcoin::secp256k1::Message::from(sighash);
                    let signature = secp.sign_ecdsa(&message, &child.private_key);
                    updated.partial_sigs.insert(
                        bitcoin::PublicKey::new(public_key),
                        bitcoin::ecdsa::Signature {
                            signature,
                            sighash_type,
                        },
                    );
                    break;
                }
                psbt.inputs[index] = updated;
                continue;
            }
            let mut owned = None;
            for (&xonly, (leaves, key_source)) in &input.tap_key_origins {
                let (origin_fp, path) = key_source;
                let components = path.as_ref();
                let path_matches = match self.kind {
                    WalletKind::Bip39 => self.bip39_path_matches(components),
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
}
