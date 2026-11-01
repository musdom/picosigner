//! Shared PicoSigner presentation and touch geometry.
//!
//! The RP2350 firmware and desktop emulator both draw through these routines,
//! so the simulator is a view of the real application rather than a mock UI.

use core::convert::Infallible;

use embedded_graphics::{
    geometry::Size,
    pixelcolor::Rgb565,
    prelude::{DrawTarget, Drawable, Point, Primitive},
    primitives::{Circle, CornerRadii, PrimitiveStyle, Rectangle, RoundedRectangle},
    text::{Alignment, Text},
};
use embedded_ttf::{FontTextStyle, FontTextStyleBuilder};
use heapless::String as FixedString;
use picosigner_qr::{
    encode_descriptor_qr, encode_export_qr, DESCRIPTOR_QR_MODULES, DESCRIPTOR_QR_SIDE,
    EXPORT_QR_MODULES, EXPORT_QR_SIDE,
};
use picosigner_ui::{Rect, Screen};

use crate::icons::draw_bitcoin_icon;
pub use crate::icons::BitcoinIcon;

pub use crate::touch::*;
use picosigner_wallet::Network;
use rusttype::Font;

use crate::{app::AppState, palette};

pub const DISPLAY_WIDTH: u32 = 240;
pub const DISPLAY_HEIGHT: u32 = 320;

const UI_FONT: &[u8] = include_bytes!("../assets/subset-font.ttf");
const FONT_SIZE: u32 = 20;
const MENU_FONT_SIZE: u32 = 26;
const SUBHEADING_FONT_SIZE: u32 = 18;
const BODY_FONT_SIZE: u32 = 16;
const MNEMONIC_FONT_SIZE: u32 = 20;

pub fn text_point(x: i32, baseline_y: i32) -> Point {
    Point::new(x, baseline_y - FONT_SIZE as i32)
}

pub(crate) fn small_text_point(x: i32, baseline_y: i32) -> Point {
    Point::new(x, baseline_y - 14)
}

pub(crate) fn subheading_text_point(x: i32, baseline_y: i32) -> Point {
    Point::new(x, baseline_y - SUBHEADING_FONT_SIZE as i32)
}

pub(crate) fn body_text_point(x: i32, baseline_y: i32) -> Point {
    Point::new(x, baseline_y - BODY_FONT_SIZE as i32)
}

/// Top edge of a `FONT_SIZE`-tall line of text centred inside `bounds`.
///
/// `Text` is positioned by the top of its line box, so vertical centring is
/// the only correct way to place a button label: the old per-call baseline
/// constants drifted with every change to a button's height.
fn centered_text_top(bounds: (i32, i32, u32, u32), font_size: u32) -> i32 {
    bounds.1 + (bounds.3 as i32 - font_size as i32) / 2
}

pub struct Fonts {
    ui_regular: Font<'static>,
}

impl Fonts {
    pub fn load() -> Self {
        Self {
            ui_regular: Font::try_from_bytes(UI_FONT).expect("invalid UI font"),
        }
    }

    pub fn regular(&self, color: Rgb565) -> FontTextStyle<Rgb565> {
        self.regular_on(color, palette::BACKGROUND)
    }

    pub fn regular_on(&self, color: Rgb565, background: Rgb565) -> FontTextStyle<Rgb565> {
        FontTextStyleBuilder::new(self.ui_regular.clone())
            .font_size(FONT_SIZE)
            .text_color(color)
            .anti_aliasing_color(background)
            .build()
    }

    pub fn menu(&self, color: Rgb565) -> FontTextStyle<Rgb565> {
        FontTextStyleBuilder::new(self.ui_regular.clone())
            .font_size(MENU_FONT_SIZE)
            .text_color(color)
            .anti_aliasing_color(palette::BACKGROUND)
            .build()
    }

    pub fn subheading(&self, color: Rgb565) -> FontTextStyle<Rgb565> {
        FontTextStyleBuilder::new(self.ui_regular.clone())
            .font_size(SUBHEADING_FONT_SIZE)
            .text_color(color)
            .anti_aliasing_color(palette::BACKGROUND)
            .build()
    }

    pub fn body(&self, color: Rgb565) -> FontTextStyle<Rgb565> {
        FontTextStyleBuilder::new(self.ui_regular.clone())
            .font_size(BODY_FONT_SIZE)
            .text_color(color)
            .anti_aliasing_color(palette::BACKGROUND)
            .build()
    }

    pub fn mnemonic(&self, color: Rgb565) -> FontTextStyle<Rgb565> {
        FontTextStyleBuilder::new(self.ui_regular.clone())
            .font_size(MNEMONIC_FONT_SIZE)
            .text_color(color)
            .anti_aliasing_color(palette::BACKGROUND)
            .build()
    }

    pub fn small(&self, color: Rgb565) -> FontTextStyle<Rgb565> {
        self.small_on(color, palette::BACKGROUND)
    }

    pub fn small_on(&self, color: Rgb565, background: Rgb565) -> FontTextStyle<Rgb565> {
        FontTextStyleBuilder::new(self.ui_regular.clone())
            .font_size(14)
            .text_color(color)
            .anti_aliasing_color(background)
            .build()
    }
}

pub fn render_app<D>(
    target: &mut D,
    fonts: &Fonts,
    app: &AppState,
    hold_action: Option<HoldAction>,
    hold_ticks: u16,
) where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    target.clear(palette::BACKGROUND).unwrap();
    let title = || fonts.regular(palette::TEXT);
    let heading = match app.screen() {
        Screen::LoadWallet => "PicoSigner",
        Screen::WalletList => "Load wallet",
        Screen::ImportWallet => "Import wallet",
        Screen::SeedQrScan => "Scan SeedQR",
        Screen::MnemonicLength => "Mnemonic length",
        Screen::MnemonicEntry => "",
        Screen::FrostThreshold => "FROST recovery",
        Screen::FrostShareIndex => "Backup key number",
        Screen::FrostMnemonicEntry => "",
        Screen::FrostMnemonicConfirm => "Confirm 25 words",
        Screen::Network => "Choose network",
        Screen::Derivation => "Derivation",
        Screen::PassphraseChoice => "BIP39 passphrase",
        Screen::PassphraseEntry => "Enter passphrase",
        Screen::GenerationWords => "Wallet size",
        Screen::EntropySources => "Extra entropy",
        Screen::DiceEntropy => "Roll a D6",
        Screen::CameraEntropy => "Camera entropy",
        Screen::Deriving => "Deriving wallet",
        Screen::Backup => "Backup required",
        Screen::BackupMenu => "Show backup",
        Screen::BackupWords => "Wallet backup",
        Screen::BackupSeedQr => "Compact SeedQR",
        Screen::Home => "",
        Screen::Settings => "Settings",
        Screen::DeleteWallet => "Delete wallet",
        Screen::CancelEntryConfirm => "Cancel entry",
        Screen::Scan => "",
        Screen::Descriptor => "Descriptor export",
        Screen::Review => "Review transaction",
        Screen::Sign => "Hold to sign",
        Screen::Signed => "Signed PSBT",
        Screen::Recovery => "Wallet error",
        Screen::Boot | Screen::Seed => "Initializing",
    };
    if !matches!(
        app.screen(),
        Screen::Descriptor | Screen::MnemonicEntry | Screen::CameraEntropy
    ) {
        if app.screen() == Screen::Home {
            if let Some(wallet) = app.wallet() {
                if let Some(fingerprint) = wallet.master_fingerprint() {
                    draw_wallet_identity(target, fonts, 8, 1, fingerprint, wallet.network(), false);
                }
            }
        } else {
            Text::new(heading, text_point(16, 24), title())
                .draw(target)
                .unwrap();
        }
    }

    crate::screens::draw_screen_body(target, fonts, app, hold_action, hold_ticks);
}

pub(crate) fn draw_indeterminate_progress<D>(target: &mut D, bounds: (i32, i32, u32, u32))
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    Rectangle::new(
        Point::new(bounds.0, bounds.1),
        Size::new(bounds.2, bounds.3),
    )
    .into_styled(PrimitiveStyle::with_stroke(palette::PROGRESS_OUTLINE, 1))
    .draw(target)
    .unwrap();

    let inner_height = bounds.3.saturating_sub(2);
    let mut left = bounds.0 + 1;
    let right = bounds.0 + bounds.2 as i32 - 1;
    while left < right {
        let width = 8u32.min((right - left) as u32);
        Rectangle::new(
            Point::new(left, bounds.1 + 1),
            Size::new(width, inner_height),
        )
        .into_styled(PrimitiveStyle::with_fill(palette::PROGRESS))
        .draw(target)
        .unwrap();
        left += 16;
    }
}

pub(crate) fn draw_confirmation_footer<D>(target: &mut D, fonts: &Fonts, hold_ticks: u16)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    draw_hold_bar(target, hold_ticks, BACKUP_CONFIRM_BAR);

    draw_confirmation_footer_icons(target, fonts, "Long press", BitcoinIcon::Cross);
}

/// The baseline every footer icon sits on.
const FOOTER_ICON_TOP: i32 = 283;

/// Draws one footer action: an outline icon with an optional label beside it.
///
/// Every bottom action in the app goes through here, so they share one visual
/// treatment. `bounds` is the action's touch area, used to centre the label
/// vertically; `icon_left` positions the icon within it.
fn draw_footer_action<D>(
    target: &mut D,
    fonts: &Fonts,
    bounds: (i32, i32, u32, u32),
    icon: BitcoinIcon,
    icon_left: i32,
    label: Option<(&str, i32)>,
) where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    draw_bitcoin_icon(
        target,
        icon,
        Point::new(icon_left, FOOTER_ICON_TOP),
        palette::TEXT,
    );
    if let Some((label, label_left)) = label {
        Text::new(
            label,
            Point::new(label_left, centered_text_top(bounds, FONT_SIZE)),
            fonts.regular(palette::TEXT),
        )
        .draw(target)
        .unwrap();
    }
}

/// The standard confirmation footer: cancel on the left, confirm on the right.
///
/// The icons are not centred in their action areas: each sits far enough left
/// to leave room for its label, which is why the two x positions differ.
pub fn draw_confirmation_footer_icons<D>(
    target: &mut D,
    fonts: &Fonts,
    confirm_label: &str,
    left_icon: BitcoinIcon,
) where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    draw_footer_action(target, fonts, BACKUP_CANCEL_BUTTON, left_icon, 18, None);
    draw_footer_action(
        target,
        fonts,
        BACKUP_CONFIRM_BUTTON,
        BitcoinIcon::Check,
        96,
        Some((confirm_label, 128)),
    );
}

/// Draws the live camera controls shared by the firmware scanner and the QR
/// example. The camera image is already present in the target; the black
/// control band is painted after decoding the camera frame.
pub fn draw_scan_overlay<D>(
    target: &mut D,
    fonts: &Fonts,
    zoom_factor: u8,
    progress: (usize, usize),
    left_icon: BitcoinIcon,
) where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    Rectangle::new(Point::new(0, 256), Size::new(DISPLAY_WIDTH, 64))
        .into_styled(PrimitiveStyle::with_fill(palette::BACKGROUND))
        .draw(target)
        .unwrap();
    Rectangle::new(
        Point::new(SCAN_PROGRESS_BAR.0, SCAN_PROGRESS_BAR.1),
        Size::new(SCAN_PROGRESS_BAR.2, SCAN_PROGRESS_BAR.3),
    )
    .into_styled(PrimitiveStyle::with_stroke(palette::PROGRESS_OUTLINE, 1))
    .draw(target)
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
        .draw(target)
        .unwrap();
    }
    draw_bitcoin_icon(target, left_icon, Point::new(8, 288), palette::TEXT);
    draw_bitcoin_icon(
        target,
        BitcoinIcon::Search,
        Point::new(172, 288),
        palette::TEXT,
    );
    Text::with_alignment(
        match zoom_factor {
            1 => "1x",
            _ => "2x",
        },
        text_point(224, 309),
        fonts.regular(palette::TEXT),
        Alignment::Right,
    )
    .draw(target)
    .unwrap();
}

/// Draws a bounded, printable representation of a decoded QR payload.
pub fn draw_qr_result<D>(target: &mut D, fonts: &Fonts, payload: &[u8])
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    Rectangle::new(Point::zero(), Size::new(DISPLAY_WIDTH, DISPLAY_HEIGHT))
        .into_styled(PrimitiveStyle::with_fill(palette::BACKGROUND))
        .draw(target)
        .unwrap();
    Text::new(
        "QR decoded",
        text_point(16, 24),
        fonts.regular(palette::TEXT),
    )
    .draw(target)
    .unwrap();

    let mut line = FixedString::<24>::new();
    let mut baseline = 52;
    for &byte in payload {
        if byte == b'\n' || line.len() == line.capacity() {
            Text::new(&line, text_point(16, baseline), fonts.body(palette::TEXT))
                .draw(target)
                .unwrap();
            line.clear();
            baseline += 17;
            if baseline > 250 {
                break;
            }
            if byte == b'\n' {
                continue;
            }
        }
        let printable = if byte.is_ascii_graphic() || byte == b' ' {
            byte as char
        } else {
            '.'
        };
        let _ = line.push(printable);
    }
    if !line.is_empty() && baseline <= 250 {
        Text::new(&line, text_point(16, baseline), fonts.body(palette::TEXT))
            .draw(target)
            .unwrap();
    }
    draw_confirmation_footer_icons(target, fonts, "Confirm", BitcoinIcon::ChevronLeft);
}

/// The FROST share footer: edit on the left, confirm on the right.
///
/// Wider action areas than the standard footer because both carry labels.
pub(crate) fn draw_frost_confirmation_footer<D>(target: &mut D, fonts: &Fonts)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    draw_footer_action(
        target,
        fonts,
        FROST_EDIT_BUTTON,
        BitcoinIcon::Edit,
        8,
        Some(("Edit", 38)),
    );
    draw_footer_action(
        target,
        fonts,
        FROST_CONFIRM_BUTTON,
        BitcoinIcon::Check,
        108,
        Some(("Confirm", 140)),
    );
}

/// Shown on the Settings screen in place of the nav-back button when a
/// setting has been changed but not yet saved: discard on the left, save
/// (and reboot to apply it) on the right.
pub(crate) fn draw_settings_save_banner<D>(target: &mut D, fonts: &Fonts)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    draw_footer_action(
        target,
        fonts,
        crate::layout::settings::DISCARD_BUTTON,
        BitcoinIcon::Cross,
        8,
        Some(("Discard", 34)),
    );
    draw_footer_action(
        target,
        fonts,
        crate::layout::settings::SAVE_BUTTON,
        BitcoinIcon::Check,
        108,
        Some(("Save", 140)),
    );
}

/// A lone back action, centred in its area since it carries no label.
pub(crate) fn draw_nav_back_button<D>(target: &mut D, fonts: &Fonts, app: &AppState)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    draw_footer_action(
        target,
        fonts,
        NAV_BACK_BUTTON,
        if app.back_returns_to_wallet() {
            BitcoinIcon::Cross
        } else {
            BitcoinIcon::ChevronLeft
        },
        108,
        None,
    );
}

/// Draw a menu action as a Bitcoin icon followed by white text on black.
pub(crate) fn draw_menu_item<D>(
    target: &mut D,
    fonts: &Fonts,
    bounds: (i32, i32, u32, u32),
    icon: BitcoinIcon,
    label: &str,
) where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    draw_menu_item_with_icon_color(target, fonts, bounds, icon, label, palette::TEXT);
}

pub(crate) fn draw_menu_item_with_icon_color<D>(
    target: &mut D,
    fonts: &Fonts,
    bounds: (i32, i32, u32, u32),
    icon: BitcoinIcon,
    label: &str,
    icon_color: Rgb565,
) where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    let icon_top = bounds.1 + (bounds.3 as i32 - 24) / 2;
    draw_bitcoin_icon(target, icon, Point::new(bounds.0 + 4, icon_top), icon_color);
    Text::new(
        label,
        Point::new(bounds.0 + 36, centered_text_top(bounds, MENU_FONT_SIZE)),
        fonts.menu(palette::TEXT),
    )
    .draw(target)
    .unwrap();
}

/// Draws a Material-style toggle switch: a rounded track with a circular
/// thumb, filled to indicate on/off.
pub(crate) fn draw_switch<D>(target: &mut D, bounds: (i32, i32, u32, u32), on: bool)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    let (x, y, w, h) = bounds;
    let track_color = if on {
        palette::PRIMARY
    } else {
        palette::TEXT_MUTED
    };
    RoundedRectangle::new(
        Rectangle::new(Point::new(x, y), Size::new(w, h)),
        CornerRadii::new(Size::new(h, h)),
    )
    .into_styled(PrimitiveStyle::with_fill(track_color))
    .draw(target)
    .unwrap();

    let thumb_diameter = h.saturating_sub(4);
    let thumb_y = y + 2;
    let thumb_x = if on {
        x + w as i32 - thumb_diameter as i32 - 2
    } else {
        x + 2
    };
    Circle::new(Point::new(thumb_x, thumb_y), thumb_diameter)
        .into_styled(PrimitiveStyle::with_fill(palette::TEXT))
        .draw(target)
        .unwrap();
}

pub(crate) fn draw_number_picker<D>(target: &mut D, fonts: &Fonts, value: u32, caption: &str)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    Text::with_alignment(
        "-",
        Point::new(16 + 30, centered_text_top((16, 80, 60, 44), FONT_SIZE)),
        fonts.regular(palette::TEXT),
        Alignment::Center,
    )
    .draw(target)
    .unwrap();
    Text::with_alignment(
        "+",
        Point::new(164 + 30, centered_text_top((164, 80, 60, 44), FONT_SIZE)),
        fonts.regular(palette::TEXT),
        Alignment::Center,
    )
    .draw(target)
    .unwrap();
    let number = alloc::format!("{}", value);
    Text::with_alignment(
        &number,
        text_point(120, 112),
        fonts.regular(palette::TEXT),
        Alignment::Center,
    )
    .draw(target)
    .unwrap();
    Text::with_alignment(
        caption,
        small_text_point(120, 140),
        fonts.small(palette::TEXT_MUTED),
        Alignment::Center,
    )
    .draw(target)
    .unwrap();
}

/// Bridges a widget [`Rect`] to the tuple the drawing primitives still take.
///
/// Temporary: it disappears as the remaining primitives move onto `Rect`.
pub(crate) fn rect_tuple(rect: Rect) -> (i32, i32, u32, u32) {
    (
        i32::from(rect.x),
        i32::from(rect.y),
        u32::from(rect.width),
        u32::from(rect.height),
    )
}

/// Draw a filled keyboard key with its label centred inside `bounds`.
pub(crate) fn draw_button<D>(
    target: &mut D,
    fonts: &Fonts,
    bounds: (i32, i32, u32, u32),
    color: Rgb565,
    label: &str,
) where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    Rectangle::new(
        Point::new(bounds.0, bounds.1),
        Size::new(bounds.2, bounds.3),
    )
    .into_styled(PrimitiveStyle::with_fill(color))
    .draw(target)
    .unwrap();
    Text::with_alignment(
        label,
        Point::new(
            bounds.0 + bounds.2 as i32 / 2,
            centered_text_top(bounds, FONT_SIZE),
        ),
        fonts.regular_on(palette::TEXT, color),
        Alignment::Center,
    )
    .draw(target)
    .unwrap();
}

pub(crate) fn fingerprint_text(fingerprint: [u8; 4]) -> alloc::string::String {
    alloc::format!(
        "{:02x}{:02x}{:02x}{:02x}",
        fingerprint[0],
        fingerprint[1],
        fingerprint[2],
        fingerprint[3]
    )
}

pub(crate) fn draw_wallet_identity<D>(
    target: &mut D,
    fonts: &Fonts,
    left: i32,
    top: i32,
    fingerprint: [u8; 4],
    network: Network,
    large: bool,
) where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    draw_bitcoin_icon(
        target,
        BitcoinIcon::Wallet,
        Point::new(left, top),
        palette::TEXT,
    );
    let fingerprint = fingerprint_text(fingerprint);
    let fingerprint_top = if large { top - 1 } else { top + 2 };
    Text::new(
        &fingerprint,
        Point::new(left + 32, fingerprint_top),
        if large {
            fonts.menu(palette::TEXT)
        } else {
            fonts.regular(palette::TEXT)
        },
    )
    .draw(target)
    .unwrap();

    let (chip_color, chip_text, text_color) = match network {
        Network::Mainnet => (palette::MAINNET, "Mainnet", palette::TEXT),
        Network::Signet => (palette::SIGNET, "Signet", palette::TEXT),
        Network::Testnet4 => (palette::TESTNET, "Testnet", palette::TEXT),
    };
    let chip = (left + 150, top + 3, 74, 18);
    Rectangle::new(Point::new(chip.0, chip.1), Size::new(chip.2, chip.3))
        .into_styled(PrimitiveStyle::with_fill(chip_color))
        .draw(target)
        .unwrap();
    Text::with_alignment(
        chip_text,
        Point::new(chip.0 + chip.2 as i32 / 2, chip.1 + 2),
        fonts.small_on(text_color, chip_color),
        Alignment::Center,
    )
    .draw(target)
    .unwrap();
}

pub(crate) fn draw_wallet_scrollbar<D>(target: &mut D, app: &AppState)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    let page_count = app.wallet_list_page_count();
    if page_count <= 1 {
        return;
    }
    const LEFT: i32 = 232;
    let height = WALLET_LIST_BOTTOM - WALLET_LIST_TOP;
    Rectangle::new(
        Point::new(LEFT, WALLET_LIST_TOP),
        Size::new(4, height as u32),
    )
    .into_styled(PrimitiveStyle::with_stroke(palette::PROGRESS_OUTLINE, 1))
    .draw(target)
    .unwrap();

    let thumb_height = height / page_count as i32;
    let thumb_top = WALLET_LIST_TOP + app.wallet_list_page() as i32 * thumb_height;
    Rectangle::new(
        Point::new(LEFT, thumb_top),
        Size::new(4, thumb_height as u32),
    )
    .into_styled(PrimitiveStyle::with_fill(palette::TEXT_MUTED))
    .draw(target)
    .unwrap();
}

pub fn draw_hold_bar<D>(display: &mut D, hold_ticks: u16, bounds: (i32, i32, u32, u32))
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    if hold_ticks > 0 {
        draw_hold_bar_progress(display, 0, hold_ticks, bounds);
    }
}

fn draw_hold_bar_outline<D>(display: &mut D, bounds: (i32, i32, u32, u32))
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    Rectangle::new(
        Point::new(bounds.0, bounds.1),
        Size::new(bounds.2, bounds.3),
    )
    .into_styled(PrimitiveStyle::with_stroke(palette::PROGRESS_OUTLINE, 1))
    .draw(display)
    .unwrap();
}

pub fn draw_hold_bar_progress<D>(
    display: &mut D,
    previous_ticks: u16,
    current_ticks: u16,
    bounds: (i32, i32, u32, u32),
) where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    let inner_width = bounds.2.saturating_sub(2);
    let previous = inner_width * u32::from(previous_ticks.min(30)) / 30;
    let current = inner_width * u32::from(current_ticks.min(30)) / 30;
    if current > previous {
        if previous_ticks == 0 {
            draw_hold_bar_outline(display, bounds);
        }
        Rectangle::new(
            Point::new(bounds.0 + 1 + previous as i32, bounds.1 + 1),
            Size::new(current - previous, bounds.3.saturating_sub(2)),
        )
        .into_styled(PrimitiveStyle::with_fill(palette::PROGRESS))
        .draw(display)
        .unwrap();
    }
}

pub fn reset_hold_bar<D>(display: &mut D, bounds: (i32, i32, u32, u32))
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    Rectangle::new(
        Point::new(bounds.0, bounds.1),
        Size::new(bounds.2, bounds.3),
    )
    .into_styled(PrimitiveStyle::with_fill(palette::BACKGROUND))
    .draw(display)
    .unwrap();
}

pub fn draw_descriptor_qr<D>(display: &mut D, descriptor: &[u8])
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    let mut modules = [false; DESCRIPTOR_QR_MODULES];
    if encode_descriptor_qr(descriptor, &mut modules).is_ok() {
        draw_qr_modules_on_white(display, &modules, DESCRIPTOR_QR_SIDE, 4, 4, 6, 28);
    }
}

pub fn draw_signed_psbt_qr<D>(
    target: &mut D,
    fonts: &Fonts,
    frame: &[u8],
    frame_index: usize,
    frame_count: usize,
) where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    target.clear(palette::BACKGROUND).unwrap();
    let mut modules = [false; EXPORT_QR_MODULES];
    if encode_export_qr(frame, &mut modules).is_err() {
        return;
    }
    let symbol = (EXPORT_QR_SIDE as u32 + 8) * 5;
    let left = (DISPLAY_WIDTH - symbol) / 2;
    draw_qr_modules_on_white(target, &modules, EXPORT_QR_SIDE, 5, 4, left, 4);
    Text::with_alignment(
        "SIGNED PSBT - BBQr",
        text_point(120, 250),
        fonts.regular(palette::TEXT),
        Alignment::Center,
    )
    .draw(target)
    .unwrap();
    let part = alloc::format!("PART {}/{}", frame_index + 1, frame_count);
    Text::with_alignment(
        &part,
        text_point(120, 270),
        fonts.regular(palette::TEXT),
        Alignment::Center,
    )
    .draw(target)
    .unwrap();
    Text::with_alignment(
        "Tap to return",
        text_point(120, 313),
        fonts.regular(palette::TEXT),
        Alignment::Center,
    )
    .draw(target)
    .unwrap();
}

pub fn render_message<D>(target: &mut D, fonts: &Fonts, title: &str, lines: &[&str])
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    target.clear(palette::BACKGROUND).unwrap();
    Text::new(
        title,
        text_point(16, 24),
        fonts.regular(palette::DANGER_TEXT),
    )
    .draw(target)
    .unwrap();
    for (index, line) in lines.iter().enumerate() {
        Text::new(
            line,
            text_point(16, 72 + index as i32 * 22),
            fonts.regular(palette::TEXT),
        )
        .draw(target)
        .unwrap();
    }
    Text::with_alignment(
        "Tap to return",
        text_point(120, 313),
        fonts.regular(palette::TEXT),
        Alignment::Center,
    )
    .draw(target)
    .unwrap();
}

pub(crate) fn draw_qr_modules_on_white<D>(
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
    .into_styled(PrimitiveStyle::with_fill(palette::QR_LIGHT))
    .draw(display)
    .unwrap();
    let black = PrimitiveStyle::with_fill(palette::QR_DARK);
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

#[cfg(test)]
mod tests {
    use super::*;
    use picosigner_wallet::AccountType;

    #[test]
    fn actions_use_shared_display_coordinates() {
        assert_eq!(action_at(Screen::Home, 120, 80), Some(HoldAction::Scan));
        assert_eq!(
            action_at(Screen::Home, 120, 132),
            Some(HoldAction::BackupMenu)
        );
        assert_eq!(
            action_at(Screen::Home, 120, 184),
            Some(HoldAction::SaveWallet)
        );
        assert_eq!(action_at(Screen::Home, 120, 236), Some(HoldAction::Logout));
        assert_eq!(
            action_at(Screen::BackupMenu, 120, 220),
            Some(HoldAction::Descriptor)
        );
        assert_eq!(
            hold_bar_bounds(HoldAction::DeleteWallet),
            Some(BACKUP_CONFIRM_BAR)
        );
        assert_eq!(
            action_at(Screen::Review, 200, 16),
            Some(HoldAction::SwipeBack)
        );
        assert!(supports_swipe_back(Screen::Review));
        assert!(!supports_swipe_back(Screen::MnemonicEntry));
        assert!(supports_swipe_back(Screen::FrostMnemonicConfirm));
        assert_eq!(
            action_at(Screen::FrostMnemonicConfirm, 40, 290),
            Some(HoldAction::FrostEditShare)
        );
        assert_eq!(
            action_at(Screen::FrostMnemonicConfirm, 180, 290),
            Some(HoldAction::FrostAcceptShare)
        );
        assert_eq!(
            action_at(Screen::Review, 120, 250),
            Some(HoldAction::SwipeBack)
        );
        assert_eq!(action_at(Screen::Network, 120, 290), Some(HoldAction::Back));
        assert_eq!(action_at(Screen::Review, 24, 285), Some(HoldAction::Back));
        assert_eq!(action_at(Screen::Review, 120, 300), Some(HoldAction::Sign));
        assert_eq!(action_at(Screen::Backup, 24, 285), Some(HoldAction::Cancel));
        assert_eq!(
            action_at(Screen::Backup, 120, 285),
            Some(HoldAction::Backup)
        );
        assert_eq!(action_at(Screen::Backup, 120, 250), None);
        assert!(point_in_u16_button(10, 300, SCAN_CANCEL_BUTTON));
        assert!(point_in_u16_button(200, 300, SCAN_ZOOM_BUTTON));
        assert!(!point_in_u16_button(200, 10, SCAN_CANCEL_BUTTON));
        assert!(in_button(24, 300, QR_RESULT_CANCEL_BUTTON));
        assert!(in_button(180, 300, QR_RESULT_CONFIRM_BUTTON));
        assert!(!in_button(120, 250, QR_RESULT_CONFIRM_BUTTON));
        assert_eq!(
            action_at(Screen::BackupWords, 120, 308),
            Some(HoldAction::Back)
        );
        assert_eq!(
            action_at(Screen::BackupWords, 20, 200),
            Some(HoldAction::BackupPage)
        );
        assert_eq!(
            action_at(Screen::MnemonicEntry, 85, 304),
            Some(HoldAction::Backspace)
        );
        assert_eq!(
            action_at(Screen::MnemonicEntry, 120, 304),
            Some(HoldAction::Backspace)
        );
        assert_eq!(
            action_at(Screen::MnemonicEntry, 155, 304),
            Some(HoldAction::Backspace)
        );
        assert_eq!(
            action_at(Screen::MnemonicEntry, 165, 304),
            Some(HoldAction::OpenCancelEntry)
        );
        assert_eq!(
            action_at(Screen::MnemonicEntry, 235, 304),
            Some(HoldAction::OpenCancelEntry)
        );
        assert_eq!(
            action_at(Screen::MnemonicEntry, 180, 88),
            Some(HoldAction::SwipeBack)
        );
        assert_eq!(
            action_at(Screen::MnemonicEntry, 60, 60),
            Some(HoldAction::Suggestion(0))
        );
        assert_eq!(
            action_at(Screen::MnemonicEntry, 60, 84),
            Some(HoldAction::SwipeBack)
        );
        assert_eq!(
            action_at(Screen::MnemonicEntry, 60, 96),
            Some(HoldAction::Suggestion(1))
        );
        assert_eq!(
            action_at(Screen::Derivation, 120, 70),
            Some(HoldAction::Derivation(AccountType::NestedSegwit))
        );
        assert_eq!(
            action_at(Screen::Derivation, 120, 120),
            Some(HoldAction::Derivation(AccountType::NativeSegwit))
        );
        assert_eq!(
            action_at(Screen::Derivation, 120, 180),
            Some(HoldAction::Derivation(AccountType::Taproot))
        );
        assert_eq!(
            action_at(Screen::PassphraseEntry, 20, 90),
            Some(HoldAction::PassphraseKey(0))
        );
        assert_eq!(
            action_at(Screen::PassphraseEntry, 180, 220),
            Some(HoldAction::PassphraseKey(28))
        );
        assert_eq!(
            action_at(Screen::DiceEntropy, 200, 150),
            Some(HoldAction::DiceRoll(6))
        );
        assert_eq!(
            action_at(Screen::EntropySources, 180, 290),
            Some(HoldAction::EntropyDone)
        );
    }
}
