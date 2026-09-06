//! Non-secret device preferences, kept outside all wallet slots.
use crate::{
    crc32, CoordinatedSeedStore, CoordinatedStorageError, FlashCoordinator, NorFlash, StorageError,
    TransactionalSeedStore, STORAGE_OFFSET, STORAGE_SLOT_BYTES,
};

pub const SETTINGS_OFFSET: usize = STORAGE_OFFSET - STORAGE_SLOT_BYTES;
const RECORD_LEN: usize = 16;
const HEADER: [u8; 7] = *b"PSET\x01\x00\x00";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeviceSettings {
    pub usb_enabled: bool,
}

pub trait SettingsStore {
    type SettingsError;
    fn load_settings(&mut self) -> Result<DeviceSettings, Self::SettingsError>;
    fn save_settings(&mut self, settings: DeviceSettings) -> Result<(), Self::SettingsError>;
}

impl DeviceSettings {
    pub fn encode(self) -> [u8; RECORD_LEN] {
        let mut raw = [0u8; RECORD_LEN];
        raw[..7].copy_from_slice(&HEADER);
        raw[7] = u8::from(self.usb_enabled);
        let crc = crc32(&raw[..8]);
        raw[8..12].copy_from_slice(&crc.to_le_bytes());
        raw
    }

    /// Erased, unsupported, corrupt and uncommitted records all mean USB off.
    pub fn decode(raw: &[u8]) -> Self {
        if raw.len() != RECORD_LEN
            || raw[..7] != HEADER
            || raw[7] > 1
            || raw[12..] != [0; 4]
            || crc32(&raw[..8]).to_le_bytes() != raw[8..12]
        {
            return Self::default();
        }
        Self {
            usb_enabled: raw[7] == 1,
        }
    }
}

impl<F: NorFlash> SettingsStore for TransactionalSeedStore<F> {
    type SettingsError = StorageError<F::Error>;
    fn load_settings(&mut self) -> Result<DeviceSettings, Self::SettingsError> {
        let mut raw = [0; RECORD_LEN];
        self.flash
            .read(SETTINGS_OFFSET, &mut raw)
            .map_err(StorageError::Flash)?;
        Ok(DeviceSettings::decode(&raw))
    }
    fn save_settings(&mut self, settings: DeviceSettings) -> Result<(), Self::SettingsError> {
        let raw = settings.encode();
        self.flash
            .erase_sector(SETTINGS_OFFSET)
            .map_err(StorageError::Flash)?;
        // Write and verify the body before committing. Interrupted writes default off.
        self.flash
            .program(SETTINGS_OFFSET, &raw[..12])
            .map_err(StorageError::Flash)?;
        let mut verify = [0; RECORD_LEN];
        self.flash
            .read(SETTINGS_OFFSET, &mut verify)
            .map_err(StorageError::Flash)?;
        if verify[..12] != raw[..12] {
            return Err(StorageError::VerifyFailed);
        }
        self.flash
            .program(SETTINGS_OFFSET + 12, &raw[12..])
            .map_err(StorageError::Flash)?;
        self.flash
            .read(SETTINGS_OFFSET, &mut verify)
            .map_err(StorageError::Flash)?;
        if verify != raw {
            return Err(StorageError::VerifyFailed);
        }
        Ok(())
    }
}

impl<F: NorFlash, C: FlashCoordinator> SettingsStore for CoordinatedSeedStore<F, C> {
    type SettingsError = CoordinatedStorageError<F::Error, C::Error>;
    fn load_settings(&mut self) -> Result<DeviceSettings, Self::SettingsError> {
        self.store
            .load_settings()
            .map_err(CoordinatedStorageError::Storage)
    }
    fn save_settings(&mut self, settings: DeviceSettings) -> Result<(), Self::SettingsError> {
        self.coordinator
            .with_flash_paused(|| self.store.save_settings(settings))
            .map_err(CoordinatedStorageError::Coordinator)?
            .map_err(CoordinatedStorageError::Storage)
    }
}
