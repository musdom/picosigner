//! Screen layouts.
//!
//! Every screen's geometry is declared here, once, and read by both the
//! renderer and the hit-test. That is the point: these were previously written
//! twice — as a drawing loop and as separate inverse arithmetic in `action_at`
//! — with different numbers in each copy, so buttons could end up touchable
//! somewhere other than where they were drawn.

use picosigner_ui::{passphrase_keyboard, KeyGrid, MenuList, Rect};

/// The vertical row lists that make up most screens.
///
/// Rows are addressed by slot, so a screen that hides one leaves its slot empty
/// instead of shifting the rows below it.
pub mod menus {
    use super::MenuList;

    /// Load wallet: slot 1 is the saved-wallet list, shown only when one
    /// exists, which is why the visible rows skip a slot.
    pub const LOAD_WALLET: MenuList = MenuList::new(60, 52, 40);
    pub const IMPORT_WALLET: MenuList = MenuList::new(52, 56, 44);
    pub const MNEMONIC_LENGTH: MenuList = MenuList::new(80, 60, 44);
    pub const NETWORK: MenuList = MenuList::new(60, 52, 40);
    pub const DERIVATION: MenuList = MenuList::new(48, 56, 44);
    pub const HOME: MenuList = MenuList::new(60, 52, 40);
    /// Wallet size reuses the mnemonic-length geometry.
    pub const GENERATION_WORDS: MenuList = MNEMONIC_LENGTH;
}

/// The BIP39 letter keyboard: A-Z over six columns, then Erase and Cancel.
pub mod mnemonic {
    use super::*;

    /// Where the grid sits on a 240x320 screen.
    pub const BOUNDS: Rect = Rect::new(0, 160, 240, 160);
    pub const COLUMNS: u16 = 6;
    /// The 26 letters.
    pub const LETTERS: usize = 26;

    pub fn grid() -> KeyGrid {
        // 40x32 cells with the key filling all but a 1px right/bottom margin,
        // matching the layout this replaced.
        KeyGrid::new(LETTERS, COLUMNS).with_gaps(0, 0).with_trim(1)
    }

    /// The label for a letter key.
    pub fn label(index: usize) -> char {
        char::from(b'A' + index as u8)
    }

    /// The action row shares the grid's last row with Y and Z: Erase spans
    /// from here to [`CANCEL_LEFT`], and Cancel runs from there to the right
    /// edge. Both are drawn as buttons at these coordinates.
    pub const ERASE_LEFT: u16 = 80;
    pub const CANCEL_LEFT: u16 = 160;
    /// The action row's top edge and height.
    pub const ACTION_TOP: u16 = 288;
    pub const ACTION_HEIGHT: u16 = 31;
    pub const ACTION_WIDTH: u16 = 79;
}

/// The BIP39 passphrase keyboard: 26 characters plus backspace, shift, and
/// space, over six columns. The key semantics live in
/// [`picosigner_ui::passphrase_keyboard`] so the label and the character typed
/// cannot disagree.
pub mod passphrase {
    use super::*;

    pub const BOUNDS: Rect = Rect::new(0, 80, 240, 160);

    pub fn grid() -> KeyGrid {
        // Same 1px border as the mnemonic keyboard, so the two keyboards
        // match visually.
        KeyGrid::new(
            passphrase_keyboard::KEYS,
            passphrase_keyboard::COLUMNS as u16,
        )
        .with_gaps(0, 0)
        .with_trim(1)
    }
}

/// The D6 entropy keypad: faces 1-6 over three columns.
pub mod dice {
    use super::*;

    pub const BOUNDS: Rect = Rect::new(0, 60, 240, 120);
    pub const COLUMNS: u16 = 3;
    pub const FACES: usize = 6;

    pub fn grid() -> KeyGrid {
        // 80x60 cells with 4px side padding and the key sitting at the top of
        // its cell, matching the layout this replaced.
        KeyGrid::new(FACES, COLUMNS)
            .with_gaps(4, 4)
            .with_offset_y(-4)
    }

    /// The die face a key shows, which is also the roll it records.
    pub fn face(index: usize) -> u8 {
        index as u8 + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use picosigner_ui::Widget;

    /// Each keyboard must fit on the 240x320 panel.
    #[test]
    fn every_keyboard_fits_on_screen() {
        for (name, bounds) in [
            ("mnemonic", mnemonic::BOUNDS),
            ("passphrase", passphrase::BOUNDS),
            ("dice", dice::BOUNDS),
        ] {
            assert!(bounds.right() <= 240, "{name} is too wide");
            assert!(bounds.bottom() <= 320, "{name} runs off the bottom");
        }
    }

    /// Every key of every keyboard is touchable where it is drawn.
    #[test]
    fn every_key_is_touchable_where_it_is_drawn() {
        for (name, mut grid, bounds) in [
            ("mnemonic", mnemonic::grid(), mnemonic::BOUNDS),
            ("passphrase", passphrase::grid(), passphrase::BOUNDS),
            ("dice", dice::grid(), dice::BOUNDS),
        ] {
            grid.set_bounds(bounds);
            for index in 0..grid.len() {
                let rect = grid.key_rect(index).expect("laid out");
                assert!(!rect.is_empty(), "{name} key {index} has no area");
                assert_eq!(
                    grid.hit(rect.center_x(), rect.center_y()),
                    Some(index),
                    "{name} key {index} is not touchable at its centre"
                );
            }
        }
    }

    /// The menu definitions must reproduce the coordinates each screen used
    /// before it was migrated. These literals are the historical layout; if a
    /// row is meant to move, change it here deliberately and re-bless the
    /// golden image.
    #[test]
    fn menus_match_the_layout_they_replaced() {
        use picosigner_ui::menu_list::{ROW_LEFT, ROW_WIDTH};
        let cases: &[(&str, MenuList, &[(u16, u16, u16)])] = &[
            (
                "LoadWallet",
                menus::LOAD_WALLET,
                &[(0, 60, 40), (2, 164, 40), (3, 216, 40)],
            ),
            (
                "ImportWallet",
                menus::IMPORT_WALLET,
                &[(0, 52, 44), (1, 108, 44), (2, 164, 44)],
            ),
            (
                "MnemonicLength",
                menus::MNEMONIC_LENGTH,
                &[(0, 80, 44), (1, 140, 44)],
            ),
            (
                "Network",
                menus::NETWORK,
                &[(0, 60, 40), (1, 112, 40), (2, 164, 40)],
            ),
            (
                "Derivation",
                menus::DERIVATION,
                &[(0, 48, 44), (1, 104, 44), (2, 160, 44)],
            ),
            (
                "Home",
                menus::HOME,
                &[(0, 60, 40), (1, 112, 40), (2, 164, 40), (3, 216, 40)],
            ),
        ];
        for (name, menu, rows) in cases {
            for &(slot, top, height) in *rows {
                let row = menu.row(slot);
                assert_eq!(
                    (row.x, row.y, row.width, row.height),
                    (ROW_LEFT, top, ROW_WIDTH, height),
                    "{name} slot {slot} moved"
                );
            }
        }
    }

    #[test]
    fn dice_faces_run_one_to_six() {
        assert_eq!(dice::face(0), 1);
        assert_eq!(dice::face(dice::FACES - 1), 6);
    }

    #[test]
    fn mnemonic_labels_run_a_to_z() {
        assert_eq!(mnemonic::label(0), 'A');
        assert_eq!(mnemonic::label(mnemonic::LETTERS - 1), 'Z');
    }
}
