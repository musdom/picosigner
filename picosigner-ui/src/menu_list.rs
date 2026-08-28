//! Evenly spaced full-width menu rows.
//!
//! Most picosigner screens are a vertical list of tappable rows. Each screen
//! previously wrote its rows' coordinates as literals twice — in the renderer
//! and again in the hit-test — so `(16, 80, 208, 44)` and its neighbours appear
//! dozens of times across the UI with no name and no shared definition.
//!
//! [`MenuList`] states a screen's row geometry once. Rows are addressed by
//! slot, so a screen that hides a row leaves its slot empty rather than
//! renumbering the rows below it.

use crate::widget::Rect;

/// The horizontal inset and width shared by every menu row.
pub const ROW_LEFT: u16 = 16;
pub const ROW_WIDTH: u16 = 208;

/// A column of equally spaced rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuList {
    top: u16,
    pitch: u16,
    height: u16,
    left: u16,
    width: u16,
}

impl MenuList {
    /// A list whose first row starts at `top`, each row `height` tall, with
    /// `pitch` pixels between the tops of consecutive rows.
    pub const fn new(top: u16, pitch: u16, height: u16) -> Self {
        Self {
            top,
            pitch,
            height,
            left: ROW_LEFT,
            width: ROW_WIDTH,
        }
    }

    /// Overrides the default horizontal inset and width.
    pub const fn with_span(mut self, left: u16, width: u16) -> Self {
        self.left = left;
        self.width = width;
        self
    }

    /// The rectangle of the row in `slot`.
    ///
    /// Slots are positions, not occupancy: a screen that shows rows in slots 0,
    /// 2, and 3 asks for exactly those, and the row it omits leaves a gap
    /// rather than shifting the others up.
    pub const fn row(&self, slot: u16) -> Rect {
        Rect::new(
            self.left,
            self.top + self.pitch * slot,
            self.width,
            self.height,
        )
    }

    /// The slot at `(x, y)`, considering only `slots`.
    ///
    /// Returns `None` for a touch in the gap between rows or outside the list,
    /// so the spacing is not silently tappable.
    pub fn slot_at(&self, x: u16, y: u16, slots: &[u16]) -> Option<u16> {
        slots
            .iter()
            .copied()
            .find(|&slot| self.row(slot).contains(x, y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every row is touchable exactly where it is drawn.
    #[test]
    fn rows_hit_test_to_themselves() {
        let list = MenuList::new(60, 52, 40);
        for slot in 0..4 {
            let row = list.row(slot);
            for (x, y) in [
                (row.x, row.y),
                (row.right() - 1, row.bottom() - 1),
                (row.center_x(), row.center_y()),
            ] {
                assert_eq!(list.slot_at(x, y, &[0, 1, 2, 3]), Some(slot));
            }
        }
    }

    /// The gap between rows belongs to neither.
    #[test]
    fn the_gap_between_rows_is_not_tappable() {
        let list = MenuList::new(60, 52, 40);
        let first = list.row(0);
        assert_eq!(list.slot_at(120, first.bottom(), &[0, 1]), None);
    }

    /// A screen that hides a row keeps the others where they were.
    #[test]
    fn omitting_a_slot_does_not_move_the_rest() {
        let list = MenuList::new(60, 52, 40);
        // LoadWallet shows slots 0, 2, and 3; slot 1 is the hidden wallet list.
        assert_eq!(list.row(0).y, 60);
        assert_eq!(list.row(2).y, 164);
        assert_eq!(list.row(3).y, 216);
        assert_eq!(list.slot_at(120, list.row(1).center_y(), &[0, 2, 3]), None);
    }

    #[test]
    fn touches_outside_the_list_miss() {
        let list = MenuList::new(60, 52, 40);
        assert_eq!(list.slot_at(120, 59, &[0]), None);
        assert_eq!(list.slot_at(0, 70, &[0]), None, "left of the inset");
    }
}
