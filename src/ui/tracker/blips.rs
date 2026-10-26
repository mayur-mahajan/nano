//! The aircraft on the scope ("blips"): projected to scope pixels once per poll or view
//! change, with their trails and where their callouts go.

use alloc::vec::Vec;

use twine::prelude::*;

use super::super::palette;
use super::scope::{CENTER, RADIUS};
use crate::traffic::geo::{Local, polar};
use crate::traffic::{Aircraft, Altitude, Category, Ident, Snapshot};

/// Callouts shown when nothing is selected: the aircraft nearest the receiver.
const MAX_CALLOUTS: usize = 6;
/// A callout's size.
pub const CALLOUT_SIZE: Size = Size::new(140, 52);
/// The zoom controls' area (scope pixels): callouts stay out of it.
pub const CONTROLS: Rect = Rect::new(0, 270, 70, 440);

/// One aircraft as the scope draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct Blip {
    /// Its address (the key).
    pub address: u32,
    /// Position in scope pixels.
    pub at: Point,
    /// Track over ground, degrees (none: drawn as a dot).
    pub track: Option<f32>,
    /// Altitude colour.
    pub color: Color,
    /// Past positions in scope pixels, oldest first.
    pub trail: Vec<Point>,
    /// Glyph size class.
    pub category: Option<Category>,
    /// Callsign, else registration, else hex.
    pub name: Ident,
    /// Altitude.
    pub altitude: Option<Altitude>,
    /// Ground speed, knots.
    pub speed_kt: Option<f32>,
    /// Distance from the receiver, nautical miles.
    pub distance_nm: f32,
    /// Emergency squawk or declared emergency.
    pub emergency: bool,
    /// The selected aircraft.
    pub selected: bool,
    /// Where its callout's top-left corner goes, when it has one.
    pub callout: Option<Point>,
}

impl Blip {
    /// `FL350` / `5,400 ft` / `GND`.
    pub fn altitude_text(&self, out: &mut heapless::String<12>) {
        use core::fmt::Write as _;
        let _ = match self.altitude {
            Some(Altitude::Ground) => out.push_str("GND").map_err(|_| core::fmt::Error),
            Some(Altitude::Feet(ft)) if ft >= 18_000 => write!(out, "FL{}", (ft + 50) / 100),
            Some(Altitude::Feet(ft)) => write!(out, "{}ft", (ft + 50) / 100 * 100),
            None => out.push_str("--").map_err(|_| core::fmt::Error),
        };
    }
}

/// The blips of `snapshot` around `center` at `range_nm`; the nearest get callouts (only the
/// selected one while something is selected).
pub fn project(
    snapshot: &Snapshot,
    center: (f32, f32),
    range_nm: u16,
    selected: Option<u32>,
) -> Vec<Blip> {
    let local = Local::new(center);
    let px_per_nm = RADIUS as f32 / f32::from(range_nm);
    let to_px = |p: (f32, f32)| {
        let (e, n) = local.offset_nm(p);
        Point::new(
            CENTER.x + (e * px_per_nm) as i32,
            CENTER.y - (n * px_per_nm) as i32,
        )
    };
    let limit = RADIUS + 12;
    let mut blips: Vec<Blip> = snapshot
        .positioned()
        .filter_map(|t| {
            let a: &Aircraft = &t.aircraft;
            let pos = a.position?;
            let (distance_nm, _) = polar(local.offset_nm(pos));
            let at = to_px(pos);
            let (dx, dy) = (at.x - CENTER.x, at.y - CENTER.y);
            if dx * dx + dy * dy > limit * limit {
                return None;
            }
            Some(Blip {
                address: a.address,
                at,
                track: a.track_deg,
                color: palette::altitude(a.altitude),
                trail: t.trail.iter().map(|&p| to_px(p)).collect(),
                category: a.category,
                name: name_of(a),
                altitude: a.altitude,
                speed_kt: a.ground_speed_kt,
                distance_nm,
                emergency: a.emergency || a.emergency_squawk(),
                selected: Some(a.address) == selected,
                callout: None,
            })
        })
        .collect();
    // Nearest first: callouts go to the closest, and later blips draw over earlier ones, so
    // the selected one is moved to the end.
    blips.sort_by(|a, b| a.distance_nm.total_cmp(&b.distance_nm));
    place_callouts(&mut blips, selected.is_some());
    if let Some(i) = blips.iter().position(|b| b.selected) {
        let b = blips.remove(i);
        blips.push(b);
    }
    blips
}

/// The middle of the traffic (when the receiver's location is unknown).
pub fn centroid(snapshot: &Snapshot) -> Option<(f32, f32)> {
    let (mut lat, mut lon, mut n) = (0.0f32, 0.0f32, 0u32);
    for t in snapshot.positioned() {
        if let Some((a, o)) = t.aircraft.position {
            lat += a;
            lon += o;
            n += 1;
        }
    }
    (n > 0).then(|| (lat / n as f32, lon / n as f32))
}

fn name_of(a: &Aircraft) -> Ident {
    if !a.callsign.is_empty() {
        a.callsign
    } else if !a.registration.is_empty() {
        a.registration
    } else {
        let mut buf = [0u8; 8];
        Ident::new(crate::traffic::aircraft::hex6(a.address, &mut buf))
    }
}

/// Greedy placement: each callout tries the four corners around its blip and takes the first
/// that stays inside the scope's square and overlaps no earlier callout or blip.
fn place_callouts(blips: &mut [Blip], selected_only: bool) {
    let mut taken: Vec<Rect> = alloc::vec![CONTROLS];
    let glyphs: Vec<Rect> = blips
        .iter()
        .map(|b| Rect::from_xywh(b.at.x - 8, b.at.y - 8, 16, 16))
        .collect();
    let (w, h) = (CALLOUT_SIZE.w, CALLOUT_SIZE.h);
    let mut shown = 0;
    for (i, b) in blips.iter_mut().enumerate() {
        if selected_only != b.selected || (!selected_only && shown == MAX_CALLOUTS) {
            continue;
        }
        let candidates = [
            Point::new(b.at.x + 12, b.at.y - h - 4),
            Point::new(b.at.x + 12, b.at.y + 4),
            Point::new(b.at.x - w - 12, b.at.y - h - 4),
            Point::new(b.at.x - w - 12, b.at.y + 4),
        ];
        let fits = |p: &Point| {
            let r = Rect::from_xywh(p.x, p.y, w, h);
            let inside = r.x0 >= 2
                && r.y0 >= 2
                && r.x0 + w <= 2 * CENTER.x - 2
                && r.y0 + h <= 2 * CENTER.y - 2;
            inside
                && !taken.iter().any(|t| t.intersects(&r))
                && !glyphs
                    .iter()
                    .enumerate()
                    .any(|(j, g)| j != i && g.intersects(&r))
        };
        if let Some(p) = candidates.iter().find(|p| fits(p)) {
            taken.push(Rect::from_xywh(p.x, p.y, w, h));
            b.callout = Some(*p);
            shown += 1;
        }
    }
}
