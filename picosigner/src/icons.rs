//! The 24px UI glyph set.
//!
//! Icons are drawn as vector primitives rather than bitmaps so they stay sharp
//! and cost no flash beyond their coordinates. Most are adapted from
//! bitcoinicons.com; the snowflake is a packed sprite because its shape does
//! not reduce to a few lines.

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Circle, Line, Polyline, PrimitiveStyle, Rectangle},
};

/// Native 24px UI glyphs, primarily adapted from bitcoinicons.com.
#[derive(Clone, Copy)]
pub enum BitcoinIcon {
    Wallet,
    Scan,
    Receive,
    Plus,
    Mnemonic,
    Snowflake,
    QrCode,
    Export,
    Globe,
    Fingerprint,
    SdCard,
    Trash,
    Exit,
    Edit,
    Cross,
    ChevronLeft,
    Check,
    Search,
}

/// One bit per foreground pixel, with bit zero at the left edge.
///
/// Ported from `assets/snowflake.c`; keeping the sprite packed avoids embedding
/// the C export's 32-bit ARGB value for every pixel.
const SNOWFLAKE2_24X24: [u32; 24] = [
    0b000000000000000000000000, // ........................
    0b000000000001100000000000, // ...........##...........
    0b000000001001100100000000, // ........#..##..#........
    0b000000000101101000000000, // .........#.##.#.........
    0b000010010011110010010000, // ....#..#..####..#..#....
    0b000001010001100010100000, // .....#.#...##...#.#.....
    0b000000110001100011000000, // ......##...##...##......
    0b000011110001100011110000, // ....####...##...####....
    0b001000001001100100000100, // ..#.....#..##..#.....#..
    0b000100000101101000001000, // ...#.....#.##.#.....#...
    0b000010000011110000010000, // ....#.....####.....#....
    0b011111111110011111111110, // .##########..##########.
    0b011111111110011111111110, // .##########..##########.
    0b000010000011110000010000, // ....#.....####.....#....
    0b000100000101101000001000, // ...#.....#.##.#.....#...
    0b001000001001100100000100, // ..#.....#..##..#.....#..
    0b000011110001100011110000, // ....####...##...####....
    0b000000110001100011000000, // ......##...##...##......
    0b000001010001100010100000, // .....#.#...##...#.#.....
    0b000010010011110010010000, // ....#..#..####..#..#....
    0b000000000101101000000000, // .........#.##.#.........
    0b000000001001100100000000, // ........#..##..#........
    0b000000000001100000000000, // ...........##...........
    0b000000000000000000000000, // ........................
];

/// Draw a UI glyph at its native 24px size.
pub fn draw_bitcoin_icon<D>(target: &mut D, icon: BitcoinIcon, origin: Point, color: Rgb565)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    let p = |x, y| origin + Point::new(x, y);
    let stroke = PrimitiveStyle::with_stroke(color, 2);
    let line = |target: &mut D, a: (i32, i32), b: (i32, i32)| {
        Line::new(p(a.0, a.1), p(b.0, b.1))
            .into_styled(stroke)
            .draw(target)
            .unwrap();
    };
    let rect = |target: &mut D, x, y, w, h| {
        Rectangle::new(p(x, y), Size::new(w, h))
            .into_styled(stroke)
            .draw(target)
            .unwrap();
    };
    match icon {
        BitcoinIcon::Wallet => {
            rect(target, 4, 6, 13, 12);
            line(target, (16, 7), (20, 7));
            line(target, (20, 7), (20, 17));
            line(target, (20, 17), (16, 17));
            Circle::new(p(7, 10), 4)
                .into_styled(stroke)
                .draw(target)
                .unwrap();
        }
        BitcoinIcon::Scan => {
            for (a, b) in [
                ((5, 9), (5, 5)),
                ((5, 5), (9, 5)),
                ((15, 5), (19, 5)),
                ((19, 5), (19, 9)),
                ((5, 15), (5, 19)),
                ((5, 19), (9, 19)),
                ((15, 19), (19, 19)),
                ((19, 19), (19, 15)),
            ] {
                line(target, a, b);
            }
        }
        BitcoinIcon::Receive => {
            line(target, (5, 16), (5, 19));
            line(target, (5, 19), (19, 19));
            line(target, (19, 19), (19, 16));
            line(target, (12, 5), (12, 15));
            line(target, (8, 11), (12, 15));
            line(target, (12, 15), (16, 11));
        }
        BitcoinIcon::Plus => {
            line(target, (12, 4), (12, 20));
            line(target, (4, 12), (20, 12));
        }
        BitcoinIcon::Mnemonic => {
            rect(target, 5, 5, 14, 14);
            for y in [8, 11, 14, 17] {
                for x in [8, 12, 16] {
                    line(target, (x, y), (x + 1, y));
                }
            }
        }
        BitcoinIcon::Snowflake => {
            target
                .draw_iter(SNOWFLAKE2_24X24.iter().enumerate().flat_map(|(y, row)| {
                    (0..24).filter_map(move |x| {
                        ((row >> x) & 1 == 1)
                            .then_some(Pixel(origin + Point::new(x, y as i32), color))
                    })
                }))
                .unwrap();
        }
        BitcoinIcon::QrCode => {
            rect(target, 5, 5, 5, 5);
            rect(target, 14, 5, 5, 5);
            rect(target, 5, 14, 5, 5);
            for (x, y) in [
                (12, 6),
                (12, 10),
                (12, 14),
                (12, 18),
                (15, 13),
                (18, 13),
                (15, 17),
                (18, 18),
            ] {
                Rectangle::new(p(x, y), Size::new(2, 2))
                    .into_styled(PrimitiveStyle::with_fill(color))
                    .draw(target)
                    .unwrap();
            }
        }
        BitcoinIcon::Export => {
            let points = [
                p(12, 7),
                p(7, 7),
                p(5, 9),
                p(5, 17),
                p(7, 19),
                p(15, 19),
                p(17, 17),
                p(17, 12),
            ];
            Polyline::new(&points)
                .into_styled(stroke)
                .draw(target)
                .unwrap();
            line(target, (12, 12), (19, 5));
            line(target, (14, 5), (19, 5));
            line(target, (19, 5), (19, 10));
        }
        BitcoinIcon::Globe => {
            Circle::new(p(4, 4), 16)
                .into_styled(stroke)
                .draw(target)
                .unwrap();
            line(target, (4, 12), (20, 12));
            rect(target, 9, 4, 7, 16);
        }
        BitcoinIcon::Fingerprint => {
            let outer = [
                p(4, 15),
                p(5, 10),
                p(8, 6),
                p(12, 4),
                p(16, 6),
                p(19, 10),
                p(19, 15),
                p(17, 19),
            ];
            Polyline::new(&outer)
                .into_styled(stroke)
                .draw(target)
                .unwrap();
            let middle = [
                p(7, 18),
                p(7, 13),
                p(9, 9),
                p(12, 8),
                p(15, 9),
                p(17, 12),
                p(16, 16),
            ];
            Polyline::new(&middle)
                .into_styled(stroke)
                .draw(target)
                .unwrap();
            let inner = [
                p(10, 20),
                p(9, 16),
                p(10, 13),
                p(12, 11),
                p(14, 13),
                p(13, 17),
                p(15, 20),
            ];
            Polyline::new(&inner)
                .into_styled(stroke)
                .draw(target)
                .unwrap();
        }
        BitcoinIcon::SdCard => {
            let points = [
                p(6, 9),
                p(10, 5),
                p(17, 5),
                p(18, 6),
                p(18, 19),
                p(7, 19),
                p(6, 18),
                p(6, 9),
            ];
            Polyline::new(&points)
                .into_styled(stroke)
                .draw(target)
                .unwrap();
            for x in [11, 14, 17] {
                line(target, (x, 7), (x, 10));
            }
        }
        BitcoinIcon::Trash => {
            line(target, (6, 7), (18, 7));
            line(target, (8, 7), (9, 20));
            line(target, (9, 20), (16, 20));
            line(target, (16, 20), (17, 7));
            line(target, (9, 7), (9, 5));
            line(target, (9, 5), (15, 5));
            line(target, (15, 5), (15, 7));
            line(target, (11, 10), (11, 17));
            line(target, (14, 10), (14, 17));
        }
        BitcoinIcon::Exit => {
            line(target, (13, 7), (13, 4));
            line(target, (13, 4), (4, 4));
            line(target, (4, 4), (4, 20));
            line(target, (4, 20), (13, 20));
            line(target, (13, 20), (13, 17));
            line(target, (11, 12), (20, 12));
            line(target, (16, 8), (20, 12));
            line(target, (20, 12), (16, 16));
        }
        BitcoinIcon::Edit => {
            // Adapted from Bitcoin Icons' 24px outline edit.svg.
            let body = [p(5, 17), p(5, 19), p(7, 19), p(16, 11), p(13, 8), p(5, 17)];
            Polyline::new(&body)
                .into_styled(stroke)
                .draw(target)
                .unwrap();
            let cap = [
                p(15, 6),
                p(18, 9),
                p(19, 8),
                p(19, 6),
                p(18, 5),
                p(16, 5),
                p(15, 6),
            ];
            Polyline::new(&cap)
                .into_styled(stroke)
                .draw(target)
                .unwrap();
        }
        BitcoinIcon::ChevronLeft => {
            line(target, (15, 6), (9, 12));
            line(target, (9, 12), (15, 18));
        }
        BitcoinIcon::Cross => {
            line(target, (6, 6), (18, 18));
            line(target, (18, 6), (6, 18));
        }
        BitcoinIcon::Check => {
            line(target, (6, 13), (11, 18));
            line(target, (11, 18), (18, 6));
        }
        BitcoinIcon::Search => {
            Circle::new(p(5, 5), 12)
                .into_styled(stroke)
                .draw(target)
                .unwrap();
            line(target, (15, 15), (20, 20));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snowflake_sprite_preserves_24px_source_bitmap() {
        assert_eq!(
            SNOWFLAKE2_24X24
                .iter()
                .map(|row| row.count_ones())
                .sum::<u32>(),
            156
        );
        assert_eq!(SNOWFLAKE2_24X24[11], 0b011111111110011111111110);
        assert_eq!(SNOWFLAKE2_24X24[12], 0b011111111110011111111110);

        // The glyph is symmetric about both axes: row `i` mirrors row `23 - i`,
        // and each row is a palindrome across its 24 significant bits.
        for (top, bottom) in SNOWFLAKE2_24X24.iter().zip(SNOWFLAKE2_24X24.iter().rev()) {
            assert_eq!(top, bottom);
            assert_eq!(*top, top.reverse_bits() >> 8);
        }
    }
}
