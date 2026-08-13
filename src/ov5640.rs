//! Minimal OV5640 RGB565 setup for the Waveshare RP2350-Touch-LCD-2.

use embedded_hal::{delay::DelayNs, i2c::I2c};

const ADDRESS: u8 = 0x3c;
const DELAY: u16 = 0xffff;

// Waveshare's portrait setup reads this centered 1440 x 1920 sensor window
// before scaling it to the 240 x 320 DVP output.
const WIDE_X_START: u16 = 352;
const WIDE_Y_START: u16 = 26;
const WIDE_X_END: u16 = 1792;
const WIDE_Y_END: u16 = 1946;

// Halving both source dimensions makes the OV5640 scaler produce a centered
// 2x digital zoom without increasing the DVP output size or data volume.
const ZOOM_2X_X_START: u16 = 712;
const ZOOM_2X_Y_START: u16 = 506;
const ZOOM_2X_X_END: u16 = 1432;
const ZOOM_2X_Y_END: u16 = 1466;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SensorZoom {
    X1,
    X2,
}

impl SensorZoom {
    pub const fn factor(self) -> u8 {
        match self {
            Self::X1 => 1,
            Self::X2 => 2,
        }
    }

    pub const fn toggle(self) -> Self {
        match self {
            Self::X1 => Self::X2,
            Self::X2 => Self::X1,
        }
    }
}

#[derive(Debug)]
pub enum Error<E> {
    Bus(E),
    WrongChipId(u16),
}

pub struct Ov5640<I2C> {
    i2c: I2C,
}

impl<I2C> Ov5640<I2C>
where
    I2C: I2c,
{
    pub fn new(i2c: I2C) -> Self {
        Self { i2c }
    }

    /// Configures 240 x 320 RGB565 output using Waveshare's register sequence.
    pub fn init(&mut self, delay: &mut impl DelayNs) -> Result<(), Error<I2C::Error>> {
        let id = (u16::from(self.read(0x300a)?) << 8) | u16::from(self.read(0x300b)?);
        if id != 0x5640 {
            return Err(Error::WrongChipId(id));
        }

        for &(register, value) in SENSOR_DEFAULT_REGS {
            if register == DELAY {
                delay.delay_ms(u32::from(value));
            } else {
                self.write(register, value as u8)?;
            }
        }

        self.write_pair(0x3800, WIDE_X_START, WIDE_Y_START)?;
        self.write_pair(0x3804, WIDE_X_END, WIDE_Y_END)?;
        self.write_pair(0x3808, 240, 320)?;
        self.write_pair(0x380c, 2592, 1944)?;
        self.write_pair(0x3810, 16, 14)?;

        let isp_control = self.read(0x5001)? | 0x20;
        self.write(0x5001, isp_control)?;
        delay.delay_ms(50);

        for &(register, value) in &[
            (0x3820, 0x01),
            (0x3821, 0x00),
            (0x4514, 0xaa),
            (0x4520, 0x0b),
            (0x3814, 0x31),
            (0x3815, 0x31),
        ] {
            self.write(register, value)?;
        }
        delay.delay_ms(50);

        for &(register, value) in &[
            (0x3039, 0x00),
            (0x3034, 0x1a),
            (0x3035, 0x11),
            // Keep Waveshare's proven PLL multiplier. Raising this also raises
            // PCLK; doubling it made the PIO sampler miss byte edges and
            // corrupted captured frames.
            (0x3036, 0x0b),
            (0x3037, 0x01),
            (0x3108, 0x16),
            (0x3824, 0x04),
            (0x460c, 0x22),
            (0x3103, 0x13),
        ] {
            self.write(register, value)?;
        }
        delay.delay_ms(50);

        self.write(0x501f, 0x01)?;
        self.write(0x4300, 0x61)?;
        delay.delay_ms(50);
        Ok(())
    }

    /// Selects a centered sensor crop while retaining 240 x 320 output.
    ///
    /// This zoom happens before the OV5640's scaler, so it preserves more
    /// source detail than cropping the already-scaled DVP frame on the RP2350.
    /// It does not change the output buffer size or external pixel-clock rate.
    pub fn set_zoom(&mut self, zoom: SensorZoom) -> Result<(), Error<I2C::Error>> {
        let (x_start, y_start, x_end, y_end) = match zoom {
            SensorZoom::X1 => (WIDE_X_START, WIDE_Y_START, WIDE_X_END, WIDE_Y_END),
            SensorZoom::X2 => (
                ZOOM_2X_X_START,
                ZOOM_2X_Y_START,
                ZOOM_2X_X_END,
                ZOOM_2X_Y_END,
            ),
        };

        self.write_pair(0x3800, x_start, y_start)?;
        self.write_pair(0x3804, x_end, y_end)
    }

    /// Enables or disables the sensor's static color-bar test pattern.
    pub fn set_test_pattern(&mut self, enabled: bool) -> Result<(), Error<I2C::Error>> {
        self.write(0x503d, if enabled { 0x80 } else { 0x00 })
    }

    /// Selects experimental 2x internal frame timing without increasing PCLK.
    ///
    /// The PLL multiplier and manual PCLK divider are both doubled. This should
    /// shorten internal row/frame timing while keeping the external byte clock
    /// within the rate already proven reliable with the PIO capture program.
    pub fn set_fast_frame_timing(&mut self, enabled: bool) -> Result<(), Error<I2C::Error>> {
        if enabled {
            // Slow PCLK first so there is no transient over-clock while the PLL
            // multiplier is raised.
            self.write(0x3824, 0x08)?;
            self.write(0x3036, 0x16)?;
        } else {
            // Lower the PLL first before restoring Waveshare's PCLK divider.
            self.write(0x3036, 0x0b)?;
            self.write(0x3824, 0x04)?;
        }
        Ok(())
    }

    fn write_pair(
        &mut self,
        register: u16,
        first: u16,
        second: u16,
    ) -> Result<(), Error<I2C::Error>> {
        self.write(register, (first >> 8) as u8)?;
        self.write(register + 1, first as u8)?;
        self.write(register + 2, (second >> 8) as u8)?;
        self.write(register + 3, second as u8)
    }

    fn write(&mut self, register: u16, value: u8) -> Result<(), Error<I2C::Error>> {
        self.i2c
            .write(ADDRESS, &[(register >> 8) as u8, register as u8, value])
            .map_err(Error::Bus)
    }

    fn read(&mut self, register: u16) -> Result<u8, Error<I2C::Error>> {
        let mut value = [0];
        self.i2c
            .write_read(
                ADDRESS,
                &[(register >> 8) as u8, register as u8],
                &mut value,
            )
            .map_err(Error::Bus)?;
        Ok(value[0])
    }
}

// This table is intentionally kept aligned with Waveshare's C/02-CAM OV5640
// setup. DELAY entries contain milliseconds instead of register values.
const SENSOR_DEFAULT_REGS: &[(u16, u16)] = &[
    (0x3008, 0x82),
    (DELAY, 10),
    (0x3008, 0x42),
    (0x3103, 0x13),
    (0x3017, 0xff),
    (0x3018, 0xff),
    (0x302c, 0xc3),
    // PCLK active high, gated outside active HREF, VSYNC active high. Gating
    // makes every PCLK edge presented to PIO correspond to valid image data.
    (0x4740, 0x25),
    (0x4713, 0x02),
    (0x5001, 0x83),
    (0x3000, 0x20),
    (DELAY, 10),
    (0x3002, 0x1c),
    (0x3004, 0xff),
    (0x3006, 0xc3),
    (0x5000, 0xa7),
    (0x5001, 0xa3),
    (0x5003, 0x08),
    (0x370c, 0x02),
    (0x3634, 0x40),
    (0x3a02, 0x03),
    (0x3a03, 0xd8),
    (0x3a08, 0x01),
    (0x3a09, 0x27),
    (0x3a0a, 0x00),
    (0x3a0b, 0xf6),
    (0x3a0d, 0x04),
    (0x3a0e, 0x03),
    (0x3a0f, 0x30),
    (0x3a10, 0x28),
    (0x3a11, 0x60),
    (0x3a13, 0x43),
    (0x3a14, 0x03),
    (0x3a15, 0xd8),
    (0x3a18, 0x00),
    (0x3a19, 0xf8),
    (0x3a1b, 0x30),
    (0x3a1e, 0x26),
    (0x3a1f, 0x14),
    (0x3600, 0x08),
    (0x3601, 0x33),
    (0x3c01, 0xa4),
    (0x3c04, 0x28),
    (0x3c05, 0x98),
    (0x3c06, 0x00),
    (0x3c07, 0x08),
    (0x3c08, 0x00),
    (0x3c09, 0x1c),
    (0x3c0a, 0x9c),
    (0x3c0b, 0x40),
    (0x460c, 0x22),
    (0x4001, 0x02),
    (0x4004, 0x02),
    (0x5180, 0xff),
    (0x5181, 0xf2),
    (0x5182, 0x00),
    (0x5183, 0x14),
    (0x5184, 0x25),
    (0x5185, 0x24),
    (0x5186, 0x09),
    (0x5187, 0x09),
    (0x5188, 0x09),
    (0x5189, 0x75),
    (0x518a, 0x54),
    (0x518b, 0xe0),
    (0x518c, 0xb2),
    (0x518d, 0x42),
    (0x518e, 0x3d),
    (0x518f, 0x56),
    (0x5190, 0x46),
    (0x5191, 0xf8),
    (0x5192, 0x04),
    (0x5193, 0x70),
    (0x5194, 0xf0),
    (0x5195, 0xf0),
    (0x5196, 0x03),
    (0x5197, 0x01),
    (0x5198, 0x04),
    (0x5199, 0x12),
    (0x519a, 0x04),
    (0x519b, 0x00),
    (0x519c, 0x06),
    (0x519d, 0x82),
    (0x519e, 0x38),
    (0x5381, 0x1e),
    (0x5382, 0x5b),
    (0x5383, 0x08),
    (0x5384, 0x0a),
    (0x5385, 0x7e),
    (0x5386, 0x88),
    (0x5387, 0x7c),
    (0x5388, 0x6c),
    (0x5389, 0x10),
    (0x538a, 0x01),
    (0x538b, 0x98),
    (0x5300, 0x10),
    (0x5301, 0x10),
    (0x5302, 0x18),
    (0x5303, 0x19),
    (0x5304, 0x10),
    (0x5305, 0x10),
    (0x5306, 0x08),
    (0x5307, 0x16),
    (0x5308, 0x40),
    (0x5309, 0x10),
    (0x530a, 0x10),
    (0x530b, 0x04),
    (0x530c, 0x06),
    (0x5480, 0x01),
    (0x5481, 0x00),
    (0x5482, 0x1e),
    (0x5483, 0x3b),
    (0x5484, 0x58),
    (0x5485, 0x66),
    (0x5486, 0x71),
    (0x5487, 0x7d),
    (0x5488, 0x83),
    (0x5489, 0x8f),
    (0x548a, 0x98),
    (0x548b, 0xa6),
    (0x548c, 0xb8),
    (0x548d, 0xca),
    (0x548e, 0xd7),
    (0x548f, 0xe3),
    (0x5490, 0x1d),
    (0x5580, 0x04),
    (0x5583, 0x40),
    (0x5584, 0x10),
    (0x5586, 0x20),
    (0x5587, 0x00),
    (0x5588, 0x01),
    (0x5589, 0x10),
    (0x558a, 0x00),
    (0x558b, 0xf8),
    (0x501d, 0x40),
    (0x3008, 0x02),
    (0x3c00, 0x04),
    (DELAY, 300),
];
