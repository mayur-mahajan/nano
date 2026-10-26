//! The RGB (DPI) panel as a twine [`FramebufferDisplay`].
//!
//! Two RGB565 framebuffers in PSRAM (`BufferMode::Full`): twine renders into the one not on
//! screen and presents it when the frame is complete. The panel is refreshed continuously by
//! [`scanout`]: the second core copies the shown framebuffer through two internal bounce buffers
//! that the LCD_CAM DMA streams to the panel in a ring, and switches to a presented framebuffer
//! only at the start of a frame, so every refresh shows one complete frame. (With one
//! framebuffer, twine's drawing raced the copying: an animated area showed half-drawn, as
//! flicker.)

mod scanout;

pub use scanout::{
    REFILL_BUDGET_US, Reader, Stats, paused, stats as scanout_stats, take_max_refill_us,
};

use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::lcd_cam::LcdCam;
use esp_hal::lcd_cam::lcd::dpi::{Config, Dpi, Format, FrameTiming};
use esp_hal::lcd_cam::lcd::{ClockMode, Phase, Polarity};
use esp_hal::peripherals::CPU_CTRL;
use esp_hal::time::Rate;
use twine::core::ColorFormat;
use twine::hal::{DisplayInfo, DrawBufferMem, FramebufferDisplay};

use crate::board::{PanelPins, panel};

/// Bytes per pixel (RGB565).
const BPP: usize = 2;
/// Bytes of one row.
const ROW_BYTES: usize = panel::WIDTH as usize * BPP;
/// Bytes of one framebuffer.
pub const FRAMEBUFFER_BYTES: usize = ROW_BYTES * panel::HEIGHT as usize;
/// One framebuffer.
pub type Framebuffer = [u8; FRAMEBUFFER_BYTES];

/// Why the panel could not be started.
#[derive(Debug)]
pub enum InitError {
    /// The pixel clock or timing was rejected.
    Config(esp_hal::lcd_cam::lcd::dpi::ConfigError),
}

/// The 800 × 480 RGB panel, refreshed from two PSRAM framebuffers by core 1.
pub struct RgbPanel {
    framebuffers: Option<(DrawBufferMem, DrawBufferMem)>,
    /// The framebuffer last presented (0 or 1).
    presented: u8,
    _scanout: scanout::Scanout,
    _backlight: Output<'static>,
}

impl RgbPanel {
    /// Clears both framebuffers to black, starts refreshing the panel from the first on core 1
    /// and turns the backlight on. Core 1 is dedicated to the panel from here on.
    ///
    /// # Errors
    /// [`InitError`] when the timing is rejected.
    ///
    /// # Panics
    /// When core 1 cannot be started.
    pub fn new(
        pins: PanelPins,
        framebuffers: [&'static mut Framebuffer; 2],
        cpu_ctrl: CPU_CTRL<'static>,
    ) -> Result<Self, InitError> {
        let [first, second] = framebuffers;
        first.fill(0);
        second.fill(0);

        let lcd_cam = LcdCam::new(pins.lcd_cam);
        let (r, g, b) = (pins.red, pins.green, pins.blue);
        // 16-bit bus: data0..4 = blue, data5..10 = green, data11..15 = red (RGB565, LSB first).
        let dpi = Dpi::new(lcd_cam.lcd, pins.dma, dpi_config(PCLK_PHASE))
            .map_err(InitError::Config)?
            .with_de(pins.de)
            .with_vsync(pins.vsync)
            .with_hsync(pins.hsync)
            .with_pclk(pins.pclk)
            .with_data0(b.0)
            .with_data1(b.1)
            .with_data2(b.2)
            .with_data3(b.3)
            .with_data4(b.4)
            .with_data5(g.0)
            .with_data6(g.1)
            .with_data7(g.2)
            .with_data8(g.3)
            .with_data9(g.4)
            .with_data10(g.5)
            .with_data11(r.0)
            .with_data12(r.1)
            .with_data13(r.2)
            .with_data14(r.3)
            .with_data15(r.4);

        let ptrs = [first.as_mut_ptr(), second.as_mut_ptr()];
        let scanout = scanout::start(dpi, ptrs.map(|p| p.cast_const()), cpu_ctrl);

        // SAFETY: core 1 only reads the framebuffers, through `ptrs`; twine is the only writer,
        // through these slices. The memory is `'static`.
        let [a, b] = ptrs.map(|p| unsafe { core::slice::from_raw_parts_mut(p, FRAMEBUFFER_BYTES) });
        let backlight = Output::new(pins.backlight, Level::High, OutputConfig::default());
        log::info!(
            "display: {}x{} RGB565 at {} MHz, framebuffers at {:p} and {:p}",
            panel::WIDTH,
            panel::HEIGHT,
            panel::PCLK_HZ / 1_000_000,
            ptrs[0],
            ptrs[1]
        );
        Ok(Self {
            framebuffers: Some((DrawBufferMem::new(a), DrawBufferMem::new(b))),
            presented: 0,
            _scanout: scanout,
            _backlight: backlight,
        })
    }
}

impl FramebufferDisplay for RgbPanel {
    type Error = core::convert::Infallible;

    fn info(&self) -> DisplayInfo {
        DisplayInfo::new(panel::WIDTH, panel::HEIGHT, ColorFormat::Rgb565)
    }

    fn framebuffers(&mut self) -> Option<(DrawBufferMem, Option<DrawBufferMem>)> {
        self.framebuffers.take().map(|(a, b)| (a, Some(b)))
    }

    fn present(&mut self, index: u8) -> Result<(), Self::Error> {
        // Core 1 switches at the start of its next frame; it reads the framebuffer through the
        // cache both cores share, so no cache maintenance is needed.
        self.presented = index & 1;
        scanout::present(self.presented);
        Ok(())
    }

    fn present_done(&mut self) -> bool {
        scanout::shown() == self.presented
    }
}

/// Logs the scan-out's frame rate, late refills and resyncs every 10 s when there were late
/// refills or resyncs, and once a minute otherwise.
#[embassy_executor::task]
pub async fn report_scanout() -> ! {
    const PERIOD_S: u32 = 10;
    let mut last = scanout::stats();
    let mut quiet = 0;
    loop {
        embassy_time::Timer::after_secs(u64::from(PERIOD_S)).await;
        let now = scanout::stats();
        let fps = now.frames.wrapping_sub(last.frames) / PERIOD_S;
        let late = now.late_refills.wrapping_sub(last.late_refills);
        let resyncs = now.resyncs.wrapping_sub(last.resyncs);
        let max_refill = scanout::take_max_refill_us(Reader::Report);
        last = now;
        quiet += 1;
        if late > 0 || resyncs > 0 {
            log::warn!(
                "display: {fps} fps, {late} late refills, {resyncs} resyncs in {PERIOD_S} s, \
                 longest refill {max_refill} us (budget {} us)",
                scanout::REFILL_BUDGET_US
            );
            quiet = 0;
        } else if quiet == 6 {
            log::info!(
                "display: {fps} fps, no late refills or resyncs, longest refill {max_refill} us \
                 (budget {} us)",
                scanout::REFILL_BUDGET_US
            );
            quiet = 0;
        }
    }
}

/// The edge the data changes on. `ShiftLow` has shown a picture on this panel; ESP-IDF's
/// `pclk_active_neg` (the vendor's setting) maps to `ShiftHigh` on paper, and esp-hal PR #5262
/// uses `ShiftHigh` for an 800 × 480 panel with that setting. The `pclk-edge-test` feature
/// alternates the two every 5 s to compare them on the real UI.
const PCLK_PHASE: Phase = Phase::ShiftLow;

/// The DPI timing and signal polarities of the panel, with the data changing on `phase`.
fn dpi_config(phase: Phase) -> Config {
    use panel::*;
    Config::default()
        .with_frequency(Rate::from_hz(PCLK_HZ))
        .with_clock_mode(ClockMode {
            polarity: Polarity::IdleLow,
            phase,
        })
        .with_format(Format {
            enable_2byte_mode: true,
            ..Default::default()
        })
        .with_timing(FrameTiming {
            horizontal_active_width: WIDTH as usize,
            horizontal_total_width: WIDTH as usize
                + HSYNC_PULSE
                + HSYNC_BACK_PORCH
                + HSYNC_FRONT_PORCH,
            horizontal_blank_front_porch: HSYNC_PULSE + HSYNC_BACK_PORCH,
            vertical_active_height: HEIGHT as usize,
            vertical_total_height: HEIGHT as usize
                + VSYNC_PULSE
                + VSYNC_BACK_PORCH
                + VSYNC_FRONT_PORCH,
            vertical_blank_front_porch: VSYNC_PULSE + VSYNC_BACK_PORCH,
            hsync_width: HSYNC_PULSE,
            vsync_width: VSYNC_PULSE,
            hsync_position: 0,
        })
        // Active-low sync pulses, active-high data enable.
        .with_vsync_idle_level(Level::High)
        .with_hsync_idle_level(Level::High)
        .with_de_idle_level(Level::Low)
        .with_disable_black_region(false)
}
