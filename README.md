# nano

Firmware for the Sunton **ESP32-8048S043** (4.3" 800 × 480 RGB panel, GT911 touch,
ESP32-S3 N16R8) with a [Twine](https://github.com/mayur-mahajan/twine) UI.

A deskside ADS-B tracker. It polls a [tar1090](https://github.com/wiedehopf/tar1090) feeder on
the local network every 5 s and shows the aircraft it sees on a radar scope: north up, to
scale, around the receiver, over coastlines, borders and airports.

- **Scope:** aircraft glyphs turned to their track and coloured by altitude (tar1090's scale),
  fading 2-minute trails, callouts with callsign, altitude and speed for the nearest aircraft,
  range rings, a compass, zoom from 5 to 250 nm. Emergency squawks are ringed in red.
- **Side panel:** the traffic nearest first with totals; tap an aircraft (on the scope or in the
  list) for its details (registration, type, altitude and climb, speed, track, distance and
  bearing, squawk, signal). Tap empty scope to deselect.
- **Title bar:** feed state, the time (SNTP, with your time zone), Wi-Fi signal and Settings.
- **Settings:** Wi-Fi (with a scan of nearby networks), the feeder's address and port, the time
  zone and 12/24-hour clock, the text weight, and the receiver's location (else it comes from the feeder's
  `receiver.json`). Saved to flash; changes apply in the background.

On boot an airplane flies along a progress track while Wi-Fi connects. Without a saved network
nano starts on the scope and asks for the settings.

## Build and flash

Needs the `esp` toolchain from [espup](https://github.com/esp-rs/espup) (`source ~/export-esp.sh`)
and `espflash`. Nothing is configured at build time:

```sh
cargo run --release
```

`cargo run` flashes the bootloader, the partition table (`partitions.csv`) and the app, then
opens the serial monitor. **Once**, flash the world map into its partition (4.6 MB: a few
minutes at the default 115 200 baud; reflashing the app keeps it):

```sh
espflash write-bin --chip esp32s3 0x410000 assets/map/world.nmap
```

Without it the scope works, without coastlines (the log says `map: not flashed`). The map is
built by `tools/mapgen` from Natural Earth 1:10m (coastlines, lakes, country and state borders)
and OurAirports (large and medium airports), both public domain; see its source for how.

To check the panel itself, build with `--features panel-test`: it shows fixed colour bars and
1-pixel lines straight from the bounce buffers, without PSRAM or the UI.

## Layout

| Path | What |
|------|------|
| `src/bin/main.rs` | start-up: heap, PSRAM, flash (settings, map), display, touch, tasks, then runs the UI |
| `src/board.rs` | the board's pins and panel timing |
| `src/display/` | the RGB panel as a twine `FramebufferDisplay`; `scanout.rs` refreshes it from the PSRAM framebuffer through internal bounce buffers on core 1 |
| `src/touch.rs` | the GT911 touch controller |
| `src/memory/` | PSRAM split (framebuffers + external heap), the memory-mapped map partition, heap statistics |
| `src/storage.rs` | the partitions; settings records in flash (written while the scan-out is paused) |
| `src/net.rs` | Wi-Fi station following the settings: join, DHCP, reconnect, scan, RSSI |
| `src/clock.rs` | SNTP time |
| `src/feeder/` | polls the feeder over HTTP (`aircraft.json`, `receiver.json`) |
| **platform-neutral** | |
| `src/settings.rs`, `src/connectivity.rs`, `src/time.rs` | state shared by the tasks and the UI (`Latest` cells) |
| `src/traffic/` | parsing tar1090's JSON, trails, positions relative to the receiver |
| `src/map/` | the map format and its decoding into scope pixels |
| `src/ui/` | `boot.rs` (airplane loader), `tracker/` (scope, side panel, title bar), `settings.rs` — only depend on `twine` |
| `tools/mapgen/` | builds `assets/map/world.nmap` (host tool) |
| `assets/` | the Twine logo (twine's `assets/logo/twine-logo-color-128.png` as RGB565A8, MIT/Apache-2.0), the airplane SVGs, the world map |

The UI code has no ESP dependencies, so it also runs in twine's simulator and in
`twine-testing`'s `TestUi`.

## Notes

- **Memory.** Large buffers (the feeder's JSON) are allocated in PSRAM explicitly: the internal
  RAM is left to Wi-Fi. The map is read through the cache from its own partition, mapped after
  the PSRAM at start-up: drawing reads only the cells of the current view, decoded into RAM when
  the view changes, so the display's scan-out never waits behind map reads.
- **Fonts.** The UI uses twine's `montserrat-<size>-smooth` fonts (`src/ui/fonts.rs`):
  unhinted with linear coverage, as LVGL renders them. twine's default fonts are fully hinted
  with a strong light-on-dark text gamma, tuned for low-density panels; on this 4.3" 800 × 480
  panel that snaps thin stems (`i`, `l`, `,` `.`) to hairlines. The LVGL theme uses them too.
  Text is at least 18 px (captions) and 20 px (body): the panel has ~217 px per inch, so 14 px
  capitals are ~1.1 mm tall. **Settings → Display → Text weight** (Light / Medium / Strong)
  sets the display's text gamma (`AppConfig::text_gamma` / `DisplayMut::set_text_gamma` in
  twine), previewed live: Light is LVGL's linear coverage, Strong twine's default lift.
- **Toolchain pin.** The `esp` toolchain is rustc 1.90. `embassy-net` 0.9 needs smoltcp 0.13,
  which requires rustc 1.91, so this project uses `embassy-net` 0.8 / `smoltcp` 0.12. For the same
  reason esp-hal stays on 1.1 (1.2 requires rustc 1.95).
- **Display.** Twine draws into a framebuffer in PSRAM. Core 1 is dedicated to the panel: as in
  ESP-IDF's bounce-buffer mode, the LCD runs continuously from a DMA ring of two 20-row bounce
  buffers in internal SRAM (16 MHz pixel clock, ~39 Hz), and core 1 refills each one from the
  framebuffer as soon as it has been sent. The DMA never reads PSRAM, so bus
  stalls (flash, Wi-Fi, rendering) cannot shift the picture. The log reports late refills if
  core 1 cannot keep up; then lower `board::panel::PCLK_HZ` (e.g. to 12 MHz).
- **PSRAM runs at 80 MHz from a 160 MHz MSPI core clock**, as ESP-IDF runs it; esp-hal's
  default core clock (80 MHz, PSRAM clock undivided) corrupts data. esp-hal 1.1 has no MSPI
  timing tuning; on this board the default sampling reads correctly, and
  `src/memory/psram_check.rs` verifies 256 KiB at every boot. If the log reports errors, fall
  back to 40 MHz (`PsramConfig::default()`) and a 14 MHz pixel clock.
- **Flash runs QIO at 80 MHz** (`espflash.toml`, read by `espflash` and `cargo espflash`;
  both default to DIO at 40 MHz), through ESP-IDF's QIO bootloader (`bootloader/README.md`):
  the header stays DIO for the ROM and the bootloader switches the flash to QIO; the app logs
  `flash: read mode QIO`. This is what keeps the display stable: flash and PSRAM share the
  memory bus and cache, and while core 0 misses the cache in flash (Wi-Fi, code, fonts, images)
  core 1's PSRAM reads wait behind it. In DIO, refills took up to 2.2 ms against a 1 ms budget
  while Wi-Fi connected and the screen flickered; in QIO they fit. `--features bus-test`
  measures the effect of flash and PSRAM traffic on the refills.
