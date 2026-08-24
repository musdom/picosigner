use alloc::vec::Vec;
use bip39::Language;
use schnorr_fun::{
    frost::{Fingerprint, SecretShare, SharedKey},
    fun::{hash::HashAdd, prelude::*},
};
use sha2::{Digest, Sha256};

pub const FROST_BACKUP_WORDS: usize = 25;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrostBackupError {
    InvalidIndex,
    InvalidWord,
    WordsChecksum,
    DuplicateIndex,
    SharesDoNotMatch,
    InvalidSecret,
}

#[derive(Clone, PartialEq)]
pub struct FrostShareBackup {
    share: SecretShare,
    poly_checksum: u8,
}

impl FrostShareBackup {
    pub fn from_word_indices(
        index: u32,
        words: [u16; FROST_BACKUP_WORDS],
    ) -> Result<Self, FrostBackupError> {
        if index == 0
            || words
                .iter()
                .any(|word| usize::from(*word) >= Language::English.word_list().len())
        {
            return Err(if index == 0 {
                FrostBackupError::InvalidIndex
            } else {
                FrostBackupError::InvalidWord
            });
        }
        let mut scalar_bytes = [0u8; 32];
        let mut poly_checksum = 0u8;
        let mut words_checksum = 0u16;
        for (bit_index, bit) in words
            .into_iter()
            .flat_map(|word| {
                (0..11)
                    .rev()
                    .map(move |offset| ((word >> offset) & 1) as u8)
            })
            .enumerate()
        {
            if bit == 0 {
                continue;
            }
            match bit_index {
                0..=255 => scalar_bytes[bit_index / 8] |= 1 << (7 - bit_index % 8),
                256..=263 => poly_checksum |= 1 << (7 - (bit_index - 256)),
                264..=274 => words_checksum |= 1 << (10 - (bit_index - 264)),
                _ => unreachable!(),
            }
        }
        let scalar = Scalar::from_bytes_mod_order(scalar_bytes);
        let hash = Sha256::new()
            .add(index.to_be_bytes())
            .add(scalar)
            .add(u16::from(poly_checksum).to_be_bytes())
            .finalize();
        let expected = u16::from_be_bytes([hash[0], hash[1]]) >> 5;
        if expected != words_checksum {
            return Err(FrostBackupError::WordsChecksum);
        }
        let index = Scalar::<Secret, Zero>::from(index)
            .non_zero()
            .ok_or(FrostBackupError::InvalidIndex)?
            .public();
        Ok(Self {
            share: SecretShare {
                index,
                share: scalar,
            },
            poly_checksum,
        })
    }

    pub fn index(&self) -> u32 {
        self.share.index.try_into().expect("u32 share index")
    }

    fn valid_for(&self, shared_key: &SharedKey<Normal, Zero>) -> bool {
        let hash = Sha256::new()
            .add(self.share.index.to_bytes())
            .add(self.share.share)
            .add(shared_key.point_polynomial())
            .finalize();
        hash[0] == self.poly_checksum
    }
}

pub fn recover_frost_secret(shares: &[FrostShareBackup]) -> Result<[u8; 32], FrostBackupError> {
    if shares.is_empty() {
        return Err(FrostBackupError::SharesDoNotMatch);
    }
    for (position, share) in shares.iter().enumerate() {
        if shares[..position]
            .iter()
            .any(|other| other.share.index == share.share.index)
        {
            return Err(FrostBackupError::DuplicateIndex);
        }
    }
    let images = shares
        .iter()
        .map(|share| share.share.share_image())
        .collect::<Vec<_>>();
    let shared_key = SharedKey::from_share_images(images);
    if shared_key
        .check_fingerprint::<Sha256>(Fingerprint::FROST_V0)
        .is_none()
        || shares.iter().any(|share| !share.valid_for(&shared_key))
    {
        return Err(FrostBackupError::SharesDoNotMatch);
    }
    let secret_shares = shares.iter().map(|share| share.share).collect::<Vec<_>>();
    let secret = SecretShare::recover_secret(&secret_shares);
    let bytes = secret.to_bytes();
    if Scalar::<Secret, NonZero>::from_bytes(bytes).is_none() {
        return Err(FrostBackupError::InvalidSecret);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(index: u32, phrase: &str) -> FrostShareBackup {
        let list = Language::English.word_list();
        let mut words = [0u16; FROST_BACKUP_WORDS];
        for (slot, word) in words.iter_mut().zip(phrase.split_whitespace()) {
            *slot = list.binary_search(&word).unwrap() as u16;
        }
        FrostShareBackup::from_word_indices(index, words).unwrap()
    }

    #[test]
    fn recovers_official_two_of_three_vector() {
        let one = parse(1, "mutual jeans snap sting bless journey moral bread room limit dose gravity sort deliver outdoor ripple donkey blouse play cart century maximum make local mobile");
        let two = parse(2, "cash trash foil prefer butter idea brave bitter item wink drift smile tomato lunch option hero three engine bless manage horse jar advice sheriff business");
        assert_eq!(recover_frost_secret(&[one, two]).unwrap(), [1u8; 32]);
    }

    #[test]
    fn rejects_bad_words_checksum() {
        let list = Language::English.word_list();
        let mut words = [0u16; FROST_BACKUP_WORDS];
        for (slot, word) in words.iter_mut().zip("mutual jeans snap sting bless journey moral bread room limit dose gravity sort deliver outdoor ripple donkey blouse play cart century maximum make local abandon".split_whitespace()) {
            *slot = list.binary_search(&word).unwrap() as u16;
        }
        assert!(matches!(
            FrostShareBackup::from_word_indices(1, words),
            Err(FrostBackupError::WordsChecksum)
        ));
    }
}
