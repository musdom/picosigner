//! Shared display bring-up for the Waveshare RP2350-Touch-LCD-2 panel.
//!
//! [`init_display!`] expands to the pin/SPI/panel reset sequence so every
//! embedded binary and the panic handler bring the panel up identically.

/// Bring up the Waveshare 2-inch LCD on SPI0 and return an initialized
/// `WaveshareLcd`.
///
/// `$pac` and `$pins` must be owned local variables: the macro moves `SPI0`
/// out of the peripherals and the five display pins out of the pin bank, and
/// borrows `RESETS`. Pin map: CLK=18, MOSI=19, DC=16, CS=17, RST=20. The
/// backlight (GPIO15) stays with the caller.
///
/// `freq:` is the peripheral clock feeding SPI0 (normally
/// `clocks.peripheral_clock.freq()`); the panic path hardcodes 150 MHz. If
/// clocks were never configured, the PL022 prescaler saturates at
/// peripheral/2, which still yields a valid, slower baud rate.
///
/// The panel is driven at 75 MHz — peripheral_clock/2, the fastest the PL022
/// prescaler can produce from a 150 MHz peripheral clock.
#[macro_export]
macro_rules! init_display {
    ($pac:ident, $pins:ident, delay: $delay:expr, freq: $freq:expr, orientation: $orientation:expr $(,)?) => {{
        let spi = $crate::hw::hal::spi::Spi::<_, _, _, 8>::new(
            $pac.SPI0,
            (
                $pins
                    .gpio19
                    .into_function::<$crate::hw::hal::gpio::FunctionSpi>(),
                $pins
                    .gpio18
                    .into_function::<$crate::hw::hal::gpio::FunctionSpi>(),
            ),
        )
        .init(
            &mut $pac.RESETS,
            $freq,
            <u32 as $crate::hw::fugit::RateExtU32>::MHz(75u32),
            $crate::hw::embedded_hal::spi::MODE_0,
        );
        let mut display = $crate::hw::board::WaveshareLcd::new(
            spi,
            $pins.gpio16.into_push_pull_output(),
            $pins.gpio17.into_push_pull_output(),
            $pins.gpio20.into_push_pull_output(),
        );
        display.init($delay, $orientation);
        display
    }};
}
