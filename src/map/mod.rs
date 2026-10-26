//! The world map behind the radar scope (platform-neutral).
//!
//! The map data ([`format`]) lives in its own flash partition, memory-mapped at start-up and
//! published in [`MAP_DATA`]. [`Geometry::build`] decodes the cells a view covers into scope
//! pixels, once per view change: the scope then draws from RAM, so drawing never reads flash.

pub mod format;

use alloc::vec::Vec;

use format::{AirportKind, Item, LineKind};
use twine::core::Point;
use twine::reactive::Latest;

use crate::traffic::geo::Local;

/// The map file, when the `map` partition holds one.
pub static MAP_DATA: Latest<Option<&'static [u8]>> = Latest::new(None);

/// Checks map bytes; returns them when they are a map file.
pub fn validate(bytes: &'static [u8]) -> Option<&'static [u8]> {
    format::check_header(bytes)?;
    Some(bytes)
}

/// What the scope shows of the map: polylines and airports in scope pixels (origin at the
/// scope's centre, y down).
#[derive(Debug, Default, PartialEq)]
pub struct Geometry {
    /// Polylines, by kind.
    pub lines: Vec<(LineKind, Vec<Point>)>,
    /// Airports: kind, ICAO ident, position.
    pub airports: Vec<(AirportKind, [u8; 4], Point)>,
}

/// Points closer than this to the previous one (pixels, squared) are dropped.
const MIN_STEP_SQ: i32 = 4;

impl Geometry {
    /// The map around `center` (`(lat, lon)`) at `px_per_nm`, out to `radius_px`.
    pub fn build(data: &[u8], center: (f32, f32), px_per_nm: f32, radius_px: i32) -> Self {
        let local = Local::new(center);
        let radius_nm = radius_px as f32 / px_per_nm;
        let (dlat, dlon) = local.span_deg(radius_nm);
        let mut geometry = Self::default();
        let rows = floor(center.0 - dlat)..=floor(center.0 + dlat);
        let cols = floor(center.1 - dlon)..=floor(center.1 + dlon);
        let limit = radius_px + 8;
        let project = |cell_origin: (f32, f32), (x, y): (u16, u16)| {
            let lat = cell_origin.0 + f32::from(y) / format::UNITS;
            let lon = cell_origin.1 + f32::from(x) / format::UNITS;
            let (east, north) = local.offset_nm((lat, lon));
            Point::new((east * px_per_nm) as i32, (-north * px_per_nm) as i32)
        };
        for row in rows {
            for col in cols.clone() {
                let Some(cell) = format::cell_at(row + 90, col + 180) else {
                    continue;
                };
                let Some(bytes) = cell_bytes(data, cell) else {
                    continue;
                };
                let origin = format::cell_origin(cell);
                format::decode_cell(bytes, |item| match item {
                    Item::Line(kind, points) => {
                        let mut out: Vec<Point> = Vec::with_capacity(points.size_hint().0);
                        let mut inside = false;
                        for p in points {
                            let q = project(origin, p);
                            inside |= q.x.abs() <= limit && q.y.abs() <= limit;
                            if let Some(last) = out.last() {
                                let (dx, dy) = (q.x - last.x, q.y - last.y);
                                if dx * dx + dy * dy < MIN_STEP_SQ {
                                    continue;
                                }
                            }
                            out.push(q);
                        }
                        if inside && out.len() >= 2 {
                            geometry.lines.push((kind, out));
                        }
                    }
                    Item::Airport(kind, ident, p) => {
                        let q = project(origin, p);
                        if q.x * q.x + q.y * q.y <= radius_px * radius_px {
                            geometry.airports.push((kind, ident, q));
                        }
                    }
                });
            }
        }
        geometry
    }

    /// Points in all lines (for logging the cost of a view).
    pub fn points(&self) -> usize {
        self.lines.iter().map(|(_, p)| p.len()).sum()
    }
}

/// The bytes of `cell`.
fn cell_bytes(data: &[u8], cell: u32) -> Option<&[u8]> {
    let at = |i: u32| -> Option<usize> {
        let e = data.get(format::index_entry(i)..format::index_entry(i) + 4)?;
        Some(u32::from_le_bytes([e[0], e[1], e[2], e[3]]) as usize)
    };
    data.get(at(cell)?..at(cell + 1)?)
}

fn floor(v: f32) -> i32 {
    let t = v as i32;
    if (t as f32) > v { t - 1 } else { t }
}
