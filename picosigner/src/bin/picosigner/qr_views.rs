use embedded_graphics::{
    geometry::Size,
    prelude::{Drawable, Point, Primitive},
    primitives::{PrimitiveStyle, Rectangle},
    text::{Alignment, Text},
};
use picosigner::{
    palette,
    ui::{draw_bitcoin_icon, text_point, BitcoinIcon, Fonts, SCAN_PROGRESS_BAR},
};
use picosigner_board::{SensorZoom, WaveshareLcd, DISPLAY_WIDTH};

use super::framebuffer::{frame_mut, PortraitFrame};

pub fn draw_scan_overlay(
    frame: &mut [u8],
    fonts: &Fonts,
    zoom: SensorZoom,
    progress: (usize, usize),
    _seed_qr: bool,
) {
    let mut target = PortraitFrame(frame);
    // Keep the preview unobstructed except for one control band at the bottom.
    Rectangle::new(Point::new(0, 256), Size::new(DISPLAY_WIDTH as u32, 64))
        .into_styled(PrimitiveStyle::with_fill(palette::BACKGROUND))
        .draw(&mut target)
        .unwrap();
    Rectangle::new(
        Point::new(SCAN_PROGRESS_BAR.0, SCAN_PROGRESS_BAR.1),
        Size::new(SCAN_PROGRESS_BAR.2, SCAN_PROGRESS_BAR.3),
    )
    .into_styled(PrimitiveStyle::with_stroke(palette::PROGRESS_OUTLINE, 1))
    .draw(&mut target)
    .unwrap();
    let (received, total) = progress;
    let inner_width = SCAN_PROGRESS_BAR.2.saturating_sub(2);
    let filled_width = if total == 0 {
        0
    } else {
        (inner_width as usize * received.min(total) / total) as u32
    };
    if filled_width > 0 {
        Rectangle::new(
            Point::new(SCAN_PROGRESS_BAR.0 + 1, SCAN_PROGRESS_BAR.1 + 1),
            Size::new(filled_width, SCAN_PROGRESS_BAR.3.saturating_sub(2)),
        )
        .into_styled(PrimitiveStyle::with_fill(palette::PROGRESS))
        .draw(&mut target)
        .unwrap();
    }
    draw_bitcoin_icon(
        &mut target,
        BitcoinIcon::Cross,
        Point::new(8, 288),
        palette::TEXT,
    );
    draw_bitcoin_icon(
        &mut target,
        BitcoinIcon::Search,
        Point::new(172, 288),
        palette::TEXT,
    );
    Text::with_alignment(
        match zoom {
            SensorZoom::X1 => "1x",
            SensorZoom::X2 => "2x",
        },
        text_point(224, 309),
        fonts.regular(palette::TEXT),
        Alignment::Right,
    )
    .draw(&mut target)
    .unwrap();
}

pub fn draw_signed_psbt_qr(
    display_frame: &mut [u8],
    fonts: &Fonts,
    frame: &[u8],
    frame_index: usize,
    frame_count: usize,
) {
    let mut target = PortraitFrame(display_frame);
    picosigner::ui::draw_signed_psbt_qr(&mut target, fonts, frame, frame_index, frame_count);
}

pub fn render_signed_export_error<SPI, DC, CS, RST>(
    display: &mut WaveshareLcd<SPI, DC, CS, RST>,
    fonts: &Fonts,
) where
    SPI: embedded_hal::spi::SpiBus<u8>,
    DC: embedded_hal::digital::OutputPin,
    CS: embedded_hal::digital::OutputPin,
    RST: embedded_hal::digital::OutputPin,
{
    let display_frame = unsafe { frame_mut() };
    display_frame.fill(0);
    {
        let mut target = PortraitFrame(display_frame);
        Text::new(
            "Signed PSBT",
            text_point(16, 24),
            fonts.regular(palette::TEXT),
        )
        .draw(&mut target)
        .unwrap();
        Text::with_alignment(
            "QR export too large",
            text_point(120, 136),
            fonts.regular(palette::DANGER_TEXT),
            Alignment::Center,
        )
        .draw(&mut target)
        .unwrap();
        Text::with_alignment(
            "Tap to return",
            text_point(120, 313),
            fonts.regular(palette::TEXT),
            Alignment::Center,
        )
        .draw(&mut target)
        .unwrap();
    }
    display.write_frame(display_frame);
}
