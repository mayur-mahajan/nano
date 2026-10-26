//! The user's settings: Wi-Fi, the ADS-B feeder, the clock and the location. Edited on the
//! settings screen and saved to flash ([`crate::storage`]); platform-neutral, so the UI and
//! its host tests use it too.
//!
//! [`SETTINGS`] holds the current values: the UI publishes a change there, and the network and
//! storage tasks pick it up.

use core::fmt::Write as _;

use heapless::String;
use twine::reactive::Latest;

/// Longest SSID (802.11).
pub const SSID_MAX: usize = 32;
/// Longest WPA2 passphrase.
pub const PASSWORD_MAX: usize = 64;
/// Longest feeder host name or address.
pub const HOST_MAX: usize = 63;

/// The feeder's default HTTP port (tar1090 behind lighttpd).
pub const DEFAULT_PORT: u16 = 8080;

/// How heavy text is drawn: the display's text gamma (light text on dark is lifted more with
/// each step). Chosen on the real panel, which the simulator cannot judge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextWeight {
    /// Linear coverage, as LVGL draws: thinnest.
    Light,
    /// A mild lift (γ 1.1 dark-on-light, 1.4 light-on-dark).
    #[default]
    Medium,
    /// twine's default lift (γ 1.2 / 1.8): heaviest.
    Strong,
}

impl TextWeight {
    /// Every weight, in the order Settings lists them.
    pub const ALL: [Self; 3] = [Self::Light, Self::Medium, Self::Strong];

    const fn to_u8(self) -> u8 {
        self as u8
    }

    const fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Light,
            2 => Self::Strong,
            _ => Self::Medium,
        }
    }
}

/// Everything the user can configure.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Wi-Fi network name; empty: not configured (nano does not connect).
    pub ssid: String<SSID_MAX>,
    /// Wi-Fi passphrase (empty for an open network).
    pub password: String<PASSWORD_MAX>,
    /// The feeder's host name or IPv4 address; empty: not configured.
    pub feeder_host: String<HOST_MAX>,
    /// The feeder's HTTP port (tar1090 is at `/tar1090/` there).
    pub feeder_port: u16,
    /// Local time's offset from UTC, in minutes.
    pub utc_offset_min: i16,
    /// 24-hour clock (else 12-hour with AM/PM).
    pub clock_24h: bool,
    /// The receiver's location `(lat, lon)`, when set by hand; else the feeder's
    /// `receiver.json` is used.
    pub location: Option<(f32, f32)>,
    /// How heavy text is drawn.
    pub text_weight: TextWeight,
}

impl Settings {
    /// Nothing configured yet.
    pub const fn new() -> Self {
        Self {
            ssid: String::new(),
            password: String::new(),
            feeder_host: String::new(),
            feeder_port: DEFAULT_PORT,
            utc_offset_min: 0,
            clock_24h: true,
            location: None,
            text_weight: TextWeight::Medium,
        }
    }

    /// Wi-Fi is configured (nano connects).
    pub fn has_wifi(&self) -> bool {
        !self.ssid.is_empty()
    }

    /// The feeder is configured.
    pub fn has_feeder(&self) -> bool {
        !self.feeder_host.is_empty()
    }

    /// The UTC offset as `UTC+5:30` / `UTC−8`.
    pub fn utc_offset_label(&self) -> String<12> {
        let mut s = String::new();
        let m = self.utc_offset_min;
        let sign = if m < 0 { '-' } else { '+' };
        let (h, min) = (m.unsigned_abs() / 60, m.unsigned_abs() % 60);
        let _ = if min == 0 {
            write!(s, "UTC{sign}{h}")
        } else {
            write!(s, "UTC{sign}{h}:{min:02}")
        };
        s
    }

    /// Encodes the settings for storage (at most [`ENCODED_MAX`] bytes).
    pub fn encode(&self, out: &mut [u8; ENCODED_MAX]) -> usize {
        let mut w = Writer { out, pos: 0 };
        w.bytes(&[FORMAT]);
        w.str(&self.ssid);
        w.str(&self.password);
        w.str(&self.feeder_host);
        w.bytes(&self.feeder_port.to_le_bytes());
        w.bytes(&self.utc_offset_min.to_le_bytes());
        w.bytes(&[u8::from(self.clock_24h)]);
        match self.location {
            Some((lat, lon)) => {
                w.bytes(&[1]);
                w.bytes(&lat.to_le_bytes());
                w.bytes(&lon.to_le_bytes());
            }
            None => w.bytes(&[0]),
        }
        w.bytes(&[self.text_weight.to_u8()]);
        w.pos
    }

    /// Decodes stored settings; `None` when malformed or of another format.
    pub fn decode(b: &[u8]) -> Option<Self> {
        let mut r = Reader { b, pos: 0 };
        // Format 1 had no text weight.
        let format = r.take(1)?[0];
        if !(1..=FORMAT).contains(&format) {
            return None;
        }
        let ssid = r.str()?;
        let password = r.str()?;
        let feeder_host = r.str()?;
        let feeder_port = u16::from_le_bytes(r.take(2)?.try_into().ok()?);
        let utc_offset_min = i16::from_le_bytes(r.take(2)?.try_into().ok()?);
        let clock_24h = r.take(1)?[0] != 0;
        let location = match r.take(1)?[0] {
            0 => None,
            _ => {
                let lat = f32::from_le_bytes(r.take(4)?.try_into().ok()?);
                let lon = f32::from_le_bytes(r.take(4)?.try_into().ok()?);
                Some((lat, lon))
            }
        };
        let text_weight = if format >= 2 {
            TextWeight::from_u8(r.take(1)?[0])
        } else {
            TextWeight::Medium
        };
        Some(Self {
            text_weight,
            ssid,
            password,
            feeder_host,
            feeder_port,
            utc_offset_min,
            clock_24h,
            location,
        })
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self::new()
    }
}

/// The current settings: loaded from flash before the UI starts, then set by the settings
/// screen. The network task follows them (it polls [`Latest::version`]).
pub static SETTINGS: Latest<Settings> = Latest::new(Settings::new());

/// The encoding's format byte (bump when the layout changes).
const FORMAT: u8 = 2;
/// The longest encoding.
pub const ENCODED_MAX: usize = 1 + 3 + SSID_MAX + PASSWORD_MAX + HOST_MAX + 2 + 2 + 1 + 9 + 1;

struct Writer<'a> {
    out: &'a mut [u8; ENCODED_MAX],
    pos: usize,
}

impl Writer<'_> {
    fn bytes(&mut self, b: &[u8]) {
        self.out[self.pos..self.pos + b.len()].copy_from_slice(b);
        self.pos += b.len();
    }

    fn str(&mut self, s: &str) {
        self.bytes(&[s.len() as u8]);
        self.bytes(s.as_bytes());
    }
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let v = self.b.get(self.pos..self.pos + n)?;
        self.pos += n;
        Some(v)
    }

    fn str<const N: usize>(&mut self) -> Option<String<N>> {
        let len = usize::from(self.take(1)?[0]);
        let mut s = String::new();
        s.push_str(core::str::from_utf8(self.take(len)?).ok()?)
            .ok()?;
        Some(s)
    }
}
