use embedded_graphics::{
    prelude::Drawable,
    text::{Alignment, Text},
};
use picosigner::{
    icons::BitcoinIcon,
    palette,
    ui::{draw_scan_overlay as draw_shared_scan_overlay, text_point, Fonts},
};
use picosigner_board::{
    framebuffer::{frame_mut, PortraitFrame},
    SensorZoom, WaveshareLcd,
};

pub fn draw_scan_overlay(
    frame: &mut [u8],
    fonts: &Fonts,
    zoom: SensorZoom,
    progress: (usize, usize),
    returns_to_previous: bool,
) {
    let mut target = PortraitFrame(frame);
    draw_shared_scan_overlay(
        &mut target,
        fonts,
        zoom.factor(),
        progress,
        if returns_to_previous {
            BitcoinIcon::ChevronLeft
        } else {
            BitcoinIcon::Cross
        },
    );
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
