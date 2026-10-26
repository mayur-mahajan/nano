//! Boot screen: an airplane flies along a progress track while Wi-Fi connects.
//!
//! Progress jumps to each connection stage's mark ([`WifiState::progress`]) and creeps towards
//! the next one while waiting, so the airplane never stands still. Once online it lands at
//! 100 %, and `on_done` is called after a short pause.

use core::fmt;

use crate::ui::fonts::{FONT_24, FONT_28, FONT_48};
use twine::prelude::*;

use super::assets::{AIRPLANE, TWINE_LOGO};
use super::palette;
use crate::connectivity::{WIFI_STATE, WifiState};
use crate::settings::SETTINGS;

/// Width of the flight track (and the progress bar under it).
const TRACK_WIDTH: i32 = 640;
/// Width of the airplane image.
const PLANE_WIDTH: i32 = 64;
/// How often the displayed progress steps towards its target.
const TICK: Duration = Duration::ms(40);
/// Ticks between two creeping steps while waiting within a stage (~1 % every 0.5 s).
const CREEP_TICKS: u8 = 12;
/// Pause on 100 % before the tracker replaces this one.
const LANDING_PAUSE: Duration = Duration::ms(700);
/// The longest the boot screen waits for Wi-Fi; the tracker then shows the connection state.
pub const MAX_WAIT: Duration = Duration::ms(30_000);

/// The boot screen; calls `on_done` once connected and landed.
pub fn screen(cx: Scope, on_done: impl FnOnce() + 'static) -> impl View {
    let state = cx.watch(&WIFI_STATE);
    let shown = cx.signal(0i32);
    let on_done = alloc::rc::Rc::new(core::cell::Cell::new(Some(on_done)));
    let give_up = on_done.clone();
    cx.timeout(MAX_WAIT, move || {
        if let Some(done) = give_up.take() {
            done();
        }
    });
    advance_progress(cx, state, shown, move || {
        if let Some(done) = on_done.take() {
            done();
        }
    });

    // A gentle up-and-down bob while flying; essential so it also runs under reduced motion
    // (it is the busy indicator).
    let bob = AnimSpec::new(Duration::ms(900))
        .ease_in_out()
        .playback(Duration::ms(900))
        .forever()
        .essential();
    let (lift, _) = cx.animation(0i32, -5, bob);
    let plane_pos = move || {
        let x = shown.get() * (TRACK_WIDTH - PLANE_WIDTH) / 100;
        let y = if shown.get() >= 100 { 0 } else { lift.get() };
        Point::new(x, y)
    };

    let track = column((
        image(AIRPLANE).offset(plane_pos),
        bar(shown)
            .range(0..=100)
            .size(TRACK_WIDTH, 6)
            .bg(palette::TRACK)
            .part(Part::Indicator, |s| s.bg(palette::ACCENT)),
    ))
    .gap(10)
    .width(TRACK_WIDTH)
    .align_items(CrossAlign::Start);

    let status = row((
        spinner().size(28, 28),
        label(text!("{}", StatusText(state.get())))
            .font(&FONT_28)
            .text_color(palette::TEXT),
        label(text!("{}%", shown.get()))
            .font(&FONT_28)
            .text_color(palette::ACCENT),
    ))
    .gap(14)
    .align_items(CrossAlign::Center);

    let title = column((
        label("nano").font(&FONT_48).text_color(palette::TEXT),
        label("Preparing for takeoff")
            .font(&FONT_24)
            .text_color(palette::TEXT_MUTED),
    ))
    .gap(6)
    .align_items(CrossAlign::Center);

    let built_with = row((
        image(&TWINE_LOGO)
            .scale(Scale::pct(25))
            .pivot(Point::new(0, 0))
            .size(32, 32),
        label("Built with Twine")
            .font(&crate::ui::fonts::FONT_18)
            .text_color(palette::TEXT_MUTED),
    ))
    .gap(8)
    .align_items(CrossAlign::Center)
    .align(Align::BottomMid)
    .y(-16)
    .floating(true);

    column((title, track, status, built_with))
        .gap(40)
        .align_items(CrossAlign::Center)
        .justify(MainAlign::Center)
        .bg(palette::SKY)
        .scrollable(false)
        .fill()
}

/// Steps `shown` towards the stage's mark (fast, easing out), then creeps it towards the
/// stage's ceiling; on 100 % schedules `on_done`.
fn advance_progress(
    cx: Scope,
    state: ReadSignal<WifiState>,
    shown: Signal<i32>,
    on_done: impl FnOnce() + 'static,
) {
    let mut creep = 0u8;
    let mut on_done = Some(on_done);
    cx.interval(TICK, move || {
        let s = state.get_untracked();
        let current = shown.get_untracked();
        let next = if current < s.progress() {
            current + ((s.progress() - current) / 5).max(1)
        } else if current < s.progress_ceiling() {
            creep += 1;
            if creep >= CREEP_TICKS {
                creep = 0;
                current + 1
            } else {
                current
            }
        } else {
            current
        };
        if next != current {
            shown.set(next);
        }
        if next >= 100
            && let Some(done) = on_done.take()
        {
            cx.timeout(LANDING_PAUSE, done);
        }
    });
}

/// The status line for a state.
struct StatusText(WifiState);

impl fmt::Display for StatusText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ssid = SETTINGS.get().ssid;
        match self.0 {
            WifiState::NotConfigured => f.write_str("No Wi-Fi network saved"),
            WifiState::Starting => f.write_str("Starting Wi-Fi radio..."),
            WifiState::Connecting { attempt: 0 | 1 } => write!(f, "Connecting to {ssid}..."),
            WifiState::Connecting { attempt } => {
                write!(f, "Connecting to {ssid} (attempt {attempt})...")
            }
            WifiState::ObtainingIp => f.write_str("Getting an IP address..."),
            WifiState::Connected { ip: [a, b, c, d] } => write!(f, "Connected  {a}.{b}.{c}.{d}"),
        }
    }
}
