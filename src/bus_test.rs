//! `bus-test`: finds which memory traffic starves the display's scan-out.
//!
//! Core 1 copies 32 MB/s from PSRAM into the bounce buffers; flash and PSRAM share one memory
//! bus (MSPI), so other traffic can make its refills late. This task cycles through phases of
//! 10 s each, about 50 % busy (5 ms busy, 5 ms idle, like twine drawing a frame), and logs the
//! scan-out's late refills, resyncs and longest refill per phase:
//!
//! - `idle`: nothing (baseline: Wi-Fi and the idle UI only);
//! - `flash-read`: reads a 256 KiB table in flash, one cache line at a time (as fonts, images
//!   and code are read);
//! - `psram-copy`: copies 256 KiB within PSRAM (as twine drawing into the framebuffers does).

use alloc::vec;
use core::hint::black_box;

use embassy_time::{Duration, Instant, Timer};

use crate::display::{REFILL_BUDGET_US, Reader, scanout_stats, take_max_refill_us};

/// Length of a phase.
const PHASE: Duration = Duration::from_secs(10);
/// Busy part of each period.
const BUSY: Duration = Duration::from_millis(5);
/// Idle part of each period.
const REST: Duration = Duration::from_millis(5);
/// Bytes read or copied per pass.
const BLOCK: usize = 256 * 1024;
/// Data cache line (`.cargo/config.toml`): one read per line misses the cache.
const LINE: usize = 64;

/// A table in flash, larger than the data cache, so its reads go to flash.
static FLASH_TABLE: [u8; BLOCK] = {
    let mut t = [0u8; BLOCK];
    let mut i = 0;
    while i < BLOCK {
        t[i] = i as u8;
        i += 1;
    }
    t
};

#[derive(Clone, Copy, Debug)]
enum Phase {
    Idle,
    FlashRead,
    PsramCopy,
}

/// Cycles through the phases forever, logging each one's effect on the scan-out.
#[embassy_executor::task]
pub async fn run() -> ! {
    // Larger than the internal heap: it lands in PSRAM.
    let mut psram = vec![0u8; 2 * BLOCK];
    log::warn!(
        "bus-test: PSRAM block at {:p}; phases of {} s, refill budget {REFILL_BUDGET_US} us",
        psram.as_ptr(),
        PHASE.as_secs()
    );
    loop {
        for phase in [Phase::Idle, Phase::FlashRead, Phase::PsramCopy] {
            let before = scanout_stats();
            take_max_refill_us(Reader::BusTest);
            let end = Instant::now() + PHASE;
            while Instant::now() < end {
                let busy_until = Instant::now() + BUSY;
                while Instant::now() < busy_until {
                    match phase {
                        Phase::Idle => break,
                        Phase::FlashRead => read_flash(),
                        Phase::PsramCopy => copy_psram(&mut psram),
                    }
                }
                Timer::after(REST).await;
            }
            let after = scanout_stats();
            log::warn!(
                "bus-test: {phase:?}: {} late refills, {} resyncs, longest refill {} us",
                after.late_refills.wrapping_sub(before.late_refills),
                after.resyncs.wrapping_sub(before.resyncs),
                take_max_refill_us(Reader::BusTest)
            );
        }
    }
}

/// Reads one byte per cache line of the flash table.
fn read_flash() {
    let mut sum = 0u8;
    for i in (0..BLOCK).step_by(LINE) {
        sum = sum.wrapping_add(black_box(&FLASH_TABLE)[i]);
    }
    black_box(sum);
}

/// Copies one half of the PSRAM block over the other.
fn copy_psram(block: &mut [u8]) {
    block.copy_within(..BLOCK, BLOCK);
    black_box(&block);
}
