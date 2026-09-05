# UI guide

Conventions for the 240x320 portrait display. See
[architecture.md](architecture.md) for where UI code lives and
[testing.md](testing.md) for how to verify a change.

## Confirmation footers

Keep bottom confirmation actions visually consistent across the application:

- Use the shared black-background confirmation footer: a navigation icon in the left
  action area and a check icon plus label in the right action area.
- For security-sensitive or destructive actions, label the right action
  `Long press` and show the shared hold-progress bar.
- For ordinary review confirmations, keep the same unfilled icon-and-label
  layout and activate the action with a normal tap. Do not replace this footer
  with filled side-by-side buttons.
- Label actions with their matching outline icons. In particular, use the edit
  SVG icon beside `Edit` and the checkmark icon beside `Confirm`; reserve the X
  icon for returning to Home, the wallet loader or wallet list, or cancelling
  a flow. Use X even when one of those is the previous page. Use a `<` chevron
  for other returns to the previous page, including dismissing a cancellation
  confirmation and returning to entry.
- Use shared footer renderers and named touch-bound constants so touch targets
  and visuals remain aligned on the device and emulator.

All three footers now go through one `draw_footer_action` primitive, so they
share a single visual treatment. The icons sit at different x-positions in each
because the action areas differ in width and an icon with a label beside it is
offset left to make room; that is deliberate, not drift.

## Geometry

Touch bounds and drawing must come from the **same** source, never from two
hand-copied coordinate literals.

Declare it in `picosigner/src/layout.rs` and read it from both
`picosigner/src/screens.rs` (drawing) and `picosigner/src/touch.rs`
(hit-testing). `MenuList` covers row lists and `KeyGrid` covers keyboards; for
anything else, a named `Rect`.

Where a screen still declares its geometry twice — a rect literal in both the
drawing and the hit-test — treat that as a bug to fix rather than a pattern to
copy.

## Changing UI behavior

- Update event handling once, in `picosigner/src/dispatch.rs`. The firmware and
  the emulator share it; neither has its own copy.
- Add touch/state tests.
- Run `just test`. Golden-image tests render every screen and will fail on any
  visual change; if the change is intentional, `just bless` and review the diff
  before committing.
