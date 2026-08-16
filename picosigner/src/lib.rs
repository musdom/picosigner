#![no_std]

pub mod app;
#[cfg(all(feature = "embedded", target_os = "none"))]
pub mod heap;
pub mod ui;

#[cfg(all(feature = "embedded", target_os = "none"))]
#[doc(hidden)]
pub mod hw {
    //! Re-exports used by the `init_display!` macro for macro hygiene.
    pub use embedded_hal;
    pub use fugit;
    pub use picosigner_board as board;
    pub use rp235x_hal as hal;
}

#[cfg(all(feature = "embedded", target_os = "none"))]
mod display;

#[cfg(all(feature = "embedded", target_os = "none"))]
mod panic;
