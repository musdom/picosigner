#![no_std]

//! Host-testable UI primitives for the 240x320 PicoSigner display.
//! One shared framebuffer is owned by the renderer; views only receive clipped contexts.

pub const DISPLAY_WIDTH: u16 = 240;
pub const DISPLAY_HEIGHT: u16 = 320;
pub const FRAMEBUFFER_PIXELS: usize = DISPLAY_WIDTH as usize * DISPLAY_HEIGHT as usize;
pub const FRAMEBUFFER_BYTES: usize = FRAMEBUFFER_PIXELS * core::mem::size_of::<u16>();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiEvent {
    TouchDown { x: u16, y: u16 },
    TouchUp { x: u16, y: u16 },
    Tick,
    Back,
    Confirm,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    Tap,
    Hold,
    SwipeLeft,
    SwipeRight,
    SwipeUp,
    SwipeDown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GestureEvent {
    pub gesture: Gesture,
    pub x: u16,
    pub y: u16,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GestureConfig {
    pub hold_ticks: u16,
    pub swipe_pixels: u16,
}
impl Default for GestureConfig {
    fn default() -> Self {
        Self {
            hold_ticks: 30,
            swipe_pixels: 32,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct TouchTracker {
    start: Option<(u16, u16)>,
    ticks: u16,
    config: GestureConfig,
}
impl TouchTracker {
    pub const fn new(config: GestureConfig) -> Self {
        Self {
            start: None,
            ticks: 0,
            config,
        }
    }

    /// Returns the current hold duration, capped at the configured
    /// confirmation duration. This is intended for rendering a live progress
    /// indicator while the finger remains down.
    pub fn hold_progress(&self) -> (u16, u16) {
        (
            self.ticks.min(self.config.hold_ticks),
            self.config.hold_ticks,
        )
    }

    pub fn active(&self) -> bool {
        self.start.is_some()
    }
    pub fn event(&mut self, event: UiEvent) -> Option<GestureEvent> {
        match event {
            UiEvent::TouchDown { x, y } => {
                self.start = Some((x, y));
                self.ticks = 0;
                None
            }
            UiEvent::Tick if self.start.is_some() => {
                self.ticks = self.ticks.saturating_add(1);
                None
            }
            UiEvent::TouchUp { x, y } => {
                let (sx, sy) = self.start.take()?;
                let dx = x as i32 - sx as i32;
                let dy = y as i32 - sy as i32;
                let gesture = if self.ticks >= self.config.hold_ticks
                    && dx.abs() < self.config.swipe_pixels as i32
                    && dy.abs() < self.config.swipe_pixels as i32
                {
                    Gesture::Hold
                } else if dx.abs() >= self.config.swipe_pixels as i32 && dx.abs() > dy.abs() {
                    if dx < 0 {
                        Gesture::SwipeLeft
                    } else {
                        Gesture::SwipeRight
                    }
                } else if dy.abs() >= self.config.swipe_pixels as i32 {
                    if dy < 0 {
                        Gesture::SwipeUp
                    } else {
                        Gesture::SwipeDown
                    }
                } else {
                    Gesture::Tap
                };
                self.ticks = 0;
                Some(GestureEvent { gesture, x, y })
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Boot,
    LoadWallet,
    WalletList,
    ImportWallet,
    SeedQrScan,
    MnemonicLength,
    MnemonicEntry,
    FrostThreshold,
    FrostShareIndex,
    FrostMnemonicEntry,
    FrostMnemonicConfirm,
    Network,
    Derivation,
    PassphraseChoice,
    PassphraseEntry,
    GenerationWords,
    EntropySources,
    DiceEntropy,
    CameraEntropy,
    Deriving,
    Seed,
    Backup,
    BackupMenu,
    BackupWords,
    BackupSeedQr,
    Home,
    DeleteWallet,
    Descriptor,
    Scan,
    Review,
    Sign,
    Signed,
    Recovery,
}
pub trait ScreenView {
    fn screen(&self) -> Screen;
    fn event(&mut self, event: UiEvent) -> Option<Screen>;
}
pub trait Navigator {
    fn current(&self) -> Screen;
    fn navigate(&mut self, screen: Screen);
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Navigation {
    current: Screen,
    previous: Option<Screen>,
}
impl Navigation {
    pub const fn new(screen: Screen) -> Self {
        Self {
            current: screen,
            previous: None,
        }
    }
    pub fn back(&mut self) -> bool {
        if let Some(s) = self.previous.take() {
            self.current = s;
            true
        } else {
            false
        }
    }
}
impl Navigator for Navigation {
    fn current(&self) -> Screen {
        self.current
    }
    fn navigate(&mut self, screen: Screen) {
        if screen != self.current {
            self.previous = Some(self.current);
            self.current = screen;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}
impl Rect {
    pub const fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
    pub fn intersect(self, other: Rect) -> Rect {
        let x = core::cmp::max(self.x, other.x);
        let y = core::cmp::max(self.y, other.y);
        let right = core::cmp::min(
            self.x.saturating_add(self.width),
            other.x.saturating_add(other.width),
        );
        let bottom = core::cmp::min(
            self.y.saturating_add(self.height),
            other.y.saturating_add(other.height),
        );
        Rect::new(x, y, right.saturating_sub(x), bottom.saturating_sub(y))
    }
}
pub struct DrawContext<'a> {
    pixels: &'a mut [u16],
    bounds: Rect,
    stride: usize,
}
impl<'a> DrawContext<'a> {
    pub fn new(pixels: &'a mut [u16], bounds: Rect, stride: usize) -> Self {
        Self {
            pixels,
            bounds,
            stride,
        }
    }
    pub fn bounds(&self) -> Rect {
        self.bounds
    }
    pub fn fill(&mut self, color: u16) {
        self.fill_rect(self.bounds, color);
    }
    pub fn fill_rect(&mut self, rect: Rect, color: u16) {
        let r = self.bounds.intersect(rect);
        for y in r.y..r.y.saturating_add(r.height) {
            let start = y as usize * self.stride + r.x as usize;
            let end = core::cmp::min(start.saturating_add(r.width as usize), self.pixels.len());
            if start < end {
                self.pixels[start..end].fill(color);
            }
        }
    }
    pub fn pixel(&mut self, x: u16, y: u16, color: u16) {
        if self.bounds.intersect(Rect::new(x, y, 1, 1)).width == 1 && self.stride != 0 {
            let i = y as usize * self.stride + x as usize;
            if i < self.pixels.len() {
                self.pixels[i] = color;
            }
        }
    }
}
pub trait Renderer {
    type Error;
    fn render(&mut self, screen: Screen) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Button {
    pub bounds: Rect,
    pub enabled: bool,
}
impl Button {
    pub const fn new(bounds: Rect) -> Self {
        Self {
            bounds,
            enabled: true,
        }
    }
    pub fn hit(&self, x: u16, y: u16) -> bool {
        self.enabled && self.bounds.intersect(Rect::new(x, y, 1, 1)).width != 0
    }
    pub fn draw(&self, ctx: &mut DrawContext<'_>, color: u16) {
        if self.enabled {
            ctx.fill_rect(self.bounds, color);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MnemonicBackup<'a> {
    words: &'a [&'a str],
    page: usize,
    acknowledged: bool,
}
impl<'a> MnemonicBackup<'a> {
    pub fn new(words: &'a [&'a str]) -> Self {
        Self {
            words,
            page: 0,
            acknowledged: false,
        }
    }
    pub fn page(&self) -> usize {
        self.page
    }
    pub fn page_count(&self) -> usize {
        self.words.len().saturating_add(3) / 4
    }
    pub fn words(&self) -> &'a [&'a str] {
        self.words
    }
    pub fn next(&mut self) {
        if self.page + 1 < self.page_count() {
            self.page += 1;
        }
    }
    pub fn previous(&mut self) {
        self.page = self.page.saturating_sub(1);
    }
    pub fn acknowledge_hold(&mut self) {
        if self.page + 1 == self.page_count() {
            self.acknowledged = true;
        }
    }
    pub fn acknowledged(&self) -> bool {
        self.acknowledged
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReviewOutput<'a> {
    pub destination: &'a str,
    pub amount_sat: u64,
    pub is_change: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewWarning {
    UnknownInput,
    UnverifiedChange,
    UnsafeSighash,
    MissingPrevout,
    NetworkMismatch,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransactionReview<'a> {
    pub outputs: &'a [ReviewOutput<'a>],
    pub fee_sat: Option<u64>,
    pub fee_rate_sat_vb: Option<u64>,
    pub locktime: u32,
    pub rbf: bool,
    pub warning: Option<ReviewWarning>,
    pub page: usize,
}
impl<'a> TransactionReview<'a> {
    pub const fn new(outputs: &'a [ReviewOutput<'a>]) -> Self {
        Self {
            outputs,
            fee_sat: None,
            fee_rate_sat_vb: None,
            locktime: 0,
            rbf: false,
            warning: None,
            page: 0,
        }
    }
    pub fn page_count(&self) -> usize {
        self.outputs.len().max(1)
    }
    pub fn next(&mut self) {
        if self.page + 1 < self.page_count() {
            self.page += 1;
        }
    }
    pub fn previous(&mut self) {
        self.page = self.page.saturating_sub(1);
    }
    pub fn total_outgoing_sat(&self) -> u64 {
        self.outputs
            .iter()
            .filter(|o| !o.is_change)
            .map(|o| o.amount_sat)
            .sum()
    }
    pub fn reviewed_all(&self) -> bool {
        self.page + 1 == self.page_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clipped_fill_preserves_canaries() {
        let mut p = [0xaaaa; 20];
        DrawContext::new(&mut p, Rect::new(2, 1, 3, 2), 5).fill_rect(Rect::new(0, 0, 9, 9), 0x55);
        assert_eq!(
            p,
            [
                0xaaaa, 0xaaaa, 0xaaaa, 0xaaaa, 0xaaaa, 0xaaaa, 0xaaaa, 0x55, 0x55, 0x55, 0xaaaa,
                0xaaaa, 0x55, 0x55, 0x55, 0xaaaa, 0xaaaa, 0xaaaa, 0xaaaa, 0xaaaa
            ]
        );
    }
    #[test]
    fn gestures_work() {
        let mut g = TouchTracker::new(GestureConfig {
            hold_ticks: 2,
            swipe_pixels: 10,
        });
        g.event(UiEvent::TouchDown { x: 20, y: 20 });
        assert_eq!(g.hold_progress(), (0, 2));
        assert_eq!(
            g.event(UiEvent::TouchUp { x: 20, y: 20 }).unwrap().gesture,
            Gesture::Tap
        );
        assert_eq!(g.hold_progress(), (0, 2));
        g.event(UiEvent::TouchDown { x: 20, y: 20 });
        g.event(UiEvent::Tick);
        g.event(UiEvent::Tick);
        assert_eq!(g.hold_progress(), (2, 2));
        assert_eq!(
            g.event(UiEvent::TouchUp { x: 20, y: 20 }).unwrap().gesture,
            Gesture::Hold
        );
        g.event(UiEvent::TouchDown { x: 20, y: 20 });
        g.event(UiEvent::Tick);
        g.event(UiEvent::Tick);
        assert_eq!(
            g.event(UiEvent::TouchUp { x: 20, y: 60 }).unwrap().gesture,
            Gesture::SwipeDown
        );
        g.event(UiEvent::TouchDown { x: 20, y: 60 });
        assert_eq!(
            g.event(UiEvent::TouchUp { x: 20, y: 20 }).unwrap().gesture,
            Gesture::SwipeUp
        );
    }
    #[test]
    fn backup_requires_last_page_hold() {
        let w = ["a", "b", "c", "d", "e"];
        let mut b = MnemonicBackup::new(&w);
        b.acknowledge_hold();
        assert!(!b.acknowledged());
        b.next();
        b.acknowledge_hold();
        assert!(b.acknowledged());
    }
}
