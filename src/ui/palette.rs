//! Colours: a night-time radar look (deep navy, phosphor cyan) and tar1090's altitude scale.

use twine::prelude::*;

use crate::traffic::Altitude;

/// Screen background.
pub const SKY: Color = Color::hex(0x060D1A);
/// Title bar and side panel surfaces.
pub const SURFACE: Color = Color::hex(0x0D1A2E);
/// Raised surfaces (cards, list rows).
pub const SURFACE_HI: Color = Color::hex(0x14253F);
/// Hairlines between surfaces.
pub const DIVIDER: Color = Color::hex(0x22385A);
/// Progress track of the boot screen.
pub const TRACK: Color = Color::hex(0x1E3557);
/// The accent: phosphor cyan.
pub const ACCENT: Color = Color::hex(0x4FC3F7);
/// Primary text.
pub const TEXT: Color = Color::hex(0xFFFFFF);
/// Secondary text (8.7:1 on `SURFACE_HI`).
pub const TEXT_MUTED: Color = Color::hex(0xB4C4DE);
/// Good state (live feed, strong signal).
pub const OK: Color = Color::hex(0x5BE38C);
/// Warning state (connecting, not configured).
pub const WARN: Color = Color::hex(0xFFB74D);
/// Error state (no feed, emergency).
pub const ALERT: Color = Color::hex(0xFF5252);

/// Scope: the disc's centre and edge (a radial gradient).
pub const SCOPE_CENTER: Color = Color::hex(0x0F2A3F);
/// See [`SCOPE_CENTER`].
pub const SCOPE_EDGE: Color = Color::hex(0x07131F);
/// Scope: range rings and compass ticks.
pub const RING: Color = Color::hex(0x2E6B7E);
/// Map: sea coasts.
pub const COAST: Color = Color::hex(0x3A8FA3);
/// Map: lake shores.
pub const LAKE: Color = Color::hex(0x2C6E86);
/// Map: country borders.
pub const BORDER: Color = Color::hex(0x6C7FA0);
/// Map: state and province borders.
pub const STATE: Color = Color::hex(0x3B4A66);
/// Map: airports.
pub const AIRPORT: Color = Color::hex(0x9575CD);
/// Text of range-ring labels (6.8:1 on the scope).
pub const RING_TEXT: Color = Color::hex(0x7FB8CC);
/// Text of airport labels (6.2:1 on the scope).
pub const AIRPORT_TEXT: Color = Color::hex(0xB39DDB);

/// The colour of an aircraft at `altitude` (tar1090's scale: orange near the ground through
/// yellow and green to blue and magenta at FL400; grey on the ground or when unknown).
pub fn altitude(altitude: Option<Altitude>) -> Color {
    let ft = match altitude {
        Some(Altitude::Feet(ft)) => ft,
        Some(Altitude::Ground) | None => return Color::hex(0x9E9E9E),
    };
    hsl(altitude_hue(ft), 85, 58)
}

/// tar1090's hue of an altitude in feet.
fn altitude_hue(ft: i32) -> i32 {
    // (altitude ft, hue °), as tar1090's `ColorByAlt.air.h`.
    const STOPS: [(i32, i32); 8] = [
        (0, 20),
        (2_000, 33),
        (4_000, 43),
        (6_000, 54),
        (8_000, 72),
        (9_000, 85),
        (11_000, 140),
        (40_000, 300),
    ];
    match STOPS.iter().position(|&(a, _)| ft < a) {
        Some(0) => STOPS[0].1,
        None => STOPS[STOPS.len() - 1].1,
        Some(i) => {
            let ((a0, h0), (a1, h1)) = (STOPS[i - 1], STOPS[i]);
            h0 + (h1 - h0) * (ft - a0) / (a1 - a0)
        }
    }
}

/// The altitude colour for text: the same hue, lighter, so it reads on the dark surfaces
/// (≥ 6:1; blues and violets are darker for the eye and get more).
pub fn altitude_text(altitude: Option<Altitude>) -> Color {
    let ft = match altitude {
        Some(Altitude::Feet(ft)) => ft,
        Some(Altitude::Ground) | None => return Color::hex(0xC8C8C8),
    };
    let hue = altitude_hue(ft);
    hsl(hue, 85, if (200..=290).contains(&hue) { 84 } else { 76 })
}

/// The legend of [`altitude`]: (label, colour) pairs.
pub fn altitude_legend() -> [(&'static str, Color); 6] {
    let c = |ft| altitude(Some(Altitude::Feet(ft)));
    [
        ("GND", altitude(Some(Altitude::Ground))),
        ("2k", c(2_000)),
        ("6k", c(6_000)),
        ("10k", c(10_000)),
        ("20k", c(20_000)),
        ("40k", c(40_000)),
    ]
}

/// A colour from hue (degrees), saturation and lightness (percent).
pub fn hsl(h: i32, s: i32, l: i32) -> Color {
    let h = h.rem_euclid(360);
    let c = (100 - (2 * l - 100).abs()) * s / 100; // chroma, percent
    let x = c * (60 - ((h % 120) - 60).abs()) / 60;
    let m = l - c / 2;
    let (r, g, b) = match h / 60 {
        0 => (c, x, 0),
        1 => (x, c, 0),
        2 => (0, c, x),
        3 => (0, x, c),
        4 => (x, 0, c),
        _ => (c, 0, x),
    };
    let to = |v: i32| ((v + m).clamp(0, 100) * 255 / 100) as u8;
    Color::new(to(r), to(g), to(b))
}
