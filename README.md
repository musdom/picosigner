# picosigner

Bare-metal Rust examples for the Waveshare RP2350-Touch-LCD-2, running on the
RP2350 Cortex-M33 cores.

## Build

Install the ARM target, then build either binary:

```console
rustup target add thumbv8m.main-none-eabihf
cargo build --release --bin usb
cargo build --release --bin lcd_rgb
cargo build --release --bin camera
cargo build --release --bin qr
cargo build --release --bin picosigner
```

The repository root is a virtual Cargo workspace. The `picosigner` firmware,
`picosigner-board`, `picosigner-ui`, `picosigner-qr`, `picosigner-wallet`, and
the local `quircs` fork are sibling crates. Their default features target the
embedded build; host-test-only helpers use the `host` feature.

The old `lcd_dma` example is intentionally removed: its framebuffer transfer
path still corrupts the display. The LCD, camera, QR, and USB diagnostics use
the shared libraries; they are retained only as hardware diagnostics and are
not compatibility interfaces for the old driver modules.

With the board in BOOTSEL mode, `cargo run --release --bin usb` (or
`lcd_rgb`, `picosigner`) uses `picotool` to load and start the ELF. Flash the
top-level artifact at
`target/thumbv8m.main-none-eabihf/release/picosigner`; files under `release/deps`
are intermediate Cargo artifacts and can be only 7 KiB.

The `picosigner` binary initializes the clocks, LCD SPI pins, reset, and
backlight, then draws a visible boot screen without allocating a framebuffer.

`usb` is a USB CDC serial echo example based on `rp2350/src/bin/usb.rs`.
It uppercases received ASCII. `lcd_rgb` initializes the onboard LCD from the
Waveshare C register sequence and uses `embedded-graphics` to fill the display
red, green, and blue at one-second intervals.

`camera` configures the onboard OV5640 for 240 x 320 RGB565 output, captures
frames from its parallel DVP bus using PIO0 and DMA channel 0, and writes them
to the portrait LCD using blocking SPI0. The camera buffer is a lease from the
shared board resource arena; DMA owns it until `wait()`, then the CPU streams
one 480-byte byte-swapping row to the LCD before DMA is rearmed. This keeps the
camera diagnostic at one 153,600-byte full-screen buffer.

## QR decoder

`quircs` is an MIT-licensed fork of `quircs` 0.10.3 converted to
`no_std + alloc`. It uses `core`, `alloc`, and `libm`, and is tested against an
upstream QR image fixture. Perspective calculations use `f32` so they execute
on the RP2350's single-precision FPU. Release builds use `opt-level = 3`. The
firmware provides a 128 KiB linked-list heap for the decoder and a dedicated
32 KiB core-1 stack.

`qr` starts with the wide 1x OV5640 view and retains one 240 x 320 RGB565
camera stream.
Use the on-screen **Zoom 1x** / **Zoom 2x** button to toggle the centered sensor
crop. It alternates between a full-preview 120 x 160 grayscale scan and a
native-pixel 120 x 160 center scan marked by a white guide box, then passes
each image to `quircs::Quirc::identify()`. A
successfully decoded payload replaces the camera preview with white text on a
black screen. Press the on-screen **Scan QR** button to resume capture. The
button uses the built-in CST816D touch controller on I2C0 (GPIO12/13). Core 0
owns camera DMA, blocking LCD SPI, and touch input; core 1 converts stable frames
to grayscale and runs QR identification and decoding in parallel with the next
display pass. LCD DMA is deliberately avoided because it corrupts the preview
in the combined camera pipeline. `FAST_CAMERA_TIMING` enables an experimental
mode that doubles both the OV5640 internal PLL multiplier and its PCLK divider;
this targets faster internal frame timing without raising the external byte
clock seen by PIO. Set the constant to `false` to restore Waveshare's defaults.
Timing instrumentation is emitted once per second over USB CDC in copy-friendly `CAP`, `LCD`, `GRAY`,
`IDENTIFY`, `DECODE`, `QR`, `WAIT`, and `FRAME` microsecond fields. `QR` combines
identification, extraction, and payload decoding; `WAIT` measures core-0 decoder
synchronization, and `FRAME` is the complete capture-to-ready interval. Run and
monitor it with:

```console
cargo run --release --bin qr
picocom -b 115200 /dev/ttyACM0
```

Example telemetry:

```text
CAP=150000us LCD=21000us GRAY=3000us IDENTIFY=8400us DECODE=50us QR=8450us WAIT=0us FRAME=174000us
QR decoded
```

Only a generic decode status is sent over USB; QR payload bytes and PSBT
contents are never logged. Failed scans aren't logged, avoiding serial floods
and any resulting slowdown.

Select the LCD orientation when bringing up the display:

```rust
display.init(&mut timer, Orientation::Landscape); // 320 x 240
// Other options:
// Orientation::Portrait           // 240 x 320
// Orientation::LandscapeInverted  // 320 x 240, rotated 180 degrees
// Orientation::PortraitInverted   // 240 x 320, rotated 180 degrees
```

## LCD pin assignment

| Signal | GPIO |
| --- | ---: |
| Backlight | 15 |
| D/C | 16 |
| Chip select | 17 |
| SPI0 clock | 18 |
| SPI0 MOSI | 19 |
| Reset | 20 |

These assignments come from
`RP2350-Touch-LCD-2/C/01-LCD/lib/Config/DEV_Config.h`.
