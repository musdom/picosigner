//! Global `alloc` heap for QR decoding on RP2350.

use embedded_alloc::LlffHeap;

/// Heap available to `Vec`, `String`, and the local `quircs` decoder.
///
/// QR decoding uses roughly 70 KiB at the current 120 x 160 analysis size. The
/// 128 KiB allocation leaves room in main SRAM for the dedicated 32 KiB core-1
/// decoder stack and the core-0 runtime stack.
pub const HEAP_SIZE: usize = 128 * 1024;

#[global_allocator]
static HEAP: LlffHeap = LlffHeap::empty();

/// Initializes the global allocator at the linker-defined end of static RAM.
///
/// # Safety
///
/// Call exactly once, before the first allocation. The linker script must keep
/// `HEAP_SIZE` bytes between `__sheap` and `_stack_start` free.
pub unsafe fn init() {
    unsafe extern "C" {
        static mut __sheap: u8;
    }

    HEAP.init(core::ptr::addr_of_mut!(__sheap) as usize, HEAP_SIZE);
}

/// Initializes the global allocator with a caller-chosen heap size.
///
/// Binaries that trade heap for a large static buffer (for example the `qr`
/// diagnostic, which double-buffers the 153,600-byte camera frame) can claim
/// less than [`HEAP_SIZE`] so the core-0 stack keeps its margin.
///
/// # Safety
///
/// Same contract as [`init`]: call exactly once, before the first allocation,
/// and `size` bytes must be free between `__sheap` and `_stack_start`.
pub unsafe fn init_with_size(size: usize) {
    unsafe extern "C" {
        static mut __sheap: u8;
    }

    HEAP.init(core::ptr::addr_of_mut!(__sheap) as usize, size);
}
