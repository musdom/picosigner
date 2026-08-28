//! Render every application screen to PNG so the UI can be reviewed offline.
//!
//! Run with: cargo run --target x86_64-unknown-linux-gnu -p picosigner-emulator --example screenshots -- OUTDIR
//!
//! The scenes themselves live in `picosigner_emulator::scenes` and are shared
//! with the golden-image tests, so what you see here is exactly what CI checks.

use std::{env, fs, path::PathBuf};

use embedded_graphics_simulator::OutputSettingsBuilder;
use picosigner_emulator::scenes;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = PathBuf::from(env::args().nth(1).unwrap_or_else(|| "screenshots".into()));
    fs::create_dir_all(&out)?;

    // Scaled up for reading on a desktop; the goldens are stored at 1x.
    let settings = OutputSettingsBuilder::new().scale(2).build();
    let scratch = tempfile::tempdir()?;

    for scene in scenes::all(scratch.path())? {
        let name = format!("{}.png", scene.name);
        scene
            .display
            .to_rgb_output_image(&settings)
            .save_png(out.join(&name))?;
        println!("wrote {name}");
    }

    println!("screenshots in {}", out.display());
    Ok(())
}
