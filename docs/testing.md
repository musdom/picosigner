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

## Heap during a scan

The QR decoder needs roughly 70 KiB of the 128 KiB heap, so nothing long-lived
may be holding the rest when a scan starts. Build with the `debug` feature and
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
