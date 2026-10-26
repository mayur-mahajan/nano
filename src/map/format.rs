//! The world map's binary format (`assets/map/world.nmap`, flashed to the `map` partition) and
//! its decoder. Written by `tools/mapgen`, which includes this file, so both sides share it.
//!
//! The world is cut into 1° × 1° cells; each cell holds its own polylines (clipped to the cell)
//! and airports, so a view reads only the cells it covers. All values are little endian.
//!
//! ```text
//! header   "NMAP", version u16, rows u16 (180), columns u16 (360), reserved u16,
//!          index offset u32
//! index    rows × columns + 1 offsets u32 (from the start); cell i is offset[i]..offset[i + 1]
//! cell     line count u16, airport count u16, lines, airports
//! line     kind u8, point count (varint), first point x u16, y u16,
//!          then per point dx, dy (zig-zag varints)
//! airport  kind u8, ident [u8; 4] (ICAO, space padded), x u16, y u16
//! ```
//!
//! Coordinates in a cell are fractions of the cell: `x = (lon - west) × 65535`,
//! `y = (lat - south) × 65535`.

/// File magic.
pub const MAGIC: [u8; 4] = *b"NMAP";
/// Format version.
pub const VERSION: u16 = 1;
/// Cell rows (1° of latitude each, from 90° S).
pub const ROWS: u16 = 180;
/// Cell columns (1° of longitude each, from 180° W).
pub const COLUMNS: u16 = 360;
/// Bytes before the index.
pub const HEADER_BYTES: usize = 16;
/// Cell coordinate units per degree.
pub const UNITS: f32 = 65535.0;

/// What a polyline outlines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    /// A sea coast.
    Coast,
    /// A lake shore.
    Lake,
    /// A border between countries.
    Country,
    /// A border between states or provinces.
    State,
}

impl LineKind {
    /// The kind of a stored byte.
    pub const fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Coast),
            1 => Some(Self::Lake),
            2 => Some(Self::Country),
            3 => Some(Self::State),
            _ => None,
        }
    }

    /// The stored byte.
    pub const fn to_u8(self) -> u8 {
        self as u8
    }
}

/// How large an airport is (OurAirports' type).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AirportKind {
    /// `large_airport`.
    Large,
    /// `medium_airport`.
    Medium,
}

impl AirportKind {
    /// The kind of a stored byte.
    pub const fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Large),
            1 => Some(Self::Medium),
            _ => None,
        }
    }

    /// The stored byte.
    pub const fn to_u8(self) -> u8 {
        self as u8
    }
}

/// The index of the cell holding `(lat, lon)` (degrees), or `None` at the poles' edge.
pub fn cell_of(lat: f32, lon: f32) -> Option<u32> {
    let row = libm_floor(lat + 90.0);
    let col = libm_floor(lon + 180.0);
    cell_at(row as i32, col as i32)
}

/// The index of the cell at `row` (from 90° S) and `col` (from 180° W, wrapping around).
pub fn cell_at(row: i32, col: i32) -> Option<u32> {
    if !(0..i32::from(ROWS)).contains(&row) {
        return None;
    }
    let col = col.rem_euclid(i32::from(COLUMNS));
    Some(row as u32 * u32::from(COLUMNS) + col as u32)
}

/// The south-west corner of a cell, in degrees `(lat, lon)`.
pub fn cell_origin(cell: u32) -> (f32, f32) {
    let row = cell / u32::from(COLUMNS);
    let col = cell % u32::from(COLUMNS);
    (row as f32 - 90.0, col as f32 - 180.0)
}

/// The byte offset of `cell`'s index entry.
pub const fn index_entry(cell: u32) -> usize {
    HEADER_BYTES + cell as usize * 4
}

/// Checks a header; returns the index offset.
pub fn check_header(h: &[u8]) -> Option<u32> {
    if h.len() < HEADER_BYTES || h[0..4] != MAGIC {
        return None;
    }
    let version = u16::from_le_bytes([h[4], h[5]]);
    let rows = u16::from_le_bytes([h[6], h[7]]);
    let cols = u16::from_le_bytes([h[8], h[9]]);
    (version == VERSION && rows == ROWS && cols == COLUMNS)
        .then(|| u32::from_le_bytes([h[12], h[13], h[14], h[15]]))
}

/// A decoded polyline's points, in cell units.
#[derive(Clone)]
pub struct Points<'a> {
    cell: &'a [u8],
    pos: usize,
    /// The first point, not yet yielded.
    first: Option<(u16, u16)>,
    /// Deltas left.
    left: u32,
    x: i32,
    y: i32,
}

impl Iterator for Points<'_> {
    type Item = (u16, u16);

    fn next(&mut self) -> Option<(u16, u16)> {
        if let Some(p) = self.first.take() {
            return Some(p);
        }
        if self.left == 0 {
            return None;
        }
        let dx = unzigzag(varint(self.cell, &mut self.pos)?);
        let dy = unzigzag(varint(self.cell, &mut self.pos)?);
        self.left -= 1;
        self.x = self.x.wrapping_add(dx);
        self.y = self.y.wrapping_add(dy);
        Some((self.x.clamp(0, 65535) as u16, self.y.clamp(0, 65535) as u16))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.left as usize + usize::from(self.first.is_some());
        (n, Some(n))
    }
}

/// One item of a cell.
pub enum Item<'a> {
    /// A polyline: its kind and points.
    Line(LineKind, Points<'a>),
    /// An airport: its kind, ICAO ident and position in cell units.
    Airport(AirportKind, [u8; 4], (u16, u16)),
}

/// Decodes a cell's items in order (lines, then airports), calling `f` for each. Returns
/// `false` at the first malformed item (an empty cell is valid).
pub fn decode_cell(cell: &[u8], mut f: impl FnMut(Item<'_>)) -> bool {
    if cell.is_empty() {
        return true;
    }
    let mut pos = 0;
    let (Some(lines), Some(airports)) = (u16_at(cell, &mut pos), u16_at(cell, &mut pos)) else {
        return false;
    };
    for _ in 0..lines {
        let Some(kind) = cell.get(pos).copied().and_then(LineKind::from_u8) else {
            return false;
        };
        pos += 1;
        let Some(count) = varint(cell, &mut pos).filter(|&c| c > 0) else {
            return false;
        };
        let (Some(x), Some(y)) = (u16_at(cell, &mut pos), u16_at(cell, &mut pos)) else {
            return false;
        };
        let points = Points {
            cell,
            pos,
            first: Some((x, y)),
            left: count - 1,
            x: i32::from(x),
            y: i32::from(y),
        };
        // Walk a copy to the end to find the next item (and to check the deltas).
        let mut rest = points.clone();
        while rest.next().is_some() {}
        if rest.left != 0 {
            return false;
        }
        pos = rest.pos;
        f(Item::Line(kind, points));
    }
    for _ in 0..airports {
        let Some(rec) = cell.get(pos..pos + AIRPORT_BYTES) else {
            return false;
        };
        let Some(kind) = AirportKind::from_u8(rec[0]) else {
            return false;
        };
        let ident = [rec[1], rec[2], rec[3], rec[4]];
        let x = u16::from_le_bytes([rec[5], rec[6]]);
        let y = u16::from_le_bytes([rec[7], rec[8]]);
        f(Item::Airport(kind, ident, (x, y)));
        pos += AIRPORT_BYTES;
    }
    true
}

/// Bytes per airport record.
pub const AIRPORT_BYTES: usize = 9;

fn u16_at(b: &[u8], pos: &mut usize) -> Option<u16> {
    let v = b.get(*pos..*pos + 2)?;
    *pos += 2;
    Some(u16::from_le_bytes([v[0], v[1]]))
}

/// Reads an unsigned LEB128 varint (at most 5 bytes).
fn varint(b: &[u8], pos: &mut usize) -> Option<u32> {
    let mut v = 0u32;
    for shift in (0..35).step_by(7) {
        let byte = *b.get(*pos)?;
        *pos += 1;
        v |= u32::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

const fn unzigzag(v: u32) -> i32 {
    ((v >> 1) as i32) ^ -((v & 1) as i32)
}

/// `floor` without libm (values are within ±400).
fn libm_floor(v: f32) -> f32 {
    let t = v as i32 as f32;
    if t > v { t - 1.0 } else { t }
}
