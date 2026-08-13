//! Minimal CST816D touch controller support for the Waveshare board.

use embedded_hal::i2c::I2c;

const ADDRESS: u8 = 0x15;
const FINGER_NUM: u8 = 0x02;
const CHIP_ID: u8 = 0xa7;
const IRQ_PULSE_WIDTH: u8 = 0xed;
const NORMAL_SCAN_PERIOD: u8 = 0xee;
const IRQ_CONTROL: u8 = 0xfa;
const DISABLE_AUTO_SLEEP: u8 = 0xfe;

/// A touch point in the controller's native portrait coordinate system.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TouchPoint {
    pub x: u16,
    pub y: u16,
}

pub struct Cst816d<I2C> {
    i2c: I2C,
}

impl<I2C> Cst816d<I2C>
where
    I2C: I2c,
{
    pub fn new(i2c: I2C) -> Self {
        Self { i2c }
    }

    /// Configures point reporting using the settings from Waveshare's C demo.
    ///
    /// The LCD reset sequence also resets the CST816D because GPIO20 is shared
    /// by both devices, so no separate reset pulse is needed here.
    pub fn init(&mut self) -> Result<u8, I2C::Error> {
        let chip_id = self.read_register(CHIP_ID)?;
        self.write_register(DISABLE_AUTO_SLEEP, 0x01)?;
        self.write_register(IRQ_CONTROL, 0x41)?;
        self.write_register(IRQ_PULSE_WIDTH, 0x01)?;
        self.write_register(NORMAL_SCAN_PERIOD, 0x01)?;
        Ok(chip_id)
    }

    /// Returns the current touch point, or `None` when no finger is present.
    pub fn point(&mut self) -> Result<Option<TouchPoint>, I2C::Error> {
        // Finger count followed by X high/low and Y high/low.
        let mut data = [0u8; 5];
        self.i2c.write_read(ADDRESS, &[FINGER_NUM], &mut data)?;
        if data[0] & 0x0f == 0 {
            return Ok(None);
        }

        let x = (u16::from(data[1] & 0x0f) << 8) | u16::from(data[2]);
        let y = (u16::from(data[3] & 0x0f) << 8) | u16::from(data[4]);
        Ok(Some(TouchPoint { x, y }))
    }

    fn read_register(&mut self, register: u8) -> Result<u8, I2C::Error> {
        let mut value = 0;
        self.i2c
            .write_read(ADDRESS, &[register], core::slice::from_mut(&mut value))?;
        Ok(value)
    }

    fn write_register(&mut self, register: u8, value: u8) -> Result<(), I2C::Error> {
        self.i2c.write(ADDRESS, &[register, value])
    }
}
