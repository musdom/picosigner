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
    pub entropy: [u8; 16],
    pub network: Network,
    pub backup_pending: bool,
}

pub trait SeedStore {
    type Error;
    fn load(&mut self) -> Result<Option<StoredSeed>, Self::Error>;
    fn store(&mut self, seed: StoredSeed) -> Result<(), Self::Error>;
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoordinatedStorageError<FlashError, CoordinatorError> {
    Storage(StorageError<FlashError>),
    Coordinator(CoordinatorError),
}

const MAGIC: u32 = 0x5049_434f; // "PICO"
const SCHEMA: u16 = 1;
const RECORD_BYTES: usize = 38;
const COMMIT: [u8; 4] = [0, 0, 0, 0];

#[derive(Clone, Copy)]
struct Record {
    seed: StoredSeed,
    sequence: u32,
}

pub struct TransactionalSeedStore<F> {
    flash: F,
    current: Option<(usize, u32)>,
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
impl<F> TransactionalSeedStore<F> {
    pub const fn new(flash: F) -> Self {
        Self {
            flash,
            current: None,
        }
    }
    pub fn into_inner(self) -> F {
        self.flash
    }
}

impl<F: NorFlash> TransactionalSeedStore<F> {
    fn read_record(&mut self, slot: usize) -> Result<Option<Record>, StorageError<F::Error>> {
        let mut raw = [0xffu8; RECORD_BYTES];
        self.flash
            .read(STORAGE_OFFSET + slot * STORAGE_SLOT_BYTES, &mut raw)
            .map_err(StorageError::Flash)?;
        if raw[34..38] != COMMIT
            || u32::from_le_bytes(raw[0..4].try_into().unwrap()) != MAGIC
            || u16::from_le_bytes(raw[4..6].try_into().unwrap()) != SCHEMA
            || raw[28..30] != [16, 0]
        {
            return Ok(None);
        }
        if crc32(&raw[..30]) != u32::from_le_bytes(raw[30..34].try_into().unwrap()) {
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
            seed: StoredSeed {
                entropy,
                network,
                backup_pending: raw[11] != 0,
            },
        }))
    }

    fn newest(&mut self) -> Result<Option<(usize, Record)>, StorageError<F::Error>> {
        let a = self.read_record(0)?;
        let b = self.read_record(1)?;
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
}

impl<F: NorFlash> SeedStore for TransactionalSeedStore<F> {
    type Error = StorageError<F::Error>;
    fn load(&mut self) -> Result<Option<StoredSeed>, Self::Error> {
        let found = self.newest()?;
        self.current = found.map(|(slot, record)| (slot, record.sequence));
        Ok(found.map(|(_, record)| record.seed))
    }
    fn store(&mut self, seed: StoredSeed) -> Result<(), Self::Error> {
        if self.current.is_none() {
            self.current = self.newest()?.map(|(slot, record)| (slot, record.sequence));
        }
        let (slot, sequence) = self
            .current
            .map_or((0, 0), |(s, seq)| (1 - s, seq.wrapping_add(1)));
        let mut raw = [0xffu8; RECORD_BYTES];
        raw[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        raw[4..6].copy_from_slice(&SCHEMA.to_le_bytes());
        raw[6..10].copy_from_slice(&sequence.to_le_bytes());
        raw[10] = seed.network as u8;
        raw[11] = seed.backup_pending as u8;
        raw[12..28].copy_from_slice(&seed.entropy);
        raw[28..30].copy_from_slice(&(16u16).to_le_bytes());
        let checksum = crc32(&raw[..30]);
        raw[30..34].copy_from_slice(&checksum.to_le_bytes());
        let offset = STORAGE_OFFSET + slot * STORAGE_SLOT_BYTES;
        self.flash
            .erase_sector(offset)
            .map_err(StorageError::Flash)?;
        self.flash
            .program(offset, &raw[..34])
            .map_err(StorageError::Flash)?;
        let mut verify = [0; 34];
        self.flash
            .read(offset, &mut verify)
            .map_err(StorageError::Flash)?;
        if verify != raw[..34] {
            return Err(StorageError::VerifyFailed);
        }
        self.flash
            .program(offset + 34, &COMMIT)
            .map_err(StorageError::Flash)?;
        self.current = Some((slot, sequence));
        Ok(())
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
    const N: usize = 2 * STORAGE_SLOT_BYTES;
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
        StoredSeed {
            entropy: [n; 16],
            network: Network::Signet,
            backup_pending: true,
        }
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
        s.store(seed(1)).unwrap();
        assert_eq!(s.load().unwrap(), Some(seed(1)));
        s.store(StoredSeed {
            backup_pending: false,
            ..seed(1)
        })
        .unwrap();
        assert!(!s.load().unwrap().unwrap().backup_pending);
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
