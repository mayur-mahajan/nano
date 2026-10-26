//! Bounce-buffered scan-out on the second core, after ESP-IDF's RGB panel driver
//! (`esp_lcd_panel_rgb.c`, bounce-buffer mode).
//!
//! The LCD_CAM DMA cannot stream the framebuffer from PSRAM directly: PSRAM shares the memory
//! bus with flash, and every stall (code fetches, Wi-Fi, rendering) starves the LCD mid-line and
//! shifts the picture. So, as in ESP-IDF, the DMA streams two small bounce buffers in internal
//! SRAM, which never stall, in an endless ring, and the panel runs continuously (the LCD's
//! "next frame" mode): no gaps in PCLK, HSYNC or VSYNC. Each time the DMA has sent one buffer,
//! the next chunk of the framebuffer is copied into it, wrapping at the end of the frame:
//!
//! ```text
//! PSRAM framebuffer ──(core 1 copies)──▶ bounce A ⇄ bounce B ──(DMA ring)──▶ LCD_CAM ──▶ panel
//! ```
//!
//! ESP-IDF refills from the DMA's EOF interrupt; here core 1 runs only this loop (no scheduler,
//! no interrupts) and polls the DMA's "last EOF descriptor" register instead. It also watches
//! VSYNC: a frame must take exactly [`CHUNKS`] buffers, else a refill was missed and the chunk
//! count is off, so the transfer restarts from the top of the frame in the vertical blank
//! (ESP-IDF: `lcd_rgb_panel_try_restart_transmission`).
//!
//! There are two framebuffers: twine presents the one it finished ([`present`]), and core 1
//! switches to it when it copies the first chunk of a frame, so a refresh never mixes two
//! frames; [`shown`] then reports the switch, and twine may draw into the other one.
//!
//! Core 1 reads the framebuffers through the data cache it shares with core 0, so it sees twine's
//! pixels without any cache maintenance. As in ESP-IDF, after each copy it starts the cache's
//! hardware preload of the next chunk, so the next copy reads mostly from the cache (64 KB with
//! 64-byte lines, set in `.cargo/config.toml`).
//!
//! With the `panel-test` feature the bounce buffers hold a fixed test pattern instead, written
//! once: bands of colour bars and 1-pixel lines, which check the panel's signals and timing
//! without PSRAM, twine or refills.

use core::cell::Cell;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use esp_hal::Blocking;
use esp_hal::dma::{
    BurstConfig, DmaDescriptor, DmaTxBuffer, InternalBurstConfig, Owner, Preparation,
    TransferDirection,
};
#[cfg(feature = "pclk-edge-test")]
use esp_hal::lcd_cam::lcd::Phase;
use esp_hal::lcd_cam::lcd::dpi::{Dpi, DpiTransfer};
use esp_hal::peripherals::{CPU_CTRL, DMA, LCD_CAM};
use esp_hal::system::{AppCoreGuard, CpuControl, Stack};
use static_cell::ConstStaticCell;

use super::{FRAMEBUFFER_BYTES, ROW_BYTES};
use crate::board::panel;

/// Rows per bounce buffer. Each takes ~1 ms to send at 16 MHz: core 1 has that long to refill
/// the other one, so it rides out bus stalls that long.
const ROWS_PER_CHUNK: usize = 20;
/// Bytes per bounce buffer.
const CHUNK_BYTES: usize = ROWS_PER_CHUNK * ROW_BYTES;
/// Chunks (bounce buffers sent) per frame.
const CHUNKS: usize = panel::HEIGHT as usize / ROWS_PER_CHUNK;
/// DMA descriptors per bounce buffer.
const DESCRIPTORS_PER_BUFFER: usize = 8;
/// Bytes per DMA descriptor (below the 4095-byte limit, 4-byte aligned).
const DESCRIPTOR_BYTES: usize = CHUNK_BYTES / DESCRIPTORS_PER_BUFFER;
/// Stack of core 1's loop (it calls no deep code: copies and a few register accesses).
const CORE1_STACK_BYTES: usize = 8 * 1024;

const _: () = assert!((panel::HEIGHT as usize).is_multiple_of(ROWS_PER_CHUNK));
// ESP-IDF: "fb size must be even multiple of bounce buffer size", so every frame starts in A.
const _: () = assert!(CHUNKS.is_multiple_of(2));
const _: () = assert!(CHUNK_BYTES.is_multiple_of(DESCRIPTORS_PER_BUFFER));
const _: () = assert!(DESCRIPTOR_BYTES < 4096 && DESCRIPTOR_BYTES.is_multiple_of(4));
const _: () = assert!(CHUNKS * CHUNK_BYTES == FRAMEBUFFER_BYTES);

/// `pclk-edge-test`: frames per clock edge (~5 s).
#[cfg(feature = "pclk-edge-test")]
const EDGE_TEST_FRAMES: u32 = 5 * 39;

/// The framebuffer twine presented last (0 or 1).
static REQUESTED: AtomicU8 = AtomicU8::new(0);
/// The framebuffer core 1 copies the current frame from (0 or 1).
static SHOWN: AtomicU8 = AtomicU8::new(0);

/// Shows framebuffer `index` from the next frame on.
pub(super) fn present(index: u8) {
    REQUESTED.store(index, Ordering::Release);
}

/// The framebuffer the current frame is copied from: once it equals the last [`present`]ed
/// one, the other framebuffer is no longer read.
pub(super) fn shown() -> u8 {
    SHOWN.load(Ordering::Acquire)
}

/// The GDMA channel the panel uses: `DMA_CH2` in [`PanelPins`](crate::board::PanelPins).
const DMA_CHANNEL: usize = 2;

/// Frames scanned out since start (counted at VSYNC).
static FRAMES: AtomicU32 = AtomicU32::new(0);
/// Core 0 asks core 1 to stop touching the cache (see [`paused`]).
static PAUSE: AtomicBool = AtomicBool::new(false);
/// Core 1 has stopped and spins in IRAM.
static PAUSED: AtomicBool = AtomicBool::new(false);
/// How long [`paused`] waits for core 1 (it acknowledges within one refill, ~1 ms).
const PAUSE_TIMEOUT_CYCLES: u32 = 50_000 * CYCLES_PER_US;
/// Bounce buffers core 1 refilled after the DMA had started sending them again (the panel
/// showed stale rows): core 1 is not keeping up with the pixel clock.
static LATE_REFILLS: AtomicU32 = AtomicU32::new(0);
/// Frames that did not take exactly [`CHUNKS`] buffers, after which the transfer restarted.
static RESYNCS: AtomicU32 = AtomicU32::new(0);

/// Who reads the longest refill: each [`Reader`] has its own maximum, so one reader's reset does
/// not hide a peak from the other.
#[derive(Clone, Copy, Debug)]
pub enum Reader {
    /// The periodic `display:` report.
    Report,
    /// The `bus-test` phases.
    BusTest,
}

/// The longest refill (copy of one chunk into a bounce buffer) since each [`Reader`]'s last
/// [`take_max_refill_us`], in CPU cycles. The budget is one chunk's send time.
static MAX_REFILL_CYCLES: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];
/// CPU cycles per microsecond (`CpuClock::max()`: 240 MHz).
const CYCLES_PER_US: u32 = 240;

/// The time one chunk takes to send, in µs: how long a refill may take.
pub const REFILL_BUDGET_US: u32 = {
    let line_clocks = panel::WIDTH as u32
        + (panel::HSYNC_PULSE + panel::HSYNC_BACK_PORCH + panel::HSYNC_FRONT_PORCH) as u32;
    ROWS_PER_CHUNK as u32 * line_clocks * 1_000 / (panel::PCLK_HZ / 1_000)
};

/// The longest refill since `reader`'s last call, in µs (and resets it for `reader`).
pub fn take_max_refill_us(reader: Reader) -> u32 {
    MAX_REFILL_CYCLES[reader as usize].swap(0, Ordering::Relaxed) / CYCLES_PER_US
}

/// Scan-out counters since start.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub frames: u32,
    pub late_refills: u32,
    pub resyncs: u32,
}

/// The counters since start.
pub fn stats() -> Stats {
    Stats {
        frames: FRAMES.load(Ordering::Relaxed),
        late_refills: LATE_REFILLS.load(Ordering::Relaxed),
        resyncs: RESYNCS.load(Ordering::Relaxed),
    }
}

/// Runs `f` while core 1 is stopped: no DMA, no cache or PSRAM access, spinning in IRAM. For
/// flash writes, which disable the cache that core 1 reads the framebuffer through. The panel
/// shows no new data meanwhile (a short flicker); the scan-out restarts afterwards. Returns
/// `None` without running `f` when core 1 does not stop in time.
pub fn paused<R>(f: impl FnOnce() -> R) -> Option<R> {
    PAUSE.store(true, Ordering::Release);
    let start = cycles();
    while !PAUSED.load(Ordering::Acquire) {
        if cycles().wrapping_sub(start) > PAUSE_TIMEOUT_CYCLES {
            PAUSE.store(false, Ordering::Release);
            log::error!("display: core 1 did not pause");
            return None;
        }
        core::hint::spin_loop();
    }
    let r = f();
    PAUSE.store(false, Ordering::Release);
    Some(r)
}

/// The two bounce buffers (internal SRAM, word aligned for the DMA).
#[repr(C, align(4))]
struct BounceBuffers([[u8; CHUNK_BYTES]; 2]);

static BOUNCE: ConstStaticCell<BounceBuffers> =
    ConstStaticCell::new(BounceBuffers([[0; CHUNK_BYTES]; 2]));
static DESCRIPTORS_MEM: ConstStaticCell<[DmaDescriptor; 2 * DESCRIPTORS_PER_BUFFER]> =
    ConstStaticCell::new([DmaDescriptor::EMPTY; 2 * DESCRIPTORS_PER_BUFFER]);
static CORE1_STACK: ConstStaticCell<Stack<CORE1_STACK_BYTES>> = ConstStaticCell::new(Stack::new());

/// Keeps core 1 running the scan-out; dropping it parks the core and the panel stops.
pub(super) struct Scanout {
    _core1: AppCoreGuard<'static>,
}

/// Starts the scan-out of `framebuffers` (from the first) on core 1. Call once.
///
/// # Panics
/// When core 1 cannot be started.
pub(super) fn start(
    dpi: Dpi<'static, Blocking>,
    framebuffers: [*const u8; 2],
    cpu_ctrl: CPU_CTRL<'static>,
) -> Scanout {
    let bounce = BOUNCE.take();
    let engine = Engine {
        framebuffers,
        source: Cell::new(0),
        bounce: [bounce.0[0].as_mut_ptr(), bounce.0[1].as_mut_ptr()],
    };
    let ring = BounceRing::new(engine.bounce);

    let mut cpu_control = CpuControl::new(cpu_ctrl);
    let guard = cpu_control
        .start_app_core(CORE1_STACK.take(), move || engine.run(dpi, ring))
        .unwrap_or_else(|e| panic!("display: cannot start core 1: {e:?}"));
    if cfg!(feature = "panel-test") {
        log::warn!("display: panel-test: showing a fixed test pattern, not the UI");
    }
    log::info!(
        "display: continuous scan-out on core 1, 2 bounce buffers of {ROWS_PER_CHUNK} rows, \
         {CHUNKS} per frame"
    );
    Scanout { _core1: guard }
}

/// Core 1's state: where to copy from and to.
struct Engine {
    framebuffers: [*const u8; 2],
    /// The framebuffer the current frame is copied from.
    source: Cell<usize>,
    bounce: [*mut u8; 2],
}

// SAFETY: moved to core 1 once and used only there. The framebuffers are `'static` PSRAM, read
// only; the bounce buffers are `'static` and written only by core 1 (and read by the DMA).
unsafe impl Send for Engine {}

/// Where the refills stand.
struct Cursor {
    /// The next chunk of the frame to copy (0..CHUNKS).
    next_chunk: usize,
    /// The bounce buffer the DMA finishes next (0 = A, 1 = B).
    next_eof: usize,
    /// Buffers the DMA finished since the last VSYNC.
    eofs: usize,
}

impl Engine {
    /// Scans out continuously, forever.
    #[esp_hal::ram]
    fn run(self, dpi: Dpi<'static, Blocking>, ring: BounceRing) {
        let last_descriptor = ring.last_descriptors();
        let mut cursor = self.prime();
        let Some(mut transfer) = start_transfer(dpi, ring) else {
            return;
        };
        let mut seen = eof_descriptor();
        let mut vsyncs = 0u32;
        #[cfg(feature = "pclk-edge-test")]
        let mut edge = super::PCLK_PHASE;
        loop {
            if PAUSE.load(Ordering::Acquire) {
                let (dpi, ring) = transfer.stop();
                wait_for_preload();
                PAUSED.store(true, Ordering::Release);
                while PAUSE.load(Ordering::Acquire) {
                    core::hint::spin_loop();
                }
                PAUSED.store(false, Ordering::Release);
                cursor = self.prime();
                let Some(restarted) = start_transfer(dpi, ring) else {
                    return;
                };
                transfer = restarted;
                seen = eof_descriptor();
                vsyncs = 0;
                continue;
            }
            // A bounce buffer was sent: refill it with the next chunk (ESP-IDF:
            // `lcd_rgb_panel_eof_handler`).
            let eof = eof_descriptor();
            if eof != seen {
                seen = eof;
                let Some(sent) = last_descriptor.iter().position(|&d| d == eof) else {
                    continue;
                };
                if sent != cursor.next_eof {
                    // A whole buffer went by unseen: the DMA is sending it again, stale, in the
                    // slot of the next chunk. Skip that chunk so the stream stays aligned with
                    // the frame (one band shows old pixels for one frame); without the skip
                    // every later chunk would land one band lower, for good.
                    LATE_REFILLS.fetch_add(1, Ordering::Relaxed);
                    cursor.eofs += 1;
                    if cursor.next_chunk == 0 {
                        // The skipped chunk starts a frame: switch framebuffers all the same.
                        self.start_frame();
                    }
                    cursor.next_chunk = (cursor.next_chunk + 1) % CHUNKS;
                }
                cursor.eofs += 1;
                cursor.next_eof = 1 - sent;
                let started = cycles();
                self.refill(&mut cursor, sent);
                // Core 1 is the only writer (core 0 only takes the value), so no atomic
                // read-modify-write is needed; one sample may be lost to a concurrent take.
                let took = cycles().wrapping_sub(started);
                for max in &MAX_REFILL_CYCLES {
                    if took > max.load(Ordering::Relaxed) {
                        max.store(took, Ordering::Relaxed);
                    }
                }
                if eof_descriptor() != seen {
                    // The other buffer finished while this one was refilled: the DMA is
                    // already sending this one again, partly stale.
                    LATE_REFILLS.fetch_add(1, Ordering::Relaxed);
                }
            }

            // End of a frame: it must have taken exactly `CHUNKS` buffers.
            if take_vsync() {
                // `pclk-edge-test`: switch the clock edge every 5 s (between frames, like a
                // resync), to compare the two on the real UI.
                #[cfg(feature = "pclk-edge-test")]
                if edge_test_due() {
                    let (mut dpi, ring) = transfer.stop();
                    edge = match edge {
                        Phase::ShiftLow => Phase::ShiftHigh,
                        Phase::ShiftHigh => Phase::ShiftLow,
                    };
                    if let Err(e) = dpi.apply_config(&super::dpi_config(edge)) {
                        log::error!("display: edge test: {edge:?} rejected: {e:?}");
                    }
                    log::warn!("display: edge test: now {edge:?}");
                    cursor = self.prime();
                    let Some(restarted) = start_transfer(dpi, ring) else {
                        return;
                    };
                    transfer = restarted;
                    seen = eof_descriptor();
                    vsyncs = 0;
                }
                vsyncs = vsyncs.saturating_add(1);
                FRAMES.fetch_add(1, Ordering::Relaxed);
                let eofs = core::mem::replace(&mut cursor.eofs, 0);
                // The first frames after a start are partial; judge from the third VSYNC on.
                // A frame must have taken exactly `CHUNKS` buffers, and the next frame's first
                // two chunks must be queued (as after `prime`): otherwise the stream is out of
                // phase with the frame and the picture would stay shifted (ESP-IDF restarts the
                // DMA in VBlank for the same reason).
                if vsyncs > 2 && (eofs != CHUNKS || cursor.next_chunk != 2 % CHUNKS) {
                    RESYNCS.fetch_add(1, Ordering::Relaxed);
                    let (dpi, ring) = transfer.stop();
                    cursor = self.prime();
                    let Some(restarted) = start_transfer(dpi, ring) else {
                        return;
                    };
                    transfer = restarted;
                    seen = eof_descriptor();
                    vsyncs = 0;
                }
            }
        }
    }

    /// Fills both bounce buffers for the start of a frame (chunks 0 and 1, or the test pattern)
    /// and returns the cursor for the refills that follow.
    #[esp_hal::ram]
    fn prime(&self) -> Cursor {
        if cfg!(feature = "panel-test") {
            self.fill_test_pattern();
        } else {
            self.copy(0, 0);
            self.copy(1, 1);
            self.preload(2 % CHUNKS);
        }
        Cursor {
            next_chunk: 2 % CHUNKS,
            next_eof: 0,
            eofs: 0,
        }
    }

    /// Refills bounce buffer `buffer`, just sent, with the next chunk.
    #[esp_hal::ram]
    fn refill(&self, cursor: &mut Cursor, buffer: usize) {
        if cfg!(feature = "panel-test") {
            return;
        }
        self.copy(cursor.next_chunk, buffer);
        cursor.next_chunk = (cursor.next_chunk + 1) % CHUNKS;
        self.preload(cursor.next_chunk);
    }

    /// Starts loading framebuffer chunk `chunk` into the data cache in the background (ESP-IDF:
    /// `Cache_Start_DCache_Preload` at the end of `lcd_rgb_panel_fill_bounce_buffer`).
    #[esp_hal::ram]
    fn preload(&self, chunk: usize) {
        unsafe extern "C" {
            fn Cache_Start_DCache_Preload(addr: u32, size: u32, order: u32) -> u32;
        }
        // SAFETY: ROM function; a preload only fills cache lines, on a range within the
        // framebuffer (`chunk < CHUNKS`). Order 0: ascending addresses.
        unsafe {
            Cache_Start_DCache_Preload(
                self.chunk_ptr(self.next_source(chunk), chunk) as u32,
                CHUNK_BYTES as u32,
                0,
            );
        }
    }

    /// The framebuffer chunk `chunk` will be copied from: the presented one at a frame start,
    /// else the current frame's.
    #[inline(always)]
    fn next_source(&self, chunk: usize) -> usize {
        if chunk == 0 {
            usize::from(REQUESTED.load(Ordering::Acquire) & 1)
        } else {
            self.source.get()
        }
    }

    /// The start of chunk `chunk` of framebuffer `fb`.
    #[inline(always)]
    fn chunk_ptr(&self, fb: usize, chunk: usize) -> *const u8 {
        // SAFETY: `chunk < CHUNKS`: within the framebuffer.
        unsafe { self.framebuffers[fb].add(chunk * CHUNK_BYTES) }
    }

    /// A frame starts: switch to the framebuffer twine presented last.
    #[inline(always)]
    fn start_frame(&self) {
        let fb = self.next_source(0);
        self.source.set(fb);
        SHOWN.store(fb as u8, Ordering::Release);
    }

    /// Copies framebuffer chunk `chunk` into bounce buffer `buffer`; at chunk 0 (a frame start)
    /// first switches to the framebuffer twine presented last.
    #[esp_hal::ram]
    fn copy(&self, chunk: usize, buffer: usize) {
        if chunk == 0 {
            self.start_frame();
        }
        // SAFETY: `chunk < CHUNKS`, so the source lies within the framebuffer; the DMA is
        // sending the other buffer. Twine may be drawing the same pixels on core 0: a frame can
        // tear, never fault.
        unsafe {
            core::ptr::copy_nonoverlapping(
                self.chunk_ptr(self.source.get(), chunk),
                self.bounce[buffer],
                CHUNK_BYTES,
            );
        }
        core::sync::atomic::compiler_fence(Ordering::SeqCst);
    }

    /// The `panel-test` pattern, the same in both buffers so it repeats every
    /// [`ROWS_PER_CHUNK`] rows: half the rows show 8 colour bars (white, yellow, cyan, green,
    /// magenta, red, blue, black), the other half alternating 1-pixel white and black columns.
    /// A correct panel shows crisp bars and an even fine grey; wrong sampling shows colour
    /// fringes at the bar edges and shimmer or noise in the lines.
    fn fill_test_pattern(&self) {
        const BARS: [u16; 8] = [
            0xFFFF, 0xFFE0, 0x07FF, 0x07E0, 0xF81F, 0xF800, 0x001F, 0x0000,
        ];
        let width = panel::WIDTH as usize;
        for buffer in self.bounce {
            let pixels = buffer.cast::<u16>();
            for row in 0..ROWS_PER_CHUNK {
                for x in 0..width {
                    let color = if row < ROWS_PER_CHUNK / 2 {
                        BARS[x * BARS.len() / width]
                    } else if x % 2 == 0 {
                        0xFFFF
                    } else {
                        0x0000
                    };
                    // SAFETY: `row * width + x` < `CHUNK_BYTES / 2`: within the buffer.
                    unsafe { pixels.add(row * width + x).write_volatile(color) };
                }
            }
        }
    }
}

/// Starts the continuous transfer of the ring; logs and returns `None` if it cannot start.
fn start_transfer(
    dpi: Dpi<'static, Blocking>,
    ring: BounceRing,
) -> Option<DpiTransfer<'static, BounceRing, Blocking>> {
    take_vsync();
    match dpi.send(true, ring) {
        Ok(transfer) => Some(transfer),
        Err((e, _, _)) => {
            log::error!("display: scan-out stopped: {e:?}");
            None
        }
    }
}

/// `pclk-edge-test`: whether the clock edge is due to switch (every [`EDGE_TEST_FRAMES`]
/// frames, not at the first one).
#[cfg(feature = "pclk-edge-test")]
fn edge_test_due() -> bool {
    let frames = FRAMES.load(Ordering::Relaxed);
    frames > 0 && frames.is_multiple_of(EDGE_TEST_FRAMES)
}

/// Waits until the last cache preload has finished (before the cache may be disabled).
#[esp_hal::ram]
fn wait_for_preload() {
    unsafe extern "C" {
        fn Cache_DCache_Preload_Done() -> u32;
    }
    // SAFETY: ROM function reading the preload status register.
    while unsafe { Cache_DCache_Preload_Done() } == 0 {
        core::hint::spin_loop();
    }
}

/// The CPU cycle counter (a register read: no flash or memory access).
#[inline(always)]
fn cycles() -> u32 {
    esp_hal::xtensa_lx::timer::get_cycle_count()
}

/// The address of the last descriptor with EOF the DMA finished.
#[inline(always)]
fn eof_descriptor() -> u32 {
    DMA::regs()
        .ch(DMA_CHANNEL)
        .out_eof_des_addr()
        .read()
        .out_eof_des_addr()
        .bits()
}

/// Whether a VSYNC happened since the last call (reads and clears the raw status; the
/// interrupt itself stays disabled).
#[inline(always)]
fn take_vsync() -> bool {
    let regs = LCD_CAM::regs();
    if regs
        .lc_dma_int_raw()
        .read()
        .lcd_vsync_int_raw()
        .bit_is_set()
    {
        regs.lc_dma_int_clr()
            .write(|w| w.lcd_vsync_int_clr().set_bit());
        true
    } else {
        false
    }
}

/// The DMA descriptor ring: bounce A then bounce B, each ending with EOF, B linking back to A
/// (ESP-IDF: `lcd_rgb_panel_init_trans_link`, bounce-buffer mode).
pub(super) struct BounceRing {
    descriptors: &'static mut [DmaDescriptor; 2 * DESCRIPTORS_PER_BUFFER],
}

// SAFETY: built on core 0, then owned by core 1's loop only.
unsafe impl Send for BounceRing {}

impl BounceRing {
    fn new(bounce: [*mut u8; 2]) -> Self {
        let descriptors = DESCRIPTORS_MEM.take();
        let first: *mut DmaDescriptor = &raw mut descriptors[0];
        for i in 0..descriptors.len() {
            let (buffer, part) = (i / DESCRIPTORS_PER_BUFFER, i % DESCRIPTORS_PER_BUFFER);
            let next = if i + 1 < descriptors.len() {
                &raw mut descriptors[i + 1]
            } else {
                first
            };
            let d = &mut descriptors[i];
            d.set_size(DESCRIPTOR_BYTES);
            d.set_length(DESCRIPTOR_BYTES);
            d.set_suc_eof(part == DESCRIPTORS_PER_BUFFER - 1);
            d.set_owner(Owner::Dma);
            // SAFETY: `part * DESCRIPTOR_BYTES < CHUNK_BYTES`: within the bounce buffer.
            d.buffer = unsafe { bounce[buffer].add(part * DESCRIPTOR_BYTES) };
            d.next = next;
        }
        Self { descriptors }
    }

    /// The addresses of the EOF descriptors of bounce A and B, as the DMA reports them.
    fn last_descriptors(&self) -> [u32; 2] {
        let last = |buffer: usize| {
            &raw const self.descriptors[(buffer + 1) * DESCRIPTORS_PER_BUFFER - 1] as u32
        };
        [last(0), last(1)]
    }
}

// SAFETY: the descriptors and the bounce buffers they point to are `'static` and never freed.
unsafe impl DmaTxBuffer for BounceRing {
    type View = Self;
    type Final = Self;

    fn prepare(&mut self) -> Preparation {
        Preparation {
            start: &raw mut self.descriptors[0],
            direction: TransferDirection::Out,
            accesses_psram: false,
            burst_transfer: BurstConfig {
                internal_memory: InternalBurstConfig::Enabled,
                ..BurstConfig::DEFAULT
            },
            // A ring never hands ownership back: the DMA must not check it.
            check_owner: Some(false),
            auto_write_back: false,
        }
    }

    fn into_view(self) -> Self {
        self
    }

    fn from_view(view: Self) -> Self {
        view
    }
}
