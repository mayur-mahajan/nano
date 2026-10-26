//! Polls the ADS-B feeder: `aircraft.json` every [`POLL_INTERVAL`], `receiver.json` once per
//! connection. Publishes [`TRAFFIC`], [`FEED`] and [`RECEIVER`] for the UI.
//!
//! tar1090 serves its data at `/tar1090/data/`; dump1090-fa's SkyAware at `/data/`. The first
//! path that answers is kept.

pub mod http;

use alloc::sync::Arc;

use embassy_net::Stack;
use embassy_time::{Duration, Instant, Timer};

use crate::connectivity::WIFI_STATE;
use crate::settings::SETTINGS;
use crate::traffic::{FEED, FeedState, RECEIVER, TRAFFIC, Tracker, parse};

/// Time between two polls.
pub const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Largest `aircraft.json` (about 600 aircraft).
const BUFFER_BYTES: usize = 384 * 1024;
/// The data directories tried, in order.
const DATA_DIRS: [&str; 2] = ["/tar1090/data/", "/data/"];

/// The feeder task.
#[embassy_executor::task]
pub async fn run(stack: Stack<'static>) -> ! {
    let buf = crate::memory::psram_buffer(BUFFER_BYTES);
    let mut tracker = Tracker::new();
    let mut seen_settings = u32::MAX;
    let mut dir = 0;
    let mut failures: u16 = 0;
    let mut receiver_known = false;
    loop {
        let start = Instant::now();
        let settings = SETTINGS.get();
        if SETTINGS.version() != seen_settings {
            // A new feeder: start over.
            seen_settings = SETTINGS.version();
            tracker = Tracker::new();
            TRAFFIC.set(None);
            RECEIVER.set(None);
            receiver_known = false;
            failures = 0;
        }
        if !settings.has_feeder() {
            FEED.set(FeedState::NotConfigured);
        } else if !WIFI_STATE.get().is_connected() {
            FEED.set(FeedState::Offline);
        } else {
            if failures == 0 && TRAFFIC.get().is_none() {
                FEED.set(FeedState::Connecting);
            }
            let host = settings.feeder_host.as_str();
            let port = settings.feeder_port;
            match poll(
                stack,
                host,
                port,
                &mut dir,
                buf,
                &mut tracker,
                !receiver_known,
            )
            .await
            {
                Ok((count, receiver)) => {
                    if let Some(r) = receiver {
                        receiver_known = true;
                        if let Some((lat, lon)) = r {
                            log::info!("feeder: receiver at {lat:.4}, {lon:.4}");
                        }
                        RECEIVER.set(r);
                    }
                    if failures > 0 || FEED.get() != FeedState::Live {
                        log::info!(
                            "feeder: live at {host}:{port}{}, {count} aircraft",
                            DATA_DIRS[dir]
                        );
                    }
                    failures = 0;
                    FEED.set(FeedState::Live);
                }
                Err(e) => {
                    failures = failures.saturating_add(1);
                    if failures == 1 || failures.is_multiple_of(12) {
                        log::warn!("feeder: {host}:{port} failed ({e:?}), attempt {failures}");
                    }
                    FEED.set(FeedState::Unreachable(failures));
                }
            }
        }
        Timer::at(start + POLL_INTERVAL).await;
    }
}

/// Why a poll failed.
enum PollError {
    Http(http::Error),
    Json,
}

impl core::fmt::Debug for PollError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Http(e) => write!(f, "{e:?}"),
            Self::Json => f.write_str("bad aircraft.json"),
        }
    }
}

/// One poll: `receiver.json` if asked, then `aircraft.json`. Returns the aircraft count and
/// the receiver's location when it was fetched.
async fn poll(
    stack: Stack<'_>,
    host: &str,
    port: u16,
    dir: &mut usize,
    buf: &mut [u8],
    tracker: &mut Tracker,
    want_receiver: bool,
) -> Result<(usize, Option<Option<(f32, f32)>>), PollError> {
    let ip = http::resolve(stack, host).await.map_err(PollError::Http)?;
    let mut receiver = None;
    if want_receiver {
        // Try each data directory until one serves the file.
        for _ in 0..DATA_DIRS.len() {
            let path = path(*dir, "receiver.json");
            match http::get(stack, ip, host, port, &path, buf).await {
                Ok(body) => {
                    receiver = Some(parse::receiver_location(body).unwrap_or(None));
                    break;
                }
                Err(http::Error::Status(404)) => *dir = (*dir + 1) % DATA_DIRS.len(),
                Err(e) => return Err(PollError::Http(e)),
            }
        }
    }
    let path = path(*dir, "aircraft.json");
    let body = http::get(stack, ip, host, port, &path, buf)
        .await
        .map_err(PollError::Http)?;
    let file = parse::aircraft_file(body).map_err(|e| {
        log::warn!("feeder: aircraft.json: {e}");
        PollError::Json
    })?;
    let count = file.aircraft.len();
    TRAFFIC.set(Some(Arc::new(tracker.update(file))));
    Ok((count, receiver))
}

fn path(dir: usize, file: &str) -> heapless::String<40> {
    let mut p = heapless::String::new();
    let _ = p.push_str(DATA_DIRS[dir]);
    let _ = p.push_str(file);
    p
}
