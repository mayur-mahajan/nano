//! Parsing tar1090's (readsb's) `aircraft.json` and `receiver.json`.
//!
//! Only the fields nano shows are read; everything else is skipped. `alt_baro` is a number of
//! feet or the string `"ground"`.

use alloc::string::String;
use alloc::vec::Vec;

use serde::Deserialize;

use super::aircraft::{Aircraft, Altitude, Category, Ident};

/// The parts of `aircraft.json` nano uses.
#[derive(Debug)]
pub struct AircraftFile {
    /// The feeder's clock when it wrote the file (Unix seconds).
    pub now: f64,
    /// Messages received since readsb started.
    pub messages: u64,
    /// The aircraft, in the file's order.
    pub aircraft: Vec<Aircraft>,
}

/// Parses `aircraft.json`.
///
/// # Errors
/// The JSON error, when the file is not valid `aircraft.json`.
pub fn aircraft_file(json: &[u8]) -> Result<AircraftFile, serde_json::Error> {
    let raw: RawFile = serde_json::from_slice(json)?;
    Ok(AircraftFile {
        now: raw.now,
        messages: raw.messages,
        aircraft: raw
            .aircraft
            .into_iter()
            .filter_map(RawAircraft::into_aircraft)
            .collect(),
    })
}

/// Parses `receiver.json`: the receiver's location `(lat, lon)`, when the feeder publishes it.
///
/// # Errors
/// The JSON error, when the file is not valid JSON.
pub fn receiver_location(json: &[u8]) -> Result<Option<(f32, f32)>, serde_json::Error> {
    let raw: RawReceiver = serde_json::from_slice(json)?;
    Ok(match (raw.lat, raw.lon) {
        (Some(lat), Some(lon)) if (lat, lon) != (0.0, 0.0) => Some((lat as f32, lon as f32)),
        _ => None,
    })
}

#[derive(Deserialize)]
struct RawFile {
    #[serde(default)]
    now: f64,
    #[serde(default)]
    messages: u64,
    #[serde(default)]
    aircraft: Vec<RawAircraft>,
}

#[derive(Deserialize)]
struct RawReceiver {
    lat: Option<f64>,
    lon: Option<f64>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawAltitude {
    Feet(f64),
    Word(String),
}

#[derive(Deserialize)]
struct RawAircraft {
    hex: String,
    flight: Option<String>,
    r: Option<String>,
    t: Option<String>,
    category: Option<String>,
    squawk: Option<String>,
    emergency: Option<String>,
    alt_baro: Option<RawAltitude>,
    alt_geom: Option<f64>,
    gs: Option<f64>,
    track: Option<f64>,
    true_heading: Option<f64>,
    baro_rate: Option<f64>,
    geom_rate: Option<f64>,
    lat: Option<f64>,
    lon: Option<f64>,
    seen_pos: Option<f64>,
    #[serde(default)]
    seen: f64,
    rssi: Option<f64>,
    #[serde(default)]
    messages: u64,
}

impl RawAircraft {
    fn into_aircraft(self) -> Option<Aircraft> {
        let (address, non_icao) = match self.hex.strip_prefix('~') {
            Some(h) => (u32::from_str_radix(h, 16).ok()?, true),
            None => (u32::from_str_radix(&self.hex, 16).ok()?, false),
        };
        let altitude = match self.alt_baro {
            Some(RawAltitude::Feet(ft)) => Some(Altitude::Feet(ft as i32)),
            Some(RawAltitude::Word(w)) if w == "ground" => Some(Altitude::Ground),
            _ => self.alt_geom.map(|ft| Altitude::Feet(ft as i32)),
        };
        let position = match (self.lat, self.lon) {
            (Some(lat), Some(lon)) => Some((lat as f32, lon as f32)),
            _ => None,
        };
        Some(Aircraft {
            address,
            non_icao,
            callsign: Ident::new(self.flight.as_deref().unwrap_or("").trim()),
            registration: Ident::new(self.r.as_deref().unwrap_or("")),
            type_code: Ident::new(self.t.as_deref().unwrap_or("")),
            category: self.category.as_deref().and_then(Category::from_code),
            squawk: self.squawk.as_deref().and_then(|s| s.parse().ok()),
            emergency: matches!(self.emergency.as_deref(), Some(e) if e != "none"),
            altitude,
            ground_speed_kt: self.gs.map(|v| v as f32),
            track_deg: self.track.or(self.true_heading).map(|v| v as f32),
            vertical_rate_fpm: self.baro_rate.or(self.geom_rate).map(|v| v as i32),
            position,
            seen_pos_s: self.seen_pos.map_or(f32::INFINITY, |v| v as f32),
            seen_s: self.seen as f32,
            rssi_dbfs: self.rssi.map(|v| v as f32),
            messages: self.messages.min(u64::from(u32::MAX)) as u32,
        })
    }
}
