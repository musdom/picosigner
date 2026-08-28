//! The widget contract.
//!
//! A widget owns its bounds, draws itself, and hit-tests itself. Keeping those
//! three together is the point: picosigner's UI previously declared each
//! screen's geometry twice — once in the renderer and again in a separate
//! hit-test function — and the two drifted, which is how buttons ended up with
//! touch targets that did not match what was drawn.
//!
//! Widgets are laid out top-down: a parent calls [`Widget::set_bounds`] with
//! the area a child may use, and the child remembers it. Drawing and
//! hit-testing then both read that same stored rectangle, so they cannot
//! disagree.

use embedded_graphics::{
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
};

/// A rectangle in display coordinates.
///
/// Display space is 240x320 and never negative, so these are unsigned. The
/// `embedded-graphics` conversion clamps rather than wrapping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Rect {
    pub const fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// An empty rectangle, used as the "not laid out yet" value.
    pub const ZERO: Self = Self::new(0, 0, 0, 0);

    pub const fn right(&self) -> u16 {
        self.x + self.width
    }

    pub const fn bottom(&self) -> u16 {
        self.y + self.height
    }

    pub const fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Whether a touch at `(x, y)` falls inside.
    ///
    /// An empty rectangle contains nothing, so a widget that was never laid out
    /// cannot swallow touches.
    pub const fn contains(&self, x: u16, y: u16) -> bool {
        !self.is_empty() && x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// The centre point, for centring text or an icon.
    pub const fn center_x(&self) -> u16 {
        self.x + self.width / 2
    }

    pub const fn center_y(&self) -> u16 {
        self.y + self.height / 2
    }

    /// Shrinks by `amount` on every side, saturating at empty.
    pub const fn inset(&self, amount: u16) -> Self {
        let inset_by = amount * 2;
        if self.width <= inset_by || self.height <= inset_by {
            return Self::new(self.x, self.y, 0, 0);
        }
        Self::new(
            self.x + amount,
            self.y + amount,
            self.width - inset_by,
            self.height - inset_by,
        )
    }

    /// Splits `height` pixels off the top, returning `(top, rest)`.
    pub const fn split_top(&self, height: u16) -> (Self, Self) {
        if height >= self.height {
            return (*self, Self::new(self.x, self.bottom(), self.width, 0));
        }
        (
            Self::new(self.x, self.y, self.width, height),
            Self::new(self.x, self.y + height, self.width, self.height - height),
        )
    }

    /// Splits `height` pixels off the bottom, returning `(rest, bottom)`.
    pub const fn split_bottom(&self, height: u16) -> (Self, Self) {
        if height >= self.height {
            return (Self::new(self.x, self.y, self.width, 0), *self);
        }
        (
            Self::new(self.x, self.y, self.width, self.height - height),
            Self::new(self.x, self.y + self.height - height, self.width, height),
        )
    }

    /// The `row`th of `count` equal rows, including any remainder in the last.
    pub const fn row(&self, row: u16, count: u16) -> Self {
        if count == 0 {
            return Self::new(self.x, self.y, self.width, 0);
        }
        let each = self.height / count;
        let top = self.y + each * row;
        let height = if row + 1 == count {
            self.height - each * row
        } else {
            each
        };
        Self::new(self.x, top, self.width, height)
    }

    /// The `column`th of `count` equal columns.
    pub const fn column(&self, column: u16, count: u16) -> Self {
        if count == 0 {
            return Self::new(self.x, self.y, 0, self.height);
        }
        let each = self.width / count;
        let left = self.x + each * column;
        let width = if column + 1 == count {
            self.width - each * column
        } else {
            each
        };
        Self::new(left, self.y, width, self.height)
    }
}

impl From<Rect> for Rectangle {
    fn from(rect: Rect) -> Self {
        Rectangle::new(
            Point::new(i32::from(rect.x), i32::from(rect.y)),
            Size::new(u32::from(rect.width), u32::from(rect.height)),
        )
    }
}

/// Fills a rectangle. Wraps the `embedded-graphics` incantation that appears
/// throughout the UI.
pub fn fill_rect<D, C>(target: &mut D, rect: Rect, color: C) -> Result<(), D::Error>
where
    D: DrawTarget<Color = C>,
    C: PixelColor,
{
    Rectangle::from(rect)
        .into_styled(PrimitiveStyle::with_fill(color))
        .draw(target)
}

/// A drawable, touchable piece of UI that owns its own geometry.
///
/// Implementors store the [`Rect`] given to [`set_bounds`](Widget::set_bounds)
/// and use it for both [`draw`](Widget::draw) and
/// [`hit`](Widget::hit). Deriving both from one stored value is what makes it
/// impossible for the drawn button and its touch target to disagree.
pub trait Widget {
    /// What a touch on this widget produces.
    type Action;

    /// Assigns the area this widget may draw into.
    ///
    /// Called before `draw` or `hit`. A widget that is never given bounds must
    /// draw nothing and hit nothing.
    fn set_bounds(&mut self, bounds: Rect);

    /// The area assigned by the last `set_bounds`.
    fn bounds(&self) -> Rect;

    /// Returns the action for a touch at `(x, y)`, or `None` if it missed.
    ///
    /// The default tests the widget's own bounds; container widgets override
    /// this to dispatch to children.
    fn hit(&self, x: u16, y: u16) -> Option<Self::Action>;

    /// Draws the widget within its bounds.
    fn draw<D, C>(&self, target: &mut D, theme: &crate::theme::Theme<C>) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = C>,
        C: PixelColor;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_excludes_the_far_edges() {
        let rect = Rect::new(10, 20, 30, 40);
        assert!(rect.contains(10, 20), "top-left corner is inside");
        assert!(rect.contains(39, 59), "last pixel is inside");
        assert!(!rect.contains(40, 59), "right edge is outside");
        assert!(!rect.contains(39, 60), "bottom edge is outside");
        assert!(!rect.contains(9, 20));
    }

    /// A widget that was never laid out must not swallow touches.
    #[test]
    fn an_empty_rect_contains_nothing() {
        assert!(!Rect::ZERO.contains(0, 0));
        assert!(!Rect::new(5, 5, 0, 10).contains(5, 5));
        assert!(!Rect::new(5, 5, 10, 0).contains(5, 5));
    }

    #[test]
    fn inset_saturates_instead_of_underflowing() {
        assert_eq!(Rect::new(0, 0, 10, 10).inset(2), Rect::new(2, 2, 6, 6));
        assert!(Rect::new(0, 0, 4, 10).inset(2).is_empty());
        assert!(Rect::new(0, 0, 10, 10).inset(50).is_empty());
    }

    #[test]
    fn splits_partition_the_original_exactly() {
        let rect = Rect::new(0, 0, 240, 320);
        let (top, rest) = rect.split_top(50);
        assert_eq!(top, Rect::new(0, 0, 240, 50));
        assert_eq!(rest, Rect::new(0, 50, 240, 270));
        assert_eq!(top.height + rest.height, rect.height);

        let (body, footer) = rect.split_bottom(50);
        assert_eq!(footer, Rect::new(0, 270, 240, 50));
        assert_eq!(body.height + footer.height, rect.height);
    }

    /// Splitting off more than exists must not wrap around.
    #[test]
    fn oversized_splits_are_clamped() {
        let rect = Rect::new(0, 0, 240, 100);
        let (top, rest) = rect.split_top(500);
        assert_eq!(top, rect);
        assert!(rest.is_empty());

        let (body, bottom) = rect.split_bottom(500);
        assert!(body.is_empty());
        assert_eq!(bottom, rect);
    }

    /// Rows and columns must tile their parent with no gap and no overlap,
    /// including when the size does not divide evenly.
    #[test]
    fn rows_and_columns_tile_exactly() {
        let rect = Rect::new(0, 0, 240, 100);
        for count in 1..=7u16 {
            let mut covered = 0;
            for index in 0..count {
                let row = rect.row(index, count);
                assert_eq!(row.y, rect.y + covered, "row {index} of {count} has a gap");
                covered += row.height;
            }
            assert_eq!(covered, rect.height, "rows of {count} do not fill");

            let mut across = 0;
            for index in 0..count {
                let column = rect.column(index, count);
                assert_eq!(column.x, rect.x + across);
                across += column.width;
            }
            assert_eq!(across, rect.width);
        }
    }
}
