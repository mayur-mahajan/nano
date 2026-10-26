//! Wall-clock time from SNTP (`pool.ntp.org`), published once a second in [`UNIX_TIME`].
//!
//! The board has no RTC: until the first sync the clock is unknown. It resyncs every hour and
//! otherwise counts with the system timer.

use embassy_net::Stack;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_time::{Duration, Instant, Timer, with_timeout};

use crate::connectivity::WIFI_STATE;
use crate::feeder::http;

pub use crate::time::UNIX_TIME;

/// The SNTP server pool.
const SERVER: &str = "pool.ntp.org";
/// Time between two syncs.
const RESYNC: Duration = Duration::from_secs(3600);
/// Time to wait after a failed sync.
const RETRY: Duration = Duration::from_secs(15);
/// Seconds from 1900 (NTP's epoch) to 1970.
const NTP_TO_UNIX: u64 = 2_208_988_800;

/// The clock task.
#[embassy_executor::task]
pub async fn run(stack: Stack<'static>) -> ! {
    // Unix milliseconds at `Instant` zero, once synced.
    let mut base_ms: Option<u64> = None;
    let mut next_sync = Instant::now();
    loop {
        if Instant::now() >= next_sync && WIFI_STATE.get().is_connected() {
            match sync(stack).await {
                Ok(unix_ms) => {
                    if base_ms.is_none() {
                        log::info!("clock: synced, unix time {}", unix_ms / 1000);
                    }
                    base_ms = Some(unix_ms.saturating_sub(Instant::now().as_millis()));
                    next_sync = Instant::now() + RESYNC;
                }
                Err(e) => {
                    log::warn!("clock: SNTP failed ({e})");
                    next_sync = Instant::now() + RETRY;
                }
            }
        }
        if let Some(base) = base_ms {
            UNIX_TIME.set(Some((base + Instant::now().as_millis()) / 1000));
        }
        // Wake on the next full second.
        let now = Instant::now().as_millis() + base_ms.unwrap_or(0);
        Timer::after(Duration::from_millis(1000 - now % 1000)).await;
    }
}

/// One SNTP exchange; returns the server's time in Unix milliseconds.
async fn sync(stack: Stack<'_>) -> Result<u64, &'static str> {
    let ip = http::resolve(stack, SERVER).await.map_err(|_| "DNS")?;
    let mut rx_meta = [PacketMetadata::EMPTY; 2];
    let mut tx_meta = [PacketMetadata::EMPTY; 2];
    let mut rx = [0u8; 128];
    let mut tx = [0u8; 128];
    let mut socket = UdpSocket::new(stack, &mut rx_meta, &mut rx, &mut tx_meta, &mut tx);
    socket.bind(0).map_err(|_| "bind")?;
    // Version 4, mode 3 (client).
    let mut request = [0u8; 48];
    request[0] = 0x23;
    socket
        .send_to(&request, (ip, 123))
        .await
        .map_err(|_| "send")?;
    let mut reply = [0u8; 48];
    let (n, _) = with_timeout(Duration::from_secs(3), socket.recv_from(&mut reply))
        .await
        .map_err(|_| "timeout")?
        .map_err(|_| "receive")?;
    if n < 48 || reply[0] & 0x07 != 4 {
        return Err("bad reply");
    }
    // Transmit timestamp: seconds and fraction since 1900.
    let secs = u64::from(u32::from_be_bytes([
        reply[40], reply[41], reply[42], reply[43],
    ]));
    let frac = u64::from(u32::from_be_bytes([
        reply[44], reply[45], reply[46], reply[47],
    ]));
    let unix = secs.checked_sub(NTP_TO_UNIX).ok_or("before 1970")?;
    Ok(unix * 1000 + ((frac * 1000) >> 32))
}
