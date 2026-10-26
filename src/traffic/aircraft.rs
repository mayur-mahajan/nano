//! One aircraft as the feeder reports it.

use core::fmt;

/// A short ASCII identifier (callsign, registration, type code), up to 8 characters.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Ident {
    bytes: [u8; 8],
    len: u8,
}

impl Ident {
    /// The first 8 printable ASCII characters of `s`.
    pub fn new(s: &str) -> Self {
        let mut id = Self::default();
        for b in s.bytes().filter(u8::is_ascii_graphic).take(8) {
            id.bytes[usize::from(id.len)] = b;
            id.len += 1;
        }
        id
    }

    /// The identifier.
    pub fn as_str(&self) -> &str {
        // Only ASCII is stored.
        core::str::from_utf8(&self.bytes[..usize::from(self.len)]).unwrap_or("")
    }

    /// Nothing stored.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl fmt::Display for Ident {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

impl fmt::Debug for Ident {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.as_str())
    }
}

/// Barometric altitude.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Altitude {
    /// On the ground.
    Ground,
    /// Feet above the 1013 hPa level.
    Feet(i32),
}

/// The ADS-B emitter category (`A1`…`C7`), reduced to what changes the glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    /// A1–A2: light and small aircraft.
    Light,
    /// A3–A4: large aircraft (airliners).
    Large,
    /// A5: heavy aircraft.
    Heavy,
    /// A6: high performance.
    Fast,
    /// A7: rotorcraft.
    Helicopter,
    /// B1–B4, B6: gliders, balloons, ultralights, drones.
    Light2,
    /// C1–C3: surface vehicles.
    Ground,
}

impl Category {
    /// The category of an emitter code such as `"A3"`.
    pub fn from_code(code: &str) -> Option<Self> {
        Some(match code {
            "A1" | "A2" => Self::Light,
            "A3" | "A4" => Self::Large,
            "A5" => Self::Heavy,
            "A6" => Self::Fast,
            "A7" => Self::Helicopter,
            "B1" | "B2" | "B3" | "B4" | "B6" => Self::Light2,
            "C1" | "C2" | "C3" => Self::Ground,
            _ => return None,
        })
    }

    /// A human description.
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Light => "Light aircraft",
            Self::Large => "Airliner",
            Self::Heavy => "Heavy",
            Self::Fast => "High performance",
            Self::Helicopter => "Helicopter",
            Self::Light2 => "Glider / UAV",
            Self::Ground => "Ground vehicle",
        }
    }
}

/// One aircraft in `aircraft.json`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aircraft {
    /// The 24-bit ICAO address (or the non-ICAO address, see `non_icao`).
    pub address: u32,
    /// The address is not an ICAO address (TIS-B, MLAT and the like: `~` in tar1090).
    pub non_icao: bool,
    /// The flight's callsign.
    pub callsign: Ident,
    /// The registration, from the feeder's database.
    pub registration: Ident,
    /// The ICAO type code (`B738`), from the feeder's database.
    pub type_code: Ident,
    /// The emitter category.
    pub category: Option<Category>,
    /// The transponder code (four octal digits as a decimal number, e.g. `7700`).
    pub squawk: Option<u16>,
    /// The aircraft declares an emergency.
    pub emergency: bool,
    /// Barometric (else geometric) altitude.
    pub altitude: Option<Altitude>,
    /// Ground speed, knots.
    pub ground_speed_kt: Option<f32>,
    /// True track over ground (else heading), degrees.
    pub track_deg: Option<f32>,
    /// Vertical rate, feet per minute.
    pub vertical_rate_fpm: Option<i32>,
    /// Position `(lat, lon)`, degrees.
    pub position: Option<(f32, f32)>,
    /// Seconds since the position was received (infinite: never).
    pub seen_pos_s: f32,
    /// Seconds since any message was received.
    pub seen_s: f32,
    /// Signal level of recent messages, dBFS.
    pub rssi_dbfs: Option<f32>,
    /// Messages received from it.
    pub messages: u32,
}

impl Aircraft {
    /// The callsign, else the registration, else the hex address.
    pub fn name(&self) -> Name {
        Name(*self)
    }

    /// Squawking an emergency code (7500 hijack, 7600 radio failure, 7700 emergency).
    pub fn emergency_squawk(&self) -> bool {
        matches!(self.squawk, Some(7500 | 7600 | 7700))
    }
}

/// [`Aircraft::name`].
pub struct Name(Aircraft);

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.0.callsign.is_empty() {
            f.pad(self.0.callsign.as_str())
        } else if !self.0.registration.is_empty() {
            f.pad(self.0.registration.as_str())
        } else {
            let mut buf = [0u8; 8];
            let s = hex6(self.0.address, &mut buf);
            f.pad(s)
        }
    }
}

/// The address as 6 upper-case hex digits.
pub fn hex6(address: u32, buf: &mut [u8; 8]) -> &str {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    for (i, b) in buf[..6].iter_mut().enumerate() {
        *b = DIGITS[((address >> (20 - 4 * i)) & 0xF) as usize];
    }
    core::str::from_utf8(&buf[..6]).unwrap_or("")
}
