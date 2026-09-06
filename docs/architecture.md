# Architecture

Where things live and why. Read this before changing code; see
[internals.md](internals.md) for hardware detail and [security.md](security.md)
for the trust boundaries.

## Crates

The repository root is a virtual Cargo workspace.

| Crate | Responsibility | `no_std` |
| --- | --- | --- |
| `picosigner` | Application state and the UI, split by concern (see below), plus the firmware binary. | yes |
| `picosigner-board` | Hardware: LCD, touch, camera, framebuffer, TRNG, flash storage, multicore coordination. Owns the traits the app is generic over, and the diagnostics in `examples/`. | yes |
| `picosigner-wallet` | Bitcoin, split by concern: `review.rs`, `sign.rs`, `descriptor.rs`, `frost_backup.rs`, with shared types in `lib.rs`. **No UI dependency.** | yes |
| `picosigner-qr` | QR encode/decode, BBQr and UR transport, camera frame extraction. Symbol construction shared by all three encoders lives in `matrix.rs`. | yes |
| `picosigner-ui` | Screen enum, gestures, touch tracking, and the widget layer: `Rect`, the `Widget` trait, `KeyGrid`, and the colour `Theme`. Host-testable, no hardware. | yes |
| `picosigner-emulator` | Desktop harness. Runs the **real** state machine and renderer under SDL. | no (`std`) |
| `quircs`, `embedded-ttf` | Vendored forks. Provenance in each `UPSTREAM.md`. | yes |

Every crate has `default = ["embedded"]` and a `host` feature that fans out to
its dependencies' `host` features. The emulator is the only `std` crate.

```
picosigner-emulator ──┐
                      ├──> picosigner ──> picosigner-{board,ui,qr,wallet}
picosigner (firmware) ┘

picosigner-board/examples/  ──(dev-dependency)──> picosigner
```

The hardware diagnostics (`camera`, `lcd_rgb`, `qr`, `usb`) live beside the
drivers they exercise and reach back to `picosigner` for its panic handler,
heap, and fonts through a dev-dependency. Cargo permits that cycle, and it keeps
the library itself free of any dependency on the firmware. Run one with
`just diagnostic <name>`.

## The seam that matters

The firmware and the emulator run the same code. What separates them is a small
set of traits in `picosigner-board`:

- `CryptoEntropy` — the RP2350 TRNG on device, `getrandom` in the emulator, a
  fixture in tests.
- `WalletStore` — transactional flash on device, a file in the emulator.
- `SettingsStore` — non-secret device preferences, including the default-off USB
  setting, stored separately from wallets.

`AppState` methods are generic over both, which is why the emulator exercises
real behaviour rather than a reimplementation. **Keep it that way**: logic that
reaches for a concrete hardware type cannot be tested.

Rendering shares the same trick through `embedded_graphics::DrawTarget`: the
device draws into a framebuffer, the emulator into a `SimulatorDisplay`.

## The UI modules

`picosigner/src/` splits the UI by concern rather than by screen:

| Module | Holds |
| --- | --- |
| `app/` | Application state. `mod.rs` holds `AppState` and the wallet lifecycle; `mnemonic.rs` and `frost.rs` hold the two word-entry flows. |
| `layout.rs` | **Every screen's geometry**, declared once. Menus, keyboards, key grids. Read by both the renderer and the hit-test. |
| `touch.rs` | `HoldAction` and `action_at`: what a touch at a point means. |
| `screens.rs` | Per-screen drawing. One function paints one screen's body. |
| `ui.rs` | Shared widgets and typography: buttons, footers, hold bars, menu rows, QR, fonts. |
| `icons.rs` | The 24px glyph set, drawn as vector primitives. |
| `dispatch.rs` | `(HoldAction, Gesture)` → state transition, shared by firmware and emulator. |

The important line is between `layout.rs` and everything else: geometry is
stated **once** and read by both the drawing and the hit-test. When they were
written separately they drifted, and controls became touchable somewhere other
than where they appeared.

## Widgets

A widget owns its geometry: it is given a [`Rect`], and both its drawing and
its hit-testing read that same rectangle. This is the fix for the defect that
shaped much of this UI — geometry was previously declared twice, in `render_app`
and again in `action_at`, and the two drifted.

```rust
pub trait Widget {
    type Action;
    fn set_bounds(&mut self, bounds: Rect);
    fn bounds(&self) -> Rect;
    fn hit(&self, x: u16, y: u16) -> Option<Self::Action>;
    fn draw<D, C>(&self, target: &mut D, theme: &Theme<C>) -> Result<(), D::Error>;
}
```

`KeyGrid` is the worked example: all three on-screen keyboards — BIP39 letters,
passphrase, and the dice keypad — are one grid type configured differently in
`picosigner/src/keyboards.rs`. Each was previously written twice with different
cell sizes in each copy.

Widgets are plain values built on the stack and allocate nothing, so they do not
compete with the QR decoder for the heap.

Colours come from `Theme`, never named directly. Each fill is paired with the
foreground legible on it (`primary`/`on_primary`), so a call site cannot put
text on a background it cannot be read against.

## Adding a screen

1. Add a variant to `Screen` in `picosigner-ui/src/lib.rs`.
2. Add state to `AppState` (`picosigner/src/app.rs`) and the methods that
   transition into and out of it.
3. Declare its geometry in `picosigner/src/layout.rs` — a `MenuList`, a
   `KeyGrid`, or named `Rect`s. Do this once; the next two steps both read it.
4. Draw it in `picosigner/src/screens.rs` and add its arm to `draw_screen_body`.
5. Map touches to actions in `picosigner/src/touch.rs`, reading the **same**
   layout definitions the drawing uses. Never restate a coordinate.
6. Handle its actions in `dispatch` (`picosigner/src/dispatch.rs`). This is
   shared by firmware and emulator; there is no second copy to update.
7. Add a scene in `picosigner-emulator/src/scenes.rs` so it is covered by the
   golden-image tests, then `just bless`.

> **Known friction.** A screen is still named in several matches — its heading,
> its body, its hit-test, and back navigation — so adding one touches a few
> places. What no longer happens is the same *coordinate* being written twice:
> keyboards and menus come from `layout.rs`. Some screens still carry rect
> literals; convert them as you touch them.

## Memory

Static RAM is the binding constraint, not flash (849 KiB of 16 MiB used).

| Region | Bytes |
| --- | ---: |
| Total SRAM | 524,288 |
| `bss` — dominated by the shared 240x320 RGB565 framebuffer | ~153,636 |
| Heap (`picosigner/src/heap.rs`) | 131,072 |
| Core-1 decoder stack | 32,768 |

Two rules follow:

- **One full-screen buffer, ever.** The 153,600-byte framebuffer is shared
  between the UI and the camera through `ResourceArena` in `picosigner-board`.
  `claim()` hands out a single `ArenaLease` whose `Drop` releases it, and the
  lease borrows the arena, so the compiler prevents the UI and a DMA target from
  holding it simultaneously. Never allocate a second full-screen buffer.
- **The QR decoder needs the heap.** Its 160 x 160 label image uses 51,200
  bytes of the 128 KiB heap, plus variable metadata and transport allocations.
  Long-lived allocations must be released before a scan;
  see the explicit `drop(decoder)` in the firmware scan loop. Check `just size`
  after any change that allocates.

## Conventions

- Errors and logs must never contain entropy, mnemonics, seeds, extended private
  keys, complete PSBTs, or signatures.
- Wallet signing operates on the exact PSBT digest shown during review; the
  digest is rechecked immediately before signing.
- New Bitcoin logic goes in `picosigner-wallet`, never in the UI or app layer.
  Signing and review are kept in their own small modules so they can be audited
  without reading the rest of the crate; keep them that way.
- Prefer adding to a shared helper over adding a bespoke one-off. The UI's main
  problem historically has been per-feature code that duplicates an existing
  pattern with slightly different numbers.
