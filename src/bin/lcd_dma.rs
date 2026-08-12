#![no_std]
#![no_main]

use embedded_graphics::{
    framebuffer::{buffer_size, Framebuffer},
    mono_font::{ascii::FONT_6X10, MonoTextStyle},
    pixelcolor::{raw::BigEndian, Rgb565, RgbColor},
    prelude::*,
    primitives::{Circle, PrimitiveStyleBuilder, Rectangle},
    text::Text,
};
use embedded_hal::{delay::DelayNs, digital::OutputPin};
use fugit::RateExtU32;
use hal::{
    clocks::Clock,
    dma::{single_buffer, DMAExt},
};
use panic_halt as _;
use picosigner::lcd::{Orientation, WaveshareLcd};
use rp235x_hal as hal;

#[link_section = ".start_block"]
#[used]
pub static IMAGE_DEF: hal::block::ImageDef = hal::block::ImageDef::secure_exe();

const XTAL_FREQ_HZ: u32 = 12_000_000;
const WIDTH: usize = 320;
const HEIGHT: usize = 240;
const FRAME_BYTES: usize = buffer_size::<Rgb565>(WIDTH, HEIGHT);

type LcdFramebuffer =
    Framebuffer<Rgb565, <Rgb565 as PixelColor>::Raw, BigEndian, WIDTH, HEIGHT, FRAME_BYTES>;

struct DmaFramebuffer(LcdFramebuffer);

// Safety: the framebuffer is allocated in static memory and cannot be
// accessed while this handle is owned by a DMA transfer. `wait()` returns the
// handle before the next drawing pass can start.
unsafe impl hal::dma::ReadTarget for &'static mut DmaFramebuffer {
    type ReceivedWord = u8;

    fn rx_treq() -> Option<u8> {
        None
    }

    fn rx_address_count(&self) -> (u32, u32) {
        (self.0.data().as_ptr() as u32, FRAME_BYTES as u32)
    }

    fn rx_increment(&self) -> bool {
        true
    }
}

struct Rng(u32);

impl Rng {
    fn next(&mut self) -> u32 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 17;
        value ^= value << 5;
        self.0 = value;
        value
    }

    fn range(&mut self, end: u32) -> u32 {
        self.next() % end
    }

    fn color(&mut self) -> Rgb565 {
        Rgb565::new(
            self.range(32) as u8,
            self.range(64) as u8,
            self.range(32) as u8,
        )
    }
}

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
    backlight.set_high().unwrap();

    let dma = pac.DMA.split(&mut pac.RESETS);
    let mut dma_channel = dma.ch0;
    let mut framebuffer =
        hal::singleton!(: DmaFramebuffer = DmaFramebuffer(LcdFramebuffer::new())).unwrap();
    let mut rng = Rng(timer.get_counter().ticks() as u32 | 1);
    let labels = ["RP2350", "DMA", "embedded-graphics", "Waveshare"];

    loop {
        framebuffer.0.clear(Rgb565::BLACK).unwrap();

        for _ in 0..18 {
            let width = 8 + rng.range(90);
            let height = 8 + rng.range(70);
            let x = rng.range(WIDTH as u32 - width);
            let y = rng.range(HEIGHT as u32 - height);
            let style = PrimitiveStyleBuilder::new()
                .fill_color(rng.color())
                .stroke_color(rng.color())
                .stroke_width(1 + rng.range(5))
                .build();
            Rectangle::new(Point::new(x as i32, y as i32), Size::new(width, height))
                .into_styled(style)
                .draw(&mut framebuffer.0)
                .unwrap();
        }

        for _ in 0..10 {
            let diameter = 8 + rng.range(60);
            let x = rng.range(WIDTH as u32 - diameter);
            let y = rng.range(HEIGHT as u32 - diameter);
            Circle::new(Point::new(x as i32, y as i32), diameter)
                .into_styled(
                    PrimitiveStyleBuilder::new()
                        .stroke_color(rng.color())
                        .stroke_width(1 + rng.range(5))
                        .build(),
                )
                .draw(&mut framebuffer.0)
                .unwrap();
        }

        for _ in 0..8 {
            let label = labels[rng.range(labels.len() as u32) as usize];
            let x = rng.range((WIDTH - label.len() * 6) as u32);
            let y = 10 + rng.range((HEIGHT - 10) as u32);
            Text::new(
                label,
                Point::new(x as i32, y as i32),
                MonoTextStyle::new(&FONT_6X10, rng.color()),
            )
            .draw(&mut framebuffer.0)
            .unwrap();
        }

        display.begin_frame_write();
        let (spi, dc, cs, rst, size) = display.release();
        let transfer = single_buffer::Config::new(dma_channel, framebuffer, spi).start();
        let (channel, frame, spi) = transfer.wait();
        dma_channel = channel;
        framebuffer = frame;
        display = WaveshareLcd::from_parts(spi, dc, cs, rst, size);
        display.end_frame_write();

        timer.delay_ms(250);
    }
}

#[link_section = ".bi_entries"]
#[used]
pub static PICOTOOL_ENTRIES: [hal::binary_info::EntryAddr; 5] = [
    hal::binary_info::rp_cargo_bin_name!(),
    hal::binary_info::rp_cargo_version!(),
    hal::binary_info::rp_program_description!(c"Waveshare LCD DMA framebuffer demo"),
    hal::binary_info::rp_cargo_homepage_url!(),
    hal::binary_info::rp_program_build_attribute!(),
];
