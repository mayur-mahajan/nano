//! Air traffic from the ADS-B feeder: the parsed aircraft, their trails, and the state shared
//! with the UI. Platform-neutral (the feeder task in [`crate::feeder`] fetches the files).
//!
//! The feeder task feeds each `aircraft.json` to a [`Tracker`], which keeps a short trail per
//! aircraft and publishes an immutable [`Snapshot`] in [`TRAFFIC`]; the UI renders it.

pub mod aircraft;
pub mod geo;
pub mod parse;

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;

use twine::reactive::Latest;

pub use aircraft::{Aircraft, Altitude, Category, Ident};
use parse::AircraftFile;

/// Trail points kept per aircraft (one per poll: 2 minutes at the 5 s poll interval).
pub const TRAIL_POINTS: usize = 24;
/// Aircraft whose position is older than this are not shown (tar1090's default).
pub const POSITION_TIMEOUT_S: f32 = 60.0;

/// One aircraft with its trail.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    /// The latest report.
    pub aircraft: Aircraft,
    /// Past positions `(lat, lon)`, oldest first, ending with the current one.
    pub trail: Vec<(f32, f32)>,
}

/// What the UI shows: the latest poll.
#[derive(Debug, Default)]
pub struct Snapshot {
    /// The feeder's clock at the poll (Unix seconds).
    pub now: f64,
    /// All aircraft the feeder currently sees (also those without a position), by address.
    pub tracks: Vec<Track>,
    /// Messages per second since the previous poll.
    pub message_rate: f32,
}

impl Snapshot {
    /// The aircraft with a recent position.
    pub fn positioned(&self) -> impl Iterator<Item = &Track> {
        self.tracks
            .iter()
            .filter(|t| t.aircraft.position.is_some() && t.aircraft.seen_pos_s < POSITION_TIMEOUT_S)
    }

    /// The track of the aircraft with `address`.
    pub fn find(&self, address: u32) -> Option<&Track> {
        self.tracks.iter().find(|t| t.aircraft.address == address)
    }
}

/// The connection to the feeder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeedState {
    /// No feeder address set.
    NotConfigured,
    /// Waiting for Wi-Fi.
    Offline,
    /// The first poll is under way.
    Connecting,
    /// The last poll succeeded.
    Live,
    /// The last polls failed (count).
    Unreachable(u16),
}

/// The latest traffic; `None` before the first successful poll.
pub static TRAFFIC: Latest<Option<Arc<Snapshot>>> = Latest::new(None);
/// The connection to the feeder.
pub static FEED: Latest<FeedState> = Latest::new(FeedState::NotConfigured);
/// The receiver's location from the feeder's `receiver.json`, if it publishes one.
pub static RECEIVER: Latest<Option<(f32, f32)>> = Latest::new(None);

/// Keeps the trails between polls.
#[derive(Default)]
pub struct Tracker {
    trails: BTreeMap<u32, Vec<(f32, f32)>>,
    last: Option<(f64, u64)>,
}

impl Tracker {
    /// A tracker without history.
    pub fn new() -> Self {
        Self::default()
    }

    /// Merges a poll and returns its snapshot. Aircraft that left the file lose their trail.
    pub fn update(&mut self, file: AircraftFile) -> Snapshot {
        let message_rate = match self.last {
            Some((t, m)) if file.now > t && file.messages >= m => {
                ((file.messages - m) as f64 / (file.now - t)) as f32
            }
            _ => 0.0,
        };
        self.last = Some((file.now, file.messages));
        let mut trails = BTreeMap::new();
        let mut tracks = Vec::with_capacity(file.aircraft.len());
        for aircraft in file.aircraft {
            let mut trail = self.trails.remove(&aircraft.address).unwrap_or_default();
            if let Some(p) = aircraft.position
                && aircraft.seen_pos_s < POSITION_TIMEOUT_S
                && trail.last() != Some(&p)
            {
                if trail.len() == TRAIL_POINTS {
                    trail.remove(0);
                }
                trail.push(p);
            }
            tracks.push(Track {
                aircraft,
                trail: trail.clone(),
            });
            trails.insert(aircraft.address, trail);
        }
        self.trails = trails;
        Snapshot {
            now: file.now,
            tracks,
            message_rate,
        }
    }
}
