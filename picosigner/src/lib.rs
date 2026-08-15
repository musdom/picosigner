#![no_std]

pub mod app;
#[cfg(all(feature = "embedded", target_os = "none"))]
pub mod heap;
pub mod ui;
