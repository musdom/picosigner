//! Golden-image tests for every application screen.
//!
//! Each scene renders the real `AppState` through the real renderer and is
//! compared byte-for-byte against a committed baseline in `tests/golden/`.
//! Any change to layout, colour, or copy fails here, which is what makes UI
//! refactoring safe.
//!
//! To re-bless the baselines after an intentional visual change:
//!
//! ```console
//! UPDATE_GOLDEN=1 cargo test -p picosigner-emulator --target x86_64-unknown-linux-gnu --test golden
//! ```
//!
//! Review the resulting diff before committing: a re-blessed baseline that was
//! not eyeballed silently stops protecting the screen it covers.

use std::path::{Path, PathBuf};

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics_simulator::{OutputSettingsBuilder, SimulatorDisplay};
use picosigner_emulator::scenes;

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
}

/// Baselines are stored as PNGs so a reviewer can look at them in the diff,
/// but comparison uses raw RGB565 pixels so it never depends on how the PNG
/// encoder happened to compress them.
fn baseline_pixels(path: &Path) -> Option<Vec<u8>> {
    let image = SimulatorDisplay::<Rgb565>::load_png(path).ok()?;
    Some(image.to_be_bytes())
}

#[test]
fn every_screen_matches_its_golden_image() {
    let update = std::env::var_os("UPDATE_GOLDEN").is_some();
    let dir = golden_dir();
    std::fs::create_dir_all(&dir).expect("create golden dir");

    let scratch = tempfile::tempdir().expect("scratch dir");
    let scenes = scenes::all(scratch.path()).expect("render scenes");
    assert!(!scenes.is_empty(), "no scenes rendered");

    // Written at 1x so the baseline holds exactly the pixels the panel gets.
    let settings = OutputSettingsBuilder::new().scale(1).build();
    let mut failures = Vec::new();

    for scene in &scenes {
        let path = dir.join(format!("{}.png", scene.name));
        let actual = scene.pixels();

        if update || !path.exists() {
            scene
                .display
                .to_rgb_output_image(&settings)
                .save_png(&path)
                .expect("save golden");
            continue;
        }

        match baseline_pixels(&path) {
            Some(expected) if expected == actual => {}
            Some(expected) => {
                let differing = expected
                    .chunks_exact(2)
                    .zip(actual.chunks_exact(2))
                    .filter(|(a, b)| a != b)
                    .count();
                let total = actual.len() / 2;
                failures.push(format!("{}: {differing}/{total} pixels differ", scene.name));
                // Write the actual render next to the baseline for inspection.
                scene
                    .display
                    .to_rgb_output_image(&settings)
                    .save_png(dir.join(format!("{}.actual.png", scene.name)))
                    .expect("save actual");
            }
            None => failures.push(format!("{}: baseline unreadable", scene.name)),
        }
    }

    assert!(
        failures.is_empty(),
        "{} screen(s) changed:\n  {}\n\nInspect the *.actual.png files next to each baseline. \
         If the change is intentional, re-bless with UPDATE_GOLDEN=1.",
        failures.len(),
        failures.join("\n  ")
    );
}

/// Scene names are the golden filenames, so duplicates would silently
/// overwrite each other and leave a screen uncovered.
#[test]
fn scene_names_are_unique() {
    let scratch = tempfile::tempdir().expect("scratch dir");
    let scenes = scenes::all(scratch.path()).expect("render scenes");
    let mut names: Vec<&str> = scenes.iter().map(|scene| scene.name.as_str()).collect();
    names.sort_unstable();
    let before = names.len();
    names.dedup();
    assert_eq!(before, names.len(), "duplicate scene name");
}

/// Rendering must not depend on wall-clock time, RNG, or leftover files.
#[test]
fn scenes_are_deterministic() {
    let a = scenes::all(tempfile::tempdir().unwrap().path()).expect("first render");
    let b = scenes::all(tempfile::tempdir().unwrap().path()).expect("second render");
    assert_eq!(a.len(), b.len());
    for (first, second) in a.iter().zip(b.iter()) {
        assert_eq!(first.name, second.name);
        assert!(
            first.pixels() == second.pixels(),
            "{} rendered differently across runs",
            first.name
        );
    }
}
