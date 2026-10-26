//! Firmware for the Sunton ESP32-8048S043 (4.3" 800 × 480 RGB touch display) with a
//! [Twine](https://github.com/mayur-mahajan/twine) UI.
//!
//! A deskside ADS-B tracker: the aircraft a tar1090 feeder sees, on a radar scope.
//!
//! Hardware and tasks:
//! - [`board`]: the board's wiring and panel timing.
//! - [`display`], [`touch`]: the RGB panel and the GT911, as twine drivers.
//! - [`memory`], [`storage`]: PSRAM, the flash partitions, the saved settings.
//! - [`net`]: Wi-Fi and the IP stack; [`clock`]: SNTP; [`feeder`]: polls the feeder.
//!
//! Platform-neutral (also built by the host simulator):
//! - [`settings`], [`connectivity`], [`time`], [`traffic`], [`map`]: the state the tasks
//!   publish and the UI shows;
//! - [`ui`]: the screens (only `twine`).
#![no_std]

extern crate alloc;

pub mod board;
#[cfg(feature = "bus-test")]
pub mod bus_test;
pub mod clock;
pub mod connectivity;
pub mod display;
pub mod feeder;
pub mod map;
pub mod memory;
pub mod net;
pub mod settings;
pub mod storage;
pub mod time;
pub mod touch;
pub mod traffic;
pub mod ui;
