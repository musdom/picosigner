//! Single source of truth for PicoSigner's colours.
//!
//! Every screen draws from these constants so the firmware and the emulator
//! stay in step and no call site invents a one-off shade. The panel is RGB565,
//! so the channels are written directly in its 5/6/5 range; the comment on each
//! constant records the 24-bit brand colour it was chosen from and the colour
//! the panel actually displays after quantisation.

use embedded_graphics::pixelcolor::Rgb565;

/// App background. Everything is drawn onto this. `#000000`
pub const BACKGROUND: Rgb565 = Rgb565::new(0, 0, 0);

/// Default text, and the label on top of every filled button. `#ffffff`
pub const TEXT: Rgb565 = Rgb565::new(31, 63, 31);

/// Primary action: menu button backgrounds. `#0057a8` -> `#0055ac`
pub const PRIMARY: Rgb565 = Rgb565::new(2, 18, 11);

/// Secondary action: destructive operations such as deleting a wallet.
/// `#ad0b0b` -> `#ac0808`
pub const SECONDARY: Rgb565 = Rgb565::new(21, 2, 1);

/// Supporting text that is quieter than [`TEXT`] but still legible on black.
/// `#9aa4b0` -> `#9ca5b4`
pub const TEXT_MUTED: Rgb565 = Rgb565::new(19, 41, 22);

/// Warnings and failure notices, matched to [`SECONDARY`] but lifted for
/// contrast against the black background. `#e94b4b` -> `#ee484a`
pub const DANGER_TEXT: Rgb565 = Rgb565::new(29, 18, 9);

/// Progress fill for hold-to-confirm bars, and the outline that frames them.
pub const PROGRESS: Rgb565 = PRIMARY;
/// `#54606d` -> `#52616a`
pub const PROGRESS_OUTLINE: Rgb565 = Rgb565::new(10, 24, 13);

/// QR symbols are always drawn as true black on true white so scanners get
/// maximum contrast regardless of the surrounding theme.
pub const QR_LIGHT: Rgb565 = Rgb565::new(31, 63, 31);
pub const QR_DARK: Rgb565 = Rgb565::new(0, 0, 0);

/// Alternating rows in the wallet list. `#12181f` -> `#101818`
pub const SURFACE: Rgb565 = Rgb565::new(2, 6, 3);
/// `#1b232c` -> `#182029`
pub const SURFACE_ALT: Rgb565 = Rgb565::new(3, 8, 5);
