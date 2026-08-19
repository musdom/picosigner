# picosigner

<img width="756" height="720" alt="IMG_20260813_110109cr" src="https://github.com/user-attachments/assets/4acafa8d-5591-4c60-82fa-633656afae20" />

> **⚠️ Still in development.** This has not been audited and the secure boot
> / anti-glitch features described below are not yet enabled. Do not use it
> to hold real bitcoin.

Turn a $20 Raspberry Pi RP2350 developer board into a Bitcoin hardware signer,
built entirely in Rust. The RP2350 is a 150MHz dual core ARM Cortex-M33
microcontroller with security features:

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

You need a Rust toolchain targeting the RP2350's Cortex-M33 core, plus
`picotool` to flash it over USB.

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

Put the board in BOOTSEL mode (hold the BOOTSEL button while plugging it in
over USB), then:

```console
just flash
```

This builds the `picosigner` firmware in release mode and uses `picotool` to
load and start it on the board.

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
- Opt-in seed storage encryption
- Enable the RP2350's secure boot and glitch-detector features
- Reproducible builds
- 3D printed case
- Web flasher (WebUSB/WebSerial, so people can flash without installing
  `picotool`)

## Learn more

Implementation details — workspace layout, the QR decoder pipeline, LCD pin
assignments, and emulator internals — are documented in
[docs/internals.md](docs/internals.md).
