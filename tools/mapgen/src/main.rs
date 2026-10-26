//! Builds nano's world map from public-domain data:
//!
//! - Natural Earth 1:10m: `ne_10m_coastline`, `ne_10m_lakes`,
//!   `ne_10m_admin_0_boundary_lines_land`, `ne_10m_admin_1_states_provinces_lines` (GeoJSON,
//!   <https://github.com/nvkelso/natural-earth-vector/tree/master/geojson>);
//! - OurAirports `airports.csv` (<https://davidmegginson.github.io/ourairports-data/>): large
//!   and medium airports.
//!
//! ```sh
//! cargo run --release -- <source dir> ../../assets/map/world.nmap
//! ```
//!
//! Lines are simplified (Douglas–Peucker, [`TOLERANCE_DEG`]), cut at every 1° cell edge and
//! written per cell in the format of `src/map/format.rs`.

#[path = "../../../src/map/format.rs"]
#[allow(dead_code)]
mod format;

use std::collections::HashMap;
use std::path::Path;
use std::{env, fs, process};

use format::{AirportKind, COLUMNS, HEADER_BYTES, LineKind, ROWS, UNITS};

/// Simplification tolerance: about 30 m, below a pixel at the closest zoom (10 nm across
/// 480 px is ~40 m per pixel).
const TOLERANCE_DEG: f64 = 0.0003;

/// The sources, their file names and the kind of line they hold.
const LAYERS: [(&str, LineKind); 4] = [
    ("ne_10m_coastline.geojson", LineKind::Coast),
    ("ne_10m_lakes.geojson", LineKind::Lake),
    ("ne_10m_admin_0_boundary_lines_land.geojson", LineKind::Country),
    ("ne_10m_admin_1_states_provinces_lines.geojson", LineKind::State),
];

/// A point `(lon, lat)` in degrees.
type Pt = (f64, f64);

/// What one cell holds before encoding.
#[derive(Default)]
struct Cell {
    lines: Vec<(LineKind, Vec<(u16, u16)>)>,
    airports: Vec<(AirportKind, [u8; 4], (u16, u16))>,
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: mapgen <source dir> <output .nmap>");
        process::exit(2);
    }
    let src = Path::new(&args[1]);
    let mut cells: HashMap<u32, Cell> = HashMap::new();
    for (file, kind) in LAYERS {
        let lines = read_lines(&src.join(file));
        let before: usize = lines.iter().map(Vec::len).sum();
        let mut after = 0;
        for line in &lines {
            let simple = simplify(line, TOLERANCE_DEG);
            after += simple.len();
            for (cell, piece) in split_into_cells(&simple) {
                cells.entry(cell).or_default().lines.push((kind, piece));
            }
        }
        eprintln!("{file}: {} lines, {before} -> {after} points", lines.len());
    }
    let airports = read_airports(&src.join("airports.csv"));
    eprintln!("airports.csv: {} large/medium airports", airports.len());
    for (kind, ident, (lon, lat)) in airports {
        if let Some(cell) = format::cell_of(lat as f32, lon as f32) {
            let p = in_cell(cell, (lon, lat));
            cells.entry(cell).or_default().airports.push((kind, ident, p));
        }
    }
    let bytes = encode(&cells);
    fs::write(&args[2], &bytes).unwrap_or_else(|e| fail(&format!("{}: {e}", args[2])));
    eprintln!(
        "wrote {}: {} cells with data, {} KiB",
        args[2],
        cells.len(),
        bytes.len() / 1024
    );
}

fn fail(msg: &str) -> ! {
    eprintln!("mapgen: {msg}");
    process::exit(1);
}

/// Every line string (and polygon ring) of a GeoJSON file.
fn read_lines(path: &Path) -> Vec<Vec<Pt>> {
    let text = fs::read_to_string(path).unwrap_or_else(|e| fail(&format!("{}: {e}", path.display())));
    let doc: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|e| fail(&format!("{}: {e}", path.display())));
    let mut out = Vec::new();
    for feature in doc["features"].as_array().into_iter().flatten() {
        let geometry = &feature["geometry"];
        let coords = &geometry["coordinates"];
        match geometry["type"].as_str() {
            Some("LineString") => out.push(points(coords)),
            Some("MultiLineString" | "Polygon") => {
                out.extend(coords.as_array().into_iter().flatten().map(points));
            }
            Some("MultiPolygon") => {
                for polygon in coords.as_array().into_iter().flatten() {
                    out.extend(polygon.as_array().into_iter().flatten().map(points));
                }
            }
            _ => {}
        }
    }
    out
}

fn points(coords: &serde_json::Value) -> Vec<Pt> {
    coords
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| Some((p[0].as_f64()?, p[1].as_f64()?)))
        .collect()
}

/// Douglas–Peucker simplification (keeps both ends).
fn simplify(line: &[Pt], tolerance: f64) -> Vec<Pt> {
    if line.len() < 3 {
        return line.to_vec();
    }
    let mut keep = vec![false; line.len()];
    keep[0] = true;
    keep[line.len() - 1] = true;
    let mut stack = vec![(0, line.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        let (mut far, mut far_d) = (0, 0.0);
        for i in a + 1..b {
            let d = distance_to_segment(line[i], line[a], line[b]);
            if d > far_d {
                (far, far_d) = (i, d);
            }
        }
        if far_d > tolerance {
            keep[far] = true;
            stack.push((a, far));
            stack.push((far, b));
        }
    }
    line.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect()
}

fn distance_to_segment(p: Pt, a: Pt, b: Pt) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0)
    };
    let (cx, cy) = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt()
}

/// Cuts a line at every cell edge; returns the pieces with their cells (in cell units).
fn split_into_cells(line: &[Pt]) -> Vec<(u32, Vec<(u16, u16)>)> {
    let mut out: Vec<(u32, Vec<(u16, u16)>)> = Vec::new();
    let mut current: Option<(u32, Vec<(u16, u16)>)> = None;
    for w in line.windows(2) {
        let (a, b) = (w[0], w[1]);
        if (b.0 - a.0).abs() > 180.0 {
            // Crosses the antimeridian: Natural Earth splits there, so this is a seam.
            out.extend(current.take());
            continue;
        }
        // Parameters where the segment crosses an integer longitude or latitude.
        let mut ts = vec![0.0, 1.0];
        crossings(a.0, b.0, &mut ts);
        crossings(a.1, b.1, &mut ts);
        ts.sort_by(f64::total_cmp);
        ts.dedup_by(|x, y| (*x - *y).abs() < 1e-12);
        for t in ts.windows(2) {
            let p0 = lerp(a, b, t[0]);
            let p1 = lerp(a, b, t[1]);
            let mid = lerp(a, b, (t[0] + t[1]) / 2.0);
            let Some(cell) = format::cell_of(mid.1 as f32, mid.0 as f32) else {
                continue;
            };
            let (q0, q1) = (in_cell(cell, p0), in_cell(cell, p1));
            match &mut current {
                Some((c, pts)) if *c == cell && pts.last() == Some(&q0) => {
                    if pts.last() != Some(&q1) {
                        pts.push(q1);
                    }
                }
                _ => {
                    out.extend(current.take());
                    current = Some((cell, if q0 == q1 { vec![q0] } else { vec![q0, q1] }));
                }
            }
        }
    }
    out.extend(current);
    out.retain(|(_, pts)| pts.len() >= 2);
    out
}

/// Adds the parameters `t` in `(0, 1)` where `v0 + t (v1 − v0)` is an integer.
fn crossings(v0: f64, v1: f64, ts: &mut Vec<f64>) {
    if v0 == v1 {
        return;
    }
    let (lo, hi) = (v0.min(v1), v0.max(v1));
    let mut k = lo.floor() + 1.0;
    while k < hi {
        ts.push((k - v0) / (v1 - v0));
        k += 1.0;
    }
}

fn lerp(a: Pt, b: Pt, t: f64) -> Pt {
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

/// A point in a cell's units (clamped to the cell).
fn in_cell(cell: u32, (lon, lat): Pt) -> (u16, u16) {
    let (south, west) = format::cell_origin(cell);
    let u = |v: f64| (v * f64::from(UNITS)).round().clamp(0.0, 65535.0) as u16;
    (u(lon - f64::from(west)), u(lat - f64::from(south)))
}

/// Large and medium airports: kind, ident (ICAO code if any, else GPS code, else ident) and
/// position `(lon, lat)`.
fn read_airports(path: &Path) -> Vec<(AirportKind, [u8; 4], Pt)> {
    let text = fs::read_to_string(path).unwrap_or_else(|e| fail(&format!("{}: {e}", path.display())));
    let mut rows = text.lines().map(csv_fields);
    let header = rows.next().unwrap_or_default();
    let col = |name: &str| {
        header
            .iter()
            .position(|h| h == name)
            .unwrap_or_else(|| fail(&format!("airports.csv: no column {name}")))
    };
    let (ty, lat, lon) = (col("type"), col("latitude_deg"), col("longitude_deg"));
    let (icao, gps, ident) = (col("icao_code"), col("gps_code"), col("ident"));
    let mut out = Vec::new();
    for row in rows {
        let kind = match row.get(ty).map(String::as_str) {
            Some("large_airport") => AirportKind::Large,
            Some("medium_airport") => AirportKind::Medium,
            _ => continue,
        };
        let (Some(la), Some(lo)) = (
            row.get(lat).and_then(|v| v.parse::<f64>().ok()),
            row.get(lon).and_then(|v| v.parse::<f64>().ok()),
        ) else {
            continue;
        };
        let code = [icao, gps, ident]
            .into_iter()
            .filter_map(|c| row.get(c))
            .find(|c| !c.is_empty())
            .cloned()
            .unwrap_or_default();
        let mut id = *b"    ";
        for (d, s) in id.iter_mut().zip(code.bytes().filter(u8::is_ascii_graphic)) {
            *d = s;
        }
        out.push((kind, id, (lo, la)));
    }
    out
}

/// The fields of one CSV line (double quotes, `""` escapes; no newlines inside fields).
fn csv_fields(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            ('"', _) => quoted = !quoted,
            (',', false) => fields.push(std::mem::take(&mut field)),
            _ => field.push(c),
        }
    }
    fields.push(field);
    fields
}

/// The file: header, index, cells.
fn encode(cells: &HashMap<u32, Cell>) -> Vec<u8> {
    let count = u32::from(ROWS) * u32::from(COLUMNS);
    let index_bytes = (count as usize + 1) * 4;
    let mut out = Vec::new();
    out.extend_from_slice(&format::MAGIC);
    out.extend_from_slice(&format::VERSION.to_le_bytes());
    out.extend_from_slice(&ROWS.to_le_bytes());
    out.extend_from_slice(&COLUMNS.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
    out.resize(HEADER_BYTES + index_bytes, 0);
    for i in 0..count {
        let offset = out.len() as u32;
        out[format::index_entry(i)..][..4].copy_from_slice(&offset.to_le_bytes());
        if let Some(cell) = cells.get(&i) {
            encode_cell(cell, &mut out);
        }
    }
    let end = out.len() as u32;
    out[format::index_entry(count)..][..4].copy_from_slice(&end.to_le_bytes());
    out
}

fn encode_cell(cell: &Cell, out: &mut Vec<u8>) {
    let lines = cell.lines.len().min(usize::from(u16::MAX));
    let airports = cell.airports.len().min(usize::from(u16::MAX));
    out.extend_from_slice(&(lines as u16).to_le_bytes());
    out.extend_from_slice(&(airports as u16).to_le_bytes());
    for (kind, pts) in &cell.lines[..lines] {
        out.push(kind.to_u8());
        varint(pts.len() as u32, out);
        out.extend_from_slice(&pts[0].0.to_le_bytes());
        out.extend_from_slice(&pts[0].1.to_le_bytes());
        for w in pts.windows(2) {
            varint(zigzag(i32::from(w[1].0) - i32::from(w[0].0)), out);
            varint(zigzag(i32::from(w[1].1) - i32::from(w[0].1)), out);
        }
    }
    for (kind, ident, (x, y)) in &cell.airports[..airports] {
        out.push(kind.to_u8());
        out.extend_from_slice(ident);
        out.extend_from_slice(&x.to_le_bytes());
        out.extend_from_slice(&y.to_le_bytes());
    }
}

fn varint(mut v: u32, out: &mut Vec<u8>) {
    while v >= 0x80 {
        out.push((v as u8 & 0x7F) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

const fn zigzag(v: i32) -> u32 {
    ((v << 1) ^ (v >> 31)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_round_trip_through_the_decoder() {
        let mut cells = HashMap::new();
        let line = vec![(0.5, 10.5), (0.9, 10.9), (1.5, 11.5)]; // crosses into the next cell
        for (cell, piece) in split_into_cells(&line) {
            cells.entry(cell).or_insert_with(Cell::default).lines.push((LineKind::Coast, piece));
        }
        let home = format::cell_of(10.5, 0.5).unwrap();
        cells.entry(home).or_default().airports.push((AirportKind::Large, *b"KSFO", (100, 200)));
        let file = encode(&cells);
        assert_eq!(format::check_header(&file), Some(HEADER_BYTES as u32));
        let range = |c: u32| {
            let at = |i: u32| {
                u32::from_le_bytes(file[format::index_entry(i)..][..4].try_into().unwrap()) as usize
            };
            at(c)..at(c + 1)
        };
        let mut seen = Vec::new();
        assert!(format::decode_cell(&file[range(home)], |item| match item {
            format::Item::Line(kind, pts) => seen.push((kind, pts.collect::<Vec<_>>())),
            format::Item::Airport(kind, id, p) => {
                assert_eq!((kind, &id, p), (AirportKind::Large, b"KSFO", (100, 200)));
            }
        }));
        assert_eq!(seen.len(), 1);
        let pts = &seen[0].1;
        assert_eq!(pts[0], in_cell(home, (0.5, 10.5)));
        assert_eq!(*pts.last().unwrap(), (65535, 65535)); // cut at the cell's corner
        let next = format::cell_of(11.5, 1.5).unwrap();
        let mut n = 0;
        assert!(format::decode_cell(&file[range(next)], |_| n += 1));
        assert_eq!(n, 1);
    }

    #[test]
    fn simplify_keeps_corners() {
        let line = [(0.0, 0.0), (0.5, 0.00001), (1.0, 0.0), (1.0, 1.0)];
        assert_eq!(simplify(&line, 0.001), vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)]);
    }

    #[test]
    fn csv_handles_quotes() {
        assert_eq!(csv_fields(r#"1,"a, ""b""",c"#), ["1", "a, \"b\"", "c"]);
    }
}
