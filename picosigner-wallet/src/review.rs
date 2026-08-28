//! Turning a PSBT into what the user is shown before signing.
//!
//! [`WalletSession::review`] is the only thing standing between a coordinator's
//! PSBT and the user's approval, so it decides what the device claims a
//! transaction does. It parses within fixed bounds, resolves each output to an
//! address, sums what leaves the wallet, and records a digest of the exact
//! bytes reviewed. [`crate::sign`] later refuses to sign anything whose digest
//! does not match.
//!
//! Change outputs are only reported as change when the derivation in the PSBT,
//! the key re-derived from the seed, and the output script all agree — an
//! output the coordinator merely *claims* is change is shown as a payment.

use alloc::{string::ToString, vec, vec::Vec};
use bitcoin::bip32::{ChildNumber, DerivationPath};

use crate::{
    digest, parse_psbt, AccountType, PsbtReview, ReviewWarning, ReviewedOutput, WalletError,
    WalletKind, WalletSession,
};

impl WalletSession {
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
                let safe = match self.account_type {
                    AccountType::Taproot => matches!(
                        sighash.taproot_hash_ty(),
                        Ok(bitcoin::sighash::TapSighashType::Default)
                            | Ok(bitcoin::sighash::TapSighashType::All)
                    ),
                    AccountType::NestedSegwit | AccountType::NativeSegwit => matches!(
                        sighash.ecdsa_hash_ty(),
                        Ok(bitcoin::sighash::EcdsaSighashType::All)
                    ),
                };
                if !safe {
                    warnings.push(ReviewWarning::UnsafeSighash);
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
        if self.account_type != AccountType::Taproot {
            return metadata
                .bip32_derivation
                .iter()
                .any(|(public_key, (fingerprint, path))| {
                    let components = path.as_ref();
                    if fingerprint.to_bytes() != export.fingerprint
                        || !self.bip39_path_matches(components)
                        || components[3] != ChildNumber::from_normal_idx(1).unwrap()
                    {
                        return false;
                    }
                    let relative = DerivationPath::from(vec![components[3], components[4]]);
                    let Ok(child) = account_xpub.derive_pub(secp, &relative) else {
                        return false;
                    };
                    if child.public_key != *public_key {
                        return false;
                    }
                    let key = bitcoin::PublicKey::new(*public_key);
                    match self.account_type {
                        AccountType::NestedSegwit => key.wpubkey_hash().ok().is_some_and(|hash| {
                            let redeem = bitcoin::ScriptBuf::new_p2wpkh(&hash);
                            txout.script_pubkey
                                == bitcoin::ScriptBuf::new_p2sh(&redeem.script_hash())
                        }),
                        AccountType::NativeSegwit => key.wpubkey_hash().ok().is_some_and(|hash| {
                            txout.script_pubkey == bitcoin::ScriptBuf::new_p2wpkh(&hash)
                        }),
                        AccountType::Taproot => false,
                    }
                });
        }
        metadata
            .tap_key_origins
            .iter()
            .any(|(&xonly, (leaf_hashes, (fingerprint, path)))| {
                let components = path.as_ref();
                if !leaf_hashes.is_empty() || fingerprint.to_bytes() != export.fingerprint {
                    return false;
                }
                let branch_position = match self.kind {
                    WalletKind::Bip39 if self.bip39_path_matches(components) => 3,
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
    /// Whether a derivation path is this wallet's BIP39 account shape.
    /// Shared with [`crate::sign`], which gates signing on the same check.
    pub(crate) fn bip39_path_matches(&self, components: &[ChildNumber]) -> bool {
        components.len() == 5
            && components[0] == ChildNumber::from_hardened_idx(self.account_type.purpose()).unwrap()
            && components[1] == ChildNumber::from_hardened_idx(self.network.coin_type()).unwrap()
            && components[2] == ChildNumber::from_hardened_idx(0).unwrap()
            && components[3].is_normal()
            && components[4].is_normal()
    }
}
