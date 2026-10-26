//! Heap and PSRAM layout.
//!
//! - Internal RAM: the heap for Wi-Fi (its buffers must be internal), the IP stack and the UI.
//! - PSRAM (8 MB octal, 80 MHz, checked at boot by [`psram_check`]): the two framebuffers
//!   first, the rest is added to the heap as external memory for large allocations.

pub mod flash_map;
pub mod psram_check;

use core::sync::atomic::{AtomicUsize, Ordering};

use esp_hal::peripherals::{PSRAM, SPI0};
use esp_hal::psram::{Psram, PsramConfig, SpiRamFreq, SpiTimingConfigCoreClock};
use twine::engine::{HeapPeak, MemInfo};

use crate::display::{FRAMEBUFFER_BYTES, Framebuffer};

/// Logs the mode the flash is read in: the bootloader sets it (`bootloader/README.md`), and
/// flash reads compete with the display's scan-out for the memory bus, QIO taking half the time
/// of DIO.
pub fn log_flash_mode() {
    let ctrl = SPI0::regs().ctrl().read();
    let mode = if ctrl.fread_qio().bit() {
        "QIO"
    } else if ctrl.fread_quad().bit() {
        "QOUT"
    } else if ctrl.fread_dio().bit() {
        "DIO"
    } else if ctrl.fread_dual().bit() {
        "DOUT"
    } else {
        "single"
    };
    log::info!("flash: read mode {mode}");
}

/// Maps the PSRAM at 80 MHz, checks it, carves the two framebuffers from its start and gives
/// the rest to the heap.
///
/// # Panics
/// When the PSRAM is missing or smaller than the framebuffer. Call once, before anything else
/// uses PSRAM and before core 1 starts.
pub fn init_psram(psram: PSRAM<'static>) -> [&'static mut Framebuffer; 2] {
    // 80 MHz DTR from a 160 MHz MSPI core clock (divider 2), as ESP-IDF runs it. esp-hal would
    // default the core clock to 80 MHz (divider 1), which corrupts data. Flash stays at 80 MHz.
    let config = PsramConfig {
        ram_frequency: SpiRamFreq::Freq80m,
        core_clock: Some(SpiTimingConfigCoreClock::SpiTimingConfigCoreClock160m),
        ..PsramConfig::default()
    };
    let psram = Psram::new(psram, config);
    let (start, size) = psram.raw_parts();
    PSRAM_END.store(start as usize + size, Ordering::Relaxed);
    assert!(
        size >= 2 * FRAMEBUFFER_BYTES,
        "PSRAM missing or too small ({size} bytes)"
    );
    // SAFETY: the PSRAM was just mapped; nothing uses it yet and core 1 is not running. The test
    // area is the framebuffer's start, cleared by the display later.
    let check = unsafe { psram_check::verify(start) };
    if check.errors == 0 {
        log::info!(
            "psram: {} KiB at {:p}, 80 MHz, {} KiB verified",
            size / 1024,
            start,
            psram_check::TEST_BYTES / 1024
        );
    } else {
        // The display and the heap would degrade: say how to fall back.
        log::error!(
            "psram: {} of {} words read back wrong at 80 MHz; use 40 MHz \
             (`PsramConfig::default()`) and a 14 MHz pixel clock",
            check.errors,
            check.words
        );
    }
    // SAFETY: the mapped PSRAM is ours alone: this runs once and the region is split in three
    // disjoint parts, the two framebuffers (returned, 64-byte aligned like `start`) and the heap
    // region after them.
    unsafe {
        esp_alloc::HEAP.add_region(esp_alloc::HeapRegion::new(
            start.add(2 * FRAMEBUFFER_BYTES),
            size - 2 * FRAMEBUFFER_BYTES,
            esp_alloc::MemoryCapability::External.into(),
        ));
        [
            &mut *start.cast::<Framebuffer>(),
            &mut *start.add(FRAMEBUFFER_BYTES).cast::<Framebuffer>(),
        ]
    }
}

/// The end of the PSRAM's address range (0 before [`init_psram`]).
static PSRAM_END: AtomicUsize = AtomicUsize::new(0);

/// Memory-maps a flash partition after the PSRAM (see [`flash_map`]). Call after
/// [`init_psram`] and before core 1 starts.
pub fn map_flash(region: crate::storage::Region) -> Option<&'static [u8]> {
    let end = PSRAM_END.load(Ordering::Relaxed);
    if end == 0 {
        return None;
    }
    flash_map::map(region, end)
}

/// Heap statistics for twine's `twine::perf` log line; warns when the heap is over 90 % full.
pub fn mem_info() -> MemInfo {
    static PEAK: HeapPeak = HeapPeak::new();
    let m = PEAK.sample(esp_alloc::HEAP.used(), esp_alloc::HEAP.free());
    if m.used_percent() > 90 {
        log::warn!("heap above 90%: {} of {} bytes", m.used, m.used + m.free);
    }
    m
}

/// A zeroed buffer of `len` bytes in PSRAM, kept for the rest of the run (for large network
/// bodies and map cells, which must not take the internal RAM Wi-Fi needs).
///
/// # Panics
/// When the PSRAM heap cannot hold it.
pub fn psram_buffer(len: usize) -> &'static mut [u8] {
    let layout = core::alloc::Layout::from_size_align(len, 4).expect("buffer layout");
    // SAFETY: `layout` is non-zero sized; the block is never freed, so the slice lives forever,
    // and it is zeroed before it is handed out.
    unsafe {
        let p = esp_alloc::HEAP.alloc_caps(esp_alloc::MemoryCapability::External.into(), layout);
        assert!(!p.is_null(), "PSRAM heap cannot hold {len} bytes");
        p.write_bytes(0, len);
        core::slice::from_raw_parts_mut(p, len)
    }
}

/// Free internal and external heap, bytes.
pub fn free_heap() -> (usize, usize) {
    (
        esp_alloc::HEAP.free_caps(esp_alloc::MemoryCapability::Internal.into()),
        esp_alloc::HEAP.free_caps(esp_alloc::MemoryCapability::External.into()),
    )
}
