//! Render every application screen to PNG so the UI can be reviewed offline.
//!
//! Run with: cargo run --target x86_64-unknown-linux-gnu -p picosigner-emulator --example screenshots -- OUTDIR

use std::{env, fs, path::PathBuf};

use embedded_graphics::{geometry::Size, pixelcolor::Rgb565};
use embedded_graphics_simulator::{OutputSettingsBuilder, SimulatorDisplay};
use picosigner::{
    app::AppState,
    ui::{draw_qr_result, render_app, Fonts, HoldAction, DISPLAY_HEIGHT, DISPLAY_WIDTH},
};
use picosigner_board::{SeedStore, StoredSeed, WalletStore};
use picosigner_emulator::{FileSeedStore, OsEntropy};
use picosigner_ui::{Navigator, Screen};
use picosigner_wallet::Network;

/// `AppError` is a `no_std` enum without `Error`, so surface it as a string.
fn ok<T>(result: Result<T, picosigner::app::AppError>) -> Result<T, String> {
    result.map_err(|error| format!("{error:?}"))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = PathBuf::from(env::args().nth(1).unwrap_or_else(|| "screenshots".into()));
    fs::create_dir_all(&out)?;

    let fonts = Fonts::load();
    let settings = OutputSettingsBuilder::new().scale(2).build();

    let tmp = out.join("shot-state.bin");
    let _ = fs::remove_file(&tmp);
    let mut store = FileSeedStore::new(tmp.clone());
    let mut entropy = OsEntropy;

    // A wallet backed by a known mnemonic keeps the screenshots reproducible.
    let seed = StoredSeed::from_entropy(&[0u8; 16], picosigner_board::Network::Signet, false)
        .expect("valid entropy");
    store.store(seed)?;
    let id = store
        .insert_wallet(seed)
        .unwrap_or(picosigner_board::WalletId(0));

    let shot = |name: &str, app: &AppState, hold: Option<HoldAction>, ticks: u16| {
        let mut display = SimulatorDisplay::<Rgb565>::new(Size::new(DISPLAY_WIDTH, DISPLAY_HEIGHT));
        render_app(&mut display, &fonts, app, hold, ticks);
        display
            .to_rgb_output_image(&settings)
            .save_png(out.join(format!("{name}.png")))
            .expect("save png");
        println!("wrote {name}.png");
    };

    let empty_tmp = out.join("empty-shot-state.bin");
    let _ = fs::remove_file(&empty_tmp);
    let mut empty_store = FileSeedStore::new(empty_tmp);
    let mut empty_app = AppState::new(Network::Signet);
    ok(empty_app.boot(&mut entropy, &mut empty_store))?;
    shot("00-empty-welcome", &empty_app, None, 0);
    empty_app.navigate(Screen::WalletList);
    shot("00b-no-wallets-saved", &empty_app, None, 0);

    // Screens reachable without a loaded wallet.
    let mut app = AppState::new(Network::Signet);
    ok(app.boot(&mut entropy, &mut store))?;
    shot("01-load-wallet", &app, None, 0);

    app.navigate(Screen::WalletList);
    shot("02-wallet-list", &app, None, 0);

    // Exercise both pages of the maximum-size wallet catalog.
    for value in 1u8..8 {
        let network = match value % 3 {
            0 => picosigner_board::Network::Mainnet,
            1 => picosigner_board::Network::Signet,
            _ => picosigner_board::Network::Testnet4,
        };
        let wallet = StoredSeed::from_entropy(&[value; 16], network, false)
            .expect("valid screenshot entropy");
        store.insert_wallet(wallet)?;
    }
    let mut full_wallet_list = AppState::new(Network::Signet);
    ok(full_wallet_list.boot(&mut entropy, &mut store))?;
    full_wallet_list.navigate(Screen::WalletList);
    shot("02b-wallet-list-page-1", &full_wallet_list, None, 0);
    full_wallet_list.next_wallet_list_page();
    shot("02c-wallet-list-page-2", &full_wallet_list, None, 0);

    app.begin_import();
    shot("03-import-wallet", &app, None, 0);

    app.choose_frost_import();
    shot("03b-frost-recovery", &app, None, 0);
    app.confirm_frost_threshold();
    shot("03c-frost-key-number", &app, None, 0);
    ok(app.start_frost_share_entry())?;
    for word in "mutual jeans snap sting bless journey moral bread room limit dose gravity sort deliver outdoor ripple donkey blouse play cart century maximum make local mobile".split_whitespace() {
        for letter in word.bytes() {
            app.mnemonic_letter(letter);
        }
        ok(app.choose_mnemonic_suggestion(0))?;
    }
    shot("03d-frost-confirm-words", &app, None, 0);

    app.begin_import();
    app.choose_mnemonic_import();
    shot("04-mnemonic-length", &app, None, 0);

    ok(app.start_mnemonic_entry(12))?;
    for letter in b"aba" {
        app.mnemonic_letter(*letter);
    }
    shot("05-mnemonic-entry", &app, None, 0);

    app.begin_create();
    shot("06-network", &app, None, 0);

    app.select_network(Network::Signet);
    shot("07-derivation", &app, None, 0);
    app.navigate(Screen::Deriving);
    shot("07b-deriving", &app, None, 0);

    // Screens that need a loaded, backed-up wallet.
    let mut app = AppState::new(Network::Signet);
    ok(app.boot(&mut entropy, &mut store))?;
    ok(app.load_wallet_id(id, &mut store))?;

    app.navigate(Screen::Backup);
    shot("08-backup-required", &app, Some(HoldAction::Backup), 18);

    app.navigate(Screen::Home);
    shot("09-home", &app, None, 0);
    app.navigate(Screen::DeleteWallet);
    shot("10-delete-wallet", &app, None, 0);
    shot(
        "10b-delete-wallet-hold",
        &app,
        Some(HoldAction::DeleteWallet),
        20,
    );

    app.navigate(Screen::Home);
    ok(app.open_backup_menu())?;
    shot("11-backup-menu", &app, None, 0);

    app.navigate(Screen::BackupWords);
    shot("12-backup-words", &app, None, 0);

    ok(app.open_backup_seed_qr())?;
    shot("13-backup-seedqr", &app, None, 0);

    app.navigate(Screen::Descriptor);
    shot("14-descriptor", &app, None, 0);

    app.navigate(Screen::Scan);
    shot("15-scan", &app, None, 0);

    let mut qr_result = SimulatorDisplay::<Rgb565>::new(Size::new(DISPLAY_WIDTH, DISPLAY_HEIGHT));
    draw_qr_result(
        &mut qr_result,
        &fonts,
        b"https://example.com/decoded-qr?example=picosigner",
    );
    qr_result
        .to_rgb_output_image(&settings)
        .save_png(out.join("15b-qr-result.png"))?;
    println!("wrote 15b-qr-result.png");

    let psbt = picosigner_emulator::read_psbt_file(&PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/input.psbt"
    )));
    match psbt.and_then(|bytes| {
        app.receive_psbt(&bytes)
            .map(|_| ())
            .map_err(|error| picosigner_emulator::EmulatorError::App(format!("{error:?}")))
    }) {
        Ok(()) => {
            shot("16-review", &app, None, 0);
            shot("17-review-hold", &app, Some(HoldAction::Sign), 22);
        }
        Err(error) => eprintln!("skipping review screens: {error}"),
    }

    app.navigate(Screen::Sign);
    shot("18-sign", &app, None, 0);
    app.navigate(Screen::Signed);
    shot("19-signed", &app, None, 0);

    app.show_error(picosigner::app::AppError::DuplicateWallet);
    shot("20-recovery", &app, None, 0);

    let _ = fs::remove_file(&tmp);
    println!("screenshots in {}", out.display());
    Ok(())
}
