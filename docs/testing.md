# Testing

```console
just test
```

That is exactly what CI runs, and it must pass before a change is done:

| Step | What it covers |
| --- | --- |
| `just fmt-check` | `cargo fmt --all --check` |
| `just lint` | Clippy. Enforced on the crates under active refactor; reported for the rest (see the comment in `justfile`). |
| `just test-host` | Host unit tests for every library crate, plus the emulator's integration and golden-image tests. |
| `just check-embedded` | Every binary still builds for `thumbv8m.main-none-eabihf`. |

Requires `libsdl2-dev` and the `thumbv8m.main-none-eabihf` target.

## Golden-image tests

Every application screen is rendered through the real `AppState` and the real
renderer, then compared byte-for-byte against a committed baseline in
`picosigner-emulator/tests/golden/`. Any change to layout, colour, or copy fails
the test. This is what makes UI refactoring safe.

The scenes live in `picosigner-emulator/src/scenes.rs` and are shared with the
`screenshots` example, so what you review offline is exactly what CI checks.

When a screen changes on purpose:

```console
just bless    # re-render baselines
git diff      # then actually look at them
```

Re-blessing without inspecting the diff silently stops protecting that screen.

On failure the test writes `<name>.actual.png` beside the baseline, and CI
uploads those as an artifact so a regression can be inspected without
reproducing locally.

Comparison uses raw RGB565 pixels — the exact bytes the panel receives — so it
never depends on how the PNG encoder compressed them. Baselines are stored as
PNGs at 1x purely so they are reviewable in a diff.

### Determinism

Scenes must not depend on the clock, RNG, or files outside `fixtures/`. A test
renders the whole set twice and compares, so accidental nondeterminism fails
immediately. The review screens use the committed
`picosigner-emulator/fixtures/review.psbt`, not the gitignored runtime
`input.psbt`.

## What tests cannot catch

Goldens run against a simulator. They do not exercise SPI, DMA, the camera, or
touch calibration. Before shipping a UI change, run it on hardware:

```console
just flash
```

## USB reset and flashing

Install the firmware once using BOOTSEL. With no saved setting, the running
application must expose no USB device, including when built with `debug`.
`picotool load -f` and `picotool reboot -f` must not reset it in this state.

Before creating or loading any wallet, open Home > Settings > Enable USB
from the initial PicoSigner page. Check that Settings is absent from the
individual wallet page, and that returning from Settings reaches the initial
page. The USB row must show a red X when off and a green check when on.
A tap on the confirmation must not
change the setting; long-pressing saves and restarts the device. Confirm that
USB now enumerates and `just flash` can reset into BOOTSEL, rediscover the same
chip serial number, verify the upload, and start the application without a
button press. Repeat with `just flash debug`; its CDC serial port must enumerate
alongside the reset interface only when enabled. Power-cycle and delete a saved
wallet to check that the device preference remains enabled.

Check `picotool reboot -f` (normal restart) and `picotool reboot -f -u`
(BOOTSEL), and try flashing while scanning and exporting signed QR frames.
Then use Disable USB and confirm that USB stays absent after restart and power
cycling; flashing once again requires the BOOT button. Blocking derivation or
a stalled application can prevent timely USB polling; BOOTSEL remains recovery.

Host tests cover setting persistence, separate wallet data, corrupt records,
interrupted enabling, confirmation gestures, and write failure without restart.
Goldens cover Home and both USB states and confirmation screens. Enumeration,
ROM reset and Flash operations still need these physical hardware checks.

## Heap during a scan

The QR decoder needs 51,200 bytes for its label image plus variable metadata
and transport allocations, so release long-lived allocations before scanning.
Build with the `debug` feature, enable USB in Settings, and
watch the serial log:

```console
just flash debug
picocom -b 115200 /dev/ttyACM0
```

Three `HEAP` lines bracket a scan — `scan-enter`, `scan-decoder`, and
`scan-exit`. `scan-enter` is the number that matters: it is what the UI left
behind, and it must stay near zero. The widget layer allocates nothing, so a
non-trivial value there means something else is holding memory across the
transition.

## Firmware size

```console
just size
```

Static RAM is the binding constraint on the RP2350 (see
[architecture.md](architecture.md#memory)). `bss` must not grow.

## Camera exposure comparison

The `CameraExposure::Screen` profile is an initial setting for laptop QR codes,
not yet a measured improvement. Automatic exposure and gain remain enabled;
the preview may look darker than before.

Compare with `CameraExposure::General` by changing `PSBT_EXPOSURE` in
`picosigner/src/bin/picosigner/main.rs`, rebuilding, and flashing with
`just flash`. For the standalone QR diagnostic, change `QR_EXPOSURE` in
`picosigner-board/examples/qr.rs` and run `just diagnostic qr`.

Use the same test PSBT, QR size, animation speed, camera position, and zoom
for each profile. Repeat scans at low, medium, and high screen brightness,
recording time to first accepted part and time to completion. Also change
brightness during a scan to check that exposure settles and collection resumes.
Compare a paused frame with the animation to distinguish trouble resolving
the symbol from trouble capturing transitions. Check that returning to SeedQR
or camera entropy restores the general profile.

Use synthetic, unfunded test data for these comparisons. Host tests verify
register writes and automatic-control bits; they cannot establish the sensor's
actual exposure duration, settling time, or scanning reliability.

### Sparrow scans that stall

In Sparrow's QR dialog, use **Change Density** to select **Low**. Scroll upward
over the QR image to slow its animation; aim for roughly one change per second
for an initial comparison. Restart the receiver after changing density or
encoding. Sparrow's controls are implemented in
[QRDisplayDialog](https://github.com/sparrowwallet/sparrow/blob/master/src/main/java/com/sparrowwallet/sparrow/control/QRDisplayDialog.java).

Record the encoding (UR or BBQr) along with whether progress stays empty or
advances and stalls. Partial progress establishes that some fragments were
accepted, but does not distinguish optical failures from rejected later parts.
The host QR adapter test covers 2008-byte BBQr symbols and a shorter final
fragment; the UR collector test covers missing the initial animation cycle,
dropping four out of five frames, duplicates, and restarting the receiver.
These tests isolate payload handling with synthetic data, not OLED capture
quality or physical scan speed.

### Native scan area

The native crop is 160 x 160. Compare the same low-density UR animation at
the same zoom, then move closer while keeping the whole symbol and its white
border in the camera view. Codes can extend beyond the native crop: the
wide-view fallback also runs, so the preview has no crop guide. The synthetic
regression covers a UR
symbol wider than the old 120-pixel crop; this establishes the additional
framing room, not camera focus or OLED scan reliability. Compare scan time and
heap headroom on hardware: removing the grayscale copy saves pixel workspace,
but the larger view can find more regions and take longer to process.
