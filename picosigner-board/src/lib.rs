#![no_std]

//! Small, mockable interfaces and safety-critical board models.
//!
//! Hardware-specific code should adapt the RP2350 peripherals to these types.
//! In particular, no caller should write the seed directly to flash or use a
//! raw XIP flash operation without going through [`TransactionalSeedStore`]
//! and [`FlashCoordinator`].

#[cfg(test)]
extern crate std;

use core::marker::PhantomData;

#[cfg(test)]
use core::convert::Infallible;

use rand_core::{CryptoRng, RngCore};

#[cfg(feature = "embedded")]
pub mod display;
#[cfg(feature = "embedded")]
pub mod ov5640;
#[cfg(feature = "embedded")]
pub mod rp2350;
#[cfg(feature = "embedded")]
pub mod touch;

#[cfg(feature = "embedded")]
pub use display::{Orientation, WaveshareLcd};
#[cfg(feature = "embedded")]
pub use ov5640::{Ov5640, SensorZoom};
#[cfg(feature = "embedded")]
pub use rp2350::{Rp2350FlashCoordinator, Rp2350NorFlash, Rp2350Trng};
#[cfg(feature = "embedded")]
pub use touch::{Cst816d, TouchPoint};

pub const DISPLAY_WIDTH: usize = 240;
pub const DISPLAY_HEIGHT: usize = 320;
pub const FRAMEBUFFER_BYTES: usize = DISPLAY_WIDTH * DISPLAY_HEIGHT * 2;
pub const CAMERA_GRAY_WIDTH: usize = 120;
pub const CAMERA_GRAY_HEIGHT: usize = 160;
pub const CAMERA_GRAY_BYTES: usize = CAMERA_GRAY_WIDTH * CAMERA_GRAY_HEIGHT;
pub const FLASH_TOTAL_BYTES: usize = 16 * 1024 * 1024;
pub const STORAGE_RESERVED_BYTES: usize = 64 * 1024;
pub const STORAGE_SLOT_BYTES: usize = 4096;
pub const STORAGE_OFFSET: usize = FLASH_TOTAL_BYTES - STORAGE_RESERVED_BYTES;
pub const MAX_DESCRIPTOR_BYTES: usize = 192;
pub const MAX_WALLETS: usize = STORAGE_RESERVED_BYTES / (2 * STORAGE_SLOT_BYTES);

pub trait DisplaySurface {
    type Error;
    fn size(&self) -> (u16, u16);
    fn present(&mut self, pixels: &[u16]) -> Result<(), Self::Error>;
}

pub trait TouchSource {
    type Error;
    fn poll(&mut self) -> Result<Option<(u16, u16)>, Self::Error>;
}

pub trait CameraSource {
    type Error;
    fn capture_grayscale(&mut self, dst: &mut [u8]) -> Result<(), Self::Error>;
}

/// The application-facing entropy seam. Implementations must fail closed on
/// health/alarm status and must never substitute a deterministic value.
pub trait CryptoEntropy {
    type Error;
    fn fill(&mut self, dst: &mut [u8]) -> Result<(), Self::Error>;
}

/// A validated hardware RNG adapter. The source is deliberately supplied by
/// the board bring-up code so health tests and alarm handling cannot be
/// accidentally omitted by this crate.
pub struct ValidatedTrng<R> {
    source: R,
    _not_debug: PhantomData<fn() -> ()>,
}

impl<R> ValidatedTrng<R> {
    pub const fn new(source: R) -> Self {
        Self {
            source,
            _not_debug: PhantomData,
        }
    }
    pub fn into_inner(self) -> R {
        self.source
    }
}

impl<R: RngCore + CryptoRng> CryptoEntropy for ValidatedTrng<R> {
    type Error = core::convert::Infallible;
    fn fill(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
        self.source.fill_bytes(dst);
        Ok(())
    }
}

impl<R: RngCore + CryptoRng> RngCore for ValidatedTrng<R> {
    fn next_u32(&mut self) -> u32 {
        self.source.next_u32()
    }
    fn next_u64(&mut self) -> u64 {
        self.source.next_u64()
    }
    fn fill_bytes(&mut self, dst: &mut [u8]) {
        self.source.fill_bytes(dst)
    }
}
impl<R: RngCore + CryptoRng> CryptoRng for ValidatedTrng<R> {}

/// Deterministic entropy is intentionally available only to unit tests.
#[cfg(test)]
pub struct TestEntropy(pub u8);
#[cfg(test)]
impl CryptoEntropy for TestEntropy {
    type Error = Infallible;
    fn fill(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
        for byte in dst {
            *byte = self.0;
            self.0 = self.0.wrapping_add(1);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Network {
    Mainnet = 0,
    Testnet4 = 1,
    Signet = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoredSeed {
    pub entropy: [u8; 32],
    pub entropy_len: u8,
    pub network: Network,
    pub backup_pending: bool,
    /// Public routing metadata. The passphrase itself is never persisted.
    pub passphrase_required: bool,
    pub fingerprint: [u8; 4],
    pub descriptor_len: u16,
    pub descriptor: [u8; MAX_DESCRIPTOR_BYTES],
    pub descriptor_tag: [u8; 32],
}

impl StoredSeed {
    pub const fn new(entropy: [u8; 16], network: Network, backup_pending: bool) -> Self {
        let mut padded = [0; 32];
        let mut index = 0;
        while index < 16 {
            padded[index] = entropy[index];
            index += 1;
        }
        Self {
            entropy: padded,
            entropy_len: 16,
            network,
            backup_pending,
            passphrase_required: false,
            fingerprint: [0; 4],
            descriptor_len: 0,
            descriptor: [0; MAX_DESCRIPTOR_BYTES],
            descriptor_tag: [0; 32],
        }
    }

    pub fn from_entropy(entropy: &[u8], network: Network, backup_pending: bool) -> Option<Self> {
        if entropy.len() != 16 && entropy.len() != 32 {
            return None;
        }
        let mut stored = Self::new([0; 16], network, backup_pending);
        stored.entropy = [0; 32];
        stored.entropy[..entropy.len()].copy_from_slice(entropy);
        stored.entropy_len = entropy.len() as u8;
        Some(stored)
    }

    pub fn entropy(&self) -> Option<&[u8]> {
        let len = usize::from(self.entropy_len);
        (len == 16 || len == 32).then(|| &self.entropy[..len])
    }

    pub fn descriptor(&self) -> Option<&[u8]> {
        let len = usize::from(self.descriptor_len);
        (len > 0 && len <= MAX_DESCRIPTOR_BYTES).then(|| &self.descriptor[..len])
    }
}

pub trait SeedStore {
    type Error;
    fn load(&mut self) -> Result<Option<StoredSeed>, Self::Error>;
    fn store(&mut self, seed: StoredSeed) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WalletId(pub u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WalletMetadata {
    pub id: WalletId,
    pub fingerprint: Option<[u8; 4]>,
    pub network: Network,
    pub backup_pending: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WalletCatalog {
    entries: [Option<WalletMetadata>; MAX_WALLETS],
    len: usize,
}

impl WalletCatalog {
    pub const fn empty() -> Self {
        Self {
            entries: [None; MAX_WALLETS],
            len: 0,
        }
    }
    pub fn push(&mut self, entry: WalletMetadata) {
        if self.len < MAX_WALLETS {
            self.entries[self.len] = Some(entry);
            self.len += 1;
        }
    }
    pub const fn len(&self) -> usize {
        self.len
    }
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn iter(&self) -> impl Iterator<Item = &WalletMetadata> {
        self.entries[..self.len].iter().filter_map(Option::as_ref)
    }
    pub fn get(&self, index: usize) -> Option<&WalletMetadata> {
        self.entries.get(index).and_then(Option::as_ref)
    }
}

pub trait WalletStore {
    type Error;
    fn catalog(&mut self) -> Result<WalletCatalog, Self::Error>;
    fn load_wallet(&mut self, id: WalletId) -> Result<Option<StoredSeed>, Self::Error>;
    fn insert_wallet(&mut self, seed: StoredSeed) -> Result<WalletId, Self::Error>;
    fn update_wallet(&mut self, id: WalletId, seed: StoredSeed) -> Result<(), Self::Error>;
    fn delete_wallet(&mut self, id: WalletId) -> Result<(), Self::Error>;
    fn error_kind(_error: &Self::Error) -> WalletStoreErrorKind {
        WalletStoreErrorKind::Storage
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WalletStoreErrorKind {
    Storage,
    Duplicate,
    Full,
}

/// The coordinator is the mandatory seam for parking core 1, stopping DMA,
/// disabling relevant interrupts, executing SRAM-safe ROM flash code, and
/// restoring XIP/cache state. The closure must not capture XIP function/data.
pub trait FlashCoordinator {
    type Error;
    fn with_flash_paused<F, T>(&mut self, op: F) -> Result<T, Self::Error>
    where
        F: FnOnce() -> T;
}

/// A byte storage arena whose claim is exclusive for the lifetime of its
/// lease. The alignment is sufficient for the RGB565 camera buffer, while
/// callers that need a different representation must use one of the checked
/// views on [`ArenaLease`].
#[repr(align(4))]
struct AlignedBytes<const N: usize>([u8; N]);

/// One exclusive use of a [`ResourceArena`]. Dropping the lease returns the
/// arena to the available state. Keeping the claim in a borrowed value makes
/// it impossible to release the arena while a DMA target or CPU view still
/// borrows its storage.
pub struct ArenaLease<'a, const N: usize> {
    bytes: &'a mut [u8; N],
    claimed: &'a mut bool,
    len: usize,
}

impl<const N: usize> ArenaLease<'_, N> {
    pub const fn len(&self) -> usize {
        self.len
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    pub fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.bytes[..self.len]
    }

    /// Returns an aligned RGB565-style view when the claimed byte range is an
    /// exact sequence of `u16`s. The arena itself guarantees the base
    /// alignment; the length check prevents a partial final word.
    pub fn words(&mut self) -> Option<&mut [u16]> {
        let bytes = self.bytes_mut();
        let (prefix, words, suffix) = unsafe { bytes.align_to_mut::<u16>() };
        if prefix.is_empty() && suffix.is_empty() {
            Some(words)
        } else {
            None
        }
    }

    pub fn words_shared(&self) -> Option<&[u16]> {
        let bytes = self.bytes();
        let (prefix, words, suffix) = unsafe { bytes.align_to::<u16>() };
        if prefix.is_empty() && suffix.is_empty() {
            Some(words)
        } else {
            None
        }
    }
}

impl<const N: usize> Drop for ArenaLease<'_, N> {
    fn drop(&mut self) {
        *self.claimed = false;
    }
}

pub struct ResourceArena<const N: usize> {
    buf: AlignedBytes<N>,
    claimed: bool,
}

impl<const N: usize> ResourceArena<N> {
    pub const fn new() -> Self {
        Self {
            buf: AlignedBytes([0; N]),
            claimed: false,
        }
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    pub fn claim(&mut self) -> Option<ArenaLease<'_, N>> {
        self.claim_slice(N)
    }

    pub fn claim_slice(&mut self, len: usize) -> Option<ArenaLease<'_, N>> {
        if self.claimed || len > N {
            return None;
        }
        self.claimed = true;
        let AlignedBytes(bytes) = &mut self.buf;
        Some(ArenaLease {
            bytes,
            claimed: &mut self.claimed,
            len,
        })
    }

    pub const fn is_claimed(&self) -> bool {
        self.claimed
    }
}

pub trait NorFlash {
    type Error;
    fn read(&mut self, offset: usize, dst: &mut [u8]) -> Result<(), Self::Error>;
    fn erase_sector(&mut self, offset: usize) -> Result<(), Self::Error>;
    fn program(&mut self, offset: usize, src: &[u8]) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageError<E> {
    Flash(E),
    InvalidRecord,
    VerifyFailed,
    Duplicate,
    Full,
    InvalidWallet,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoordinatedStorageError<FlashError, CoordinatorError> {
    Storage(StorageError<FlashError>),
    Coordinator(CoordinatorError),
}

const MAGIC: u32 = 0x5049_434f; // "PICO"
const SCHEMA_V1: u16 = 1;
const SCHEMA_V2: u16 = 2;
const SCHEMA: u16 = 3;
const V1_RECORD_BYTES: usize = 38;
const V2_DESCRIPTOR_OFFSET: usize = 30;
const V2_TAG_OFFSET: usize = V2_DESCRIPTOR_OFFSET + MAX_DESCRIPTOR_BYTES;
const V2_CRC_OFFSET: usize = V2_TAG_OFFSET + 32;
const V2_COMMIT_OFFSET: usize = V2_CRC_OFFSET + 4;
const V2_RECORD_BYTES: usize = V2_COMMIT_OFFSET + 4;
const FINGERPRINT_OFFSET: usize = 13;
const ENTROPY_OFFSET: usize = 17;
const DESCRIPTOR_OFFSET: usize = ENTROPY_OFFSET + 32 + 2;
const TAG_OFFSET: usize = DESCRIPTOR_OFFSET + MAX_DESCRIPTOR_BYTES;
const CRC_OFFSET: usize = TAG_OFFSET + 32;
const COMMIT_OFFSET: usize = CRC_OFFSET + 4;
const RECORD_BYTES: usize = COMMIT_OFFSET + 4;
const FLASH_PROGRAM_PAGE_BYTES: usize = 256;
const COMMIT: [u8; 4] = [0, 0, 0, 0];

#[derive(Clone, Copy)]
struct Record {
    seed: StoredSeed,
    sequence: u32,
}

pub struct TransactionalSeedStore<F> {
    flash: F,
    current: [Option<(usize, u32)>; MAX_WALLETS],
}

/// Transactional storage whose every write is executed inside the board's
/// flash-pause protocol. Reads remain ordinary XIP reads; writes and the
/// commit marker cannot bypass [`FlashCoordinator`].
pub struct CoordinatedSeedStore<F, C> {
    store: TransactionalSeedStore<F>,
    coordinator: C,
}

impl<F, C> CoordinatedSeedStore<F, C> {
    pub const fn new(store: TransactionalSeedStore<F>, coordinator: C) -> Self {
        Self { store, coordinator }
    }

    pub fn into_parts(self) -> (TransactionalSeedStore<F>, C) {
        (self.store, self.coordinator)
    }
}

impl<F, C> SeedStore for CoordinatedSeedStore<F, C>
where
    F: NorFlash,
    C: FlashCoordinator,
{
    type Error = CoordinatedStorageError<F::Error, C::Error>;

    fn load(&mut self) -> Result<Option<StoredSeed>, Self::Error> {
        self.store.load().map_err(CoordinatedStorageError::Storage)
    }

    fn store(&mut self, seed: StoredSeed) -> Result<(), Self::Error> {
        self.store
            .store_with_coordinator(&mut self.coordinator, seed)
    }
}

impl<F, C> WalletStore for CoordinatedSeedStore<F, C>
where
    F: NorFlash,
    C: FlashCoordinator,
{
    type Error = CoordinatedStorageError<F::Error, C::Error>;

    fn catalog(&mut self) -> Result<WalletCatalog, Self::Error> {
        self.store
            .catalog()
            .map_err(CoordinatedStorageError::Storage)
    }

    fn load_wallet(&mut self, id: WalletId) -> Result<Option<StoredSeed>, Self::Error> {
        self.store
            .load_wallet(id)
            .map_err(CoordinatedStorageError::Storage)
    }

    fn insert_wallet(&mut self, seed: StoredSeed) -> Result<WalletId, Self::Error> {
        self.coordinator
            .with_flash_paused(|| self.store.insert_wallet(seed))
            .map_err(CoordinatedStorageError::Coordinator)?
            .map_err(CoordinatedStorageError::Storage)
    }

    fn update_wallet(&mut self, id: WalletId, seed: StoredSeed) -> Result<(), Self::Error> {
        self.coordinator
            .with_flash_paused(|| self.store.update_wallet(id, seed))
            .map_err(CoordinatedStorageError::Coordinator)?
            .map_err(CoordinatedStorageError::Storage)
    }

    fn delete_wallet(&mut self, id: WalletId) -> Result<(), Self::Error> {
        self.coordinator
            .with_flash_paused(|| self.store.delete_wallet(id))
            .map_err(CoordinatedStorageError::Coordinator)?
            .map_err(CoordinatedStorageError::Storage)
    }
    fn error_kind(error: &Self::Error) -> WalletStoreErrorKind {
        match error {
            CoordinatedStorageError::Storage(StorageError::Duplicate) => {
                WalletStoreErrorKind::Duplicate
            }
            CoordinatedStorageError::Storage(StorageError::Full) => WalletStoreErrorKind::Full,
            _ => WalletStoreErrorKind::Storage,
        }
    }
}
impl<F> TransactionalSeedStore<F> {
    pub const fn new(flash: F) -> Self {
        Self {
            flash,
            current: [None; MAX_WALLETS],
        }
    }
    pub fn into_inner(self) -> F {
        self.flash
    }
}

impl<F: NorFlash> TransactionalSeedStore<F> {
    fn program_all(&mut self, offset: usize, bytes: &[u8]) -> Result<(), StorageError<F::Error>> {
        let mut written = 0;
        while written < bytes.len() {
            let address = offset + written;
            let page_remaining = FLASH_PROGRAM_PAGE_BYTES - address % FLASH_PROGRAM_PAGE_BYTES;
            let count = core::cmp::min(page_remaining, bytes.len() - written);
            self.flash
                .program(address, &bytes[written..written + count])
                .map_err(StorageError::Flash)?;
            written += count;
        }
        Ok(())
    }

    fn read_record(
        &mut self,
        wallet: usize,
        copy: usize,
    ) -> Result<Option<Record>, StorageError<F::Error>> {
        let mut raw = [0xffu8; RECORD_BYTES];
        let slot = wallet * 2 + copy;
        self.flash
            .read(STORAGE_OFFSET + slot * STORAGE_SLOT_BYTES, &mut raw)
            .map_err(StorageError::Flash)?;
        if u32::from_le_bytes(raw[0..4].try_into().unwrap()) != MAGIC {
            return Ok(None);
        }
        let schema = u16::from_le_bytes(raw[4..6].try_into().unwrap());
        if schema == SCHEMA_V1 {
            return (wallet == 0)
                .then(|| self.decode_v1(&raw[..V1_RECORD_BYTES]))
                .transpose()
                .map(Option::flatten);
        }
        if schema == SCHEMA_V2 {
            return (wallet == 0)
                .then(|| self.decode_v2(&raw[..V2_RECORD_BYTES]))
                .transpose()
                .map(Option::flatten);
        }
        if schema != SCHEMA || raw[COMMIT_OFFSET..RECORD_BYTES] != COMMIT {
            return Ok(None);
        }
        if crc32(&raw[..CRC_OFFSET])
            != u32::from_le_bytes(raw[CRC_OFFSET..COMMIT_OFFSET].try_into().unwrap())
        {
            return Ok(None);
        }
        let entropy_len = raw[12];
        if entropy_len != 16 && entropy_len != 32 {
            return Ok(None);
        }
        let descriptor_len = u16::from_le_bytes(raw[49..51].try_into().unwrap());
        if usize::from(descriptor_len) > MAX_DESCRIPTOR_BYTES {
            return Ok(None);
        }
        let network = match raw[10] {
            0 => Network::Mainnet,
            1 => Network::Testnet4,
            2 => Network::Signet,
            _ => return Ok(None),
        };
        let mut entropy = [0; 32];
        entropy.copy_from_slice(&raw[ENTROPY_OFFSET..ENTROPY_OFFSET + 32]);
        let mut descriptor = [0; MAX_DESCRIPTOR_BYTES];
        descriptor.copy_from_slice(&raw[DESCRIPTOR_OFFSET..TAG_OFFSET]);
        let mut descriptor_tag = [0; 32];
        descriptor_tag.copy_from_slice(&raw[TAG_OFFSET..CRC_OFFSET]);
        Ok(Some(Record {
            sequence: u32::from_le_bytes(raw[6..10].try_into().unwrap()),
            seed: StoredSeed {
                entropy,
                entropy_len,
                network,
                backup_pending: raw[11] & 1 != 0,
                passphrase_required: raw[11] & 2 != 0,
                fingerprint: raw[FINGERPRINT_OFFSET..FINGERPRINT_OFFSET + 4]
                    .try_into()
                    .unwrap(),
                descriptor_len,
                descriptor,
                descriptor_tag,
            },
        }))
    }

    fn decode_v2(&self, raw: &[u8]) -> Result<Option<Record>, StorageError<F::Error>> {
        if raw[V2_COMMIT_OFFSET..V2_RECORD_BYTES] != COMMIT
            || crc32(&raw[..V2_CRC_OFFSET])
                != u32::from_le_bytes(raw[V2_CRC_OFFSET..V2_COMMIT_OFFSET].try_into().unwrap())
        {
            return Ok(None);
        }
        let descriptor_len = u16::from_le_bytes(raw[28..30].try_into().unwrap());
        if usize::from(descriptor_len) > MAX_DESCRIPTOR_BYTES {
            return Ok(None);
        }
        let network = match raw[10] {
            0 => Network::Mainnet,
            1 => Network::Testnet4,
            2 => Network::Signet,
            _ => return Ok(None),
        };
        let mut entropy16 = [0; 16];
        entropy16.copy_from_slice(&raw[12..28]);
        let mut seed = StoredSeed::new(entropy16, network, raw[11] != 0);
        seed.descriptor_len = descriptor_len;
        seed.descriptor
            .copy_from_slice(&raw[V2_DESCRIPTOR_OFFSET..V2_TAG_OFFSET]);
        seed.descriptor_tag
            .copy_from_slice(&raw[V2_TAG_OFFSET..V2_CRC_OFFSET]);
        Ok(Some(Record {
            sequence: u32::from_le_bytes(raw[6..10].try_into().unwrap()),
            seed,
        }))
    }

    fn decode_v1(&self, raw: &[u8]) -> Result<Option<Record>, StorageError<F::Error>> {
        if raw[34..38] != COMMIT
            || raw[28..30] != [16, 0]
            || crc32(&raw[..30]) != u32::from_le_bytes(raw[30..34].try_into().unwrap())
        {
            return Ok(None);
        }
        let network = match raw[10] {
            0 => Network::Mainnet,
            1 => Network::Testnet4,
            2 => Network::Signet,
            _ => return Ok(None),
        };
        let mut entropy = [0; 16];
        entropy.copy_from_slice(&raw[12..28]);
        Ok(Some(Record {
            sequence: u32::from_le_bytes(raw[6..10].try_into().unwrap()),
            seed: StoredSeed::new(entropy, network, raw[11] != 0),
        }))
    }

    fn newest(&mut self, wallet: usize) -> Result<Option<(usize, Record)>, StorageError<F::Error>> {
        if wallet >= MAX_WALLETS {
            return Err(StorageError::InvalidWallet);
        }
        let a = self.read_record(wallet, 0)?;
        let b = self.read_record(wallet, 1)?;
        Ok(match (a, b) {
            (None, None) => None,
            (Some(x), None) => Some((0, x)),
            (None, Some(x)) => Some((1, x)),
            (Some(x), Some(y)) => {
                if newer(y.sequence, x.sequence) {
                    Some((1, y))
                } else {
                    Some((0, x))
                }
            }
        })
    }

    /// Stores a record inside the board's Flash pause protocol. The
    /// coordinator implementation is responsible for parking core 1,
    /// stopping DMA, masking relevant interrupts, running SRAM-safe ROM code,
    /// and restoring XIP/cache state before this returns.
    pub fn store_with_coordinator<C: FlashCoordinator>(
        &mut self,
        coordinator: &mut C,
        seed: StoredSeed,
    ) -> Result<(), CoordinatedStorageError<F::Error, C::Error>> {
        coordinator
            .with_flash_paused(|| self.store(seed))
            .map_err(CoordinatedStorageError::Coordinator)?
            .map_err(CoordinatedStorageError::Storage)
    }

    fn write_wallet(
        &mut self,
        wallet: usize,
        seed: StoredSeed,
    ) -> Result<(), StorageError<F::Error>> {
        if wallet >= MAX_WALLETS
            || seed.entropy().is_none()
            || usize::from(seed.descriptor_len) > MAX_DESCRIPTOR_BYTES
        {
            return Err(StorageError::InvalidRecord);
        }
        if self.current[wallet].is_none() {
            self.current[wallet] = self
                .newest(wallet)?
                .map(|(copy, record)| (copy, record.sequence));
        }
        let (copy, sequence) = self.current[wallet].map_or((0, 0), |(copy, sequence)| {
            (1 - copy, sequence.wrapping_add(1))
        });
        let mut raw = [0xffu8; RECORD_BYTES];
        raw[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        raw[4..6].copy_from_slice(&SCHEMA.to_le_bytes());
        raw[6..10].copy_from_slice(&sequence.to_le_bytes());
        raw[10] = seed.network as u8;
        raw[11] = u8::from(seed.backup_pending) | (u8::from(seed.passphrase_required) << 1);
        raw[12] = seed.entropy_len;
        raw[FINGERPRINT_OFFSET..FINGERPRINT_OFFSET + 4].copy_from_slice(&seed.fingerprint);
        raw[ENTROPY_OFFSET..ENTROPY_OFFSET + 32].copy_from_slice(&seed.entropy);
        raw[49..51].copy_from_slice(&seed.descriptor_len.to_le_bytes());
        raw[DESCRIPTOR_OFFSET..TAG_OFFSET].copy_from_slice(&seed.descriptor);
        raw[TAG_OFFSET..CRC_OFFSET].copy_from_slice(&seed.descriptor_tag);
        let checksum = crc32(&raw[..CRC_OFFSET]);
        raw[CRC_OFFSET..COMMIT_OFFSET].copy_from_slice(&checksum.to_le_bytes());
        let offset = STORAGE_OFFSET + (wallet * 2 + copy) * STORAGE_SLOT_BYTES;
        self.flash
            .erase_sector(offset)
            .map_err(StorageError::Flash)?;
        self.program_all(offset, &raw[..COMMIT_OFFSET])?;
        let mut verify = [0; COMMIT_OFFSET];
        self.flash
            .read(offset, &mut verify)
            .map_err(StorageError::Flash)?;
        if verify != raw[..COMMIT_OFFSET] {
            return Err(StorageError::VerifyFailed);
        }
        self.flash
            .program(offset + COMMIT_OFFSET, &COMMIT)
            .map_err(StorageError::Flash)?;
        self.current[wallet] = Some((copy, sequence));
        Ok(())
    }
}

impl<F: NorFlash> SeedStore for TransactionalSeedStore<F> {
    type Error = StorageError<F::Error>;
    fn load(&mut self) -> Result<Option<StoredSeed>, Self::Error> {
        let found = self.newest(0)?;
        self.current[0] = found.map(|(slot, record)| (slot, record.sequence));
        Ok(found.map(|(_, record)| record.seed))
    }
    fn store(&mut self, seed: StoredSeed) -> Result<(), Self::Error> {
        self.write_wallet(0, seed)
    }
}

impl<F: NorFlash> WalletStore for TransactionalSeedStore<F> {
    type Error = StorageError<F::Error>;

    fn catalog(&mut self) -> Result<WalletCatalog, Self::Error> {
        let mut catalog = WalletCatalog::empty();
        for wallet in 0..MAX_WALLETS {
            if let Some((copy, record)) = self.newest(wallet)? {
                self.current[wallet] = Some((copy, record.sequence));
                catalog.push(WalletMetadata {
                    id: WalletId(wallet as u8),
                    fingerprint: (record.seed.fingerprint != [0; 4])
                        .then_some(record.seed.fingerprint),
                    network: record.seed.network,
                    backup_pending: record.seed.backup_pending,
                });
            }
        }
        Ok(catalog)
    }

    fn load_wallet(&mut self, id: WalletId) -> Result<Option<StoredSeed>, Self::Error> {
        let wallet = usize::from(id.0);
        let found = self.newest(wallet)?;
        self.current[wallet] = found.map(|(copy, record)| (copy, record.sequence));
        Ok(found.map(|(_, record)| record.seed))
    }

    fn insert_wallet(&mut self, seed: StoredSeed) -> Result<WalletId, Self::Error> {
        let mut empty = None;
        for wallet in 0..MAX_WALLETS {
            match self.newest(wallet)? {
                Some((_, record))
                    if record.seed.network == seed.network
                        && ((seed.fingerprint != [0; 4]
                            && record.seed.fingerprint == seed.fingerprint
                            && record.seed.descriptor() == seed.descriptor())
                            || (seed.fingerprint == [0; 4]
                                && record.seed.entropy() == seed.entropy())) =>
                {
                    return Err(StorageError::Duplicate)
                }
                Some(_) => {}
                None if empty.is_none() => empty = Some(wallet),
                None => {}
            }
        }
        let wallet = empty.ok_or(StorageError::Full)?;
        self.write_wallet(wallet, seed)?;
        Ok(WalletId(wallet as u8))
    }

    fn update_wallet(&mut self, id: WalletId, seed: StoredSeed) -> Result<(), Self::Error> {
        let wallet = usize::from(id.0);
        if self.newest(wallet)?.is_none() {
            return Err(StorageError::InvalidWallet);
        }
        self.write_wallet(wallet, seed)
    }

    fn delete_wallet(&mut self, id: WalletId) -> Result<(), Self::Error> {
        let wallet = usize::from(id.0);
        if self.newest(wallet)?.is_none() {
            return Err(StorageError::InvalidWallet);
        }
        for copy in 0..2 {
            let offset = STORAGE_OFFSET + (wallet * 2 + copy) * STORAGE_SLOT_BYTES;
            self.flash
                .erase_sector(offset)
                .map_err(StorageError::Flash)?;
        }
        self.current[wallet] = None;
        Ok(())
    }
    fn error_kind(error: &Self::Error) -> WalletStoreErrorKind {
        match error {
            StorageError::Duplicate => WalletStoreErrorKind::Duplicate,
            StorageError::Full => WalletStoreErrorKind::Full,
            _ => WalletStoreErrorKind::Storage,
        }
    }
}

fn newer(a: u32, b: u32) -> bool {
    a != b && a.wrapping_sub(b) < 0x8000_0000
}
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    const N: usize = STORAGE_RESERVED_BYTES;
    struct FakeNor {
        bytes: [u8; N],
        fail_after: Option<usize>,
    }
    impl FakeNor {
        fn new() -> Self {
            Self {
                bytes: [0xff; N],
                fail_after: None,
            }
        }
        fn cut_after(&mut self, n: usize) {
            self.fail_after = Some(n);
        }
        fn step(&mut self) -> bool {
            match &mut self.fail_after {
                Some(n) if *n == 0 => true,
                Some(n) => {
                    *n -= 1;
                    false
                }
                None => false,
            }
        }
    }
    impl NorFlash for FakeNor {
        type Error = ();
        fn read(&mut self, o: usize, d: &mut [u8]) -> Result<(), ()> {
            let o = o - STORAGE_OFFSET;
            d.copy_from_slice(&self.bytes[o..o + d.len()]);
            Ok(())
        }
        fn erase_sector(&mut self, o: usize) -> Result<(), ()> {
            if self.step() {
                return Err(());
            }
            let o = o - STORAGE_OFFSET;
            self.bytes[o..o + 4096].fill(0xff);
            Ok(())
        }
        fn program(&mut self, o: usize, s: &[u8]) -> Result<(), ()> {
            if s.len() > FLASH_PROGRAM_PAGE_BYTES
                || o % FLASH_PROGRAM_PAGE_BYTES + s.len() > FLASH_PROGRAM_PAGE_BYTES
            {
                return Err(());
            }
            let o = o - STORAGE_OFFSET;
            for (i, &v) in s.iter().enumerate() {
                if self.step() {
                    return Err(());
                }
                self.bytes[o + i] &= v;
            }
            Ok(())
        }
    }
    fn seed(n: u8) -> StoredSeed {
        StoredSeed::new([n; 16], Network::Signet, true)
    }

    fn v1_record(seed: StoredSeed, sequence: u32) -> [u8; V1_RECORD_BYTES] {
        let mut raw = [0xff; V1_RECORD_BYTES];
        raw[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        raw[4..6].copy_from_slice(&SCHEMA_V1.to_le_bytes());
        raw[6..10].copy_from_slice(&sequence.to_le_bytes());
        raw[10] = seed.network as u8;
        raw[11] = seed.backup_pending as u8;
        raw[12..28].copy_from_slice(&seed.entropy[..16]);
        raw[28..30].copy_from_slice(&16u16.to_le_bytes());
        let checksum = crc32(&raw[..30]);
        raw[30..34].copy_from_slice(&checksum.to_le_bytes());
        raw[34..38].copy_from_slice(&COMMIT);
        raw
    }

    struct TestCoordinator {
        pauses: usize,
    }
    impl FlashCoordinator for TestCoordinator {
        type Error = Infallible;

        fn with_flash_paused<F, T>(&mut self, op: F) -> Result<T, Self::Error>
        where
            F: FnOnce() -> T,
        {
            self.pauses += 1;
            Ok(op())
        }
    }
    #[test]
    fn round_trip_and_backup_update() {
        let mut s = TransactionalSeedStore::new(FakeNor::new());
        assert_eq!(s.load().unwrap(), None);
        let mut passphrase_seed = seed(1);
        passphrase_seed.passphrase_required = true;
        s.store(passphrase_seed).unwrap();
        assert_eq!(s.load().unwrap(), Some(passphrase_seed));
        s.store(StoredSeed {
            backup_pending: false,
            ..passphrase_seed
        })
        .unwrap();
        let loaded = s.load().unwrap().unwrap();
        assert!(!loaded.backup_pending);
        assert!(loaded.passphrase_required);
    }

    #[test]
    fn schema_one_record_loads_and_next_write_upgrades_to_schema_two() {
        let mut flash = FakeNor::new();
        let old_seed = seed(9);
        flash.bytes[..V1_RECORD_BYTES].copy_from_slice(&v1_record(old_seed, 7));
        let mut store = TransactionalSeedStore::new(flash);
        assert_eq!(store.load().unwrap(), Some(old_seed));
        store.store(old_seed).unwrap();
        let flash = store.into_inner();
        assert_eq!(
            u16::from_le_bytes(
                flash.bytes[STORAGE_SLOT_BYTES + 4..STORAGE_SLOT_BYTES + 6]
                    .try_into()
                    .unwrap()
            ),
            SCHEMA
        );
    }
    #[test]
    fn corrupt_new_slot_keeps_old() {
        let mut f = FakeNor::new();
        let mut s = TransactionalSeedStore::new(f);
        s.store(seed(1)).unwrap();
        f = s.into_inner();
        f.bytes[STORAGE_SLOT_BYTES + 12] = 0;
        let mut s = TransactionalSeedStore::new(f);
        assert_eq!(s.load().unwrap(), Some(seed(1)));
    }
    #[test]
    fn interrupted_commit_is_invalid() {
        let mut f = FakeNor::new();
        let mut s = TransactionalSeedStore::new(f);
        s.store(seed(1)).unwrap();
        f = s.into_inner();
        f.cut_after(1);
        let mut s = TransactionalSeedStore::new(f);
        assert!(s.store(seed(2)).is_err());
        assert_eq!(s.load().unwrap(), Some(seed(1)));
    }

    #[test]
    fn store_can_be_bound_to_flash_coordinator() {
        let mut store = TransactionalSeedStore::new(FakeNor::new());
        let mut coordinator = TestCoordinator { pauses: 0 };
        store
            .store_with_coordinator(&mut coordinator, seed(3))
            .unwrap();
        assert_eq!(coordinator.pauses, 1);
        assert_eq!(store.load().unwrap(), Some(seed(3)));
    }

    #[test]
    fn eight_wallet_catalog_duplicate_and_full_behavior() {
        let mut store = TransactionalSeedStore::new(FakeNor::new());
        for value in 0..MAX_WALLETS as u8 {
            let mut record = seed(value + 1);
            record.fingerprint = [value, 1, 2, 3];
            assert_eq!(
                WalletStore::insert_wallet(&mut store, record).unwrap(),
                WalletId(value)
            );
        }
        assert_eq!(WalletStore::catalog(&mut store).unwrap().len(), MAX_WALLETS);
        assert_eq!(
            WalletStore::insert_wallet(&mut store, seed(1)),
            Err(StorageError::Duplicate)
        );
        assert_eq!(
            WalletStore::insert_wallet(&mut store, seed(99)),
            Err(StorageError::Full)
        );
        let mut changed = WalletStore::load_wallet(&mut store, WalletId(4))
            .unwrap()
            .unwrap();
        changed.backup_pending = false;
        WalletStore::update_wallet(&mut store, WalletId(4), changed).unwrap();
        assert!(
            !WalletStore::load_wallet(&mut store, WalletId(4))
                .unwrap()
                .unwrap()
                .backup_pending
        );
        assert!(
            WalletStore::load_wallet(&mut store, WalletId(3))
                .unwrap()
                .unwrap()
                .backup_pending
        );
        WalletStore::delete_wallet(&mut store, WalletId(4)).unwrap();
        assert!(WalletStore::load_wallet(&mut store, WalletId(4))
            .unwrap()
            .is_none());
        assert_eq!(
            WalletStore::catalog(&mut store).unwrap().len(),
            MAX_WALLETS - 1
        );
        assert_eq!(
            WalletStore::insert_wallet(&mut store, seed(99)).unwrap(),
            WalletId(4)
        );
    }
    #[test]
    fn arena_lease_is_exclusive_until_drop() {
        let mut a = ResourceArena::<8>::new();
        {
            let mut lease = a.claim_slice(8).unwrap();
            assert_eq!(lease.len(), 8);
            lease.bytes_mut()[0] = 0x5a;
            assert_eq!(lease.bytes()[0], 0x5a);
        }
        assert!(!a.is_claimed());
        assert!(a.claim_slice(4).is_some());
    }

    #[test]
    fn arena_exposes_aligned_word_view() {
        let mut a = ResourceArena::<8>::new();
        let mut lease = a.claim().unwrap();
        let words = lease.words().unwrap();
        words[0] = 0x1234;
        words[1] = 0xabcd;
        assert_eq!(&lease.words_shared().unwrap()[..2], &[0x1234, 0xabcd]);
    }

    #[test]
    fn arena_rejects_oversized_claim_without_consuming_it() {
        let mut a = ResourceArena::<8>::new();
        assert!(a.claim_slice(9).is_none());
        assert!(!a.is_claimed());
        assert!(a.claim().is_some());
    }
}
