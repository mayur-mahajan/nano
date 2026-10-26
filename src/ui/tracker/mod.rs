//! The tracker: the radar scope with the aircraft around the receiver (north up, to scale), a
//! side panel with the traffic list or the selected aircraft's details, and a title bar with the
//! time, the Wi-Fi signal and the settings button.
//!
//! The screen's state is a [`Store`] (twine's store pattern, `twine::guide::app_state`):
//! provided once, read by the parts. Tapping an aircraft on the scope or in the list selects
//! it; tapping elsewhere on the scope clears the selection.

mod blips;
mod panel;
mod scope;
mod title_bar;

use alloc::sync::Arc;
use alloc::vec::Vec;

use twine::prelude::*;

pub use blips::Blip;

use super::palette;
use crate::connectivity::{WIFI_STATE, WifiState};
use crate::map::MAP_DATA;
use crate::settings::{SETTINGS, Settings};
use crate::traffic::{FEED, FeedState, RECEIVER, Snapshot, TRAFFIC, Track};

/// The scope's ranges (radius, nautical miles); the zoom buttons step through them.
pub const RANGES_NM: [u16; 7] = [5, 10, 25, 50, 100, 150, 250];
/// The range at start-up.
const DEFAULT_RANGE: usize = 3;

/// The tracker's state, shared by its parts.
#[derive(Clone, Copy)]
pub struct Store {
    /// The latest poll of the feeder.
    pub traffic: ReadSignal<Option<Arc<Snapshot>>>,
    /// The connection to the feeder.
    pub feed: ReadSignal<FeedState>,
    /// The Wi-Fi connection.
    pub wifi: ReadSignal<WifiState>,
    /// The settings (location override, feeder address, clock).
    pub settings: ReadSignal<Settings>,
    /// The selected aircraft's address.
    pub selected: Signal<Option<u32>>,
    /// Index into [`RANGES_NM`].
    pub range: Signal<usize>,
    /// The scope's centre: the location from the settings, else the feeder's, else the middle
    /// of the traffic.
    pub center: Memo<Option<(f32, f32)>>,
    /// Where the centre came from.
    pub center_source: Memo<CenterSource>,
    /// The aircraft on the scope, in scope pixels.
    pub blips: Memo<Vec<Blip>>,
    /// The selected aircraft's track, while it is in the feed.
    pub selected_track: Memo<Option<Track>>,
    /// The world map, if flashed.
    pub map: ReadSignal<Option<&'static [u8]>>,
}

/// Where the scope's centre comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CenterSource {
    /// The location set in the settings.
    Settings,
    /// The feeder's `receiver.json`.
    Feeder,
    /// The middle of the traffic (no location known).
    Traffic,
    /// Nothing known yet.
    Unknown,
}

impl Store {
    fn new(cx: Scope) -> Self {
        let traffic = cx.watch(&TRAFFIC);
        let settings = cx.watch(&SETTINGS);
        let receiver = cx.watch(&RECEIVER);
        let center_source = cx.memo(move || {
            if settings.with(|s| s.location.is_some()) {
                CenterSource::Settings
            } else if receiver.get().is_some() {
                CenterSource::Feeder
            } else if traffic.with(|t| t.as_ref().is_some_and(|t| t.positioned().next().is_some()))
            {
                CenterSource::Traffic
            } else {
                CenterSource::Unknown
            }
        });
        let center = cx.memo(move || match center_source.get() {
            CenterSource::Settings => settings.with(|s| s.location),
            CenterSource::Feeder => receiver.get(),
            CenterSource::Traffic => traffic.with(|t| t.as_deref().and_then(blips::centroid)),
            CenterSource::Unknown => None,
        });
        let selected = cx.signal(None::<u32>);
        let range = cx.signal(DEFAULT_RANGE);
        let blips = cx.memo(move || {
            let (Some(center), Some(t)) = (center.get(), traffic.get()) else {
                return Vec::new();
            };
            blips::project(&t, center, RANGES_NM[range.get()], selected.get())
        });
        let selected_track = cx.memo(move || {
            let address = selected.get()?;
            traffic.with(|t| t.as_ref()?.find(address).cloned())
        });
        Self {
            traffic,
            feed: cx.watch(&FEED),
            wifi: cx.watch(&WIFI_STATE),
            settings,
            selected,
            range,
            center,
            center_source,
            blips,
            selected_track,
            map: cx.watch(&MAP_DATA),
        }
    }

    /// Zooms in (`-1`) or out (`+1`) one step.
    pub fn zoom(self, step: i32) {
        self.range.update(|r| {
            *r = (*r as i32 + step).clamp(0, RANGES_NM.len() as i32 - 1) as usize;
        });
    }

    /// The scope's range, nautical miles.
    pub fn range_nm(self) -> u16 {
        RANGES_NM[self.range.get()]
    }
}

/// The tracker screen; `open_settings` shows the settings.
pub fn screen(cx: Scope, open_settings: impl Fn() + Copy + 'static) -> impl View {
    let store = Store::new(cx);
    cx.provide(store);
    // A selected aircraft that left the feed is deselected.
    cx.effect(move || {
        if store.selected.get().is_some()
            && store.traffic.with(|t| t.is_some())
            && store.selected_track.with(Option::is_none)
        {
            store.selected.set(None);
        }
    });
    column((
        title_bar::view(cx, open_settings),
        row((scope::view(cx, open_settings), panel::view(cx)))
            .gap(0)
            .fill_width()
            .flex_grow(1),
    ))
    .gap(0)
    .bg(palette::SKY)
    .scrollable(false)
    .fill()
}
