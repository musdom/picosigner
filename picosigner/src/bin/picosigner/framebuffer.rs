use alloc::vec;
use alloc::vec::Vec;
use core::hint::spin_loop;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use core::{cell::UnsafeCell, convert::Infallible};

use embedded_graphics::{
    geometry::{OriginDimensions, Size},
    pixelcolor::Rgb565,
    prelude::{DrawTarget, IntoStorage, Pixel},
};
use hal::dma::WriteTarget;
use picosigner_board::{CAMERA_GRAY_HEIGHT, CAMERA_GRAY_WIDTH, DISPLAY_HEIGHT, DISPLAY_WIDTH};
use picosigner_qr::{extract_rgb565_volatile, QuircDecoder};
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

// ---------------------------------------------------------------------------
// Core-1 QR decode pipeline
// ---------------------------------------------------------------------------

pub const QR_PAYLOAD_CAPACITY: usize = 1024;

pub const DECODER_IDLE: u8 = 0;
pub const FRAME_READY: u8 = 1;
pub const DECODING: u8 = 2;
pub const RESULT_READY: u8 = 3;
pub const RELEASE_REQ: u8 = 4;

pub static DECODER_STATE: AtomicU8 = AtomicU8::new(DECODER_IDLE);
pub static PAYLOAD_LENGTH: AtomicUsize = AtomicUsize::new(0);

struct SharedPayload(UnsafeCell<[u8; QR_PAYLOAD_CAPACITY]>);

unsafe impl Sync for SharedPayload {}

static DECODED_PAYLOAD: SharedPayload = SharedPayload(UnsafeCell::new([0; QR_PAYLOAD_CAPACITY]));

/// Set by the flash coordinator before any XIP-touching operation. Core 1 must
/// confirm with `PARKED` and sit in a WFE loop until the request is cleared,
/// because its code lives in flash and cannot execute while XIP is disabled.
pub static PARK_REQ: AtomicBool = AtomicBool::new(false);
pub static PARKED: AtomicBool = AtomicBool::new(false);

/// Dedicated core-1 stack. quircs keeps an ~18 KiB decode scratch on the stack.
pub static CORE1_STACK: hal::multicore::Stack<8192> = hal::multicore::Stack::new();

/// Set by core 1 once it has finished initialisation and entered its loop.
pub static CORE1_BOOTED: AtomicBool = AtomicBool::new(false);
/// Core 0's view of core 1: set after the boot handshake, cleared permanently
/// if core 1 ever fails to acknowledge a park request.
pub static CORE1_LIVE: AtomicBool = AtomicBool::new(false);

pub fn set_core1_live(live: bool) {
    CORE1_LIVE.store(live, Ordering::Release);
}

pub fn core1_live() -> bool {
    CORE1_LIVE.load(Ordering::Acquire)
}

/// Payload published by core 1. Only valid while `DECODER_STATE == RESULT_READY`;
/// the consumer must reset the state to `DECODER_IDLE` afterwards.
pub fn take_payload() -> &'static [u8] {
    let len = PAYLOAD_LENGTH.load(Ordering::Relaxed);
    let payload = unsafe { &*DECODED_PAYLOAD.0.get() };
    &payload[..len]
}

/// Wakes a parked core; the event flag is latched if the core is not in WFE yet.
#[inline]
pub fn sev() {
    unsafe { core::arch::asm!("sev") }
}


/// Approximate spin budget for waiting on core 1 to release the camera frame.
const FRAME_WAIT_TIMEOUT_ITERS: u32 = 10_000_000;

/// Waits for core 1 to release `CAMERA_FRAME` (leave `FRAME_READY`). Returns
/// false and marks core 1 dead if it does not release within the budget, so a
/// wedged decoder core cannot freeze the scanner.
pub fn wait_frame_release_bounded() -> bool {
    let mut tries = 0u32;
    while DECODER_STATE.load(Ordering::Acquire) == FRAME_READY {
        tries += 1;
        if tries > FRAME_WAIT_TIMEOUT_ITERS {
            CORE1_LIVE.store(false, Ordering::Release);
            DECODER_STATE.store(DECODER_IDLE, Ordering::Release);
            return false;
        }
        spin_loop();
    }
    true
}

/// Waits for core 1 to finish decoding the previous frame. Returns false and
/// marks core 1 dead if it does not finish within the budget.
pub fn wait_decode_finished_bounded() -> bool {
    let mut tries = 0u32;
    while DECODER_STATE.load(Ordering::Acquire) == DECODING {
        tries += 1;
        if tries > FRAME_WAIT_TIMEOUT_ITERS {
            CORE1_LIVE.store(false, Ordering::Release);
            DECODER_STATE.store(DECODER_IDLE, Ordering::Release);
            return false;
        }
        spin_loop();
    }
    true
}
/// Approximate spin budget for the parking handshake (~few hundred ms at
/// 150 MHz); a healthy core 1 acknowledges within microseconds.
const PARK_TIMEOUT_ITERS: u32 = 10_000_000;

/// Request core 1 to park and block until it confirms — but never forever.
/// If core 1 does not acknowledge within the budget it is declared dead:
/// a halted core cannot fault on XIP, so the flash operation proceeds and
/// the device keeps working instead of freezing.
pub fn park_core1() {
    if !CORE1_LIVE.load(Ordering::Acquire) {
        return;
    }
    PARK_REQ.store(true, Ordering::Release);
    sev();
    let mut tries = 0u32;
    while !PARKED.load(Ordering::Acquire) {
        tries += 1;
        if tries > PARK_TIMEOUT_ITERS {
            CORE1_LIVE.store(false, Ordering::Release);
            PARK_REQ.store(false, Ordering::Release);
            return;
        }
        spin_loop();
    }
}

/// Release a parked core 1.
pub fn unpark_core1() {
    PARK_REQ.store(false, Ordering::Release);
    sev();
}

/// Parks core 1 whenever the flash coordinator requests it. Checked between
/// work units, so an in-flight decode always completes first and the
/// coordinator's wait is bounded by one decode pass.
fn park_until_released() {
    if !PARK_REQ.load(Ordering::Acquire) {
        return;
    }
    PARKED.store(true, Ordering::Release);
    while PARK_REQ.load(Ordering::Acquire) {
        unsafe { core::arch::asm!("wfe") }
    }
    PARKED.store(false, Ordering::Release);
}

/// Core-1 entry: extracts grayscale from the captured camera frame and runs QR
/// identification/decoding, publishing the payload through the shared state
/// machine. Extraction always runs on the DMA-native little-endian buffer;
/// core 0 may only byte-swap the frame for display after `DECODING` is
/// published.
pub fn decoder_core() {
    // The decoder and its grayscale buffer are built lazily on the first
    // frame: core 1 must hold no heap while the wallet UI runs, because
    // BIP32 derivation on load needs most of the 128 KiB arena.
    let mut grayscale: Option<Vec<u8>> = None;
    let mut decoder: Option<
        QuircDecoder<CAMERA_GRAY_WIDTH, CAMERA_GRAY_HEIGHT, QR_PAYLOAD_CAPACITY>,
    > = None;
    let mut centered_zoom = false;
    // Signal boot BEFORE any heap allocation so the core-0 handshake never
    // depends on allocator state.
    CORE1_BOOTED.store(true, Ordering::Release);

    loop {
        park_until_released();
        loop {
            let state = DECODER_STATE.load(Ordering::Acquire);
            if state == FRAME_READY {
                break;
            }
            if state == RELEASE_REQ {
                // Core 0 needs the heap back (e.g. PSBT assembly). Drop the
                // decoder buffers and acknowledge; they are rebuilt lazily on
                // the next submitted frame.
                decoder = None;
                grayscale = None;
                centered_zoom = false;
                DECODER_STATE.store(DECODER_IDLE, Ordering::Release);
            }
            park_until_released();
            spin_loop();
        }

        // Lazy build: no heap is held by core 1 until the first frame arrives.
        let grayscale =
            grayscale.get_or_insert_with(|| vec![0u8; CAMERA_GRAY_WIDTH * CAMERA_GRAY_HEIGHT]);
        let decoder = decoder.get_or_insert_with(
            || QuircDecoder::<CAMERA_GRAY_WIDTH, CAMERA_GRAY_HEIGHT, QR_PAYLOAD_CAPACITY>::new(),
        );

        let pixels = unsafe { camera_pixels() };
        let extracted = unsafe {
            extract_rgb565_volatile(
                pixels,
                DISPLAY_WIDTH,
                DISPLAY_HEIGHT,
                centered_zoom,
                grayscale,
            )
        };
        if extracted.is_err() {
            DECODER_STATE.store(DECODER_IDLE, Ordering::Release);
            continue;
        }
        centered_zoom = !centered_zoom;

        // CAMERA_FRAME is no longer read after this point; core 0 may re-arm
        // camera DMA and swap the buffer for display.
        DECODER_STATE.store(DECODING, Ordering::Release);

        match decoder.decode_with_timing(grayscale, || 0) {
            Ok((Some(payload), _)) => {
                let len = payload.0.len().min(QR_PAYLOAD_CAPACITY);
                unsafe {
                    (&mut *DECODED_PAYLOAD.0.get())[..len].copy_from_slice(&payload.0[..len]);
                }
                PAYLOAD_LENGTH.store(len, Ordering::Release);
                DECODER_STATE.store(RESULT_READY, Ordering::Release);
            }
            _ => DECODER_STATE.store(DECODER_IDLE, Ordering::Release),
        }
    }
}


/// Ask core 1 to drop its decoder and grayscale buffers and wait until it
/// acknowledges. The quircs decoder holds most of the 128 KiB heap; freeing
/// it before PSBT assembly keeps `finish()`/`receive_psbt()` from exhausting
/// the arena. No-op when core 1 is not live or never allocated.
pub fn release_decoder_heap() {
    if !CORE1_LIVE.load(Ordering::Acquire) {
        return;
    }
    // Let any in-flight extraction/decode finish first so we never free
    // buffers core 1 is still using.
    if !wait_frame_release_bounded() || !wait_decode_finished_bounded() {
        return;
    }
    DECODER_STATE.store(RELEASE_REQ, Ordering::Release);
    let mut tries = 0u32;
    while DECODER_STATE.load(Ordering::Acquire) == RELEASE_REQ {
        tries += 1;
        if tries > FRAME_WAIT_TIMEOUT_ITERS {
            CORE1_LIVE.store(false, Ordering::Release);
            DECODER_STATE.store(DECODER_IDLE, Ordering::Release);
            return;
        }
        spin_loop();
    }
}
/// Synchronous decode fallback for when core 1 is unavailable.
///
/// Runs extraction and quircs decoding on the calling core. It must be used
/// BEFORE the display pass mutates the shared frame buffer, because it reads
/// the DMA-native little-endian pixels directly.
pub struct SyncDecoder {
    grayscale: Vec<u8>,
    decoder: QuircDecoder<CAMERA_GRAY_WIDTH, CAMERA_GRAY_HEIGHT, QR_PAYLOAD_CAPACITY>,
    centered_zoom: bool,
}

impl SyncDecoder {
    pub fn new() -> Self {
        Self {
            grayscale: vec![0u8; CAMERA_GRAY_WIDTH * CAMERA_GRAY_HEIGHT],
            decoder:
                QuircDecoder::<CAMERA_GRAY_WIDTH, CAMERA_GRAY_HEIGHT, QR_PAYLOAD_CAPACITY>::new(),
            centered_zoom: false,
        }
    }

    /// Decodes the frame currently held in the shared camera buffer.
    pub fn decode_current_frame(&mut self) -> Option<Vec<u8>> {
        let pixels = unsafe { camera_pixels() };
        let extracted = unsafe {
            extract_rgb565_volatile(
                pixels,
                DISPLAY_WIDTH,
                DISPLAY_HEIGHT,
                self.centered_zoom,
                &mut self.grayscale,
            )
        };
        if extracted.is_err() {
            return None;
        }
        self.centered_zoom = !self.centered_zoom;
        match self.decoder.decode_with_timing(&self.grayscale, || 0) {
            Ok((Some(payload), _)) => Some(payload.0.to_vec()),
            _ => None,
        }
    }
}
