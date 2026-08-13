//! RP2350 hardware adapters for the application seams.
//!
//! These adapters deliberately live beside the transactional storage and
//! entropy contracts. The application never receives a raw XIP pointer or a
//! raw TRNG register block and therefore cannot accidentally bypass the
//! validation and fail-closed paths.

use core::convert::Infallible;

use rand_core::{CryptoRng, RngCore};
use rp235x_hal as hal;

use crate::{CryptoEntropy, FlashCoordinator, NorFlash};

/// Errors reported by the RP2350 TRNG health/alarm path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrngError {
    HealthAlarm,
    Timeout,
}

/// RP2350 hardware TRNG adapter.
pub struct Rp2350Trng {
    trng: hal::pac::TRNG,
}

impl Rp2350Trng {
    /// Takes ownership of the TRNG peripheral and enables its entropy source.
    /// The reset is released here so callers cannot read an uninitialized
    /// peripheral.
    pub fn new(trng: hal::pac::TRNG, resets: &mut hal::pac::RESETS) -> Self {
        resets.reset().modify(|_, w| w.trng().clear_bit());
        while !resets.reset_done().read().trng().bit_is_set() {
            core::hint::spin_loop();
        }
        trng.rnd_source_enable().write(|w| w.rnd_src_en().set_bit());
        Self { trng }
    }

    fn health_ok(&self) -> bool {
        let status = self.trng.rng_isr().read();
        !status.autocorr_err().bit_is_set()
            && !status.crngt_err().bit_is_set()
            && !status.vn_err().bit_is_set()
    }

    fn fill_checked(&mut self, dst: &mut [u8]) -> Result<(), TrngError> {
        for chunk in dst.chunks_mut(24) {
            let mut spins = 0u32;
            loop {
                if !self.health_ok() {
                    return Err(TrngError::HealthAlarm);
                }
                if self.trng.trng_valid().read().ehr_valid().bit_is_set() {
                    break;
                }
                spins = spins.saturating_add(1);
                if spins == 10_000_000 {
                    dst.fill(0);
                    return Err(TrngError::Timeout);
                }
                core::hint::spin_loop();
            }

            let words = [
                self.trng.ehr_data0().read().ehr_data0().bits(),
                self.trng.ehr_data1().read().ehr_data1().bits(),
                self.trng.ehr_data2().read().ehr_data2().bits(),
                self.trng.ehr_data3().read().ehr_data3().bits(),
                self.trng.ehr_data4().read().ehr_data4().bits(),
                self.trng.ehr_data5().read().ehr_data5().bits(),
            ];
            let mut raw = [0u8; 24];
            for (index, word) in words.into_iter().enumerate() {
                raw[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
            }
            chunk.copy_from_slice(&raw[..chunk.len()]);
        }
        if self.health_ok() {
            Ok(())
        } else {
            dst.fill(0);
            Err(TrngError::HealthAlarm)
        }
    }
}

impl CryptoEntropy for Rp2350Trng {
    type Error = TrngError;

    fn fill(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
        self.fill_checked(dst)
    }
}

impl RngCore for Rp2350Trng {
    fn next_u32(&mut self) -> u32 {
        let mut value = [0; 4];
        self.fill_checked(&mut value)
            .unwrap_or_else(|_| panic!("TRNG health alarm"));
        u32::from_le_bytes(value)
    }

    fn next_u64(&mut self) -> u64 {
        let mut value = [0; 8];
        self.fill_checked(&mut value)
            .unwrap_or_else(|_| panic!("TRNG health alarm"));
        u64::from_le_bytes(value)
    }

    fn fill_bytes(&mut self, dst: &mut [u8]) {
        self.fill_checked(dst)
            .unwrap_or_else(|_| panic!("TRNG health alarm"));
    }
}

impl CryptoRng for Rp2350Trng {}

/// Errors reported by the RP2350 ROM flash adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlashError {
    Bounds,
    Rom,
}

/// NOR-flash adapter for the final 64 KiB storage partition.
pub struct Rp2350NorFlash;

impl Rp2350NorFlash {
    pub const fn new() -> Self {
        Self
    }

    fn valid_range(offset: usize, len: usize) -> bool {
        offset <= crate::FLASH_TOTAL_BYTES && len <= crate::FLASH_TOTAL_BYTES.saturating_sub(offset)
    }
}

impl Default for Rp2350NorFlash {
    fn default() -> Self {
        Self::new()
    }
}

impl NorFlash for Rp2350NorFlash {
    type Error = FlashError;

    fn read(&mut self, offset: usize, dst: &mut [u8]) -> Result<(), Self::Error> {
        if !Self::valid_range(offset, dst.len()) {
            return Err(FlashError::Bounds);
        }
        let result = unsafe {
            hal::rom_data::flash_op(
                flash_flags(2),
                (0x1000_0000usize + offset) as u32,
                dst.len() as u32,
                dst.as_mut_ptr(),
            )
        };
        (result == 0).then_some(()).ok_or(FlashError::Rom)
    }

    fn erase_sector(&mut self, offset: usize) -> Result<(), Self::Error> {
        if offset % crate::STORAGE_SLOT_BYTES != 0
            || !Self::valid_range(offset, crate::STORAGE_SLOT_BYTES)
        {
            return Err(FlashError::Bounds);
        }
        let result = unsafe {
            hal::rom_data::flash_op(
                flash_flags(0),
                (0x1000_0000usize + offset) as u32,
                crate::STORAGE_SLOT_BYTES as u32,
                core::ptr::null_mut(),
            )
        };
        (result == 0).then_some(()).ok_or(FlashError::Rom)
    }

    fn program(&mut self, offset: usize, src: &[u8]) -> Result<(), Self::Error> {
        if !Self::valid_range(offset, src.len()) || src.len() > 256 {
            return Err(FlashError::Bounds);
        }
        let page = offset & !255;
        let within = offset - page;
        if within + src.len() > 256 {
            return Err(FlashError::Bounds);
        }

        let mut page_data = [0xffu8; 256];
        self.read(page, &mut page_data)?;
        page_data[within..within + src.len()].copy_from_slice(src);
        let result = unsafe {
            hal::rom_data::flash_op(
                flash_flags(1),
                (0x1000_0000usize + page) as u32,
                256,
                page_data.as_mut_ptr(),
            )
        };
        (result == 0).then_some(()).ok_or(FlashError::Rom)
    }
}

/// Secure, storage-addressed RP2350 checked-flash operation flags. The ROM
/// checked API owns the XIP exit/restore sequence, unlike the low-level ROM
/// calls, so this adapter can safely be called from flash-resident Rust code.
const fn flash_flags(operation: u32) -> u32 {
    (1 << 8) | (operation << 16)
}

/// Flash coordinator for the single-core application path.
///
/// The application does not start core 1 or any camera/DMA activity before
/// using storage. The critical section therefore gives the ROM operation
/// exclusive core-0 ownership and masks interrupts. A multicore camera path
/// must replace this with a coordinator that parks core 1 before enabling it.
pub struct Rp2350FlashCoordinator {
    _not_send: core::marker::PhantomData<*mut ()>,
}

impl Rp2350FlashCoordinator {
    pub const fn new() -> Self {
        Self {
            _not_send: core::marker::PhantomData,
        }
    }
}

impl Default for Rp2350FlashCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl FlashCoordinator for Rp2350FlashCoordinator {
    type Error = Infallible;

    fn with_flash_paused<F, T>(&mut self, op: F) -> Result<T, Self::Error>
    where
        F: FnOnce() -> T,
    {
        Ok(critical_section::with(|_| op()))
    }
}
