//! Positions relative to the receiver: a local flat-earth (equirectangular) projection, exact
//! to well under a pixel within the scope's 250 nm.

use libm::{atan2f, cosf, sqrtf};

/// Nautical miles per degree of latitude.
pub const NM_PER_DEG: f32 = 60.0;

/// A projection centred on the receiver.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Local {
    /// The centre `(lat, lon)`, degrees.
    pub center: (f32, f32),
    /// `cos(lat)` of the centre: nautical miles per degree of longitude / 60.
    cos_lat: f32,
}

impl Local {
    /// The projection around `center` `(lat, lon)`.
    pub fn new(center: (f32, f32)) -> Self {
        Self {
            center,
            cos_lat: cosf(center.0.to_radians()).max(0.01),
        }
    }

    /// `(east, north)` of `p` `(lat, lon)` from the centre, nautical miles.
    pub fn offset_nm(&self, p: (f32, f32)) -> (f32, f32) {
        let mut dlon = p.1 - self.center.1;
        if dlon > 180.0 {
            dlon -= 360.0;
        } else if dlon < -180.0 {
            dlon += 360.0;
        }
        (
            dlon * NM_PER_DEG * self.cos_lat,
            (p.0 - self.center.0) * NM_PER_DEG,
        )
    }

    /// Degrees of latitude and longitude spanned by `nm` nautical miles.
    pub fn span_deg(&self, nm: f32) -> (f32, f32) {
        (nm / NM_PER_DEG, nm / (NM_PER_DEG * self.cos_lat))
    }
}

/// Distance (nm) and bearing (degrees from true north, 0..360) of an `(east, north)` offset.
pub fn polar((east, north): (f32, f32)) -> (f32, f32) {
    let bearing = atan2f(east, north).to_degrees();
    (
        sqrtf(east * east + north * north),
        if bearing < 0.0 {
            bearing + 360.0
        } else {
            bearing
        },
    )
}

/// The 16-point compass name of a bearing.
pub fn compass_point(bearing: f32) -> &'static str {
    const NAMES: [&str; 16] = [
        "N", "NNE", "NE", "ENE", "E", "ESE", "SE", "SSE", "S", "SSW", "SW", "WSW", "W", "WNW",
        "NW", "NNW",
    ];
    let b = bearing % 360.0;
    let b = if b < 0.0 { b + 360.0 } else { b };
    let i = ((b + 11.25) / 22.5) as usize % 16;
    NAMES[i]
}
