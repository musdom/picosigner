use core::{cell::UnsafeCell, convert::Infallible};

use embedded_graphics::{
    geometry::{OriginDimensions, Size},
    pixelcolor::Rgb565,
    prelude::{DrawTarget, IntoStorage, Pixel},
};
use hal::dma::WriteTarget;
use picosigner_board::{DISPLAY_HEIGHT, DISPLAY_WIDTH};
use rp235x_hal as hal;

pub const CAMERA_PIXELS: usize = DISPLAY_WIDTH * DISPLAY_HEIGHT;

#[repr(align(4))]
struct CameraFrame(UnsafeCell<[u8; CAMERA_PIXELS * 2]>);

unsafe impl Sync for CameraFrame {}

static CAMERA_FRAME: CameraFrame = CameraFrame(UnsafeCell::new([0; CAMERA_PIXELS * 2]));

#[derive(Clone, Copy)]
pub struct CameraDmaTarget;

unsafe impl WriteTarget for CameraDmaTarget {
    type TransmittedWord = u16;

    fn tx_treq() -> Option<u8> {
        None
    }

    fn tx_address_count(&mut self) -> (u32, u32) {
        (
            CAMERA_FRAME.0.get().cast::<u16>() as u32,
            CAMERA_PIXELS as u32,
        )
    }

    fn tx_increment(&self) -> bool {
        true
    }
}

/// Returns the shared camera/display framebuffer.
///
/// # Safety
/// The caller must ensure camera DMA is idle and no other framebuffer borrow
/// exists for the returned slice's lifetime.
pub unsafe fn frame_mut() -> &'static mut [u8] {
    unsafe { &mut *CAMERA_FRAME.0.get() }
}

/// Returns the DMA framebuffer as RGB565 halfwords.
///
/// # Safety
/// The caller must ensure camera DMA is idle while dereferencing this pointer.
pub unsafe fn camera_pixels() -> *const u16 {
    CAMERA_FRAME.0.get().cast::<u16>()
}

pub struct PortraitFrame<'a>(pub &'a mut [u8]);

impl OriginDimensions for PortraitFrame<'_> {
    fn size(&self) -> Size {
        Size::new(DISPLAY_WIDTH as u32, DISPLAY_HEIGHT as u32)
    }
}

impl DrawTarget for PortraitFrame<'_> {
    type Color = Rgb565;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            if point.x >= 0
                && point.y >= 0
                && point.x < DISPLAY_WIDTH as i32
                && point.y < DISPLAY_HEIGHT as i32
            {
                let offset = (point.y as usize * DISPLAY_WIDTH + point.x as usize) * 2;
                self.0[offset..offset + 2].copy_from_slice(&color.into_storage().to_be_bytes());
            }
        }
        Ok(())
    }
}

pub fn prepare_camera_preview(frame: &mut [u8]) {
    // Camera rows arrive in the opposite X direction from the touch-aligned
    // portrait LCD. Mirror each row while also converting native little-endian
    // DMA halfwords into the panel's high-byte-first RGB565 stream.
    for row in frame.chunks_exact_mut(DISPLAY_WIDTH * 2) {
        for x in 0..DISPLAY_WIDTH / 2 {
            let left = x * 2;
            let right = (DISPLAY_WIDTH - 1 - x) * 2;
            let left_pixel = [row[left], row[left + 1]];
            let right_pixel = [row[right], row[right + 1]];
            row[left] = right_pixel[1];
            row[left + 1] = right_pixel[0];
            row[right] = left_pixel[1];
            row[right + 1] = left_pixel[0];
        }
    }
}
