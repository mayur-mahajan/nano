//! Flash storage: the partitions of `partitions.csv` and the settings records.
//!
//! Settings are stored as 256-byte records, each with a sequence number and a CRC, in two 4 KiB
//! sectors of the `settings` partition. A save writes the next free record; when a sector is
//! full the other one is erased and used. Loading takes the valid record with the highest
//! sequence number, so a save cut short by a power loss leaves the previous settings in place.
//!
//! Flash writes disable the cache, which core 1 reads the framebuffer through: they run while
//! the scan-out is paused ([`crate::display::paused`]), a short flicker.

use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use esp_bootloader_esp_idf::partitions::{self, PARTITION_TABLE_MAX_LEN};
use esp_hal::peripherals::FLASH;
use esp_storage::FlashStorage;

use crate::settings::{ENCODED_MAX, Settings};

/// A partition's place in flash.
#[derive(Clone, Copy, Debug)]
pub struct Region {
    /// Offset from the start of flash.
    pub offset: u32,
    /// Bytes.
    pub len: u32,
}

/// Record magic, `NCFG`.
const MAGIC: [u8; 4] = *b"NCFG";
/// Bytes per record (header and settings, padded).
const RECORD: u32 = 256;
/// Header: magic, sequence u32, length u16, reserved u16, CRC-32 u32.
const HEADER: usize = 16;
/// Erase unit.
const SECTOR: u32 = 4096;
/// Records per sector.
const PER_SECTOR: u32 = SECTOR / RECORD;
/// Sectors used.
const SECTORS: u32 = 2;

const _: () = assert!(HEADER + ENCODED_MAX <= RECORD as usize);

/// The flash, with the partitions nano uses.
pub struct Storage {
    flash: FlashStorage<'static>,
    settings: Option<Region>,
    /// The record slot of the last valid settings, and its sequence number.
    last: Option<(u32, u32)>,
}

impl Storage {
    /// Opens the flash and finds the `settings` and `map` partitions. Call before core 1
    /// starts (reads are not paused).
    pub fn open(flash: FLASH<'static>) -> (Self, Option<Region>) {
        // SAFETY: writes pause core 1 themselves (`paused`); reads happen at start-up only.
        let mut flash = unsafe { FlashStorage::new(flash).multicore_ignore() };
        let mut table = [0u8; PARTITION_TABLE_MAX_LEN];
        let (mut settings, mut map) = (None, None);
        match partitions::read_partition_table(&mut flash, &mut table) {
            Ok(table) => {
                for p in table.iter() {
                    let region = Region {
                        offset: p.offset(),
                        len: p.len(),
                    };
                    match p.label_as_str() {
                        "settings" => settings = Some(region),
                        "map" => map = Some(region),
                        _ => {}
                    }
                }
            }
            Err(e) => log::error!("storage: no partition table ({e:?})"),
        }
        if settings.is_none() {
            log::error!("storage: no `settings` partition: settings are not saved");
        }
        (
            Self {
                flash,
                settings,
                last: None,
            },
            map,
        )
    }

    /// The saved settings, if any.
    pub fn load(&mut self) -> Option<Settings> {
        let region = self.settings?;
        let mut best: Option<(u32, u32, Settings)> = None;
        let mut buf = [0u8; RECORD as usize];
        for slot in 0..SECTORS * PER_SECTOR {
            if self
                .flash
                .read(region.offset + slot * RECORD, &mut buf)
                .is_err()
            {
                continue;
            }
            let Some((seq, settings)) = decode_record(&buf) else {
                continue;
            };
            if best.as_ref().is_none_or(|(s, _, _)| seq > *s) {
                best = Some((seq, slot, settings));
            }
        }
        let (seq, slot, settings) = best?;
        self.last = Some((slot, seq));
        log::info!("storage: settings loaded (record {seq})");
        Some(settings)
    }

    /// Saves `settings` as a new record.
    ///
    /// # Errors
    /// When there is no settings partition, the flash write fails, or core 1 does not pause.
    pub fn save(&mut self, settings: &Settings) -> Result<(), &'static str> {
        let region = self.settings.ok_or("no settings partition")?;
        let (slot, seq) = match self.last {
            Some((slot, seq)) => ((slot + 1) % (SECTORS * PER_SECTOR), seq.wrapping_add(1)),
            None => (0, 1),
        };
        let record = encode_record(seq, settings);
        let at = region.offset + slot * RECORD;
        let flash = &mut self.flash;
        crate::display::paused(|| {
            if slot % PER_SECTOR == 0 {
                flash.erase(at, at + SECTOR).map_err(|_| "erase failed")?;
            }
            flash.write(at, &record).map_err(|_| "write failed")
        })
        .ok_or("display did not pause")??;
        self.last = Some((slot, seq));
        log::info!("storage: settings saved (record {seq})");
        Ok(())
    }
}

fn encode_record(seq: u32, settings: &Settings) -> [u8; RECORD as usize] {
    let mut payload = [0u8; ENCODED_MAX];
    let len = settings.encode(&mut payload);
    let mut record = [0xFF; RECORD as usize];
    record[0..4].copy_from_slice(&MAGIC);
    record[4..8].copy_from_slice(&seq.to_le_bytes());
    record[8..10].copy_from_slice(&(len as u16).to_le_bytes());
    record[10..12].copy_from_slice(&[0, 0]);
    record[12..16].copy_from_slice(&crc32(&payload[..len]).to_le_bytes());
    record[HEADER..HEADER + len].copy_from_slice(&payload[..len]);
    record
}

fn decode_record(r: &[u8; RECORD as usize]) -> Option<(u32, Settings)> {
    if r[0..4] != MAGIC {
        return None;
    }
    let seq = u32::from_le_bytes(r[4..8].try_into().ok()?);
    let len = usize::from(u16::from_le_bytes([r[8], r[9]]));
    let crc = u32::from_le_bytes(r[12..16].try_into().ok()?);
    let payload = r.get(HEADER..HEADER + len)?;
    if crc32(payload) != crc {
        return None;
    }
    Some((seq, Settings::decode(payload)?))
}

/// CRC-32 (IEEE), bitwise: records are small and rare.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Saves the settings whenever the UI publishes new ones.
#[embassy_executor::task]
pub async fn persist(mut storage: Storage) -> ! {
    let mut seen = crate::settings::SETTINGS.version();
    loop {
        embassy_time::Timer::after(embassy_time::Duration::from_millis(500)).await;
        let version = crate::settings::SETTINGS.version();
        if version != seen {
            seen = version;
            if let Err(e) = storage.save(&crate::settings::SETTINGS.get()) {
                log::error!("storage: settings not saved: {e}");
            }
        }
    }
}
