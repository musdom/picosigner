//! Per-screen drawing.
//!
//! Each function here paints one screen's body; the header, heading, and any
//! shared footer are painted by [`crate::ui::render_app`] around them. Screen
//! geometry comes from [`crate::layout`], which the hit-tests read too, so a
//! control is touchable exactly where it is drawn.

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::{DrawTarget, Drawable, Point, Primitive},
    primitives::{Line, PrimitiveStyle},
    text::{Alignment, Text},
};
use picosigner_ui::{passphrase_keyboard, Rect, Screen, Widget};

use crate::{
    app::{AppState, WALLETS_PER_PAGE},
    icons::BitcoinIcon,
    layout, palette,
    ui::*,
};
use picosigner_qr::encode_compact_seed_qr;
use picosigner_wallet::{AccountType, EntropySource, Network, WalletKind};
use zeroize::Zeroize;

fn draw_passphrase_keyboard<D>(target: &mut D, fonts: &Fonts, app: &AppState)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    let passphrase = app.passphrase_display();
    if passphrase.is_empty() {
        Text::new(
            "Passphrase appears here",
            small_text_point(8, 58),
            fonts.small(palette::TEXT_MUTED),
        )
        .draw(target)
        .unwrap();
    } else {
        // On-screen entry is bounded ASCII, so byte slicing cannot split a
        // UTF-8 codepoint. Keep both lines within the 240-pixel display.
        let split = passphrase.len().min(32);
        Text::new(
            &passphrase[..split],
            small_text_point(8, 50),
            fonts.small(palette::TEXT),
        )
        .draw(target)
        .unwrap();
        if split < passphrase.len() {
            Text::new(
                &passphrase[split..],
                small_text_point(8, 68),
                fonts.small(palette::TEXT),
            )
            .draw(target)
            .unwrap();
        }
    }
    let mut grid = layout::passphrase::grid();
    grid.set_bounds(layout::passphrase::BOUNDS);
    for index in 0..grid.len() {
        // The two rightmost cells in the last row both type a space; drawing
        // them as separate buttons would show two identical keys side by
        // side, so the second cell is folded into the first's button.
        if index == passphrase_keyboard::SPACE + 1 {
            continue;
        }
        let Some(mut rect) = grid.key_rect(index) else {
            continue;
        };
        if index == passphrase_keyboard::SPACE {
            if let Some(next) = grid.key_rect(index + 1) {
                rect.width = next.right().saturating_sub(rect.x);
            }
        }
        // The layout lives in picosigner-ui so the label drawn here and the
        // character app.rs inserts can never disagree.
        let label = alloc::format!(
            "{}",
            passphrase_keyboard::label(index, app.passphrase_mode())
        );
        let color = if index == passphrase_keyboard::BACKSPACE {
            palette::SECONDARY
        } else {
            palette::SURFACE_ALT
        };
        draw_button(target, fonts, rect_tuple(rect), color, &label);
    }
    draw_confirmation_footer_icons(target, fonts, "Confirm", BitcoinIcon::Cross);
}

fn draw_load_wallet<D>(target: &mut D, fonts: &Fonts)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    for (slot, (icon, label)) in [
        (BitcoinIcon::Wallet, "Load wallet"),
        (BitcoinIcon::Receive, "Import wallet"),
        (BitcoinIcon::Plus, "Create wallet"),
        (BitcoinIcon::Settings, "Settings"),
    ]
    .into_iter()
    .enumerate()
    {
        draw_menu_item(
            target,
            fonts,
            rect_tuple(layout::menus::LOAD_WALLET.row(slot as u16)),
            icon,
            label,
        );
    }
}

fn draw_wallet_list<D>(target: &mut D, fonts: &Fonts, app: &AppState)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    if app.wallets().is_empty() {
        Text::with_alignment(
            "No wallets saved",
            subheading_text_point(120, 112),
            fonts.subheading(palette::TEXT_MUTED),
            Alignment::Center,
        )
        .draw(target)
        .unwrap();
    }
    for (visible_index, wallet) in app
        .wallets()
        .iter()
        .skip(app.wallet_list_page() * WALLETS_PER_PAGE)
        .take(WALLETS_PER_PAGE)
        .enumerate()
    {
        let row_top = WALLET_LIST_TOP + visible_index as i32 * WALLET_LIST_ROW_HEIGHT;
        let identity_top = row_top + (WALLET_LIST_ROW_HEIGHT - 24) / 2;
        let network = match wallet.network {
            picosigner_board::Network::Mainnet => Network::Mainnet,
            picosigner_board::Network::Testnet4 => Network::Testnet4,
            picosigner_board::Network::Signet => Network::Signet,
        };
        draw_wallet_identity(
            target,
            fonts,
            8,
            identity_top,
            wallet.fingerprint.unwrap_or([0; 4]),
            network,
            true,
        );
    }
    draw_wallet_scrollbar(target, app);
    draw_nav_back_button(target, fonts, app);
}

fn draw_mnemonic_keyboard<D>(target: &mut D, fonts: &Fonts, app: &AppState)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    let word_number = (app.mnemonic_entered() + 1).min(app.mnemonic_target());
    let status = if app.screen() == Screen::FrostMnemonicEntry {
        alloc::format!("KEY #{}  WORD {}/25", app.frost_share_index(), word_number)
    } else {
        alloc::format!("WORD {}/{}", word_number, app.mnemonic_target())
    };
    Text::new(
        &status,
        text_point(8, 24),
        fonts.regular(if app.mnemonic_error() {
            palette::DANGER_TEXT
        } else {
            palette::TEXT
        }),
    )
    .draw(target)
    .unwrap();
    let prefix = alloc::format!("> {}", app.mnemonic_prefix().to_ascii_uppercase());
    Text::new(
        &prefix,
        small_text_point(8, 44),
        fonts.small(palette::TEXT_MUTED),
    )
    .draw(target)
    .unwrap();
    for (index, word) in app.mnemonic_suggestions().iter().take(3).enumerate() {
        let top = 50 + index as i32 * 36;
        let word = word.to_ascii_uppercase();
        draw_button(target, fonts, (4, top, 116, 32), palette::PRIMARY, &word);
    }
    Text::new(
        "Recent",
        small_text_point(148, 44),
        fonts.small(palette::TEXT_MUTED),
    )
    .draw(target)
    .unwrap();
    for (row, entry) in app
        .recent_mnemonic_words()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let label = alloc::format!("{:>2}. {}", entry.0, entry.1.to_ascii_uppercase());
        Text::new(
            &label,
            small_text_point(126, 66 + row as i32 * 22),
            fonts.small(palette::TEXT),
        )
        .draw(target)
        .unwrap();
    }
    let mut grid = layout::mnemonic::grid();
    grid.set_bounds(layout::mnemonic::BOUNDS);
    for index in 0..grid.len() {
        let Some(rect) = grid.key_rect(index) else {
            continue;
        };
        let label = alloc::format!("{}", layout::mnemonic::label(index));
        draw_button(
            target,
            fonts,
            rect_tuple(rect),
            palette::SURFACE_ALT,
            &label,
        );
    }
    // Same constants the hit-test reads, so the buttons are touchable exactly
    // where they are drawn.
    for (left, label) in [
        (layout::mnemonic::ERASE_LEFT, "Erase"),
        (layout::mnemonic::CANCEL_LEFT, "Cancel"),
    ] {
        draw_button(
            target,
            fonts,
            rect_tuple(Rect::new(
                left,
                layout::mnemonic::ACTION_TOP,
                layout::mnemonic::ACTION_WIDTH,
                layout::mnemonic::ACTION_HEIGHT,
            )),
            palette::SECONDARY,
            label,
        );
    }
}

fn draw_frost_mnemonic_confirmation<D>(target: &mut D, fonts: &Fonts, app: &AppState)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    Line::new(Point::new(119, 34), Point::new(119, 255))
        .into_styled(PrimitiveStyle::with_stroke(palette::SURFACE_ALT, 1))
        .draw(target)
        .unwrap();

    for position in 0..25 {
        let (column, row) = if position < 13 {
            (0, position)
        } else {
            (1, position - 13)
        };
        if let Some(word) = app.mnemonic_word(position) {
            let label = alloc::format!("{:>2}. {}", position + 1, word);
            Text::new(
                &label,
                small_text_point(4 + column as i32 * 120, 47 + row as i32 * 17),
                fonts.small(palette::TEXT),
            )
            .draw(target)
            .unwrap();
        }
    }

    draw_frost_confirmation_footer(target, fonts);
}

fn draw_compact_seed_qr<D>(target: &mut D, fonts: &Fonts, app: &AppState)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    if let Some(wallet) = app.wallet() {
        let secret = wallet.entropy();
        if let Ok(qr) = encode_compact_seed_qr(secret.as_bytes()) {
            let scale = if qr.side == 21 { 8 } else { 7 };
            let symbol = (qr.side as u32 + 8) * scale;
            draw_qr_modules_on_white(
                target,
                &qr.modules,
                qr.side,
                scale,
                4,
                (DISPLAY_WIDTH - symbol) / 2,
                32,
            );
        }
    }
    Text::with_alignment(
        "Private backup - keep safe",
        text_point(120, 282),
        fonts.regular(palette::DANGER_TEXT),
        Alignment::Center,
    )
    .draw(target)
    .unwrap();
    draw_nav_back_button(target, fonts, app);
}

fn draw_home<D>(target: &mut D, fonts: &Fonts, app: &AppState)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    let (storage_icon, storage_label) = if app.wallet_is_saved() {
        (BitcoinIcon::Trash, "Delete wallet")
    } else {
        (BitcoinIcon::SdCard, "Save wallet")
    };
    let buttons = [
        (HOME_SCAN_BUTTON, BitcoinIcon::Scan, "Scan PSBT"),
        (HOME_BACKUP_BUTTON, BitcoinIcon::Mnemonic, "Show backup"),
        (HOME_STORAGE_BUTTON, storage_icon, storage_label),
        (HOME_LOGOUT_BUTTON, BitcoinIcon::Exit, "Log out"),
    ];
    for (bounds, icon, label) in buttons {
        draw_menu_item(target, fonts, bounds, icon, label);
    }
}

fn draw_mnemonic<D>(target: &mut D, fonts: &Fonts, app: &AppState, page: usize)
where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    if let Ok(mut mnemonic) = app.mnemonic() {
        for (visible, word) in mnemonic
            .split_whitespace()
            .skip(page * 12)
            .take(12)
            .enumerate()
        {
            let index = page * 12 + visible;
            let column = visible / 6;
            let row = visible % 6;
            let label = alloc::format!("{:>2}. {}", index + 1, word.to_ascii_uppercase());
            Text::new(
                &label,
                Point::new(12 + column as i32 * 116, 62 + row as i32 * 25),
                fonts.mnemonic(palette::TEXT),
            )
            .draw(target)
            .unwrap();
        }
        mnemonic.zeroize();
    }
}

fn draw_review<D>(
    target: &mut D,
    fonts: &Fonts,
    app: &AppState,
    hold_action: Option<HoldAction>,
    hold_ticks: u16,
) where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    let Some(review) = app.review() else { return };
    let mut y = 48;
    let mut recipients = 0usize;
    for output in review.outputs.iter().filter(|output| !output.is_change) {
        recipients += 1;
        let amount = alloc::format!("SEND {} sats", output.value_sat);
        Text::new(
            &amount,
            subheading_text_point(12, y),
            fonts.subheading(palette::TEXT),
        )
        .draw(target)
        .unwrap();
        y += 20;
        if let Some(address) = &output.address {
            for chunk in address.as_bytes().chunks(29) {
                if y > 204 {
                    break;
                }
                if let Ok(line) = core::str::from_utf8(chunk) {
                    Text::new(line, body_text_point(12, y), fonts.body(palette::TEXT))
                        .draw(target)
                        .unwrap();
                }
                y += 16;
            }
        } else {
            Text::new(
                "Unknown output script",
                body_text_point(12, y),
                fonts.body(palette::TEXT),
            )
            .draw(target)
            .unwrap();
            y += 16;
        }
        y += 8;
    }
    let change_count = review
        .outputs
        .iter()
        .filter(|output| output.is_change)
        .count();
    let summary = alloc::format!("Recipients: {recipients}  Change: {change_count}");
    Text::new(
        &summary,
        body_text_point(12, 236),
        fonts.body(palette::TEXT_MUTED),
    )
    .draw(target)
    .unwrap();
    let fee = match review.fee_sat {
        Some(value) => alloc::format!("Fee: {value} sats"),
        None => alloc::format!("Fee: unavailable"),
    };
    Text::new(
        &fee,
        body_text_point(12, 256),
        fonts.body(palette::TEXT_MUTED),
    )
    .draw(target)
    .unwrap();
    draw_confirmation_footer(
        target,
        fonts,
        if matches!(hold_action, Some(HoldAction::Sign)) {
            hold_ticks
        } else {
            0
        },
    );
}

/// Draws the body of the current screen.
///
/// Split out of `render_app` so each screen's drawing is reachable on its own;
/// the header and heading are already painted when this is called.
pub(crate) fn draw_screen_body<D>(
    target: &mut D,
    fonts: &Fonts,
    app: &AppState,
    hold_action: Option<HoldAction>,
    hold_ticks: u16,
) where
    D: DrawTarget<Color = Rgb565, Error = Infallible>,
{
    let title = || fonts.regular(palette::TEXT);
    let accent = || fonts.subheading(palette::TEXT);
    let body = || fonts.body(palette::TEXT_MUTED);
    match app.screen() {
        Screen::LoadWallet => draw_load_wallet(target, fonts),
        Screen::WalletList => draw_wallet_list(target, fonts, app),
        Screen::ImportWallet => {
            draw_menu_item(
                target,
                fonts,
                rect_tuple(layout::menus::IMPORT_WALLET.row(0)),
                BitcoinIcon::Scan,
                "Scan SeedQR",
            );
            draw_menu_item(
                target,
                fonts,
                rect_tuple(layout::menus::IMPORT_WALLET.row(1)),
                BitcoinIcon::Mnemonic,
                "Enter mnemonic",
            );
            draw_menu_item(
                target,
                fonts,
                rect_tuple(layout::menus::IMPORT_WALLET.row(2)),
                BitcoinIcon::Snowflake,
                "FROST",
            );
            draw_nav_back_button(target, fonts, app);
        }
        Screen::MnemonicLength => {
            draw_menu_item(
                target,
                fonts,
                rect_tuple(layout::menus::MNEMONIC_LENGTH.row(0)),
                BitcoinIcon::Mnemonic,
                "12 words",
            );
            draw_menu_item(
                target,
                fonts,
                rect_tuple(layout::menus::MNEMONIC_LENGTH.row(1)),
                BitcoinIcon::Mnemonic,
                "24 words",
            );
            draw_nav_back_button(target, fonts, app);
        }
        Screen::FrostThreshold => {
            draw_number_picker(
                target,
                fonts,
                app.frost_threshold() as u32,
                "backups required",
            );
            Text::with_alignment(
                "EMERGENCY RECOVERY ONLY",
                small_text_point(120, 216),
                fonts.small(palette::DANGER_TEXT),
                Alignment::Center,
            )
            .draw(target)
            .unwrap();
            for (line, baseline) in [
                ("This tool combines", 232),
                ("FROST backups into a", 248),
                ("single key", 264),
            ] {
                Text::with_alignment(
                    line,
                    small_text_point(120, baseline),
                    fonts.small(palette::TEXT_MUTED),
                    Alignment::Center,
                )
                .draw(target)
                .unwrap();
            }
            draw_confirmation_footer_icons(target, fonts, "Continue", BitcoinIcon::ChevronLeft);
        }
        Screen::FrostShareIndex => {
            let progress = alloc::format!(
                "Backup {} of {}",
                app.frost_shares_entered() + 1,
                app.frost_threshold()
            );
            draw_number_picker(target, fonts, app.frost_share_index(), &progress);
            draw_menu_item(
                target,
                fonts,
                (16, 150, 208, 44),
                BitcoinIcon::Mnemonic,
                "Enter 25 words",
            );
            if let Some(notice) = app.notice() {
                Text::with_alignment(
                    notice,
                    small_text_point(120, 220),
                    fonts.small(palette::DANGER_TEXT),
                    Alignment::Center,
                )
                .draw(target)
                .unwrap();
            }
            draw_nav_back_button(target, fonts, app);
        }
        Screen::MnemonicEntry | Screen::FrostMnemonicEntry => {
            draw_mnemonic_keyboard(target, fonts, app)
        }
        Screen::FrostMnemonicConfirm => draw_frost_mnemonic_confirmation(target, fonts, app),
        Screen::Network => {
            draw_menu_item_with_icon_color(
                target,
                fonts,
                rect_tuple(layout::menus::NETWORK.row(0)),
                BitcoinIcon::Globe,
                "Mainnet",
                palette::MAINNET,
            );
            draw_menu_item_with_icon_color(
                target,
                fonts,
                rect_tuple(layout::menus::NETWORK.row(1)),
                BitcoinIcon::Globe,
                "Testnet",
                palette::TESTNET,
            );
            draw_menu_item_with_icon_color(
                target,
                fonts,
                rect_tuple(layout::menus::NETWORK.row(2)),
                BitcoinIcon::Globe,
                "Signet",
                palette::SIGNET,
            );
            draw_nav_back_button(target, fonts, app);
        }
        Screen::Derivation => {
            if app.pending_wallet_kind() == WalletKind::Frost {
                // A FROST wallet has only one policy, shown in the middle slot.
                draw_menu_item(
                    target,
                    fonts,
                    rect_tuple(layout::menus::DERIVATION.row(1)),
                    BitcoinIcon::Wallet,
                    "Frostsnap Taproot",
                );
                Text::new("m/0/0/0/0", body_text_point(52, 174), body())
                    .draw(target)
                    .unwrap();
            } else {
                for (slot, label) in [(0, "BIP49 P2SH"), (1, "BIP84 Native"), (2, "BIP86 Taproot")]
                {
                    draw_menu_item(
                        target,
                        fonts,
                        rect_tuple(layout::menus::DERIVATION.row(slot)),
                        BitcoinIcon::Wallet,
                        label,
                    );
                }
            }
            draw_nav_back_button(target, fonts, app);
        }
        Screen::PassphraseChoice => {
            draw_menu_item(
                target,
                fonts,
                (16, 80, 208, 44),
                BitcoinIcon::Check,
                "No passphrase",
            );
            draw_menu_item(
                target,
                fonts,
                (16, 140, 208, 44),
                BitcoinIcon::Edit,
                "Enter passphrase",
            );
            Text::with_alignment(
                app.notice().unwrap_or("Passphrases cannot be recovered"),
                small_text_point(120, 220),
                fonts.small(palette::DANGER_TEXT),
                Alignment::Center,
            )
            .draw(target)
            .unwrap();
            draw_nav_back_button(target, fonts, app);
        }
        Screen::PassphraseEntry => draw_passphrase_keyboard(target, fonts, app),
        Screen::GenerationWords => {
            draw_menu_item(
                target,
                fonts,
                (16, 80, 208, 44),
                BitcoinIcon::Mnemonic,
                "12 words",
            );
            draw_menu_item(
                target,
                fonts,
                (16, 140, 208, 44),
                BitcoinIcon::Mnemonic,
                "24 words",
            );
            draw_nav_back_button(target, fonts, app);
        }
        Screen::EntropySources => {
            let camera = if app.has_entropy_source(EntropySource::Camera) {
                "Camera added"
            } else {
                "Add camera"
            };
            let dice = if app.has_entropy_source(EntropySource::D6Dice) {
                "Add more D6 rolls"
            } else {
                "Add D6 dice"
            };
            draw_menu_item(target, fonts, (16, 70, 208, 44), BitcoinIcon::Plus, camera);
            draw_menu_item(target, fonts, (16, 130, 208, 44), BitcoinIcon::Plus, dice);
            Text::new(
                "Hardware TRNG is always mixed",
                small_text_point(16, 212),
                fonts.small(palette::TEXT_MUTED),
            )
            .draw(target)
            .unwrap();
            draw_confirmation_footer_icons(target, fonts, "Create", BitcoinIcon::Cross);
        }
        Screen::DiceEntropy => {
            let mut grid = layout::dice::grid();
            grid.set_bounds(layout::dice::BOUNDS);
            for index in 0..grid.len() {
                let Some(rect) = grid.key_rect(index) else {
                    continue;
                };
                let label = alloc::format!("{}", layout::dice::face(index));
                draw_button(target, fonts, rect_tuple(rect), palette::PRIMARY, &label);
            }
            let count = alloc::format!("{} rolls recorded", app.dice_rolls());
            Text::with_alignment(&count, body_text_point(120, 220), body(), Alignment::Center)
                .draw(target)
                .unwrap();
            draw_confirmation_footer_icons(target, fonts, "Done", BitcoinIcon::Cross);
        }
        Screen::CameraEntropy => {}
        Screen::Deriving => {
            let derivation_label = if app.pending_wallet_kind() == WalletKind::Frost {
                "Deriving Frostsnap wallet"
            } else {
                match app.account_type() {
                    AccountType::NestedSegwit => "Deriving BIP49 wallet",
                    AccountType::NativeSegwit => "Deriving BIP84 wallet",
                    AccountType::Taproot => "Deriving BIP86 wallet",
                }
            };
            Text::new(derivation_label, subheading_text_point(16, 82), accent())
                .draw(target)
                .unwrap();
            Text::new("Please wait...", body_text_point(16, 108), body())
                .draw(target)
                .unwrap();
            draw_indeterminate_progress(target, DERIVATION_PROGRESS_BAR);
        }
        Screen::Backup => {
            let word_count = app
                .wallet()
                .map_or(12, |wallet| wallet.entropy().len() / 4 * 3);
            let backup_label = alloc::format!("Write down these {word_count} words");
            Text::new(&backup_label, subheading_text_point(16, 48), accent())
                .draw(target)
                .unwrap();
            let sources = match (
                app.used_entropy_source(EntropySource::Camera),
                app.used_entropy_source(EntropySource::D6Dice),
            ) {
                (true, true) => "TRNG + camera + D6",
                (true, false) => "TRNG + camera",
                (false, true) => "TRNG + D6",
                (false, false) => "Hardware TRNG",
            };
            Text::new(
                sources,
                small_text_point(16, 257),
                fonts.small(palette::TEXT_MUTED),
            )
            .draw(target)
            .unwrap();
            draw_mnemonic(target, fonts, app, 0);
            draw_confirmation_footer(
                target,
                fonts,
                if matches!(hold_action, Some(HoldAction::Backup)) {
                    hold_ticks
                } else {
                    0
                },
            );
            // Keep the security context above the mnemonic visually explicit.
            Text::new("Backup required", text_point(16, 24), title())
                .draw(target)
                .unwrap();
        }
        Screen::BackupMenu => {
            if app
                .wallet()
                .is_some_and(|wallet| wallet.kind() == WalletKind::Frost)
            {
                draw_menu_item(
                    target,
                    fonts,
                    (16, 80, 208, 44),
                    BitcoinIcon::Export,
                    "Descriptor",
                );
                Text::new(
                    "Restore again from your FROST cards",
                    small_text_point(16, 154),
                    fonts.small(palette::TEXT_MUTED),
                )
                .draw(target)
                .unwrap();
            } else {
                draw_menu_item(
                    target,
                    fonts,
                    (16, 80, 208, 44),
                    BitcoinIcon::Mnemonic,
                    "Show mnemonic",
                );
                draw_menu_item(
                    target,
                    fonts,
                    (16, 140, 208, 44),
                    BitcoinIcon::QrCode,
                    "SeedQR",
                );
                draw_menu_item(
                    target,
                    fonts,
                    (16, 200, 208, 44),
                    BitcoinIcon::Export,
                    "Descriptor",
                );
            }
            draw_nav_back_button(target, fonts, app);
        }
        Screen::BackupWords => {
            Text::new(
                "Keep these words private",
                subheading_text_point(16, 48),
                accent(),
            )
            .draw(target)
            .unwrap();
            draw_mnemonic(target, fonts, app, app.backup_page());
            draw_nav_back_button(target, fonts, app);
            Text::new("Wallet backup", text_point(16, 24), title())
                .draw(target)
                .unwrap();
        }
        Screen::BackupSeedQr => draw_compact_seed_qr(target, fonts, app),
        Screen::Recovery => {
            Text::with_alignment(
                app.notice().unwrap_or("Wallet operation failed"),
                text_point(120, 130),
                fonts.regular(palette::DANGER_TEXT),
                Alignment::Center,
            )
            .draw(target)
            .unwrap();
            draw_nav_back_button(target, fonts, app);
        }
        Screen::Home => draw_home(target, fonts, app),
        Screen::Settings => {
            draw_menu_item(
                target,
                fonts,
                layout::settings::USB_ROW,
                BitcoinIcon::Settings,
                "USB",
            );
            draw_switch(target, layout::settings::USB_SWITCH, app.usb_enabled());
            if app.settings_dirty() {
                draw_settings_save_banner(target, fonts);
            } else {
                draw_nav_back_button(target, fonts, app);
            }
        }
        Screen::DeleteWallet => {
            let fingerprint = app
                .wallet()
                .and_then(|wallet| wallet.master_fingerprint())
                .map(fingerprint_text)
                .unwrap_or_else(|| "????????".into());
            Text::new(
                "Are you sure you want to",
                subheading_text_point(16, 92),
                fonts.subheading(palette::TEXT),
            )
            .draw(target)
            .unwrap();
            let question = alloc::format!("delete wallet {fingerprint}?");
            Text::new(
                &question,
                subheading_text_point(16, 120),
                fonts.subheading(palette::TEXT),
            )
            .draw(target)
            .unwrap();
            draw_confirmation_footer(
                target,
                fonts,
                if matches!(hold_action, Some(HoldAction::DeleteWallet)) {
                    hold_ticks
                } else {
                    0
                },
            );
        }
        Screen::CancelEntryConfirm => {
            Text::new(
                "Are you sure you want to",
                subheading_text_point(16, 92),
                fonts.subheading(palette::TEXT),
            )
            .draw(target)
            .unwrap();
            Text::new(
                "cancel and lose what",
                subheading_text_point(16, 120),
                fonts.subheading(palette::TEXT),
            )
            .draw(target)
            .unwrap();
            Text::new(
                "you've entered?",
                subheading_text_point(16, 148),
                fonts.subheading(palette::TEXT),
            )
            .draw(target)
            .unwrap();
            draw_confirmation_footer_icons(target, fonts, "Confirm", BitcoinIcon::ChevronLeft);
        }
        Screen::Scan => {}
        Screen::Review => draw_review(target, fonts, app, hold_action, hold_ticks),
        Screen::Sign => {
            Text::new(
                "Signing reviewed PSBT...",
                subheading_text_point(16, 76),
                accent(),
            )
            .draw(target)
            .unwrap();
            Text::new(
                "Verifying transaction \ndigest",
                body_text_point(16, 102),
                body(),
            )
            .draw(target)
            .unwrap();
        }
        Screen::Signed => {
            Text::new(
                "PSBT signed successfully",
                subheading_text_point(16, 76),
                accent(),
            )
            .draw(target)
            .unwrap();
            Text::new(
                "Preparing animated BBQr...",
                body_text_point(16, 102),
                body(),
            )
            .draw(target)
            .unwrap();
        }
        Screen::Descriptor => {
            let rendered = app
                .export_descriptor()
                .ok()
                .and_then(|export| draw_descriptor_qr(target, export.descriptor.as_bytes()).ok());
            if rendered.is_none() {
                Text::with_alignment(
                    "Descriptor unavailable",
                    text_point(120, 144),
                    fonts.regular(palette::DANGER_TEXT),
                    Alignment::Center,
                )
                .draw(target)
                .unwrap();
            }
            draw_nav_back_button(target, fonts, app);
        }
        _ => {
            Text::new("State ready", subheading_text_point(16, 76), accent())
                .draw(target)
                .unwrap();
        }
    }
}
