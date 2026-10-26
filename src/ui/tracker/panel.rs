//! The side panel: the traffic list (nearest first) when nothing is selected, the selected
//! aircraft's details otherwise. Tapping a row selects; the close button deselects.

use alloc::vec::Vec;
use core::fmt::{self, Write as _};

use crate::ui::fonts::{FONT_18, FONT_20, FONT_24, FONT_28, FONT_48};
use twine::core::{Fx, Transform};
use twine::prelude::*;
use twine::render::FillRule;
use twine::vector::{FxPoint, Paint, VectorDsc};

use super::Store;
use crate::traffic::geo::{Local, compass_point, polar};
use crate::traffic::{Aircraft, Altitude};
use crate::ui::palette;

/// The panel's width.
pub const WIDTH: i32 = 280;
/// Rows the list shows at most.
const MAX_ROWS: usize = 40;

/// One row of the traffic list.
#[derive(Clone, Debug, PartialEq)]
struct Row {
    address: u32,
    name: crate::traffic::Ident,
    type_code: crate::traffic::Ident,
    altitude: heapless::String<12>,
    distance_nm: f32,
    bearing: f32,
    color: Color,
    /// The altitude colour for text.
    text: Color,
    emergency: bool,
}

/// The panel.
pub fn view(cx: Scope) -> impl View {
    let store = cx.expect_context::<Store>();
    let selected = move || store.selected.get().is_some();
    column((
        when(selected, move |cx| details(cx, store)).otherwise(move |cx| list(cx, store)),
        legend(),
    ))
    .gap(10)
    .padding_x(12)
    .padding_y(10)
    .size(WIDTH, super::scope::SIZE.h)
    .bg(palette::SURFACE)
    .border_side(BorderSide::LEFT)
    .border(1, palette::DIVIDER)
    .scrollable(false)
}

/// The traffic list with the totals above it.
fn list(cx: Scope, store: Store) -> impl View {
    let rows = cx.memo(move || {
        let Some(center) = store.center.get() else {
            return Vec::new();
        };
        let local = Local::new(center);
        store.traffic.with(|t| {
            let Some(t) = t else {
                return Vec::new();
            };
            let mut rows: Vec<Row> = t
                .positioned()
                .filter_map(|tr| {
                    let a = &tr.aircraft;
                    let (distance_nm, bearing) = polar(local.offset_nm(a.position?));
                    Some(Row {
                        address: a.address,
                        name: name_of(a),
                        type_code: a.type_code,
                        altitude: compact_altitude(a.altitude),
                        distance_nm,
                        bearing,
                        color: palette::altitude(a.altitude),
                        text: palette::altitude_text(a.altitude),
                        emergency: a.emergency || a.emergency_squawk(),
                    })
                })
                .collect();
            rows.sort_by(|a, b| a.distance_nm.total_cmp(&b.distance_nm));
            rows.truncate(MAX_ROWS);
            rows
        })
    });
    let counts = cx.memo(move || {
        store.traffic.with(|t| match t {
            Some(t) => (
                t.tracks.len(),
                t.positioned().count(),
                t.message_rate as u32,
            ),
            None => (0, 0, 0),
        })
    });
    column((
        // The summary: one lead figure, the secondary ones small at the right.
        row((
            column((
                label(text!("{}", counts.get().0))
                    .font(&FONT_48)
                    .text_color(palette::TEXT),
                label("AIRCRAFT")
                    .font(&FONT_18)
                    .text_color(palette::TEXT_MUTED),
            ))
            .gap(0)
            .flex_grow(1)
            .scrollable(false),
            column((
                secondary(move || counts.get().1, "on scope"),
                secondary(move || counts.get().2 as usize, "msg/s"),
            ))
            .gap(4)
            .align_items(CrossAlign::End)
            .scrollable(false),
        ))
        .align_items(CrossAlign::End)
        .padding_y(2)
        .padding_bottom(8)
        .fill_width()
        .border_side(BorderSide::BOTTOM)
        .border(1, palette::DIVIDER)
        .scrollable(false),
        scroll_view(
            Axis::Vertical,
            (
                for_each(rows, |r: &Row| r.address, move |_, r| list_row(store, r))
                    .max_items(MAX_ROWS as u16),
            ),
        )
        .gap(0)
        .fill_width()
        .flex_grow(1)
        .scrollbar(ScrollbarMode::Off),
    ))
    .gap(10)
    .fill_width()
    .flex_grow(1)
    .scrollable(false)
}

/// A secondary figure: the value, then its unit or caption, muted.
fn secondary(value: impl Fn() -> usize + 'static, caption: &'static str) -> impl View {
    row((
        label(text!("{}", value()))
            .font(&FONT_20)
            .text_color(palette::TEXT),
        label(caption)
            .font(&FONT_18)
            .text_color(palette::TEXT_MUTED),
    ))
    .gap(6)
    .align_items(CrossAlign::End)
    .scrollable(false)
}

/// A traffic row, one line of four columns: the callsign (red in an emergency), the altitude
/// in its colour, the distance, and an arrow pointing towards the aircraft. Hairlines between
/// rows, a highlight only while pressed.
fn list_row(store: Store, r: ReadSignal<Row>) -> impl View {
    // Only the row takes taps: its parts are not clickable, so a tap anywhere on it selects.
    row((
        // The callsign takes the room left by the fixed columns. One line high, so a long one
        // ends in "..." instead of wrapping.
        label(text!("{}", r.with(|r| r.name)))
            .font(&FONT_20)
            .text_color(move || r.with(|r| if r.emergency { palette::ALERT } else { palette::TEXT }))
            .long_mode(twine::text::LongMode::Dots)
            .height(FONT_20_LINE)
            .flex_grow(1)
            .clickable(false),
        cell(
            label(text!("{}", r.with(|r| r.altitude.clone())))
                .font(&FONT_18)
                .text_color(move || r.with(|r| r.text))
                .clickable(false),
            58,
        ),
        cell(
            label(text!("{}", Nm(r.with(|r| r.distance_nm))))
                .font(&FONT_18)
                .text_color(palette::TEXT_MUTED)
                .clickable(false),
            64,
        ),
        bearing_arrow(move || r.with(|r| r.bearing)),
    ))
    .gap(6)
    .align_items(CrossAlign::Center)
    .padding_y(10)
    .padding_x(2)
    .fill_width()
    .border_side(BorderSide::BOTTOM)
    .border(1, palette::DIVIDER)
    .clickable(true)
    .scrollable(false)
    .on_state(State::PRESSED, |s| s.bg(palette::SURFACE_HI))
    .on_click(move || store.selected.set(Some(r.with_untracked(|r| r.address))))
}

/// `FONT_20`'s line height.
const FONT_20_LINE: i32 = 26;

/// `content` right-aligned in a fixed-width column (the list's figures line up).
fn cell(content: impl View, width: i32) -> impl View {
    column((content,))
        .width(width)
        .align_items(CrossAlign::End)
        .clickable(false)
        .scrollable(false)
}

/// The list's distance: `12 nm`; from 100 nm on without the space, to fit the column.
struct Nm(f32);

impl fmt::Display for Nm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 < 99.5 {
            write!(f, "{:.0} nm", self.0)
        } else {
            write!(f, "{:.0}nm", self.0)
        }
    }
}

/// The arrow's size (square).
const ARROW: i32 = 16;

/// The navigation arrow (tenths of a pixel, pointing up, around its middle): a tip, two
/// swept-back wings and a notch between them.
const NAV_ARROW: [(i32, i32); 4] = [(0, -75), (60, 65), (0, 35), (-60, 65)];

/// A filled arrow pointing from the receiver towards the aircraft: up is north, turned by
/// the bearing (degrees, clockwise).
fn bearing_arrow(bearing: impl Fn() -> f32 + 'static) -> impl View {
    vector_canvas(move |scene| {
        let Some((path, dsc)) = scene.path_mut() else {
            return;
        };
        let [first, rest @ ..] = NAV_ARROW;
        path.move_to(tenths(first));
        for p in rest {
            path.line_to(tenths(p));
        }
        path.close();
        let half = Fx::from_ratio(ARROW, 2);
        *dsc = VectorDsc {
            fill: Some((Paint::Solid(palette::ACCENT), FillRule::NonZero)),
            transform: Transform::rotate(Angle::deci_deg((bearing() * 10.0) as i32))
                .then(Transform::translate(half, half)),
            ..VectorDsc::default()
        };
    })
    .size(ARROW, ARROW)
    .clickable(false)
}

fn tenths((x, y): (i32, i32)) -> FxPoint {
    FxPoint::new(Fx::from_ratio(x, 10), Fx::from_ratio(y, 10))
}

/// The selected aircraft.
fn details(cx: Scope, store: Store) -> impl View {
    let track = store.selected_track;
    let a = cx.memo(move || track.with(|t| t.as_ref().map(|t| t.aircraft)));
    let geo = cx.memo(move || {
        let (Some(center), Some(pos)) = (store.center.get(), a.get().and_then(|a| a.position))
        else {
            return None;
        };
        Some(polar(Local::new(center).offset_nm(pos)))
    });
    let get = move || a.get();
    let color = move || palette::altitude_text(get().and_then(|a| a.altitude));
    column((
        // Header (stays put): callsign over the description, a plain close icon.
        row((
            column((
                label(text!("{}", Show(get(), |a, f| write!(f, "{}", a.name()))))
                    .font(&FONT_28)
                    .text_color(palette::TEXT),
                label(text!("{}", Show(get(), describe)))
                    .font(&FONT_18)
                    .text_color(palette::TEXT_MUTED),
            ))
            .gap(2)
            .flex_grow(1),
            button((label(Symbol::Close).font(&FONT_24),))
                .size(40, 40)
                .radius(4)
                .bg(palette::SURFACE)
                .text_color(palette::TEXT_MUTED)
                .on_state(State::PRESSED, |s| s.bg(palette::SURFACE_HI))
                .on_click(move || store.selected.set(None)),
        ))
        .fill_width()
        .align_items(CrossAlign::Center)
        .padding_y(4)
        .border_side(BorderSide::BOTTOM)
        .border(1, palette::DIVIDER),
        // The readout scrolls under the header: nothing is cut off on small panels.
        scroll_view(
            Axis::Vertical,
            (
                // The altitude, big, with the climb/descent under it.
                column((
                    label(text!(
                        "{}",
                        Show(get(), |a, f| f.write_str(&altitude_text(a.altitude)))
                    ))
                    .font(&FONT_48)
                    .text_color(color),
                    label(text!("{}", Show(get(), climb)))
                        .font(&FONT_24)
                        .text_color(palette::TEXT_MUTED),
                ))
                .gap(0)
                .padding_y(6)
                .fill_width()
                .border_side(BorderSide::BOTTOM)
                .border(1, palette::DIVIDER)
                .scrollable(false),
                when(
                    move || get().is_some_and(|a| a.emergency || a.emergency_squawk()),
                    move |_| {
                        label(text!(
                            "{}",
                            Show(get(), |a, f| f.write_str(match a.squawk {
                                Some(7500) => "HIJACK  7500",
                                Some(7600) => "RADIO FAILURE  7600",
                                _ => "EMERGENCY",
                            }))
                        ))
                        .font(&FONT_24)
                        .text_color(palette::TEXT)
                        .bg(palette::ALERT)
                        .padding_x(10)
                        .padding_y(6)
                        .fill_width()
                    },
                ),
                data_row(
                    (
                        "SPEED",
                        text!("{}", Show(get(), |a, f| opt(f, a.ground_speed_kt, "kt"))),
                    ),
                    (
                        "TRACK",
                        text!(
                            "{}",
                            Show(get(), |a, f| match a.track_deg {
                                Some(t) => write!(f, "{t:03.0}° {}", compass_point(t)),
                                None => f.write_str("--"),
                            })
                        ),
                    ),
                ),
                data_row(
                    ("DISTANCE", text!("{}", Distance(geo.get()))),
                    ("BEARING", text!("{}", Bearing(geo.get()))),
                ),
                data_row(
                    (
                        "SQUAWK",
                        text!(
                            "{}",
                            Show(get(), |a, f| match a.squawk {
                                Some(s) => write!(f, "{s:04}"),
                                None => f.write_str("--"),
                            })
                        ),
                    ),
                    (
                        "SIGNAL",
                        text!("{}", Show(get(), |a, f| opt(f, a.rssi_dbfs, "dB"))),
                    ),
                ),
            ),
        )
        .gap(0)
        .fill_width()
        .flex_grow(1)
        .scrollbar(ScrollbarMode::Active),
    ))
    .gap(4)
    .fill_width()
    .flex_grow(1)
    .scrollable(false)
}

/// A row of the data block: two values side by side, each under its small caption, with a
/// hairline under the row.
fn data_row<M1, M2>(a: (&'static str, impl IntoText<M1>), b: (&'static str, impl IntoText<M2>)) -> impl View {
    row((datum(a.0, a.1), datum(b.0, b.1)))
        .gap(12)
        .padding_y(8)
        .fill_width()
        .border_side(BorderSide::BOTTOM)
        .border(1, palette::DIVIDER)
        .scrollable(false)
}

/// A value under its caption, taking half the row.
fn datum<M>(caption: &'static str, value: impl IntoText<M>) -> impl View {
    column((
        label(caption)
            .font(&FONT_18)
            .text_color(palette::TEXT_MUTED),
        label(value).font(&FONT_24).text_color(palette::TEXT),
    ))
    .gap(2)
    .flex_grow(1)
    .scrollable(false)
}

/// A legend entry's width: six fill the panel's content (255 px), "GND" (43 px) fits one.
const LEGEND_STEP: i32 = 42;

/// The altitude colour scale: one continuous bar, each label centred under its colour.
fn legend() -> impl View {
    let entries = palette::altitude_legend();
    row(entries.map(|(text, color)| {
        column((
            column(()).height(4).fill_width().bg(color),
            label(text).font(&FONT_18).text_color(palette::TEXT_MUTED),
        ))
        .width(LEGEND_STEP)
        .gap(4)
        .align_items(CrossAlign::Center)
        .scrollable(false)
    }))
    .gap(0)
    .justify(MainAlign::Center)
    .fill_width()
    .padding_y(4)
    .border_side(BorderSide::TOP)
    .border(1, palette::DIVIDER)
}

fn name_of(a: &Aircraft) -> crate::traffic::Ident {
    let mut s = heapless::String::<8>::new();
    let _ = write!(s, "{}", a.name());
    crate::traffic::Ident::new(&s)
}

/// The altitude for the list: `FL273`, `3000` (feet, no unit: the column is narrow), `GND`.
fn compact_altitude(altitude: Option<Altitude>) -> heapless::String<12> {
    let mut s = altitude_text(altitude);
    if s.ends_with(" ft") {
        s.truncate(s.len() - 3);
    }
    s
}

fn altitude_text(altitude: Option<Altitude>) -> heapless::String<12> {
    let mut s = heapless::String::new();
    let _ = match altitude {
        Some(Altitude::Ground) => s.push_str("GND").map_err(|_| fmt::Error),
        Some(Altitude::Feet(ft)) if ft >= 18_000 => write!(s, "FL{}", (ft + 50) / 100),
        Some(Altitude::Feet(ft)) => write!(s, "{} ft", (ft + 50) / 100 * 100),
        None => s.push_str("--").map_err(|_| fmt::Error),
    };
    s
}

/// Formats the selected aircraft with `f`, or `--` without one.
struct Show(
    Option<Aircraft>,
    fn(&Aircraft, &mut fmt::Formatter<'_>) -> fmt::Result,
);

impl fmt::Display for Show {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some(a) => (self.1)(a, f),
            None => f.write_str("--"),
        }
    }
}

fn opt(f: &mut fmt::Formatter<'_>, v: Option<f32>, unit: &str) -> fmt::Result {
    match v {
        Some(v) => write!(f, "{v:.0} {unit}"),
        None => f.write_str("--"),
    }
}

/// Registration, type and category.
fn describe(a: &Aircraft, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let mut sep = "";
    for part in [a.registration.as_str(), a.type_code.as_str()] {
        if !part.is_empty() {
            write!(f, "{sep}{part}")?;
            sep = "  •  ";
        }
    }
    if let Some(c) = a.category {
        write!(f, "{sep}{}", c.describe())?;
    } else if sep.is_empty() {
        f.write_str("Unknown type")?;
    }
    Ok(())
}

/// `↑ 1,200 fpm` / level.
fn climb(a: &Aircraft, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match a.vertical_rate_fpm {
        Some(v) if v > 150 => write!(f, "{} {v} fpm", Symbol::Up),
        Some(v) if v < -150 => write!(f, "{} {} fpm", Symbol::Down, -v),
        Some(_) => f.write_str("level"),
        None => Ok(()),
    }
}

/// Distance from the receiver.
struct Distance(Option<(f32, f32)>);

impl fmt::Display for Distance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some((d, _)) => write!(f, "{d:.1} nm"),
            None => f.write_str("--"),
        }
    }
}

/// Bearing from the receiver: degrees and compass point.
struct Bearing(Option<(f32, f32)>);

impl fmt::Display for Bearing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some((_, b)) => write!(f, "{b:03.0}° {}", compass_point(b)),
            None => f.write_str("--"),
        }
    }
}
