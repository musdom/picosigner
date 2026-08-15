use std::{env, process::ExitCode, thread, time::Duration};

use embedded_graphics::{geometry::Size, pixelcolor::Rgb565};
use embedded_graphics_simulator::{
    sdl2::MouseButton, OutputSettingsBuilder, SimulatorDisplay, SimulatorEvent, Window,
};
use picosigner::ui::{
    draw_signed_psbt_qr, render_app, render_message, Fonts, DISPLAY_HEIGHT, DISPLAY_WIDTH,
};
use picosigner_emulator::{help, Config, Controller};
use picosigner_qr::BbqrExport;
use picosigner_ui::Screen;

fn main() -> ExitCode {
    if env::args_os()
        .skip(1)
        .any(|arg| arg == "--help" || arg == "-h")
    {
        println!("{}", help());
        return ExitCode::SUCCESS;
    }
    let config = match Config::parse(env::args_os().skip(1)) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("picosigner-emulator: {error}\n{}", help());
            return ExitCode::from(2);
        }
    };
    match run(config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("picosigner-emulator: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let settings = OutputSettingsBuilder::new().scale(config.scale).build();
    let mut window = Window::new("PicoSigner emulator", &settings);
    let mut display = SimulatorDisplay::<Rgb565>::new(Size::new(DISPLAY_WIDTH, DISPLAY_HEIGHT));
    let fonts = Fonts::load();
    let mut controller = Controller::new(config)?;
    let mut export_frame = 0usize;
    let mut export_ticks = 0u8;

    'running: loop {
        controller.tick();
        if controller.app.screen() == Screen::Signed && controller.status().is_none() {
            export_ticks = export_ticks.wrapping_add(1);
            if export_ticks >= 16 {
                export_ticks = 0;
                export_frame = export_frame.wrapping_add(1);
            }
        } else {
            export_frame = 0;
            export_ticks = 0;
        }
        render(&mut display, &fonts, &controller, export_frame);
        window.update(&display);

        for event in window.events() {
            match event {
                SimulatorEvent::Quit => break 'running,
                SimulatorEvent::MouseButtonDown {
                    mouse_btn: MouseButton::Left,
                    point,
                } => {
                    if let Some((x, y)) = display_point(point.x, point.y) {
                        controller.mouse_down(x, y);
                    }
                }
                SimulatorEvent::MouseMove { point } => {
                    if let Some((x, y)) = display_point(point.x, point.y) {
                        controller.mouse_move(x, y);
                    }
                }
                SimulatorEvent::MouseButtonUp {
                    mouse_btn: MouseButton::Left,
                    point,
                } => {
                    if let Some((x, y)) = display_point(point.x, point.y) {
                        controller.mouse_up(x, y);
                    }
                }
                _ => {}
            }
        }
        thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

fn render(
    display: &mut SimulatorDisplay<Rgb565>,
    fonts: &Fonts,
    controller: &Controller,
    frame_index: usize,
) {
    if let Some(message) = controller.status() {
        let lines = wrap_message(message);
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        render_message(display, fonts, "EMULATOR ERROR", &refs);
        return;
    }
    if controller.app.screen() == Screen::Signed {
        if let Some(signed) = controller.app.signed_psbt() {
            if let Ok(export) = BbqrExport::new_psbt(&signed.bytes, 45) {
                let frame_index = frame_index % export.frame_count();
                let mut frame = heapless::Vec::<u8, 84>::new();
                if export.frame(frame_index, &mut frame).is_ok() {
                    draw_signed_psbt_qr(display, fonts, &frame, frame_index, export.frame_count());
                    return;
                }
            }
        }
    }
    let (hold_ticks, _) = controller.hold_progress();
    render_app(
        display,
        fonts,
        &controller.app,
        controller.action(),
        hold_ticks,
    );
}

fn display_point(x: i32, y: i32) -> Option<(u16, u16)> {
    (x >= 0 && y >= 0 && x < DISPLAY_WIDTH as i32 && y < DISPLAY_HEIGHT as i32)
        .then_some((x as u16, y as u16))
}

fn wrap_message(message: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for words in message.split_whitespace().collect::<Vec<_>>().chunks(4) {
        lines.push(words.join(" "));
        if lines.len() == 8 {
            break;
        }
    }
    lines
}
