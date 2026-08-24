# PicoSigner UI guide

Keep bottom confirmation actions visually consistent across the application:

- Use the shared black-background confirmation footer: an X icon in the left action area and a check icon plus label in the right action area.
- For security-sensitive or destructive actions, label the right action `Long press` and show the shared hold-progress bar.
- For ordinary review confirmations, keep the same unfilled icon-and-label layout and activate the action with a normal tap. Do not replace this footer with filled side-by-side buttons.
- Label actions with their matching outline icons. In particular, use the edit SVG icon beside `Edit` and the checkmark icon beside `Confirm`; reserve the X icon for cancel or back.
- Use shared footer renderers and named touch-bound constants so touch targets and visuals remain aligned on the device and emulator.

When changing UI behavior, update both firmware and emulator event handling, add touch/state tests, and inspect a 240x320 emulator screenshot for clipping.
