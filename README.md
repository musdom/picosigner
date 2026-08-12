# picosigner

Bare-metal Rust examples for the Waveshare RP2350-Touch-LCD-2, running on the
RP2350 Cortex-M33 cores.

## Build

Install the ARM target, then build either binary:

```console
rustup target add thumbv8m.main-none-eabihf
cargo build --release --bin usb
cargo build --release --bin lcd_rgb
cargo build --release --bin lcd_dma
```

With the board in BOOTSEL mode, `cargo run --release --bin usb` (or `lcd_rgb`)
uses `picotool` to load and start the ELF.

`usb` is a USB CDC serial echo example based on `rp2350/src/bin/usb.rs`.
It uppercases received ASCII. `lcd_rgb` initializes the onboard LCD from the
Waveshare C register sequence and uses `embedded-graphics` to fill the display
red, green, and blue at one-second intervals.

`lcd_dma` draws randomized `embedded-graphics` rectangles, circles, and text
into a 320 x 240 RGB565 framebuffer, then transfers the complete frame through
SPI0 using DMA channel 0. The framebuffer occupies 153,600 bytes of SRAM.

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
