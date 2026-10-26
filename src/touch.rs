//! The GT911 capacitive touch controller.
//!
//! Twine's driver is polled (the board's INT line is not used). It reads the resolution the
//! controller is configured for (some 8048S043 batches ship configured for 480 × 272) and
//! scales the points to the display itself.

use esp_hal::Blocking;
use esp_hal::i2c::master::{Config, I2c};
use esp_hal::time::Rate;
use twine::drivers::NoPin;
use twine::drivers::touch::{Gt911, gt911};

use crate::board::TouchPins;

/// The touch input as the UI takes it.
pub type Touch = Gt911<I2c<'static, Blocking>, NoPin>;

/// Opens the controller on I2C0 and finds its address (`0x5D`, else `0x14`: latched at reset).
/// When the controller does not answer, the UI reports the touch input as failed and runs
/// without it.
pub fn init(pins: TouchPins) -> Touch {
    let config = Config::default().with_frequency(Rate::from_khz(400));
    let i2c = match I2c::new(pins.i2c, config) {
        Ok(i2c) => i2c.with_sda(pins.sda).with_scl(pins.scl),
        Err(e) => panic!("touch: I2C configuration rejected: {e:?}"),
    };
    let mut driver = Gt911::new(i2c, None::<NoPin>);
    for addr in [gt911::ADDR, gt911::ADDR_ALT] {
        driver = driver.with_address(addr);
        if driver.probe().is_ok() {
            log::info!("touch: GT911 at {addr:#04x}");
            return driver;
        }
    }
    log::warn!("touch: no GT911 answered at 0x5D or 0x14");
    driver.with_address(gt911::ADDR)
}
