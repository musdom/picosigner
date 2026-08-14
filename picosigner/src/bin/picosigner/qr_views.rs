use core::convert::Infallible;

use embedded_graphics::{
    geometry::Size,
    pixelcolor::{Rgb565, RgbColor},
    prelude::{DrawTarget, Drawable, Point, Primitive},
    primitives::{PrimitiveStyle, PrimitiveStyleBuilder, Rectangle},
    text::{Alignment, Text},
};
use picosigner_board::{SensorZoom, WaveshareLcd, DISPLAY_WIDTH};
use picosigner_qr::{
    encode_descriptor_qr, encode_export_qr, DESCRIPTOR_QR_MODULES, DESCRIPTOR_QR_SIDE,
    EXPORT_QR_MODULES, EXPORT_QR_SIDE,
};

use super::{
    controls::{SCAN_PROGRESS_BAR, SCAN_ZOOM_BUTTON},
    fonts::{text_point, Fonts},
    framebuffer::{frame_mut, PortraitFrame},
};

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

pub fn draw_descriptor_qr<D>(display: &mut D, descriptor: &[u8])
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    let mut modules = [false; DESCRIPTOR_QR_MODULES];
    if encode_descriptor_qr(descriptor, &mut modules).is_err() {
        return;
    }
    const SCALE: u32 = 4;
    const QUIET: u32 = 4;
    let symbol = (DESCRIPTOR_QR_SIDE as u32 + QUIET * 2) * SCALE;
    let left = (DISPLAY_WIDTH as u32 - symbol) / 2;
    draw_qr_modules_on_white(
        display,
        &modules,
        DESCRIPTOR_QR_SIDE,
        SCALE,
        QUIET,
        left,
        28,
    );
}

pub fn draw_signed_psbt_qr(
    display_frame: &mut [u8],
    fonts: &Fonts,
    frame: &[u8],
    frame_index: usize,
    frame_count: usize,
) {
    display_frame.fill(0);
    let mut target = PortraitFrame(display_frame);
    let mut modules = [false; EXPORT_QR_MODULES];
    if encode_export_qr(frame, &mut modules).is_err() {
        return;
    }
    const SCALE: u32 = 5;
    const QUIET: u32 = 4;
    let symbol = (EXPORT_QR_SIDE as u32 + QUIET * 2) * SCALE;
    let left = (DISPLAY_WIDTH as u32 - symbol) / 2;
    draw_qr_modules_on_white(&mut target, &modules, EXPORT_QR_SIDE, SCALE, QUIET, left, 4);

    Text::with_alignment(
        "SIGNED PSBT - BBQr",
        text_point(120, 250),
        fonts.regular(Rgb565::CYAN),
        Alignment::Center,
    )
    .draw(&mut target)
    .unwrap();
    let part = alloc::format!("PART {}/{}", frame_index + 1, frame_count);
    Text::with_alignment(
        &part,
        text_point(120, 270),
        fonts.regular(Rgb565::WHITE),
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

fn draw_qr_modules_on_white<D>(
    display: &mut D,
    modules: &[bool],
    side: usize,
    scale: u32,
    quiet: u32,
    left: u32,
    top: u32,
) where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    let symbol = (side as u32 + quiet * 2) * scale;
    Rectangle::new(
        Point::new(left as i32, top as i32),
        Size::new(symbol, symbol),
    )
    .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
    .draw(display)
    .unwrap();

    let black = PrimitiveStyle::with_fill(Rgb565::BLACK);
    for y in 0..side {
        let mut x = 0;
        while x < side {
            if !modules[y * side + x] {
                x += 1;
                continue;
            }
            let run_start = x;
            while x < side && modules[y * side + x] {
                x += 1;
            }
            Rectangle::new(
                Point::new(
                    (left + (quiet + run_start as u32) * scale) as i32,
                    (top + (quiet + y as u32) * scale) as i32,
                ),
                Size::new((x - run_start) as u32 * scale, scale),
            )
            .into_styled(black)
            .draw(display)
            .unwrap();
        }
    }
}
