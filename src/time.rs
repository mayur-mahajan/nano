//! Wall-clock time as the UI sees it (platform-neutral): the clock task publishes
//! [`UNIX_TIME`]; [`LocalTime`] splits it for display.

use twine::reactive::Latest;

/// Seconds since 1970 (UTC); `None` until the clock has synced.
pub static UNIX_TIME: Latest<Option<u64>> = Latest::new(None);

/// A local time of day.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTime {
    /// 0–23.
    pub hour: u8,
    /// 0–59.
    pub minute: u8,
    /// 0–59.
    pub second: u8,
    /// Day of the week, 0 = Monday.
    pub weekday: u8,
    /// Day of the month, 1–31.
    pub day: u8,
    /// Month, 1–12.
    pub month: u8,
}

impl LocalTime {
    /// `unix` seconds shifted by `offset_min` minutes.
    pub fn new(unix: u64, offset_min: i16) -> Self {
        let t = unix as i64 + i64::from(offset_min) * 60;
        let days = t.div_euclid(86_400);
        let secs = t.rem_euclid(86_400);
        let (_, month, day) = civil_from_days(days);
        Self {
            hour: (secs / 3600) as u8,
            minute: (secs / 60 % 60) as u8,
            second: (secs % 60) as u8,
            // 1970-01-01 was a Thursday (3 with Monday = 0).
            weekday: ((days + 3).rem_euclid(7)) as u8,
            day,
            month,
        }
    }

    /// The weekday's three-letter name.
    pub const fn weekday_name(self) -> &'static str {
        const NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
        NAMES[self.weekday as usize % 7]
    }

    /// The month's three-letter name.
    pub const fn month_name(self) -> &'static str {
        const NAMES: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        NAMES[(self.month as usize + 11) % 12]
    }
}

/// Howard Hinnant's `civil_from_days`: `(year, month, day)` of days since 1970-01-01.
fn civil_from_days(z: i64) -> (i64, u8, u8) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}
