#![no_std]
#![no_main]

use embedded_graphics::{
    pixelcolor::{Rgb565, RgbColor},
    prelude::DrawTarget,
};
use embedded_hal::delay::DelayNs;
use fugit::RateExtU32;
use hal::Clock;
use panic_halt as _;
use picosigner::lcd::{Orientation, WaveshareLcd};
use rp235x_hal as hal;

#[link_section = ".start_block"]
#[used]
pub static IMAGE_DEF: hal::block::ImageDef = hal::block::ImageDef::secure_exe();

const XTAL_FREQ_HZ: u32 = 12_000_000;

#[hal::entry]
fn main() -> ! {
    let mut pac = hal::pac::Peripherals::take().unwrap();
    let mut watchdog = hal::Watchdog::new(pac.WATCHDOG);
    let clocks = hal::clocks::init_clocks_and_plls(
        XTAL_FREQ_HZ,
        pac.XOSC,
        pac.CLOCKS,
        pac.PLL_SYS,
        pac.PLL_USB,
        &mut pac.RESETS,
        &mut watchdog,
    )
    .unwrap();
    let mut timer = hal::Timer::new_timer0(pac.TIMER0, &mut pac.RESETS, &clocks);
    let sio = hal::Sio::new(pac.SIO);
    let pins = hal::gpio::Pins::new(
        pac.IO_BANK0,
        pac.PADS_BANK0,
        sio.gpio_bank0,
        &mut pac.RESETS,
    );

    // Waveshare RP2350-Touch-LCD-2: SPI0 CLK=18, MOSI=19.
    let clk = pins.gpio18.into_function::<hal::gpio::FunctionSpi>();
    let mosi = pins.gpio19.into_function::<hal::gpio::FunctionSpi>();
    let spi = hal::spi::Spi::<_, _, _, 8>::new(pac.SPI0, (mosi, clk)).init(
        &mut pac.RESETS,
        clocks.peripheral_clock.freq(),
        62_500.kHz(),
        embedded_hal::spi::MODE_0,
    );

    let dc = pins.gpio16.into_push_pull_output();
    let cs = pins.gpio17.into_push_pull_output();
    let rst = pins.gpio20.into_push_pull_output();
    let mut backlight = pins.gpio15.into_push_pull_output();

    let mut display = WaveshareLcd::new(spi, dc, cs, rst);
    display.init(&mut timer, Orientation::Landscape);
    embedded_hal::digital::OutputPin::set_high(&mut backlight).unwrap();

    loop {
        display.clear(Rgb565::RED).unwrap();
        timer.delay_ms(1_000);
        display.clear(Rgb565::GREEN).unwrap();
        timer.delay_ms(1_000);
        display.clear(Rgb565::BLUE).unwrap();
        timer.delay_ms(1_000);
    }
}

#[link_section = ".bi_entries"]
#[used]
pub static PICOTOOL_ENTRIES: [hal::binary_info::EntryAddr; 5] = [
    hal::binary_info::rp_cargo_bin_name!(),
    hal::binary_info::rp_cargo_version!(),
    hal::binary_info::rp_program_description!(c"Waveshare LCD RGB cycle"),
    hal::binary_info::rp_cargo_homepage_url!(),
    hal::binary_info::rp_program_build_attribute!(),
];
