//! Deterministic UI scenes shared by the screenshot example and the golden tests.
//!
//! Every scene drives the real [`AppState`] and the real
//! [`render_app`][picosigner::ui::render_app], so a golden mismatch means the
//! shipped UI changed. Scenes must stay reproducible: no clock, no OS entropy,
//! and no dependency on files outside `fixtures/`.

use std::path::{Path, PathBuf};

use embedded_graphics::{geometry::Size, pixelcolor::Rgb565};
use embedded_graphics_simulator::SimulatorDisplay;
use picosigner::{
    app::{AppError, AppState},
    ui::{draw_qr_result, render_app, Fonts, HoldAction, DISPLAY_HEIGHT, DISPLAY_WIDTH},
};
use picosigner_board::{SeedStore, StoredSeed, WalletStore};
use picosigner_ui::{Navigator, Screen};
use picosigner_wallet::{AccountType, Network};

use crate::{FileSeedStore, OsEntropy};

/// A single rendered screen: a stable name and its framebuffer.
pub struct Scene {
    pub name: String,
    pub display: SimulatorDisplay<Rgb565>,
}

impl Scene {
    /// Raw big-endian RGB565 pixels, the exact bytes the panel would receive.
    pub fn pixels(&self) -> Vec<u8> {
        self.display.to_be_bytes()
    }
}

/// The committed PSBT fixture used by the review screens.
///
/// Built from the all-zero seed by `fixtures/review.psbt`, so the review and
/// hold-to-sign screens render identically on every machine. The emulator's
/// runtime `input.psbt` is deliberately not used: it is gitignored.
pub fn fixture_psbt() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("review.psbt")
}

fn ok<T>(result: Result<T, AppError>) -> Result<T, String> {
    result.map_err(|error| format!("{error:?}"))
}

fn shot(
    scenes: &mut Vec<Scene>,
    fonts: &Fonts,
    name: &str,
    app: &AppState,
    hold: Option<HoldAction>,
    ticks: u16,
) {
    let mut display = SimulatorDisplay::<Rgb565>::new(Size::new(DISPLAY_WIDTH, DISPLAY_HEIGHT));
    render_app(&mut display, fonts, app, hold, ticks);
    scenes.push(Scene {
        name: name.to_string(),
        display,
    });
}

/// Renders every application screen in a fixed order.
///
/// `scratch` holds throwaway wallet-state files; the caller owns its lifetime
/// (a `tempfile::TempDir` in tests, the output directory in the example).
pub fn all(scratch: &Path) -> Result<Vec<Scene>, String> {
    let fonts = Fonts::load();
    let mut scenes = Vec::new();
    let mut entropy = OsEntropy;

    let store_path = scratch.join("shot-state.bin");
    let _ = std::fs::remove_file(&store_path);
    let mut store = FileSeedStore::new(store_path.clone());

    // A wallet backed by a known mnemonic keeps the screenshots reproducible.
    let seed = StoredSeed::from_entropy(&[0u8; 16], picosigner_board::Network::Signet, false)
        .expect("valid entropy");
    store.store(seed).map_err(|error| format!("{error:?}"))?;
    let id = store
        .insert_wallet(seed)
        .unwrap_or(picosigner_board::WalletId(0));

    let empty_path = scratch.join("empty-shot-state.bin");
    let _ = std::fs::remove_file(&empty_path);
    let mut empty_store = FileSeedStore::new(empty_path);
    let mut empty_app = AppState::new(Network::Signet);
    ok(empty_app.boot(&mut entropy, &mut empty_store))?;
    shot(&mut scenes, &fonts, "00-empty-welcome", &empty_app, None, 0);
    empty_app.navigate(Screen::WalletList);
    shot(
        &mut scenes,
        &fonts,
        "00b-no-wallets-saved",
        &empty_app,
        None,
        0,
    );

    // Screens reachable without a loaded wallet.
    let mut app = AppState::new(Network::Signet);
    ok(app.boot(&mut entropy, &mut store))?;
    shot(&mut scenes, &fonts, "01-load-wallet", &app, None, 0);

    app.navigate(Screen::WalletList);
    shot(&mut scenes, &fonts, "02-wallet-list", &app, None, 0);

    // Exercise both pages of the maximum-size wallet catalog.
    for value in 1u8..8 {
        let network = match value % 3 {
            0 => picosigner_board::Network::Mainnet,
            1 => picosigner_board::Network::Signet,
            _ => picosigner_board::Network::Testnet4,
        };
        let wallet = StoredSeed::from_entropy(&[value; 16], network, false)
            .expect("valid screenshot entropy");
        store
            .insert_wallet(wallet)
            .map_err(|error| format!("{error:?}"))?;
    }
    let mut full_wallet_list = AppState::new(Network::Signet);
    ok(full_wallet_list.boot(&mut entropy, &mut store))?;
    full_wallet_list.navigate(Screen::WalletList);
    shot(
        &mut scenes,
        &fonts,
        "02b-wallet-list-page-1",
        &full_wallet_list,
        None,
        0,
    );
    full_wallet_list.next_wallet_list_page();
    shot(
        &mut scenes,
        &fonts,
        "02c-wallet-list-page-2",
        &full_wallet_list,
        None,
        0,
    );

    app.begin_import();
    shot(&mut scenes, &fonts, "03-import-wallet", &app, None, 0);

    app.choose_frost_import();
    shot(&mut scenes, &fonts, "03b-frost-recovery", &app, None, 0);
    app.confirm_frost_threshold();
    shot(&mut scenes, &fonts, "03c-frost-key-number", &app, None, 0);
    ok(app.start_frost_share_entry())?;
    for word in "mutual jeans snap sting bless journey moral bread room limit dose gravity sort deliver outdoor ripple donkey blouse play cart century maximum make local mobile".split_whitespace() {
        for letter in word.bytes() {
            app.mnemonic_letter(letter);
        }
        ok(app.choose_mnemonic_suggestion(0))?;
    }
    shot(
        &mut scenes,
        &fonts,
        "03d-frost-confirm-words",
        &app,
        None,
        0,
    );

    app.begin_import();
    app.choose_mnemonic_import();
    shot(&mut scenes, &fonts, "04-mnemonic-length", &app, None, 0);

    ok(app.start_mnemonic_entry(12))?;
    for letter in b"aba" {
        app.mnemonic_letter(*letter);
    }
    shot(&mut scenes, &fonts, "05-mnemonic-entry", &app, None, 0);

    app.begin_create();
    shot(&mut scenes, &fonts, "06-network", &app, None, 0);

    app.select_network(Network::Signet);
    shot(&mut scenes, &fonts, "07-derivation", &app, None, 0);
    app.select_account_type(AccountType::NativeSegwit);
    shot(&mut scenes, &fonts, "07a-passphrase-choice", &app, None, 0);
    app.begin_passphrase_entry();
    for letter in b"optional passphrase" {
        if *letter == b' ' {
            app.passphrase_space();
        } else {
            app.passphrase_letter(*letter);
        }
    }
    shot(&mut scenes, &fonts, "07b-passphrase-entry", &app, None, 0);
    app.open_cancel_entry_confirm();
    shot(
        &mut scenes,
        &fonts,
        "07b1-cancel-entry-confirm",
        &app,
        None,
        0,
    );
    // "No": what was typed survives underneath the prompt.
    app.go_back();
    app.confirm_passphrase();
    shot(&mut scenes, &fonts, "07c-wallet-size", &app, None, 0);
    ok(app.select_generation_words(24))?;
    shot(&mut scenes, &fonts, "07d-entropy-sources", &app, None, 0);
    app.begin_dice_entropy();
    for roll in [1, 6, 3, 4] {
        ok(app.add_dice_roll(roll))?;
    }
    shot(&mut scenes, &fonts, "07e-dice-entropy", &app, None, 0);
    app.navigate(Screen::Deriving);
    shot(&mut scenes, &fonts, "07f-deriving", &app, None, 0);

    // Screens that need a loaded, backed-up wallet.
    let mut app = AppState::new(Network::Signet);
    ok(app.boot(&mut entropy, &mut store))?;
    ok(app.load_wallet_id(id, &mut store))?;

    app.navigate(Screen::Backup);
    shot(
        &mut scenes,
        &fonts,
        "08-backup-required",
        &app,
        Some(HoldAction::Backup),
        18,
    );

    app.navigate(Screen::Home);
    shot(&mut scenes, &fonts, "09-home", &app, None, 0);
    app.navigate(Screen::DeleteWallet);
    shot(&mut scenes, &fonts, "10-delete-wallet", &app, None, 0);
    shot(
        &mut scenes,
        &fonts,
        "10b-delete-wallet-hold",
        &app,
        Some(HoldAction::DeleteWallet),
        20,
    );

    app.navigate(Screen::Home);
    ok(app.open_backup_menu())?;
    shot(&mut scenes, &fonts, "11-backup-menu", &app, None, 0);

    app.navigate(Screen::BackupWords);
    shot(&mut scenes, &fonts, "12-backup-words", &app, None, 0);

    ok(app.open_backup_seed_qr())?;
    shot(&mut scenes, &fonts, "13-backup-seedqr", &app, None, 0);

    app.navigate(Screen::Descriptor);
    shot(&mut scenes, &fonts, "14-descriptor", &app, None, 0);

    app.navigate(Screen::Scan);
    shot(&mut scenes, &fonts, "15-scan", &app, None, 0);

    // Camera controls bypass render_app on hardware, so render both navigation
    // variants explicitly: PSBT exits home; SeedQR and entropy return to setup.
    for (name, icon) in [
        ("15c-scan-home", picosigner::icons::BitcoinIcon::Cross),
        ("15d-scan-back", picosigner::icons::BitcoinIcon::ChevronLeft),
    ] {
        let mut display = SimulatorDisplay::<Rgb565>::new(Size::new(DISPLAY_WIDTH, DISPLAY_HEIGHT));
        picosigner::ui::draw_scan_overlay(&mut display, &fonts, 1, (0, 0), icon);
        scenes.push(Scene {
            name: name.to_string(),
            display,
        });
    }

    let mut qr_result = SimulatorDisplay::<Rgb565>::new(Size::new(DISPLAY_WIDTH, DISPLAY_HEIGHT));
    draw_qr_result(
        &mut qr_result,
        &fonts,
        b"https://example.com/decoded-qr?example=picosigner",
    );
    scenes.push(Scene {
        name: "15b-qr-result".to_string(),
        display: qr_result,
    });

    // The committed fixture must parse; unlike the runtime `input.psbt` this is
    // not optional, so a regression in PSBT review fails the golden run.
    let bytes = std::fs::read(fixture_psbt())
        .map_err(|error| format!("read {}: {error}", fixture_psbt().display()))?;
    ok(app.receive_psbt(&bytes))?;
    shot(&mut scenes, &fonts, "16-review", &app, None, 0);
    shot(
        &mut scenes,
        &fonts,
        "17-review-hold",
        &app,
        Some(HoldAction::Sign),
        22,
    );

    app.navigate(Screen::Sign);
    shot(&mut scenes, &fonts, "18-sign", &app, None, 0);
    app.navigate(Screen::Signed);
    shot(&mut scenes, &fonts, "19-signed", &app, None, 0);

    app.show_error(AppError::DuplicateWallet);
    shot(&mut scenes, &fonts, "20-recovery", &app, None, 0);

    let _ = std::fs::remove_file(&store_path);
    Ok(scenes)
}
