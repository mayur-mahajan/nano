//! The title bar: the app's name and the feed's state on the left, the clock in the middle,
//! the Wi-Fi signal and the settings button on the right (like a phone's status bar).

use crate::ui::fonts::{FONT_18, FONT_20, FONT_24, FONT_28};
use twine::prelude::*;

use super::Store;
use crate::connectivity::{WIFI_RSSI, WIFI_STATE, WifiState, bars};
use crate::time::{LocalTime, UNIX_TIME};
use crate::traffic::FeedState;
use crate::ui::assets::PLANE_TOP;
use crate::ui::palette;

/// The bar's height.
pub const HEIGHT: i32 = 40;

/// The title bar.
pub fn view(cx: Scope, open_settings: impl Fn() + Copy + 'static) -> impl View {
    let store = cx.expect_context::<Store>();
    let unix = cx.watch(&UNIX_TIME);
    let local = move || {
        let offset = store.settings.with(|s| s.utc_offset_min);
        unix.get().map(|t| LocalTime::new(t, offset))
    };

    row((
        // Left: name and feed state.
        row((
            image(PLANE_TOP),
            label("nano").font(&FONT_24).text_color(palette::TEXT),
            feed_pill(store),
        ))
        .gap(10)
        .align_items(CrossAlign::Center),
        // Middle: the clock, centred between the feed state and the Wi-Fi (the sides take
        // their content's width).
        row((
            row((
                label(text!(
                    "{}",
                    Clock(local(), store.settings.with(|s| s.clock_24h))
                ))
                .font(&FONT_28)
                .text_color(palette::TEXT),
                // AM/PM small, so the 12-hour clock fits the slot as the 24-hour one does.
                label(move || match local() {
                    Some(t) if !store.settings.with(|s| s.clock_24h) => meridiem(t),
                    _ => "",
                })
                .font(&FONT_18)
                .text_color(palette::TEXT_MUTED),
            ))
            .gap(4)
            .align_items(CrossAlign::Center)
            .scrollable(false),
            label(text!("{}", Date(local())))
                .font(&FONT_28)
                .text_color(palette::TEXT_MUTED),
        ))
        .gap(12)
        .align_items(CrossAlign::Center)
        .justify(MainAlign::Center)
        .flex_grow(1),
        // Right: Wi-Fi and settings.
        row((
            wifi_bars(cx),
            button((label(Symbol::Settings).font(&FONT_24),))
                .size(36, 32)
                .radius(8)
                .bg(palette::SURFACE_HI)
                .text_color(palette::TEXT)
                .on_state(State::PRESSED, |s| s.bg(palette::DIVIDER))
                .on_click(open_settings),
        ))
        .gap(12)
        .align_items(CrossAlign::Center),
    ))
    .gap(12)
    .align_items(CrossAlign::Center)
    .padding_x(10)
    .size(800, HEIGHT)
    .bg(palette::SURFACE)
    .border_side(BorderSide::BOTTOM)
    .border(1, palette::DIVIDER)
    .scrollable(false)
}

/// `● LIVE  42` / `● NO FEED` / `● SETUP`.
fn feed_pill(store: Store) -> impl View {
    let (text, color) = (
        move || match store.feed.get() {
            FeedState::Live => "LIVE",
            FeedState::Connecting => "CONNECTING",
            FeedState::Unreachable(_) => "NO FEED",
            FeedState::Offline => "OFFLINE",
            FeedState::NotConfigured => "SETUP",
        },
        move || match store.feed.get() {
            FeedState::Live => palette::OK,
            FeedState::Connecting | FeedState::NotConfigured => palette::WARN,
            FeedState::Unreachable(_) | FeedState::Offline => palette::ALERT,
        },
    );
    row((
        led(true).color(color).size(10, 10),
        label(text).font(&FONT_18).text_color(color),
    ))
    .gap(6)
    .align_items(CrossAlign::Center)
    .padding_x(10)
    .padding_y(4)
    .radius(Radius::Circle)
    .bg(palette::SURFACE_HI)
    .scrollable(false)
}

/// Four bars, as many lit as the signal is strong; a cross when not connected.
fn wifi_bars(cx: Scope) -> impl View {
    let rssi = cx.watch(&WIFI_RSSI);
    let state = cx.watch(&WIFI_STATE);
    let lit = move || match (state.get(), rssi.get()) {
        (WifiState::Connected { .. }, Some(r)) => bars(r),
        (WifiState::Connected { .. }, None) => 4,
        _ => 0,
    };
    let bar = move |n: u8| {
        column(())
            .size(5, 4 + 4 * i32::from(n))
            .radius(1)
            .bg(move || {
                if lit() >= n {
                    palette::TEXT
                } else {
                    palette::DIVIDER
                }
            })
    };
    row((
        bar(1),
        bar(2),
        bar(3),
        bar(4),
        label(move || match state.get() {
            WifiState::Connected { .. } => "",
            WifiState::NotConfigured => "off",
            _ => "...",
        })
        .font(&FONT_20)
        .text_color(palette::TEXT_MUTED),
    ))
    .gap(2)
    .align_items(CrossAlign::End)
    .scrollable(false)
}

/// `14:05` or `2:05` (AM/PM is [`meridiem`]); `--:--` before the clock synced.
struct Clock(Option<LocalTime>, bool);

impl core::fmt::Display for Clock {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0 {
            Some(t) if self.1 => write!(f, "{:02}:{:02}", t.hour, t.minute),
            Some(t) => {
                let h = if t.hour % 12 == 0 { 12 } else { t.hour % 12 };
                write!(f, "{h}:{:02}", t.minute)
            }
            None => f.write_str("--:--"),
        }
    }
}

/// `AM` or `PM`.
fn meridiem(t: LocalTime) -> &'static str {
    if t.hour < 12 { "AM" } else { "PM" }
}

/// `Sat 24 Oct`.
struct Date(Option<LocalTime>);

impl core::fmt::Display for Date {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0 {
            Some(t) => write!(f, "{} {} {}", t.weekday_name(), t.day, t.month_name()),
            None => Ok(()),
        }
    }
}
