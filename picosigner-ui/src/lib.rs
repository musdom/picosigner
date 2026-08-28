#![no_std]

//! Host-testable UI primitives for the 240x320 PicoSigner display.
//! One shared framebuffer is owned by the renderer; views only receive clipped contexts.

pub mod key_grid;
pub mod menu_list;
pub mod theme;
pub mod widget;

pub use key_grid::{Key, KeyGrid, KeyStyle};
pub use menu_list::MenuList;
pub use widget::{fill_rect, Rect, Widget};

/// The on-screen keyboard used for BIP39 passphrase entry.
///
/// The key layout is defined once here because two places need it and they must
/// agree: the renderer draws these labels, and the app inserts the character
/// they name. If the two ever disagreed the keyboard would silently type
/// something other than what it shows.
pub mod passphrase_keyboard {
    /// Keys per row.
    pub const COLUMNS: usize = 6;
    /// Total keys, including the three action keys at the end.
    pub const KEYS: usize = 30;
    /// Number of shift modes: lowercase, uppercase, symbols.
    pub const MODES: u8 = 3;

    /// Index of the backspace key.
    pub const BACKSPACE: usize = 26;
    /// Index of the key cycling through [`MODES`].
    pub const SHIFT: usize = 27;
    /// Index of the space key, drawn double-wide across the last two grid
    /// cells so the keyboard has only one space button rather than two
    /// identical ones side by side.
    pub const SPACE: usize = 28;

    const SYMBOLS: &[u8; 26] = b"0123456789!@#$%^&*()-_=+[]";

    /// What a key does when pressed.
    pub enum Key {
        /// Insert this character.
        Char(u8),
        Backspace,
        /// Advance to the next shift mode.
        Shift,
        Space,
    }

    /// Resolves a key index in the given shift mode.
    ///
    /// `mode` is taken modulo [`MODES`], so an out-of-range mode cannot panic.
    pub fn key(index: usize, mode: u8) -> Key {
        match index {
            0..=25 => Key::Char(match mode % MODES {
                0 => b'a' + index as u8,
                1 => b'A' + index as u8,
                _ => SYMBOLS[index],
            }),
            BACKSPACE => Key::Backspace,
            SHIFT => Key::Shift,
            _ => Key::Space,
        }
    }

    /// The label drawn on a key. Action keys use a short glyph.
    pub fn label(index: usize, mode: u8) -> char {
        match key(index, mode) {
            Key::Char(byte) => byte as char,
            Key::Backspace => '<',
            // Show the mode the key switches *to*.
            Key::Shift => match mode % MODES {
                0 => 'A',
                1 => '#',
                _ => 'a',
            },
            Key::Space => '_',
        }
    }
}

pub const DISPLAY_WIDTH: u16 = 240;
pub const DISPLAY_HEIGHT: u16 = 320;
pub const FRAMEBUFFER_PIXELS: usize = DISPLAY_WIDTH as usize * DISPLAY_HEIGHT as usize;
pub const FRAMEBUFFER_BYTES: usize = FRAMEBUFFER_PIXELS * core::mem::size_of::<u16>();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiEvent {
    TouchDown { x: u16, y: u16 },
    TouchUp { x: u16, y: u16 },
    Tick,
    Back,
    Confirm,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    Tap,
    Hold,
    SwipeLeft,
    SwipeRight,
    SwipeUp,
    SwipeDown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GestureEvent {
    pub gesture: Gesture,
    pub x: u16,
    pub y: u16,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GestureConfig {
    pub hold_ticks: u16,
    pub swipe_pixels: u16,
}
impl Default for GestureConfig {
    fn default() -> Self {
        Self {
            hold_ticks: 30,
            swipe_pixels: 32,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct TouchTracker {
    start: Option<(u16, u16)>,
    ticks: u16,
    config: GestureConfig,
}
impl TouchTracker {
    pub const fn new(config: GestureConfig) -> Self {
        Self {
            start: None,
            ticks: 0,
            config,
        }
    }

    /// Returns the current hold duration, capped at the configured
    /// confirmation duration. This is intended for rendering a live progress
    /// indicator while the finger remains down.
    pub fn hold_progress(&self) -> (u16, u16) {
        (
            self.ticks.min(self.config.hold_ticks),
            self.config.hold_ticks,
        )
    }

    pub fn active(&self) -> bool {
        self.start.is_some()
    }
    pub fn event(&mut self, event: UiEvent) -> Option<GestureEvent> {
        match event {
            UiEvent::TouchDown { x, y } => {
                self.start = Some((x, y));
                self.ticks = 0;
                None
            }
            UiEvent::Tick if self.start.is_some() => {
                self.ticks = self.ticks.saturating_add(1);
                None
            }
            UiEvent::TouchUp { x, y } => {
                let (sx, sy) = self.start.take()?;
                let dx = x as i32 - sx as i32;
                let dy = y as i32 - sy as i32;
                let gesture = if self.ticks >= self.config.hold_ticks
                    && dx.abs() < self.config.swipe_pixels as i32
                    && dy.abs() < self.config.swipe_pixels as i32
                {
                    Gesture::Hold
                } else if dx.abs() >= self.config.swipe_pixels as i32 && dx.abs() > dy.abs() {
                    if dx < 0 {
                        Gesture::SwipeLeft
                    } else {
                        Gesture::SwipeRight
                    }
                } else if dy.abs() >= self.config.swipe_pixels as i32 {
                    if dy < 0 {
                        Gesture::SwipeUp
                    } else {
                        Gesture::SwipeDown
                    }
                } else {
                    Gesture::Tap
                };
                self.ticks = 0;
                Some(GestureEvent { gesture, x, y })
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Boot,
    LoadWallet,
    WalletList,
    ImportWallet,
    SeedQrScan,
    MnemonicLength,
    MnemonicEntry,
    FrostThreshold,
    FrostShareIndex,
    FrostMnemonicEntry,
    FrostMnemonicConfirm,
    Network,
    Derivation,
    PassphraseChoice,
    PassphraseEntry,
    GenerationWords,
    EntropySources,
    DiceEntropy,
    CameraEntropy,
    Deriving,
    Seed,
    Backup,
    BackupMenu,
    BackupWords,
    BackupSeedQr,
    Home,
    DeleteWallet,
    CancelEntryConfirm,
    Descriptor,
    Scan,
    Review,
    Sign,
    Signed,
    Recovery,
}
pub trait Navigator {
    fn current(&self) -> Screen;
    fn navigate(&mut self, screen: Screen);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The label drawn on a character key must be the character it types.
    /// These were once two independent tables and could silently disagree.
    #[test]
    fn passphrase_key_labels_match_what_they_type() {
        use passphrase_keyboard::{key, label, Key, KEYS, MODES};
        for mode in 0..MODES {
            for index in 0..KEYS {
                if let Key::Char(byte) = key(index, mode) {
                    assert_eq!(
                        byte as char,
                        label(index, mode),
                        "key {index} in mode {mode} types something other than its label"
                    );
                }
            }
        }
    }

    /// Every key index resolves, and the action keys stay where the hit-test
    /// expects them.
    #[test]
    fn passphrase_keyboard_covers_every_index() {
        use passphrase_keyboard::{key, Key, BACKSPACE, KEYS, MODES, SHIFT};
        assert!(matches!(key(BACKSPACE, 0), Key::Backspace));
        assert!(matches!(key(SHIFT, 0), Key::Shift));
        for index in 0..KEYS {
            for mode in 0..MODES {
                // Must not panic for any valid index, in any mode.
                let _ = key(index, mode);
            }
        }
        // An out-of-range mode is clamped rather than panicking.
        let _ = key(0, MODES + 7);
    }
    #[test]
    fn gestures_work() {
        let mut g = TouchTracker::new(GestureConfig {
            hold_ticks: 2,
            swipe_pixels: 10,
        });
        g.event(UiEvent::TouchDown { x: 20, y: 20 });
        assert_eq!(g.hold_progress(), (0, 2));
        assert_eq!(
            g.event(UiEvent::TouchUp { x: 20, y: 20 }).unwrap().gesture,
            Gesture::Tap
        );
        assert_eq!(g.hold_progress(), (0, 2));
        g.event(UiEvent::TouchDown { x: 20, y: 20 });
        g.event(UiEvent::Tick);
        g.event(UiEvent::Tick);
        assert_eq!(g.hold_progress(), (2, 2));
        assert_eq!(
            g.event(UiEvent::TouchUp { x: 20, y: 20 }).unwrap().gesture,
            Gesture::Hold
        );
        g.event(UiEvent::TouchDown { x: 20, y: 20 });
        g.event(UiEvent::Tick);
        g.event(UiEvent::Tick);
        assert_eq!(
            g.event(UiEvent::TouchUp { x: 20, y: 60 }).unwrap().gesture,
            Gesture::SwipeDown
        );
        g.event(UiEvent::TouchDown { x: 20, y: 60 });
        assert_eq!(
            g.event(UiEvent::TouchUp { x: 20, y: 20 }).unwrap().gesture,
            Gesture::SwipeUp
        );
    }
}
