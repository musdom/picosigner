//! Panic handler that reports the failure on the LCD.
//!
//! The first rendered line carries the line number and filename of the panic
//! site, followed by the wrapped panic message. Text is red, uses the
//! embedded-graphics default 6x10 ASCII font, and appears on a black screen.
//! Display bring-up reuses [`crate::init_display`] over stolen peripherals;
//! the whole path is best-effort and never allocates.

use core::fmt::{self, Write as _};
use core::panic::PanicInfo;

use embedded_graphics::{
    mono_font::{ascii::FONT_6X10, MonoTextStyle},
    pixelcolor::{Rgb565, RgbColor},
    prelude::{DrawTarget, Drawable, Point},
    text::Text,
};
use embedded_hal::{delay::DelayNs, digital::OutputPin};
use fugit::RateExtU32;
use picosigner_board::Orientation;
use rp235x_hal as hal;

/// 240 px / 6 px per glyph, minus a one-glyph margin on each side.
const LINE_CHARS: usize = 38;
const LINE_HEIGHT: i32 = 12;
const FIRST_Y: i32 = 12;
const LAST_Y: i32 = 312;
const MARGIN_X: i32 = 6;

/// Cycle-burn delay for the panic path, where the normal timer stack may be
/// unusable.
struct BusyDelay;

impl DelayNs for BusyDelay {
    fn delay_ns(&mut self, ns: u32) {
        // 5 ns per iteration is deliberately shorter than the ~6.7 ns a cycle
        // takes at 150 MHz, so this always overshoots the requested delay and
        // panel reset and command timings never fall short. `spin_loop` may
        // compile to nothing on Cortex-M, making the real delay loop-overhead
        // bound rather than the figure above — fine for a panic screen, but do
        // not rely on this as an accurate nanosecond delay.
        let mut ticks = ns / 5 + 1;
        while ticks > 0 {
            ticks -= 1;
            core::hint::spin_loop();
        }
    }
}

/// Fixed-capacity line writer: the panic handler must not allocate.
struct PanicWriter<'a, D>
where
    D: DrawTarget<Color = Rgb565>,
{
    display: &'a mut D,
    style: MonoTextStyle<'static, Rgb565>,
    y: i32,
    line: [u8; LINE_CHARS],
    len: usize,
}

impl<'a, D> PanicWriter<'a, D>
where
    D: DrawTarget<Color = Rgb565>,
{
    fn new(display: &'a mut D, style: MonoTextStyle<'static, Rgb565>) -> Self {
        Self {
            display,
            style,
            y: FIRST_Y,
            line: [0; LINE_CHARS],
            len: 0,
        }
    }

    fn newline(&mut self) {
        self.flush_line();
    }

    fn finish(&mut self) {
        self.flush_line();
    }

    fn flush_line(&mut self) {
        if self.len > 0 && self.y <= LAST_Y {
            if let Ok(text) = core::str::from_utf8(&self.line[..self.len]) {
                let _ = Text::new(text, Point::new(MARGIN_X, self.y), self.style)
                    .draw(self.display);
            }
            self.y += LINE_HEIGHT;
        }
        self.len = 0;
    }

    fn push_char(&mut self, c: char) {
        if self.y > LAST_Y {
            return;
        }

        if c == '\n' {
            self.flush_line();
            return;
        }

        let mut char_buf = [0u8; 4];
        let encoded = c.encode_utf8(&mut char_buf);
        let bytes = encoded.as_bytes();

        if self.len + bytes.len() > LINE_CHARS {
            self.flush_line();
        }

        if self.y <= LAST_Y {
            self.line[self.len..self.len + bytes.len()].copy_from_slice(bytes);
            self.len += bytes.len();
        }
    }
}

impl<D> fmt::Write for PanicWriter<'_, D>
where
    D: DrawTarget<Color = Rgb565>,
{
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            self.push_char(c);
        }
        Ok(())
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Freeze interrupts so the recovery path owns the hardware.
    unsafe { core::arch::asm!("cpsid i") };

    // A panic on core 1 must not fight core 0 for the display bus, so only
    // core 0 drives the panel; a second-core panic halts without rendering.
    let cpuid = unsafe { (*hal::pac::SIO::ptr()).cpuid().read().bits() };
    if cpuid != 0 {
        loop {
            core::hint::spin_loop();
        }
    }

    // Best-effort bring-up from stolen peripherals; the PL022 prescaler
    // saturates to a slow-but-valid rate if clocks were never configured.
    let mut pac = unsafe { hal::pac::Peripherals::steal() };
    let sio = hal::Sio::new(pac.SIO);
    let pins = hal::gpio::Pins::new(
        pac.IO_BANK0,
        pac.PADS_BANK0,
        sio.gpio_bank0,
        &mut pac.RESETS,
    );
    let mut delay = BusyDelay;
    let mut display = crate::init_display!(
        pac,
        pins,
        delay: &mut delay,
        freq: 150_000_000u32.Hz(),
        orientation: Orientation::PortraitTouchAligned,
    );

    // Re-light the backlight (GPIO15): Pins::new reset every pin to its
    // default, which kills the PWM-driven backlight and leaves the panel
    // dark even though it is rendering. Drive it as a plain push-pull high.
    let mut backlight = pins.gpio15.into_push_pull_output();
    let _ = backlight.set_high();
    let _ = display.clear(Rgb565::BLACK);
    let style = MonoTextStyle::new(&FONT_6X10, Rgb565::RED);
    let mut writer = PanicWriter::new(&mut display, style);

    // Required format: line number and filename first.
    match info.location() {
        Some(loc) => {
            let _ = write!(writer, "{}:{}", loc.line(), loc.file());
        }
        None => {
            let _ = writer.write_str("0:<unknown location>");
        }
    }
    writer.newline();

    // PanicMessage implements Display, so the payload needs no downcast and
    // no heap allocation.
    let _ = write!(writer, "{}", info.message());
    writer.finish();

    loop {
        core::hint::spin_loop();
    }
}
