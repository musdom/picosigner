//! QR symbol construction shared by every encoder in this crate.
//!
//! picosigner generates three fixed-size QR symbols — a Compact SeedQR, a v5
//! signed-PSBT export, and a v8 descriptor — and each once carried its own copy
//! of the finder, alignment, format, and codeword-placement logic. The copies
//! were identical apart from a hardcoded side length, so a fix to one would
//! silently miss the others.
//!
//! These routines take `side` as a parameter and operate on plain slices, so a
//! single implementation serves all three symbol sizes.
//!
//! Everything here follows ISO/IEC 18004. The encoders remain
//! cross-checked against the `qrcode` crate in this crate's tests.

/// Index of module `(x, y)` in a row-major symbol of the given side.
pub fn index(side: usize, x: usize, y: usize) -> usize {
    y * side + x
}

/// Sets a module and marks it reserved, so codeword placement skips it.
pub fn set_function(
    modules: &mut [bool],
    reserved: &mut [bool],
    side: usize,
    x: usize,
    y: usize,
    value: bool,
) {
    let index = index(side, x, y);
    modules[index] = value;
    reserved[index] = true;
}

/// Draws one 7x7 finder pattern with its separator, clipped to the symbol.
///
/// `left` and `top` are the pattern's top-left corner; the loop runs one module
/// wider on every side to lay the separator, discarding anything off-symbol.
pub fn finder(modules: &mut [bool], reserved: &mut [bool], side: usize, left: usize, top: usize) {
    for dy in 0usize..9 {
        for dx in 0usize..9 {
            let x = left as i32 + dx as i32 - 1;
            let y = top as i32 + dy as i32 - 1;
            if (0..side as i32).contains(&x) && (0..side as i32).contains(&y) {
                let ix = dx.saturating_sub(1);
                let iy = dy.saturating_sub(1);
                let inside = (1..8).contains(&dx) && (1..8).contains(&dy);
                let dark = inside
                    && (ix == 0
                        || ix == 6
                        || iy == 0
                        || iy == 6
                        || (2..5).contains(&ix) && (2..5).contains(&iy));
                set_function(modules, reserved, side, x as usize, y as usize, dark);
            }
        }
    }
}

/// Draws one 5x5 alignment pattern centred on `(cx, cy)`.
pub fn alignment(modules: &mut [bool], reserved: &mut [bool], side: usize, cx: usize, cy: usize) {
    for dy in 0..5 {
        for dx in 0..5 {
            set_function(
                modules,
                reserved,
                side,
                cx + dx - 2,
                cy + dy - 2,
                dx == 0 || dx == 4 || dy == 0 || dy == 4 || (dx == 2 && dy == 2),
            );
        }
    }
}

/// Draws the horizontal and vertical timing patterns.
///
/// They run between the finder patterns along row and column 6, alternating
/// dark and light, and let a scanner establish the module grid.
pub fn timing(modules: &mut [bool], reserved: &mut [bool], side: usize) {
    for coordinate in 8..side - 8 {
        let dark = coordinate % 2 == 0;
        set_function(modules, reserved, side, coordinate, 6, dark);
        set_function(modules, reserved, side, 6, coordinate, dark);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three finder patterns must land in the corners with a light
    /// separator between them and the data region, at every symbol size.
    #[test]
    fn finders_have_a_dark_ring_and_a_light_separator() {
        for side in [21usize, 25, 37, 49] {
            let mut modules = alloc::vec![false; side * side];
            let mut reserved = alloc::vec![false; side * side];
            finder(&mut modules, &mut reserved, side, 0, 0);

            // Outer ring dark, inner ring light, 3x3 core dark.
            assert!(modules[index(side, 0, 0)], "side {side}: corner");
            assert!(!modules[index(side, 1, 1)], "side {side}: inner ring");
            assert!(modules[index(side, 3, 3)], "side {side}: core");
            // Separator column just past the pattern.
            assert!(!modules[index(side, 7, 0)], "side {side}: separator");
            assert!(
                reserved[index(side, 7, 0)],
                "side {side}: separator reserved"
            );
        }
    }

    #[test]
    fn alignment_is_a_ring_around_a_dark_centre() {
        let side = 37usize;
        let mut modules = alloc::vec![false; side * side];
        let mut reserved = alloc::vec![false; side * side];
        alignment(&mut modules, &mut reserved, side, 28, 28);
        assert!(modules[index(side, 26, 26)], "outer ring");
        assert!(!modules[index(side, 27, 27)], "inner ring is light");
        assert!(modules[index(side, 28, 28)], "centre is dark");
    }

    /// Function modules must be marked reserved so data never overwrites them.
    #[test]
    fn function_modules_are_reserved() {
        let side = 25usize;
        let mut modules = alloc::vec![false; side * side];
        let mut reserved = alloc::vec![false; side * side];
        finder(&mut modules, &mut reserved, side, 0, 0);
        for y in 0..8 {
            for x in 0..8 {
                assert!(reserved[index(side, x, y)], "({x}, {y}) not reserved");
            }
        }
    }
}
