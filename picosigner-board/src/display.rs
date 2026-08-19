//! Driver for the built-in 2-inch LCD on the Waveshare RP2350-Touch-LCD-2.
//!
//! The pin assignment and initialization sequence mirror Waveshare's
//! `C/01-LCD` example. The panel is exposed as a 320 x 240 landscape
//! [`embedded_graphics`] draw target using RGB565 colors.

use core::convert::Infallible;

use embedded_graphics::{
    geometry::{Dimensions, OriginDimensions, Size},
    pixelcolor::Rgb565,
    prelude::{DrawTarget, IntoStorage, Pixel},
    primitives::Rectangle,
};
use embedded_hal::{delay::DelayNs, digital::OutputPin, spi::SpiBus};

/// Display orientation selected during panel initialization.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Orientation {
    /// 320 pixels wide by 240 pixels high.
    Landscape,
    /// 240 pixels wide by 320 pixels high.
    Portrait,
    /// 240 pixels wide by 320 pixels high with X increasing in the same
    /// physical direction as the board's touch controller.
    PortraitTouchAligned,
    /// 320 pixels wide by 240 pixels high, rotated 180 degrees from
    /// [`Landscape`](Self::Landscape).
    LandscapeInverted,
    /// 240 pixels wide by 320 pixels high, rotated 180 degrees from
    /// [`Portrait`](Self::Portrait).
    PortraitInverted,
}

impl Orientation {
    const fn dimensions(self) -> Size {
        match self {
            Self::Landscape | Self::LandscapeInverted => Size::new(320, 240),
            Self::Portrait | Self::PortraitTouchAligned | Self::PortraitInverted => {
                Size::new(240, 320)
            }
        }
    }

    const fn memory_access_control(self) -> u8 {
        match self {
            // MADCTL: MY=bit 7, MX=bit 6, MV=bit 5, BGR=bit 3.
            // The first two values match Waveshare's HORIZONTAL and VERTICAL
            // modes; toggling MX and MY produces their 180-degree rotations.
            Self::Landscape => 0x28,
            Self::Portrait => 0x08,
            // MX mirrors the controller's portrait column direction so LCD
            // X=0 and CST816D touch X=0 refer to the same physical edge.
            Self::PortraitTouchAligned => 0x48,
            Self::LandscapeInverted => 0xe8,
            Self::PortraitInverted => 0xc8,
        }
    }
}

pub struct WaveshareLcd<SPI, DC, CS, RST> {
    spi: SPI,
    dc: DC,
    cs: CS,
    rst: RST,
    size: Size,
}

impl<SPI, DC, CS, RST> WaveshareLcd<SPI, DC, CS, RST>
where
    SPI: SpiBus<u8>,
    DC: OutputPin,
    CS: OutputPin,
    RST: OutputPin,
{
    pub fn new(spi: SPI, dc: DC, cs: CS, rst: RST) -> Self {
        Self {
            spi,
            dc,
            cs,
            rst,
            size: Orientation::Landscape.dimensions(),
        }
    }

    /// Reassembles a display previously consumed by [`Self::release`].
    pub fn from_parts(spi: SPI, dc: DC, cs: CS, rst: RST, size: Size) -> Self {
        Self {
            spi,
            dc,
            cs,
            rst,
            size,
        }
    }

    /// Releases the SPI peripheral and control pins.
    ///
    /// This is useful when a DMA transfer needs to take ownership of the SPI
    /// peripheral. Pass the returned values to [`Self::from_parts`] after the
    /// transfer completes.
    pub fn release(self) -> (SPI, DC, CS, RST, Size) {
        (self.spi, self.dc, self.cs, self.rst, self.size)
    }

    /// Initializes the panel using the requested display orientation.
    pub fn init(&mut self, delay: &mut impl DelayNs, orientation: Orientation) {
        self.size = orientation.dimensions();

        let _ = self.cs.set_high();
        let _ = self.rst.set_high();
        delay.delay_ms(100);
        let _ = self.rst.set_low();
        delay.delay_ms(100);
        let _ = self.rst.set_high();
        delay.delay_ms(100);

        // Scan direction from Waveshare LCD_2IN_SetAttributes().
        self.command(0x11, &[]);
        delay.delay_ms(120);
        self.command(0x36, &[orientation.memory_access_control()]);

        self.command(0x3a, &[0x05]);
        self.command(0xf0, &[0xc3]);
        self.command(0xf0, &[0x96]);
        self.command(0xb4, &[0x01]);
        self.command(0xb7, &[0xc6]);
        self.command(0xc0, &[0x80, 0x45]);
        self.command(0xc1, &[0x13]);
        self.command(0xc2, &[0xa7]);
        self.command(0xc5, &[0x0a]);
        self.command(0xe8, &[0x40, 0x8a, 0x00, 0x00, 0x29, 0x19, 0xa5, 0x33]);
        self.command(
            0xe0,
            &[
                0xd0, 0x08, 0x0f, 0x06, 0x06, 0x33, 0x30, 0x33, 0x47, 0x17, 0x13, 0x13, 0x2b, 0x31,
            ],
        );
        self.command(
            0xe1,
            &[
                0xd0, 0x0a, 0x11, 0x0b, 0x09, 0x07, 0x2f, 0x33, 0x47, 0x38, 0x15, 0x16, 0x2c, 0x32,
            ],
        );
        self.command(0xf0, &[0x3c]);
        self.command(0xf0, &[0x69]);
        delay.delay_ms(120);
        self.command(0x21, &[]);
        self.command(0x29, &[]);
    }

    fn command(&mut self, command: u8, data: &[u8]) {
        let _ = self.dc.set_low();
        let _ = self.cs.set_low();
        let _ = self.spi.write(&[command]);
        let _ = self.cs.set_high();

        if !data.is_empty() {
            let _ = self.dc.set_high();
            let _ = self.cs.set_low();
            let _ = self.spi.write(data);
            let _ = self.cs.set_high();
        }
    }

    fn set_window(&mut self, x: u16, y: u16, width: u16, height: u16) {
        let x_end = x + width - 1;
        let y_end = y + height - 1;
        self.command(
            0x2a,
            &[(x >> 8) as u8, x as u8, (x_end >> 8) as u8, x_end as u8],
        );
        self.command(
            0x2b,
            &[(y >> 8) as u8, y as u8, (y_end >> 8) as u8, y_end as u8],
        );
        self.command(0x2c, &[]);
    }

    fn begin_pixels(&mut self) {
        let _ = self.dc.set_high();
        let _ = self.cs.set_low();
    }

    fn end_pixels(&mut self) {
        let _ = self.cs.set_high();
    }

    /// Selects the entire visible area and starts a continuous pixel write.
    ///
    /// Call this before releasing the SPI peripheral for a framebuffer DMA
    /// transfer.
    pub fn begin_frame_write(&mut self) {
        self.set_window(0, 0, self.size.width as u16, self.size.height as u16);
        self.begin_pixels();
    }

    /// Waits for SPI to become idle and ends a continuous pixel write.
    pub fn end_frame_write(&mut self) {
        let _ = self.spi.flush();
        self.end_pixels();
    }

    /// Writes a complete panel-ready RGB565 framebuffer using blocking SPI.
    ///
    /// Pixels must be supplied high byte first and the slice length must match
    /// the selected orientation's width × height × 2 bytes.
    pub fn write_frame(&mut self, pixels: &[u8]) {
        debug_assert_eq!(
            pixels.len(),
            self.size.width as usize * self.size.height as usize * 2
        );
        self.begin_frame_write();
        let _ = self.spi.write(pixels);
        self.end_frame_write();
    }

    /// Selects the full screen and starts a continuous pixel write, then hands
    /// back the SPI peripheral for a DMA transfer.
    ///
    /// The caller must feed the panel-ready framebuffer to SPI0 by DMA and then
    /// call [`Self::from_parts`] followed by [`Self::end_dma_frame`] so CS is
    /// released and the panel sees a complete frame. `cs` stays asserted for the
    /// whole transfer, which is what lets the panel treat it as one continuous
    /// stream of pixels.
    pub fn begin_dma_frame(mut self) -> (SPI, DC, CS, RST, Size) {
        self.begin_frame_write();
        self.release()
    }

    /// Completes a frame started by [`Self::begin_dma_frame`].
    ///
    /// Call after the DMA transfer has finished and the display has been
    /// rebuilt with [`Self::from_parts`].
    pub fn end_dma_frame(&mut self) {
        self.end_frame_write();
    }

    fn write_color(&mut self, color: Rgb565, count: u32) {
        let bytes = color.into_storage().to_be_bytes();
        let mut chunk = [0u8; 128];
        for pair in chunk.chunks_exact_mut(2) {
            pair.copy_from_slice(&bytes);
        }

        let mut remaining = count;
        while remaining != 0 {
            let pixels = remaining.min((chunk.len() / 2) as u32);
            let _ = self.spi.write(&chunk[..pixels as usize * 2]);
            remaining -= pixels;
        }
    }
}

impl<SPI, DC, CS, RST> OriginDimensions for WaveshareLcd<SPI, DC, CS, RST> {
    fn size(&self) -> Size {
        self.size
    }
}

impl<SPI, DC, CS, RST> DrawTarget for WaveshareLcd<SPI, DC, CS, RST>
where
    SPI: SpiBus<u8>,
    DC: OutputPin,
    CS: OutputPin,
    RST: OutputPin,
{
    type Color = Rgb565;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            if point.x >= 0
                && point.y >= 0
                && point.x < self.size.width as i32
                && point.y < self.size.height as i32
            {
                self.set_window(point.x as u16, point.y as u16, 1, 1);
                self.begin_pixels();
                let _ = self.spi.write(&color.into_storage().to_be_bytes());
                self.end_pixels();
            }
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let clipped = area.intersection(&self.bounding_box());
        if clipped.size.width == 0 || clipped.size.height == 0 {
            return Ok(());
        }

        self.set_window(
            clipped.top_left.x as u16,
            clipped.top_left.y as u16,
            clipped.size.width as u16,
            clipped.size.height as u16,
        );
        self.begin_pixels();
        self.write_color(color, clipped.size.width * clipped.size.height);
        self.end_pixels();
        Ok(())
    }
}
