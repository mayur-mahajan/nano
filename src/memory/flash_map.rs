//! Memory-maps a flash partition (the world map) into the data bus after the PSRAM, so it is
//! read through the cache like the app's own constants: no flash driver calls, which would
//! need the display paused.

use esp_hal::peripherals::EXTMEM;

use crate::storage::Region;

/// The MMU page size.
const PAGE: u32 = 0x1_0000;
/// End of the data bus's external memory range.
const DBUS_END: usize = 0x3E00_0000;

/// Maps `region` (64 KiB aligned) at `vaddr` (the first free page after the PSRAM); returns it
/// as a slice. Call before core 1 starts.
pub fn map(region: Region, vaddr: usize) -> Option<&'static [u8]> {
    if !region.offset.is_multiple_of(PAGE) || region.len == 0 {
        log::error!(
            "map: partition at {:#x} is not 64 KiB aligned",
            region.offset
        );
        return None;
    }
    let vaddr = (vaddr as u32).next_multiple_of(PAGE);
    let pages = region.len.div_ceil(PAGE);
    if vaddr as usize + (pages * PAGE) as usize > DBUS_END {
        log::error!("map: no address space left for {} KiB", region.len / 1024);
        return None;
    }
    if !map_pages(vaddr, region.offset, pages) {
        log::error!("map: cache_dbus_mmu_set failed");
        return None;
    }
    // SAFETY: the pages were just mapped read-only from flash and stay mapped for the rest of
    // the run; nothing else maps this range (it follows the PSRAM).
    Some(unsafe { core::slice::from_raw_parts(vaddr as *const u8, region.len as usize) })
}

/// Maps `pages` flash pages from `paddr` at `vaddr` (as esp-hal maps the PSRAM).
#[esp_hal::ram]
fn map_pages(vaddr: u32, paddr: u32, pages: u32) -> bool {
    const MMU_ACCESS_FLASH: u32 = 0;
    unsafe extern "C" {
        fn Cache_Suspend_DCache();
        fn Cache_Resume_DCache(param: u32);
        fn cache_dbus_mmu_set(
            ext_ram: u32,
            vaddr: u32,
            paddr: u32,
            psize: u32,
            num: u32,
            fixed: u32,
        ) -> i32;
    }
    // SAFETY: ROM functions; the data cache is suspended while its MMU table changes, and this
    // runs from IRAM with core 1 not started.
    unsafe {
        Cache_Suspend_DCache();
        let rc = cache_dbus_mmu_set(MMU_ACCESS_FLASH, vaddr, paddr, 64, pages, 0);
        EXTMEM::regs().dcache_ctrl1().modify(|_, w| {
            w.dcache_shut_core0_bus().clear_bit();
            w.dcache_shut_core1_bus().clear_bit()
        });
        // No invalidation: the new addresses were never cached (and invalidating would drop
        // dirty PSRAM lines).
        Cache_Resume_DCache(0);
        rc == 0
    }
}
