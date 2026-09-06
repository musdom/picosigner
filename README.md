# picosigner

<img width="756" height="720" alt="IMG_20260813_110109cr" src="https://github.com/user-attachments/assets/4acafa8d-5591-4c60-82fa-633656afae20" />

> **⚠️ Still in development.** This has not been audited and the secure boot
> / anti-glitch features described below are not yet enabled. Do not use it
> to hold real bitcoin.

Turn a $20 Raspberry Pi RP2350 developer board into a Bitcoin hardware signer,
built entirely in Rust. The [RP2350](https://www.raspberrypi.com/products/rp2350/) is a 150MHz dual core microcontroller with security features:

- One-time programmable fuses
- Secure boot
- Voltage and clock glitch detectors

The firmware is `no_std` Rust running directly on the RP2350 — no OS, no C
SDK. It talks to the board's LCD, touchscreen, and camera to scan PSBTs as QR
codes, sign them, and show the result, without ever exposing key material off
the device.

## Hardware

- [Waveshare RP2350-Touch-LCD-2](https://www.waveshare.com/rp2350-touch-lcd-2.htm?sku=30718)
- OV5640 camera (make sure it is included)

## Toolchain setup

You need a Rust toolchain targeting the RP2350's ARM core, plus `picotool` to 
flash it over USB.

1. **Install Rust**, if you don't already have it:

   ```console
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

2. **Add the embedded target:**

   ```console
   rustup target add thumbv8m.main-none-eabihf
   ```

3. **Install `picotool`**, Raspberry Pi's tool for loading binaries onto the
   board over USB. Grab a prebuilt release from the
   [picotool releases page](https://github.com/raspberrypi/picotool/releases)
   and put the binary on your `PATH`, e.g.:

   ```console
   curl -LO https://github.com/raspberrypi/picotool/releases/download/2.3.0/picotool-2.3.0-x86_64-lin.tar.gz
   tar xf picotool-2.3.0-x86_64-lin.tar.gz
   sudo install picotool-2.3.0-x86_64-lin/picotool/picotool /usr/local/bin/picotool
   ```

   Confirm it's on your `PATH`:

   ```console
   picotool version
   ```

4. **Install [`just`](https://github.com/casey/just)**, the command runner
   used by this repo:

   ```console
   cargo install just
   ```

That's the whole toolchain. `cargo` handles everything else per-crate.

## Build and flash

For the first installation, put the board in BOOTSEL mode (hold the BOOTSEL
button while plugging it in over USB), then:

```console
just flash
```

This builds the `picosigner` firmware in release mode and uses `picotool` to
load, verify, and start it. USB data access is **disabled by default**, including
in debug builds. To enable button-free flashing, open **Home > Settings >
Enable USB** on the initial PicoSigner page, before opening a wallet, and
long-press to confirm. The choice is saved to Flash and the device restarts. Subsequent
`just flash` / `just flash debug` runs use `picotool load -f` without BOOTSEL.

Use **Home > Settings > Disable USB** to turn it off again. With USB disabled,
older firmware, hardware diagnostics, or a stalled application, hold BOOTSEL
while connecting USB to flash. The setting survives ordinary firmware uploads
and wallet deletion; erasing the settings sector restores USB-off behavior.

## Tests

```console
just test
```

That is exactly what CI runs: formatting, clippy, the host test suite, the
golden-image UI tests, and an embedded build of every binary. It needs
`libsdl2-dev` (see below) and the `thumbv8m.main-none-eabihf` target.

Every application screen is rendered and compared against a committed baseline
in `picosigner-emulator/tests/golden/`, so any change to layout, colour, or copy
shows up as a failing test rather than as a surprise on the device. When a
screen changes on purpose:

```console
just bless    # re-render the baselines, then review the diff before committing
```

## Desktop emulator

You can develop and test the wallet UI on your own machine without any
hardware, using the `picosigner-emulator` crate:

```console
sudo apt install libsdl2-dev
just emulate
```

This runs the real wallet state machine and renderer in an SDL window,
letting you create/import a wallet, scan a PSBT, and sign it entirely on your
desktop. See [docs/internals.md](docs/internals.md#desktop-emulator) for
emulator options (custom PSBT paths, wallet persistence, screenshots).

## TODO

- Many Bitcoin wallet features
- Flash encryption
- Password encrypted seed storage
- Anti-exfil to prevent [Dark Skippy](https://darkskippy.com/) attack
- Enable the RP2350's secure boot and glitch-detector features
- Reproducible builds
- 3D printed case
- Web flasher (WebUSB/WebSerial, so people can flash without installing
  `picotool`)

## Security

picosigner is **not audited** and several hardening features are not yet
implemented. [docs/security.md](docs/security.md) records the current gaps and
reproduces a review of the Taproot signing path, so you can see what has and
has not been checked before trusting it with anything.

## Learn more

Implementation details — workspace layout, the QR decoder pipeline, LCD pin
assignments, and emulator internals — are documented in
[docs/internals.md](docs/internals.md). Security posture and the signing-path
review are in [docs/security.md](docs/security.md).
