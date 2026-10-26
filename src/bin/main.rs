#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use embassy_executor::Spawner;
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::rng::Rng;
use esp_hal::timer::timg::TimerGroup;
use nano::board::Board;
use nano::display::RgbPanel;
use nano::settings::SETTINGS;
use nano::storage::Storage;
use nano::{clock, feeder, map, memory, net, storage, touch, ui};
use twine::embassy::UiBuilderExt;
use twine::prelude::*;

extern crate alloc;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();

    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let (board, system) = Board::split(peripherals);
    memory::log_flash_mode();
    // The PSRAM heap is added first: the allocator tries regions in the order they were added,
    // so the UI and the app allocate from PSRAM, and the internal RAM below stays for Wi-Fi and
    // the scheduler (they ask for internal memory explicitly).
    let framebuffers = memory::init_psram(system.psram);
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 73744);
    esp_alloc::heap_allocator!(size: 64 * 1024);

    // Flash: the saved settings and the world map (both read before core 1 starts).
    let (mut store, map_region) = Storage::open(system.flash);
    match store.load() {
        Some(settings) => SETTINGS.set(settings),
        None => log::warn!("settings: none saved yet: set up Wi-Fi and the feeder in Settings"),
    }
    match map_region
        .and_then(memory::map_flash)
        .and_then(map::validate)
    {
        Some(data) => {
            log::info!(
                "map: {} KiB mapped at {:p}",
                data.len() / 1024,
                data.as_ptr()
            );
            map::MAP_DATA.set(Some(data));
        }
        None => log::warn!("map: not flashed: the scope shows no coastlines (see README)"),
    }

    let timg0 = TimerGroup::new(system.timg0);
    let sw_interrupt = SoftwareInterruptControl::new(system.sw_interrupt);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    let display = RgbPanel::new(board.panel, framebuffers, system.cpu_ctrl)
        .unwrap_or_else(|e| panic!("display: cannot start the RGB panel: {e:?}"));
    spawner.spawn(nano::display::report_scanout().expect("scan-out report already spawned"));
    let touch = touch::init(board.touch);

    // Wi-Fi follows the settings; the clock and the feeder poller need the network.
    let (controller, interfaces) = esp_radio::wifi::new(system.wifi, Default::default())
        .expect("Failed to initialize Wi-Fi controller");
    let rng = Rng::new();
    let seed = u64::from(rng.random()) << 32 | u64::from(rng.random());
    let (stack, runner) = net::stack(interfaces.station, seed);
    spawner.spawn(net::net_runner(runner).expect("net runner already spawned"));
    spawner.spawn(net::connection(controller, stack).expect("wifi task already spawned"));
    spawner.spawn(clock::run(stack).expect("clock task already spawned"));
    spawner.spawn(feeder::run(stack).expect("feeder task already spawned"));
    spawner.spawn(storage::persist(store).expect("storage task already spawned"));
    #[cfg(feature = "bus-test")]
    spawner.spawn(nano::bus_test::run().expect("bus test already spawned"));

    // The UI owns the display and the touch controller and runs in this task.
    let mut config = ui::config();
    config.engine.mem_info = Some(memory::mem_info);
    config.engine.hires_timer = Some(twine::embassy::hires_now);
    let ui = Ui::builder_fb(display)
        .runtime(Runtime::take().expect("runtime already taken"))
        // Two framebuffers: twine draws the next frame off screen (tear-free).
        .buffers(BufferMode::Full)
        .input(touch)
        .with_embassy_platform()
        .app_config(config)
        .try_build(ui::app)
        .unwrap_or_else(|e| panic!("twine: cannot build the UI: {e}"));
    let (internal, external) = memory::free_heap();
    log::info!(
        "twine: UI running; heap free: {} KiB internal, {} KiB PSRAM",
        internal / 1024,
        external / 1024
    );
    twine::embassy::run(ui).await
}
