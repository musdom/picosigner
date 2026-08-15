use embedded_graphics::{
    geometry::Size,
    pixelcolor::{Rgb565, RgbColor},
    prelude::{Drawable, Point, Primitive},
    primitives::{PrimitiveStyle, PrimitiveStyleBuilder, Rectangle},
    text::{Alignment, Text},
};
use picosigner::ui::{text_point, Fonts, SCAN_PROGRESS_BAR, SCAN_ZOOM_BUTTON};
use picosigner_board::{SensorZoom, WaveshareLcd, DISPLAY_WIDTH};

use super::framebuffer::{frame_mut, PortraitFrame};

pub fn draw_scan_overlay(
    frame: &mut [u8],
    fonts: &Fonts,
    zoom: SensorZoom,
    progress: (usize, usize),
) {
    let mut target = PortraitFrame(frame);
    let panel = PrimitiveStyleBuilder::new()
        .fill_color(Rgb565::BLACK)
        .stroke_color(Rgb565::WHITE)
        .stroke_width(1)
        .build();
    Rectangle::new(Point::new(0, 0), Size::new(DISPLAY_WIDTH as u32, 24))
        .into_styled(panel)
        .draw(&mut target)
        .unwrap();
    Rectangle::new(Point::new(0, 256), Size::new(DISPLAY_WIDTH as u32, 24))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
        .draw(&mut target)
        .unwrap();
    Rectangle::new(
        Point::new(SCAN_PROGRESS_BAR.0, SCAN_PROGRESS_BAR.1),
        Size::new(SCAN_PROGRESS_BAR.2, SCAN_PROGRESS_BAR.3),
    )
    .into_styled(PrimitiveStyle::with_stroke(Rgb565::WHITE, 1))
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
        .into_styled(PrimitiveStyle::with_fill(Rgb565::GREEN))
        .draw(&mut target)
        .unwrap();
    }
    Rectangle::new(
        Point::new(SCAN_ZOOM_BUTTON.0 as i32, SCAN_ZOOM_BUTTON.1 as i32),
        Size::new(SCAN_ZOOM_BUTTON.2 as u32, SCAN_ZOOM_BUTTON.3 as u32),
    )
    .into_styled(panel)
    .draw(&mut target)
    .unwrap();
    Text::new(
        "SCAN PSBT QR",
        text_point(8, 16),
        fonts.regular(Rgb565::WHITE),
    )
    .draw(&mut target)
    .unwrap();
    Text::with_alignment(
        "CANCEL",
        text_point(232, 16),
        fonts.regular(Rgb565::WHITE),
        Alignment::Right,
    )
    .draw(&mut target)
    .unwrap();
    Text::with_alignment(
        match zoom {
            SensorZoom::X1 => "ZOOM 1x",
            SensorZoom::X2 => "ZOOM 2x",
        },
        text_point(120, 305),
        fonts.regular(Rgb565::WHITE),
        Alignment::Center,
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
            "SIGNED PSBT",
            text_point(16, 24),
            fonts.regular(Rgb565::WHITE),
        )
        .draw(&mut target)
        .unwrap();
        Text::with_alignment(
            "QR EXPORT TOO LARGE",
            text_point(120, 136),
            fonts.regular(Rgb565::RED),
            Alignment::Center,
        )
        .draw(&mut target)
        .unwrap();
        Text::with_alignment(
            "TAP TO RETURN",
            text_point(120, 313),
            fonts.regular(Rgb565::WHITE),
            Alignment::Center,
        )
        .draw(&mut target)
        .unwrap();
    }
    display.write_frame(display_frame);
}
