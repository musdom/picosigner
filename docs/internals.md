# Internals

Implementation notes for people modifying picosigner. See the top-level
[README.md](../README.md) for build and flash instructions.

## Workspace layout

The repository root is a virtual Cargo workspace. The `picosigner` firmware,
`picosigner-board`, `picosigner-ui`, `picosigner-qr`, `picosigner-wallet`, the
local `embedded-ttf` and `quircs` forks, and the desktop emulator are sibling
crates. Their default features target the embedded build; host-test-only
helpers use the `host` feature. Fork provenance and local changes are recorded
in each fork's `UPSTREAM.md`.

The old `lcd_dma` example is intentionally removed: its framebuffer transfer
path still corrupts the display. The LCD, camera, QR, and USB diagnostics live
in `picosigner-board/examples/` and use the shared libraries; they are retained
only as hardware diagnostics and are not compatibility interfaces for the old
driver modules. They reach `picosigner` for its panic handler, heap, and (for
`qr`) fonts and overlay through a dev-dependency, which keeps them beside the
drivers they exercise without making the library depend on the firmware.

With the board in BOOTSEL mode, `just flash` loads the firmware and
`just diagnostic <name>` loads one of the hardware diagnostics (`camera`,
`lcd_rgb`, `qr`, `usb`); both use `picotool` to load and start the ELF. Flash
the top-level artifact at
`target/thumbv8m.main-none-eabihf/release/picosigner`; files under `release/deps`
are intermediate Cargo artifacts and can be only 7 KiB.

The `picosigner` binary initializes the clocks, LCD SPI pins, reset, and
backlight, then draws a visible boot screen without allocating a framebuffer.

## Desktop emulator

The `picosigner-emulator` crate runs the real wallet state machine, renderer,
button geometry, and hold gestures in an SDL window on x86-64. It is a native
host build, not an RP2350 or camera emulator.

Install SDL2 development files on Debian or Ubuntu and launch it from the
workspace root:

```console
sudo apt install libsdl2-dev
just emulate
```

The first launch opens the wallet loader. Create a 12- or 24-word BIP39 wallet,
choose BIP49 nested SegWit, BIP84 native SegWit, or BIP86 Taproot, and
optionally enter a BIP39 passphrase. Hardware entropy is always mixed into a
domain-separated SHA-256 transcript; camera frames and D6 rolls can add
entropy but can never replace the platform source. Imported 12/24-word seeds
use the on-screen keyboard or a SeedQR fixture. Newly created wallets show the
same mandatory backup screen as the device. Hold the left mouse button for
about 600 ms to confirm security-sensitive actions.
On `SCAN PSBT`, the emulator reads `picosigner-emulator/input.psbt` as either
raw PSBT bytes or Base64 text. After review and hold-to-sign it writes raw PSBT
bytes atomically to `picosigner-emulator/signed.psbt` and shows the real
animated BBQr export view.

Wallets remain session-only until `SAVE WALLET` is selected. Up to eight
wallets can be persisted in `picosigner-emulator/wallet-state.bin`, including
descriptor caches; deleting a saved wallet requires a hold confirmation. Pass `--seedqr PATH`
to select the raw Standard or Compact SeedQR payload used by the emulator's
scanner action.
This file contains plaintext wallet entropy for development only. **Never fund
or use an emulated wallet with real money.** The default input, output, state,
and temporary files are ignored by Git.

Paths and window scale can be overridden:

```console
just emulate --input /tmp/input.psbt --output /tmp/signed.psbt \
  --state /tmp/picosigner-state --seedqr /tmp/seedqr.bin --scale 3
```

The simulator's screenshot support works unchanged. For example, this renders
the first view to a PNG without opening a visible window:

```console
SDL_VIDEODRIVER=dummy EG_SIMULATOR_DUMP=/tmp/picosigner.png just emulate
```

Each device wallet record also caches its public descriptor and account xpub
data. This avoids repeating BIP39 and hardened BIP32 derivation when loading a
wallet. Existing schema-1 and schema-2 records remain readable as wallet slot
zero and are atomically upgraded to the multi-wallet schema. A SHA-256 tag
binds each cache to its seed, network, normalized passphrase, and descriptor;
missing, malformed, or stale cache data is ignored and regenerated.

BIP39 passphrases are normalized and held in zeroizing session memory. They
are intentionally not added to the current plaintext Flash schema. A saved
passphrase wallet therefore asks for the passphrase again and verifies it
against the descriptor-cache tag before opening; the passphrase is never
written to the emulator state file or device Flash. Records contain only a
non-secret `passphrase_required` routing bit, allowing cached wallets to load
without first running an unnecessary empty-passphrase BIP39 derivation.

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

## QR encoders

The device generates three fixed-size QR symbols: a Compact SeedQR, a v5
signed-PSBT export frame, and a v8 descriptor. They are hand-rolled rather than
pulled from a crate because each is a single known size, which avoids a general
encoder's tables and allocation.

The finder, alignment, timing, and module-indexing routines they share live in
`picosigner-qr/src/matrix.rs`, parameterised by symbol side. They were once
copied per encoder, so a fix to one silently missed the others. The encoders are
cross-checked against the `qrcode` crate in this crate's tests.

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
just diagnostic qr
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
