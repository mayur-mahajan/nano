//! Boot-time check that octal PSRAM works at 80 MHz.
//!
//! esp-hal 1.1 runs octal PSRAM at 80 MHz without ESP-IDF's MSPI timing tuning. Measured on the
//! 8048S043, with the MSPI core clock at 160 MHz (ESP-IDF's setting for 80 MHz DTR), the default
//! sampling (no input delay, no extra dummy) reads PSRAM back intact; ESP-IDF's tuning candidates
//! with extra dummy cycles all read garbage through SPI0, and sweeping them hung the controller.
//! (With esp-hal's default core clock of 80 MHz, i.e. the PSRAM clock undivided, PSRAM corrupted
//! data.) So the timing registers are left at their defaults and this module verifies them: it
//! writes varied patterns over a test area, reads them back from PSRAM and counts mismatches.

use esp_hal::peripherals::SPI0;

/// Bytes of PSRAM tested (from the start of the framebuffer, which is cleared afterwards).
pub const TEST_BYTES: usize = 256 * 1024;
const WORDS: usize = TEST_BYTES / 4;
/// ESP-IDF's tuning reference word.
const PATTERN: u32 = 0xa5ff_005a;

/// The outcome of the check.
#[derive(Debug)]
pub struct Outcome {
    /// Words written and read back.
    pub words: usize,
    /// Words that read back different from what was written.
    pub errors: usize,
}

/// Writes and reads back two patterns over `test_area` and counts the mismatches.
///
/// # Safety
/// `test_area` must be valid for [`TEST_BYTES`] bytes, 64-byte aligned and not used by anything
/// else; its content is overwritten.
pub unsafe fn verify(test_area: *mut u8) -> Outcome {
    // Default sampling (ESP-IDF: `mspi_timing_ll_set_psram_din_mode/num`,
    // `…_set_octal_psram_extra_dummy` with 0); the timing clock is needed for these registers.
    let regs = SPI0::regs();
    regs.spi_smem_timing_cali()
        .modify(|_, w| w.spi_smem_timing_clk_ena().set_bit());
    // SAFETY: zero fits every field.
    regs.spi_smem_din_mode().write(|w| unsafe { w.bits(0) });
    regs.spi_smem_din_num().write(|w| unsafe { w.bits(0) });
    regs.spi_smem_timing_cali().modify(|_, w| unsafe {
        w.spi_smem_timing_cali().clear_bit();
        w.spi_smem_extra_dummy_cyclelen().bits(0)
    });

    let words = test_area.cast::<u32>();
    let mut errors = 0;
    // A word depending on its address (catches address and stuck-bit faults), then its inverse
    // (every bit both ways).
    for pass in 0..2u32 {
        let expected = |i: usize| {
            let v = PATTERN ^ (i as u32).wrapping_mul(0x9e37_79b9);
            if pass == 0 { v } else { !v }
        };
        for i in 0..WORDS {
            // SAFETY: within `test_area` (caller).
            unsafe { words.add(i).write_volatile(expected(i)) };
        }
        // SAFETY: ROM functions on the test area: write the pattern back to PSRAM, then drop it
        // from the cache so the reads below come from PSRAM.
        unsafe {
            rom::write_back(test_area as u32, TEST_BYTES as u32);
            rom::invalidate(test_area as u32, TEST_BYTES as u32);
        }
        for i in 0..WORDS {
            // SAFETY: within `test_area` (caller).
            if unsafe { words.add(i).read_volatile() } != expected(i) {
                errors += 1;
            }
        }
    }
    Outcome {
        words: 2 * WORDS,
        errors,
    }
}

/// The ESP32-S3 ROM's data-cache functions (autoload suspended around them, as esp-hal does).
mod rom {
    unsafe extern "C" {
        fn rom_Cache_WriteBack_Addr(addr: u32, size: u32);
        fn Cache_Invalidate_Addr(addr: u32, size: u32) -> i32;
        fn Cache_Suspend_DCache_Autoload() -> u32;
        fn Cache_Resume_DCache_Autoload(value: u32);
    }

    /// Writes the dirty lines of a range back to PSRAM.
    pub(super) unsafe fn write_back(addr: u32, size: u32) {
        unsafe {
            let autoload = Cache_Suspend_DCache_Autoload();
            rom_Cache_WriteBack_Addr(addr, size);
            Cache_Resume_DCache_Autoload(autoload);
        }
    }

    /// Drops the lines of a range from the cache (dirty data in it is lost).
    pub(super) unsafe fn invalidate(addr: u32, size: u32) {
        unsafe {
            let autoload = Cache_Suspend_DCache_Autoload();
            Cache_Invalidate_Addr(addr, size);
            Cache_Resume_DCache_Autoload(autoload);
        }
    }
}
