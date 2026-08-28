use embedded_hal::{delay::DelayNs, i2c::I2c, pwm::SetDutyCycle};
use picosigner::app::AppState;
use picosigner::ui::{in_button, Fonts, DESCRIPTOR_BACK_BUTTON};
use picosigner_board::{Cst816d, WaveshareLcd};
use picosigner_qr::BbqrExport;
use picosigner_ui::{Gesture, GestureConfig, TouchTracker, UiEvent};

use picosigner_board::framebuffer::frame_mut;

use super::{
    qr_views::{draw_signed_psbt_qr, render_signed_export_error},
    BACKLIGHT_DUTY,
};

pub fn run_signed_export<SPI, DC, CS, RST, TouchI2c, PWM, DELAY>(
    app: &AppState,
    fonts: &Fonts,
    display: &mut WaveshareLcd<SPI, DC, CS, RST>,
    touch: &mut Cst816d<TouchI2c>,
    backlight: &mut PWM,
    timer: &mut DELAY,
    backlight_level: &mut usize,
) where
    SPI: embedded_hal::spi::SpiBus<u8>,
    DC: embedded_hal::digital::OutputPin,
    CS: embedded_hal::digital::OutputPin,
    RST: embedded_hal::digital::OutputPin,
    TouchI2c: I2c,
    PWM: SetDutyCycle,
    DELAY: DelayNs,
{
    let export = app
        .signed_psbt()
        .ok_or(())
        // A multipart BBQr Base32 block must encode a whole number of bytes.
        // Forty-five bytes becomes exactly 72 Base32 characters; using 47
        // bytes introduced padding bits at every fragment boundary.
        .and_then(|signed| BbqrExport::new_psbt(&signed.bytes, 45).map_err(|_| ()));
    let mut touch_active = true;
    let mut last_touch = (0u16, 0u16);
    let mut tracker = TouchTracker::new(GestureConfig::default());
    let mut exit = false;

    if let Ok(export) = export {
        let mut frame = heapless::Vec::<u8, 84>::new();
        let mut frame_index = 0usize;
        while !exit {
            if export.frame(frame_index, &mut frame).is_ok() {
                let display_frame = unsafe { frame_mut() };
                draw_signed_psbt_qr(
                    display_frame,
                    fonts,
                    &frame,
                    frame_index,
                    export.frame_count(),
                );
                display.write_frame(display_frame);
            }
            // About three frames per second while polling touch frequently.
            for _ in 0..16 {
                handle_touch(
                    touch,
                    backlight,
                    backlight_level,
                    &mut touch_active,
                    &mut last_touch,
                    &mut tracker,
                    &mut exit,
                );
                if exit {
                    break;
                }
                timer.delay_ms(20);
            }
            frame_index = (frame_index + 1) % export.frame_count();
        }
    } else {
        render_signed_export_error(display, fonts);
        while !exit {
            handle_touch(
                touch,
                backlight,
                backlight_level,
                &mut touch_active,
                &mut last_touch,
                &mut tracker,
                &mut exit,
            );
            timer.delay_ms(20);
        }
    }
}

fn handle_touch<I2C, PWM>(
    touch: &mut Cst816d<I2C>,
    backlight: &mut PWM,
    backlight_level: &mut usize,
    touch_active: &mut bool,
    last_touch: &mut (u16, u16),
    tracker: &mut TouchTracker,
    exit: &mut bool,
) where
    I2C: embedded_hal::i2c::I2c,
    PWM: SetDutyCycle,
{
    match touch.point() {
        Ok(None) if *touch_active => {
            *touch_active = false;
            let Some(event) = tracker.event(UiEvent::TouchUp {
                x: last_touch.0,
                y: last_touch.1,
            }) else {
                return;
            };
            match event.gesture {
                Gesture::SwipeRight => *exit = true,
                Gesture::Tap if in_button(event.x, event.y, DESCRIPTOR_BACK_BUTTON) => *exit = true,
                Gesture::Tap if event.y < DESCRIPTOR_BACK_BUTTON.1 as u16 => {
                    *backlight_level = (*backlight_level + 1) % BACKLIGHT_DUTY.len();
                    backlight
                        .set_duty_cycle(BACKLIGHT_DUTY[*backlight_level])
                        .unwrap();
                }
                _ => {}
            }
        }
        Ok(None) => {}
        Ok(Some(point)) if !*touch_active => {
            *touch_active = true;
            *last_touch = (point.x, point.y);
            let _ = tracker.event(UiEvent::TouchDown {
                x: point.x,
                y: point.y,
            });
        }
        Ok(Some(point)) => *last_touch = (point.x, point.y),
        _ => {}
    }
}
