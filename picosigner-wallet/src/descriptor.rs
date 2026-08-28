//! Output descriptors: building them, parsing them back, and binding a cache.
//!
//! The device exports a multipath descriptor so a coordinator can watch the
//! wallet, and caches it in flash to avoid repeating hardened BIP32 derivation
//! on every load. Two properties matter:
//!
//! - **A cached descriptor is bound to the wallet it came from.**
//!   [`descriptor_cache_tag`] hashes the entropy, network, normalized
//!   passphrase, and descriptor together; a cache that does not match its tag is
//!   discarded and regenerated rather than trusted.
//! - **Parsing is checked by re-serialization.** [`parse_cached_descriptor`]
//!   rebuilds the canonical string from what it parsed and rejects the input
//!   unless the two match, so a malformed descriptor cannot round-trip into
//!   something subtly different.

use alloc::{
    string::{String, ToString},
    vec,
    vec::Vec,
};
use bip39::{Language, Mnemonic};
use bitcoin::{
    bip32::{ChildNumber, DerivationPath, Xpriv, Xpub},
    hashes::{sha256, Hash, HashEngine},
};
use core::str::FromStr;
use zeroize::Zeroize;

use crate::{AccountType, Network, WalletError, WalletExport, WalletKind, WalletSecret};

pub(crate) fn derive_export(
    secret: &WalletSecret,
    network: Network,
    kind: WalletKind,
    account_type: AccountType,
    passphrase: &str,
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
        let mut seed = mnemonic.to_seed(passphrase);
        let master =
            Xpriv::new_master(network.bitcoin(), &seed).map_err(|_| WalletError::Descriptor)?;
        seed.zeroize();
        master
    };
    let path = match kind {
        WalletKind::Bip39 => DerivationPath::from(vec![
            ChildNumber::from_hardened_idx(account_type.purpose()).unwrap(),
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
        WalletKind::Bip39 => {
            let key = alloc::format!(
                "[{}/{}h/{}h/0h]{}/<0;1>/*",
                fp,
                account_type.purpose(),
                network.coin_type(),
                xpub
            );
            match account_type {
                AccountType::NestedSegwit => alloc::format!("sh(wpkh({key}))"),
                AccountType::NativeSegwit => alloc::format!("wpkh({key})"),
                AccountType::Taproot => alloc::format!("tr({key})"),
            }
        }
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

pub(crate) fn descriptor_cache_tag(
    entropy: &[u8],
    network: Network,
    passphrase: &[u8],
    descriptor: &[u8],
) -> [u8; 32] {
    let mut engine = sha256::Hash::engine();
    engine.input(b"picosigner descriptor cache v2\0");
    engine.input(entropy);
    engine.input(&[match network {
        Network::Mainnet => 0,
        Network::Testnet4 => 1,
        Network::Signet => 2,
    }]);
    engine.input(&(passphrase.len() as u32).to_le_bytes());
    engine.input(passphrase);
    engine.input(descriptor);
    sha256::Hash::from_engine(engine).to_byte_array()
}

pub(crate) fn parse_cached_descriptor(
    descriptor: &str,
    network: Network,
) -> Result<(WalletExport, Xpub, WalletKind, AccountType), WalletError> {
    let (raw, supplied_checksum) = descriptor.rsplit_once('#').ok_or(WalletError::Descriptor)?;
    if descriptor_checksum(raw)? != supplied_checksum {
        return Err(WalletError::Descriptor);
    }
    let (body, wrapper_account) = if let Some(body) = raw
        .strip_prefix("sh(wpkh([")
        .and_then(|value| value.strip_suffix("/<0;1>/*))"))
    {
        (body, AccountType::NestedSegwit)
    } else if let Some(body) = raw
        .strip_prefix("wpkh([")
        .and_then(|value| value.strip_suffix("/<0;1>/*)"))
    {
        (body, AccountType::NativeSegwit)
    } else if let Some(body) = raw
        .strip_prefix("tr([")
        .and_then(|value| value.strip_suffix("/<0;1>/*)"))
    {
        (body, AccountType::Taproot)
    } else {
        return Err(WalletError::Descriptor);
    };
    let (origin, xpub_text) = body.split_once(']').ok_or(WalletError::Descriptor)?;
    let mut origin_parts = origin.split('/');
    let fingerprint_text = origin_parts.next().ok_or(WalletError::Descriptor)?;
    let origin_path = origin_parts.collect::<Vec<_>>();
    let expected_coin = alloc::format!("{}h", network.coin_type());
    let expected_purpose = alloc::format!("{}h", wrapper_account.purpose());
    let kind = if origin_path == [expected_purpose.as_str(), expected_coin.as_str(), "0h"] {
        WalletKind::Bip39
    } else if wrapper_account == AccountType::Taproot && origin_path == ["0", "0", "0", "0"] {
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
        WalletKind::Bip39 => {
            let key = alloc::format!(
                "[{}/{}h/{}h/0h]{}/<0;1>/*",
                fingerprint_text,
                wrapper_account.purpose(),
                network.coin_type(),
                xpub
            );
            match wrapper_account {
                AccountType::NestedSegwit => alloc::format!("sh(wpkh({key}))"),
                AccountType::NativeSegwit => alloc::format!("wpkh({key})"),
                AccountType::Taproot => alloc::format!("tr({key})"),
            }
        }
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
        wrapper_account,
    ))
}

pub fn account_type_from_descriptor(descriptor: &str) -> Result<AccountType, WalletError> {
    let raw = descriptor
        .rsplit_once('#')
        .map_or(descriptor, |(raw, _)| raw);
    if raw.starts_with("sh(wpkh(") {
        Ok(AccountType::NestedSegwit)
    } else if raw.starts_with("wpkh(") {
        Ok(AccountType::NativeSegwit)
    } else if raw.starts_with("tr(") {
        Ok(AccountType::Taproot)
    } else {
        Err(WalletError::Descriptor)
    }
}

fn hex_nibble(byte: u8) -> Result<u8, WalletError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(WalletError::Descriptor),
    }
}

fn descriptor_checksum(raw: &str) -> Result<String, WalletError> {
    miniscript::descriptor::checksum::desc_checksum(raw).map_err(|_| WalletError::Descriptor)
}
