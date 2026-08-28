//! A grid of keys.
//!
//! picosigner has three on-screen keyboards — the BIP39 letter keyboard, the
//! passphrase keyboard, and the dice keypad — which were each written twice:
//! once as a drawing loop and once as the inverse arithmetic in a hit-test
//! function far away. The two copies used different cell sizes and drifted.
//!
//! [`KeyGrid`] states the geometry once. Both the drawing and the hit-testing
//! read the same cell rectangles, so a key is always touchable exactly where it
//! is drawn.

use crate::{
    theme::Theme,
    widget::{fill_rect, Rect, Widget},
};
use embedded_graphics::prelude::{DrawTarget, PixelColor};

/// How a key is filled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyStyle {
    /// The primary action fill.
    Primary,
    /// A raised surface, for keys that are secondary to the main set.
    Surface,
    /// The destructive fill, for erase and cancel.
    Danger,
}

/// One key: what it looks like and what it produces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key<A> {
    /// The action a touch produces.
    pub action: A,
    pub style: KeyStyle,
}

impl<A> Key<A> {
    pub const fn new(action: A) -> Self {
        Self {
            action,
            style: KeyStyle::Primary,
        }
    }

    pub const fn styled(action: A, style: KeyStyle) -> Self {
        Self { action, style }
    }
}

/// A fixed-column grid of equally sized keys.
///
/// Keys fill row-major from the top-left. The gaps are the padding left inside
/// each cell, so the drawn key is smaller than the cell it hit-tests to and a
/// touch near an edge still registers on the intended key.
#[derive(Clone, Debug)]
pub struct KeyGrid {
    bounds: Rect,
    columns: u16,
    rows: u16,
    count: usize,
    gap_x: u16,
    gap_y: u16,
    /// Shifts the drawn key within its cell, for layouts whose padding is not
    /// vertically centred.
    offset_y: i16,
    /// Trims the drawn key's right and bottom edges, for layouts that separate
    /// keys with a margin on one side rather than padding on both.
    trim: u16,
}

impl KeyGrid {
    /// Creates a grid holding `count` keys, `columns` wide.
    ///
    /// The row count follows from `count`, so callers cannot state a row count
    /// that disagrees with the number of keys.
    pub fn new(count: usize, columns: u16) -> Self {
        let columns = columns.max(1);
        let rows = count.div_ceil(usize::from(columns)).max(1) as u16;
        Self {
            bounds: Rect::ZERO,
            columns,
            rows,
            count,
            gap_x: 1,
            gap_y: 1,
            offset_y: 0,
            trim: 0,
        }
    }

    /// Sets the padding left inside each cell, horizontally and vertically.
    pub fn with_gap(mut self, gap: u16) -> Self {
        self.gap_x = gap;
        self.gap_y = gap;
        self
    }

    /// Sets the horizontal and vertical padding separately.
    pub fn with_gaps(mut self, gap_x: u16, gap_y: u16) -> Self {
        self.gap_x = gap_x;
        self.gap_y = gap_y;
        self
    }

    /// Nudges the drawn key up (negative) or down within its cell.
    ///
    /// Hit-testing still uses the whole cell, so this changes only appearance.
    pub fn with_offset_y(mut self, offset_y: i16) -> Self {
        self.offset_y = offset_y;
        self
    }

    /// Trims the drawn key's right and bottom edges by `trim` pixels, leaving a
    /// margin on one side rather than padding on both.
    ///
    /// Hit-testing still uses the whole cell, so the margin is not dead space.
    pub fn with_trim(mut self, trim: u16) -> Self {
        self.trim = trim;
        self
    }

    pub fn columns(&self) -> u16 {
        self.columns
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The cell a key occupies, before the gap is applied.
    ///
    /// Returns `None` for an index past the end, so a partly filled last row
    /// has no phantom keys.
    pub fn cell(&self, index: usize) -> Option<Rect> {
        if index >= self.count || self.bounds.is_empty() {
            return None;
        }
        let column = (index % usize::from(self.columns)) as u16;
        let row = (index / usize::from(self.columns)) as u16;
        Some(self.bounds.row(row, self.rows).column(column, self.columns))
    }

    /// The rectangle a key is actually drawn in: its cell, less the gaps.
    pub fn key_rect(&self, index: usize) -> Option<Rect> {
        let cell = self.cell(index)?;
        let inset_x = self.gap_x * 2;
        let inset_y = self.gap_y * 2;
        if cell.width <= inset_x || cell.height <= inset_y {
            return Some(Rect::new(cell.x, cell.y, 0, 0));
        }
        let y = i32::from(cell.y) + i32::from(self.gap_y) + i32::from(self.offset_y);
        let width = (cell.width - inset_x).saturating_sub(self.trim);
        let height = (cell.height - inset_y).saturating_sub(self.trim);
        Some(Rect::new(
            cell.x + self.gap_x,
            y.clamp(0, i32::from(u16::MAX)) as u16,
            width,
            height,
        ))
    }

    /// The key index at `(x, y)`, or `None`.
    ///
    /// Hit-testing uses the *cell*, not the inset key rectangle: the gap
    /// belongs to the nearer key rather than being dead space.
    pub fn index_at(&self, x: u16, y: u16) -> Option<usize> {
        if !self.bounds.contains(x, y) {
            return None;
        }
        // Derived from the same `row`/`column` split used to lay the keys out,
        // by searching rather than by inverse arithmetic, so uneven division
        // cannot make the two disagree.
        (0..self.count).find(|&index| self.cell(index).is_some_and(|cell| cell.contains(x, y)))
    }

    /// Draws every key, taking each key's definition from `key_at`.
    ///
    /// The label is drawn by `draw_label`, which receives the key's rectangle;
    /// this crate has no font, so text rendering stays with the caller.
    pub fn draw_keys<D, C, A, F, L>(
        &self,
        target: &mut D,
        theme: &Theme<C>,
        key_at: F,
        mut draw_label: L,
    ) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = C>,
        C: PixelColor,
        F: Fn(usize) -> Option<Key<A>>,
        L: FnMut(&mut D, usize, Rect, C) -> Result<(), D::Error>,
    {
        for index in 0..self.count {
            let (Some(rect), Some(key)) = (self.key_rect(index), key_at(index)) else {
                continue;
            };
            if rect.is_empty() {
                continue;
            }
            let pair = match key.style {
                KeyStyle::Primary => theme.primary,
                KeyStyle::Surface => theme.surface,
                KeyStyle::Danger => theme.secondary,
            };
            fill_rect(target, rect, pair.surface)?;
            draw_label(target, index, rect, pair.on_surface)?;
        }
        Ok(())
    }
}

impl Widget for KeyGrid {
    /// The index of the key touched.
    type Action = usize;

    fn set_bounds(&mut self, bounds: Rect) {
        self.bounds = bounds;
    }

    fn bounds(&self) -> Rect {
        self.bounds
    }

    fn hit(&self, x: u16, y: u16) -> Option<usize> {
        self.index_at(x, y)
    }

    /// Filling keys needs each key's style and label, which only the caller
    /// knows; use [`draw_keys`](KeyGrid::draw_keys).
    fn draw<D, C>(&self, _target: &mut D, _theme: &Theme<C>) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = C>,
        C: PixelColor,
    {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(count: usize, columns: u16, bounds: Rect) -> KeyGrid {
        let mut grid = KeyGrid::new(count, columns);
        grid.set_bounds(bounds);
        grid
    }

    /// The property the old code could not guarantee: every pixel of a key's
    /// drawn rectangle hit-tests back to that same key.
    #[test]
    fn every_drawn_key_hit_tests_to_itself() {
        // The three real keyboards: BIP39 letters, passphrase, dice.
        for (count, columns, bounds) in [
            (26usize, 6u16, Rect::new(0, 160, 240, 160)),
            (30, 6, Rect::new(0, 80, 240, 160)),
            (6, 3, Rect::new(0, 60, 240, 120)),
        ] {
            let grid = grid(count, columns, bounds);
            for index in 0..count {
                let rect = grid.key_rect(index).expect("key laid out");
                for (x, y) in [
                    (rect.x, rect.y),
                    (rect.right() - 1, rect.y),
                    (rect.x, rect.bottom() - 1),
                    (rect.right() - 1, rect.bottom() - 1),
                    (rect.center_x(), rect.center_y()),
                ] {
                    assert_eq!(
                        grid.index_at(x, y),
                        Some(index),
                        "key {index} of {count} is not touchable at ({x}, {y})"
                    );
                }
            }
        }
    }

    /// A partly filled last row must not create phantom keys.
    #[test]
    fn indices_past_the_end_do_not_exist() {
        let grid = grid(26, 6, Rect::new(0, 160, 240, 160));
        assert_eq!(grid.rows(), 5, "26 keys over 6 columns needs 5 rows");
        assert!(grid.key_rect(26).is_none());
        // The last row has 2 keys; the rest of that row is dead space.
        let last = grid.key_rect(25).expect("key 25 exists");
        assert_eq!(grid.index_at(last.right() + 40, last.center_y()), None);
    }

    #[test]
    fn touches_outside_the_grid_miss() {
        let grid = grid(30, 6, Rect::new(0, 80, 240, 160));
        assert_eq!(grid.index_at(120, 79), None, "above the grid");
        assert_eq!(grid.index_at(120, 240), None, "below the grid");
    }

    /// A grid that was never laid out must not claim touches.
    #[test]
    fn an_unbounded_grid_hits_nothing() {
        let grid = KeyGrid::new(26, 6);
        assert_eq!(grid.index_at(0, 0), None);
        assert!(grid.key_rect(0).is_none());
    }

    /// Keys are laid out row-major, matching how the labels are indexed.
    #[test]
    fn keys_fill_row_major() {
        let grid = grid(30, 6, Rect::new(0, 80, 240, 160));
        let first = grid.key_rect(0).unwrap();
        let second = grid.key_rect(1).unwrap();
        let next_row = grid.key_rect(6).unwrap();
        assert!(second.x > first.x, "index 1 is to the right of index 0");
        assert_eq!(second.y, first.y, "index 1 shares a row with index 0");
        assert!(next_row.y > first.y, "index 6 starts the next row");
        assert_eq!(next_row.x, first.x);
    }

    #[test]
    fn the_gap_belongs_to_the_nearest_key() {
        let grid = grid(6, 3, Rect::new(0, 60, 240, 120)).with_gap(4);
        let cell = grid.cell(0).unwrap();
        let key = grid.key_rect(0).unwrap();
        assert!(key.width < cell.width, "the key is inset from its cell");
        // A touch in the gap still selects the key whose cell it is in.
        assert_eq!(grid.index_at(cell.x, cell.y), Some(0));
    }
}
