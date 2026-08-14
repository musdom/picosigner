use embedded_graphics::{
    pixelcolor::{Rgb565, RgbColor},
    prelude::Point,
};
use embedded_ttf::{FontTextStyle, FontTextStyleBuilder};
use rusttype::Font;

const FIRA_CODE_REGULAR: &[u8] = include_bytes!("FiraCode-Medium.ttf");
const FONT_SIZE: u32 = 20;

/// Converts the UI's existing baseline coordinates to embedded-ttf's top edge.
pub fn text_point(x: i32, baseline_y: i32) -> Point {
    Point::new(x, baseline_y - FONT_SIZE as i32)
}

pub struct Fonts {
    fira_code_regular: Font<'static>,
}

impl Fonts {
    pub fn load() -> Self {
        Self {
            fira_code_regular: Font::try_from_bytes(FIRA_CODE_REGULAR)
                .expect("invalid Fira Code Regular font"),
        }
    }

    pub fn regular(&self, color: Rgb565) -> FontTextStyle<Rgb565> {
        self.regular_on(color, Rgb565::BLACK)
    }

    pub fn regular_on(&self, color: Rgb565, background: Rgb565) -> FontTextStyle<Rgb565> {
        FontTextStyleBuilder::new(self.fira_code_regular.clone())
            .font_size(FONT_SIZE)
            .text_color(color)
            .anti_aliasing_color(background)
            .build()
    }
}
