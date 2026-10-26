//! The settings: a modal with tabs for Wi-Fi (with a scan of nearby networks), the feeder,
//! the clock and the receiver's location, and an on-screen keyboard docked at the bottom.
//!
//! The fields edit a draft; **Save** publishes it in [`SETTINGS`]. The tasks follow from there:
//! the storage task writes it to flash, the Wi-Fi task joins a changed network and the feeder
//! task reconnects, all in the background.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::ui::fonts::{FONT_18, FONT_20, FONT_24};
use twine::prelude::*;
use twine::widgets::keyboard::KeyboardMode;
use twine::widgets::textarea::Textarea;

use super::palette;
use crate::connectivity::{NETWORKS, Network, SCAN_REQUEST, WIFI_STATE, WifiState, bars};
use crate::settings::{
    DEFAULT_PORT, HOST_MAX, PASSWORD_MAX, SETTINGS, SSID_MAX, Settings, TextWeight,
};
use crate::traffic::{FEED, FeedState, RECEIVER};

/// UTC offsets offered: −12:00 to +14:00 in half hours.
const OFFSET_STEPS: usize = 53;
/// The index of UTC+0.
const OFFSET_ZERO: usize = 24;

/// The draft being edited.
#[derive(Clone, Copy)]
struct Draft {
    ssid: Signal<String>,
    password: Signal<String>,
    host: Signal<String>,
    port: Signal<String>,
    offset: Signal<usize>,
    clock_24h: Signal<bool>,
    manual_location: Signal<bool>,
    text_weight: Signal<TextWeight>,
    lat: Signal<String>,
    lon: Signal<String>,
}

impl Draft {
    fn new(cx: Scope, s: &Settings) -> Self {
        let offset = (i32::from(s.utc_offset_min) / 30 + OFFSET_ZERO as i32)
            .clamp(0, OFFSET_STEPS as i32 - 1) as usize;
        let (lat, lon) = match s.location {
            Some((lat, lon)) => (format!("{lat:.4}"), format!("{lon:.4}")),
            None => (String::new(), String::new()),
        };
        Self {
            ssid: cx.signal(s.ssid.to_string()),
            password: cx.signal(s.password.to_string()),
            host: cx.signal(s.feeder_host.to_string()),
            port: cx.signal(s.feeder_port.to_string()),
            offset: cx.signal(offset),
            clock_24h: cx.signal(s.clock_24h),
            manual_location: cx.signal(s.location.is_some()),
            text_weight: cx.signal(s.text_weight),
            lat: cx.signal(lat),
            lon: cx.signal(lon),
        }
    }

    /// The settings the draft describes (invalid numbers fall back to the defaults).
    fn settings(self) -> Settings {
        let mut s = Settings::new();
        let fill = |dst: &mut dyn FnMut(&str), src: Signal<String>| src.with(|v| dst(v.trim()));
        fill(
            &mut |v| {
                let _ = s.ssid.push_str(&v[..v.len().min(SSID_MAX)]);
            },
            self.ssid,
        );
        fill(
            &mut |v| {
                let _ = s.password.push_str(&v[..v.len().min(PASSWORD_MAX)]);
            },
            self.password,
        );
        fill(
            &mut |v| {
                let _ = s.feeder_host.push_str(&v[..v.len().min(HOST_MAX)]);
            },
            self.host,
        );
        s.feeder_port = self.port.with(|p| p.trim().parse().unwrap_or(DEFAULT_PORT));
        s.utc_offset_min = ((self.offset.get() as i32 - OFFSET_ZERO as i32) * 30) as i16;
        s.clock_24h = self.clock_24h.get();
        s.text_weight = self.text_weight.get();
        s.location = if self.manual_location.get() {
            let lat = self.lat.with(|v| v.trim().parse::<f32>().ok());
            let lon = self.lon.with(|v| v.trim().parse::<f32>().ok());
            match (lat, lon) {
                (Some(lat), Some(lon)) if lat.abs() <= 90.0 && lon.abs() <= 180.0 => {
                    Some((lat, lon))
                }
                _ => None,
            }
        } else {
            None
        };
        s
    }
}

/// The field the keyboard types into, and the key map it shows.
#[derive(Clone, Copy)]
struct Focus {
    field: Signal<Option<NodeRef<Textarea>>>,
    mode: Signal<KeyboardMode>,
}

/// Shows the settings modal over the current screen.
pub fn open(cx: Scope) {
    let _ = cx.show_modal(move |cx, modal| {
        let draft = Draft::new(cx, &SETTINGS.get());
        let focus = Focus {
            field: cx.signal(None),
            mode: cx.signal(KeyboardMode::TextLower),
        };
        let tab_index = cx.signal(0usize);
        // The text weight previews live; closing without saving restores the saved one.
        cx.effect(move || super::apply_text_weight(cx, draft.text_weight.get()));
        cx.on_cleanup(move || super::apply_text_weight(cx, SETTINGS.get().text_weight));
        // Look for networks right away.
        SCAN_REQUEST.set(SCAN_REQUEST.get().wrapping_add(1));
        column((
            header(modal, draft),
            tabview(
                tab_index,
                (
                    tab("Wi-Fi", (wifi_tab(cx, draft, focus),)),
                    tab("Feeder", (feeder_tab(cx, draft, focus),)),
                    tab("Clock", (clock_tab(draft),)),
                    tab("Location", (location_tab(cx, draft, focus),)),
                    tab("Display", (display_tab(draft),)),
                ),
            )
            .bar_size(44)
            .on_change(move |_| focus.field.set(None))
            .fill_width()
            .flex_grow(1),
            when(
                move || focus.field.get().is_some(),
                move |_| {
                    keyboard(move || focus.field.get())
                        .mode(move || focus.mode.get())
                        .docked(true)
                        .popovers(true)
                        .height(210)
                        .font(&FONT_24)
                        .on_ready(move || focus.field.set(None))
                        .on_cancel(move || focus.field.set(None))
                },
            ),
        ))
        .gap(0)
        .size(800, 480)
        .bg(palette::SURFACE)
        .text_color(palette::TEXT)
        .font(&FONT_20)
        .scrollable(false)
    });
}

/// Title, Cancel and Save.
fn header(modal: ModalHandle, draft: Draft) -> impl View {
    row((
        label(Symbol::Settings)
            .font(&FONT_24)
            .text_color(palette::ACCENT),
        label("Settings").font(&FONT_24),
        spacer(),
        button((label("Cancel"),))
            .bg(palette::SURFACE_HI)
            .padding_x(18)
            .padding_y(8)
            .on_click(move || modal.close()),
        button((label(text!("{} Save", Symbol::Ok)),))
            .bg(palette::ACCENT)
            .text_color(palette::SKY)
            .padding_x(18)
            .padding_y(8)
            .on_click(move || {
                let new = draft.settings();
                if new != SETTINGS.get() {
                    SETTINGS.set(new);
                }
                modal.close();
            }),
    ))
    .gap(10)
    .align_items(CrossAlign::Center)
    .padding_x(14)
    .padding_y(8)
    .fill_width()
    .border_side(BorderSide::BOTTOM)
    .border(1, palette::DIVIDER)
    .scrollable(false)
}

/// A captioned one-line text field that brings up the keyboard in `mode`.
fn field(
    cx: Scope,
    focus: Focus,
    caption: &'static str,
    text: Signal<String>,
    mode: KeyboardMode,
    configure: impl FnOnce(WidgetView<Textarea>) -> WidgetView<Textarea>,
) -> Flex {
    let r = cx.node_ref::<Textarea>();
    let edit = move || {
        focus.mode.set(mode);
        focus.field.set(Some(r));
    };
    column((
        label(caption)
            .font(&FONT_18)
            .text_color(palette::TEXT_MUTED),
        configure(
            textarea(text)
                .one_line(true)
                .node_ref(r)
                .font(&FONT_24)
                .fill_width()
                .on_click(edit)
                .on_ready(move || focus.field.set(None)),
        ),
    ))
    .gap(4)
    .fill_width()
    .scrollable(false)
}

fn wifi_tab(cx: Scope, draft: Draft, focus: Focus) -> impl View {
    let networks = cx.watch(&NETWORKS);
    let state = cx.watch(&WIFI_STATE);
    let show_password = cx.signal(false);
    let list = cx.memo(move || networks.with(|n| n.as_deref().cloned().unwrap_or_default()));
    row((
        column((
            field(
                cx,
                focus,
                "NETWORK NAME (SSID)",
                draft.ssid,
                KeyboardMode::TextLower,
                |t| t.placeholder("e.g. home-wifi").max_length(SSID_MAX as i32),
            ),
            field(
                cx,
                focus,
                "PASSWORD",
                draft.password,
                KeyboardMode::TextLower,
                |t| {
                    t.placeholder("empty for an open network")
                        .password(move || !show_password.get())
                        .max_length(PASSWORD_MAX as i32)
                },
            ),
            row((
                switch(show_password),
                label("Show password").text_color(palette::TEXT_MUTED),
            ))
            .gap(8)
            .align_items(CrossAlign::Center),
            label(move || match state.get() {
                WifiState::NotConfigured => "Not connected: no network saved".to_string(),
                WifiState::Connected { ip: [a, b, c, d] } => {
                    format!("Connected, IP {a}.{b}.{c}.{d}")
                }
                WifiState::ObtainingIp => "Getting an IP address...".to_string(),
                _ => "Connecting...".to_string(),
            })
            .font(&FONT_18)
            .text_color(palette::ACCENT),
        ))
        .gap(12)
        .width(Length::pct(55))
        .scrollable(false),
        column((
            row((
                label("NEARBY")
                    .font(&FONT_18)
                    .text_color(palette::TEXT_MUTED),
                spacer(),
                button((label(text!("{} Scan", Symbol::Refresh)).font(&FONT_18),))
                    .bg(palette::SURFACE_HI)
                    .padding_x(10)
                    .padding_y(4)
                    .on_click(|| SCAN_REQUEST.set(SCAN_REQUEST.get().wrapping_add(1))),
            ))
            .align_items(CrossAlign::Center)
            .fill_width(),
            scroll_view(
                Axis::Vertical,
                (for_each(
                    list,
                    |n: &Network| n.ssid.clone(),
                    move |_, n| network_row(draft, focus, n),
                )
                .max_items(24),),
            )
            .gap(4)
            .fill_width()
            .flex_grow(1),
        ))
        .gap(8)
        .flex_grow(1)
        .height(250)
        .scrollable(false),
    ))
    .gap(16)
    .fill_width()
    .scrollable(false)
}

fn network_row(draft: Draft, focus: Focus, n: ReadSignal<Network>) -> impl View {
    row((
        label(text!("{}", n.with(|n| n.ssid.clone())))
            .font(&FONT_20)
            .flex_grow(1),
        label(move || {
            if n.with(|n| n.secured) {
                "secured"
            } else {
                "open"
            }
        })
        .font(&FONT_18)
        .text_color(palette::TEXT_MUTED),
        label(text!("{}/4", bars(n.with(|n| n.rssi))))
            .font(&FONT_18)
            .text_color(palette::ACCENT),
    ))
    .gap(8)
    .align_items(CrossAlign::Center)
    .padding_x(10)
    .padding_y(8)
    .fill_width()
    .bg(palette::SURFACE_HI)
    .radius(8)
    .clickable(true)
    .scrollable(false)
    .on_state(State::PRESSED, |s| s.bg(palette::DIVIDER))
    .on_click(move || {
        draft.ssid.set(n.with_untracked(|n| n.ssid.to_string()));
        draft.password.set(String::new());
        let _ = focus;
    })
}

fn feeder_tab(cx: Scope, draft: Draft, focus: Focus) -> impl View {
    let feed = cx.watch(&FEED);
    column((
        row((
            field(
                cx,
                focus,
                "FEEDER ADDRESS (IP OR HOST NAME)",
                draft.host,
                KeyboardMode::TextLower,
                |t| {
                    t.placeholder("e.g. 192.168.1.42")
                        .max_length(HOST_MAX as i32)
                },
            )
            .flex_grow(1),
            field(cx, focus, "PORT", draft.port, KeyboardMode::Number, |t| {
                t.accepted_chars("0123456789").max_length(5)
            })
            .width(140),
        ))
        .gap(12)
        .fill_width(),
        label(
            "nano reads tar1090's data at http://<address>:<port>/tar1090/data/ (or \
               dump1090's /data/). tar1090 behind lighttpd usually uses port 8080 or 80.",
        )
        .font(&FONT_18)
        .text_color(palette::TEXT_MUTED)
        .fill_width(),
        label(move || match feed.get() {
            FeedState::NotConfigured => "Not set up".to_string(),
            FeedState::Offline => "Waiting for Wi-Fi".to_string(),
            FeedState::Connecting => "Connecting...".to_string(),
            FeedState::Live => "Live: receiving aircraft".to_string(),
            FeedState::Unreachable(n) => format!("Unreachable ({n} failed polls)"),
        })
        .font(&FONT_20)
        .text_color(move || match feed.get() {
            FeedState::Live => palette::OK,
            FeedState::Unreachable(_) => palette::ALERT,
            _ => palette::WARN,
        }),
    ))
    .gap(14)
    .fill_width()
    .scrollable(false)
}

fn clock_tab(draft: Draft) -> impl View {
    let offsets: Vec<String> = (0..OFFSET_STEPS)
        .map(|i| {
            let m = (i as i32 - OFFSET_ZERO as i32) * 30;
            let sign = if m < 0 { '-' } else { '+' };
            format!("UTC {sign}{}:{:02}", m.abs() / 60, m.abs() % 60)
        })
        .collect();
    column((
        label("TIME ZONE")
            .font(&FONT_18)
            .text_color(palette::TEXT_MUTED),
        dropdown(offsets, draft.offset).width(220),
        row((switch(draft.clock_24h), label("24-hour clock")))
            .gap(10)
            .align_items(CrossAlign::Center),
        label("The time comes from pool.ntp.org once Wi-Fi is up.")
            .font(&FONT_18)
            .text_color(palette::TEXT_MUTED),
    ))
    .gap(14)
    .fill_width()
    .scrollable(false)
}

/// Text weight: three choices and a sample of the UI's text, previewed live on the panel.
fn display_tab(draft: Draft) -> impl View {
    let choice = move |weight: TextWeight, name: &'static str| {
        button((label(name).font(&FONT_20),))
            .padding_x(20)
            .padding_y(10)
            .bg(palette::SURFACE_HI)
            .state(State::CHECKED, move || draft.text_weight.get() == weight)
            .on_state(State::CHECKED, |s| {
                s.bg(palette::ACCENT).text_color(palette::SKY)
            })
            .on_click(move || draft.text_weight.set(weight))
    };
    let sample = column((
        label("UAL889D  FL273  412 kt")
            .font(&FONT_24)
            .text_color(palette::TEXT),
        label("B738  •  10 nm  SSE  •  il1,.:;")
            .font(&FONT_20)
            .text_color(palette::TEXT_MUTED),
        label("Tracked Flights  0123456789")
            .font(&FONT_18)
            .text_color(palette::ACCENT),
    ))
    .gap(6)
    .padding(12)
    .bg(palette::SURFACE_HI)
    .radius(8)
    .fill_width()
    .scrollable(false);
    column((
        label("TEXT WEIGHT")
            .font(&FONT_18)
            .text_color(palette::TEXT_MUTED),
        row((
            choice(TextWeight::Light, "Light"),
            choice(TextWeight::Medium, "Medium"),
            choice(TextWeight::Strong, "Strong"),
        ))
        .gap(10),
        sample,
        label("Light is LVGL's rendering; Medium and Strong lift light text on dark. Pick what reads best on this panel.")
            .font(&FONT_18)
            .text_color(palette::TEXT_MUTED)
            .fill_width(),
    ))
    .gap(14)
    .fill_width()
    .scrollable(false)
}

fn location_tab(cx: Scope, draft: Draft, focus: Focus) -> impl View {
    let receiver = cx.watch(&RECEIVER);
    column((
        row((
            switch(draft.manual_location),
            label("Set the receiver's location by hand"),
        ))
        .gap(10)
        .align_items(CrossAlign::Center),
        label(move || match receiver.get() {
            Some((lat, lon)) => format!("The feeder reports {lat:.4}, {lon:.4}"),
            None => "The feeder does not report its location (receiver.json)".to_string(),
        })
        .font(&FONT_18)
        .text_color(palette::TEXT_MUTED),
        row((
            field(
                cx,
                focus,
                "LATITUDE",
                draft.lat,
                KeyboardMode::Number,
                |t| {
                    t.accepted_chars("-0123456789.")
                        .max_length(10)
                        .placeholder("37.6213")
                },
            )
            .flex_grow(1),
            field(
                cx,
                focus,
                "LONGITUDE",
                draft.lon,
                KeyboardMode::Number,
                |t| {
                    t.accepted_chars("-0123456789.")
                        .max_length(11)
                        .placeholder("-122.3790")
                },
            )
            .flex_grow(1),
        ))
        .gap(12)
        .fill_width()
        .hidden(move || !draft.manual_location.get()),
    ))
    .gap(14)
    .fill_width()
    .scrollable(false)
}
