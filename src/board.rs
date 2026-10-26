//! Sunton ESP32-8048S043 ("Cheap Yellow Display", 4.3" 800 × 480): the board's wiring.
//!
//! | Function | Pins |
//! |----------|------|
//! | RGB panel (16-bit DPI) | DE 40, VSYNC 41, HSYNC 39, PCLK 42 |
//! | — red R0..R4 | 45, 48, 47, 21, 14 |
//! | — green G0..G5 | 5, 6, 7, 15, 16, 4 |
//! | — blue B0..B4 | 8, 3, 46, 9, 1 |
//! | Backlight | 2 (active high) |
//! | GT911 touch (I2C) | SDA 19, SCL 20 (INT/RST not used: the driver polls) |
//!
//! The module is an ESP32-S3-WROOM-1 N16R8: 16 MB flash, 8 MB octal PSRAM (holds the
//! framebuffer).

use esp_hal::peripherals::{
    CPU_CTRL, DMA_CH2, FLASH, GPIO1, GPIO2, GPIO3, GPIO4, GPIO5, GPIO6, GPIO7, GPIO8, GPIO9,
    GPIO14, GPIO15, GPIO16, GPIO19, GPIO20, GPIO21, GPIO39, GPIO40, GPIO41, GPIO42, GPIO45, GPIO46,
    GPIO47, GPIO48, I2C0, LCD_CAM, PSRAM, Peripherals, SW_INTERRUPT, TIMG0, WIFI,
};

/// The panel's timing (from the vendor's configuration: sync pulses of 4, porches of 8) at a
/// 16 MHz pixel clock (~39 Hz refresh).
pub mod panel {
    /// Visible width in pixels.
    pub const WIDTH: u16 = 800;
    /// Visible height in pixels.
    pub const HEIGHT: u16 = 480;
    /// Pixel clock: the vendor's. Core 1 copies `2 × PCLK_HZ` bytes/s from PSRAM to feed the
    /// panel (32 MB/s); Espressif measured ~22 MHz as the limit for octal PSRAM at 80 MHz
    /// (see `memory`). At 40 MHz PSRAM it must drop to 14 MHz; 12 MHz leaves this panel blank.
    pub const PCLK_HZ: u32 = 16_000_000;
    /// HSYNC pulse width (pixel clocks).
    pub const HSYNC_PULSE: usize = 4;
    /// Horizontal back porch (pixel clocks).
    pub const HSYNC_BACK_PORCH: usize = 8;
    /// Horizontal front porch (pixel clocks).
    pub const HSYNC_FRONT_PORCH: usize = 8;
    /// VSYNC pulse width (lines).
    pub const VSYNC_PULSE: usize = 4;
    /// Vertical back porch (lines).
    pub const VSYNC_BACK_PORCH: usize = 8;
    /// Vertical front porch (lines).
    pub const VSYNC_FRONT_PORCH: usize = 8;
}

/// The pins and peripherals of the RGB panel.
pub struct PanelPins {
    pub lcd_cam: LCD_CAM<'static>,
    pub dma: DMA_CH2<'static>,
    pub de: GPIO40<'static>,
    pub vsync: GPIO41<'static>,
    pub hsync: GPIO39<'static>,
    pub pclk: GPIO42<'static>,
    pub backlight: GPIO2<'static>,
    pub red: (
        GPIO45<'static>,
        GPIO48<'static>,
        GPIO47<'static>,
        GPIO21<'static>,
        GPIO14<'static>,
    ),
    pub green: (
        GPIO5<'static>,
        GPIO6<'static>,
        GPIO7<'static>,
        GPIO15<'static>,
        GPIO16<'static>,
        GPIO4<'static>,
    ),
    pub blue: (
        GPIO8<'static>,
        GPIO3<'static>,
        GPIO46<'static>,
        GPIO9<'static>,
        GPIO1<'static>,
    ),
}

/// The pins and peripheral of the touch controller.
pub struct TouchPins {
    pub i2c: I2C0<'static>,
    pub sda: GPIO19<'static>,
    pub scl: GPIO20<'static>,
}

/// The chip peripherals the firmware uses besides the board's wiring.
pub struct System {
    pub wifi: WIFI<'static>,
    pub psram: PSRAM<'static>,
    pub timg0: TIMG0<'static>,
    pub sw_interrupt: SW_INTERRUPT<'static>,
    /// Starts core 1, which the display's scan-out runs on.
    pub cpu_ctrl: CPU_CTRL<'static>,
    /// The SPI flash: settings and the world map.
    pub flash: FLASH<'static>,
}

/// The board's peripherals, grouped by function.
pub struct Board {
    pub panel: PanelPins,
    pub touch: TouchPins,
}

impl Board {
    /// Splits the chip's peripherals into the board's functions and the system peripherals.
    pub fn split(p: Peripherals) -> (Self, System) {
        let board = Self {
            panel: PanelPins {
                lcd_cam: p.LCD_CAM,
                dma: p.DMA_CH2,
                de: p.GPIO40,
                vsync: p.GPIO41,
                hsync: p.GPIO39,
                pclk: p.GPIO42,
                backlight: p.GPIO2,
                red: (p.GPIO45, p.GPIO48, p.GPIO47, p.GPIO21, p.GPIO14),
                green: (p.GPIO5, p.GPIO6, p.GPIO7, p.GPIO15, p.GPIO16, p.GPIO4),
                blue: (p.GPIO8, p.GPIO3, p.GPIO46, p.GPIO9, p.GPIO1),
            },
            touch: TouchPins {
                i2c: p.I2C0,
                sda: p.GPIO19,
                scl: p.GPIO20,
            },
        };
        let system = System {
            wifi: p.WIFI,
            psram: p.PSRAM,
            timg0: p.TIMG0,
            sw_interrupt: p.SW_INTERRUPT,
            cpu_ctrl: p.CPU_CTRL,
            flash: p.FLASH,
        };
        (board, system)
    }
}
