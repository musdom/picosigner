//! Colour roles.
//!
//! Widgets never name a colour; they name a *role* and the theme supplies it.
//! Every surface role is paired with the foreground that belongs on it — a
//! button carries `primary`/`on_primary` together — so a widget cannot draw
//! text in a colour that is unreadable against the fill it just painted.
//!
//! [`Theme`] is generic over the colour type so this crate stays free of any
//! particular pixel format; the firmware instantiates it with `Rgb565`.

/// A surface colour and the foreground that is legible on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pair<C> {
    /// The fill.
    pub surface: C,
    /// Text and icons drawn on that fill.
    pub on_surface: C,
}

impl<C: Copy> Pair<C> {
    pub const fn new(surface: C, on_surface: C) -> Self {
        Self {
            surface,
            on_surface,
        }
    }
}

/// The colour roles every widget draws from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme<C> {
    /// The page background.
    pub background: C,
    /// Default text on [`background`](Self::background).
    pub text: C,
    /// Supporting text: quieter, still legible.
    pub text_muted: C,
    /// Warnings and failures.
    pub danger_text: C,
    /// Primary action fill, e.g. a keyboard key.
    pub primary: Pair<C>,
    /// Destructive action fill.
    pub secondary: Pair<C>,
    /// A raised surface, e.g. an alternate key style.
    pub surface: Pair<C>,
    /// Hold-to-confirm progress fill and its outline.
    pub progress: C,
    pub progress_outline: C,
}
