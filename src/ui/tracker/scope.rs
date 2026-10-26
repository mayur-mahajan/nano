//! The radar scope: north up, the receiver in the middle, to scale.
//!
//! Layers, bottom to top:
//! 1. the **background** canvas: the disc, the world map, range rings and compass ticks. It
//!    reads only the view (centre and range), so it is drawn again only when that changes;
//! 2. the **traffic** canvas: trails, aircraft glyphs (rotated to their track, coloured by
//!    altitude), callout leaders and the selection. It changes once per poll; it also takes
//!    the taps (hit-tested against the blips);
//! 3. labels: ring distances, compass points, airports and the aircraft callouts;
//! 4. the receiver's pulse, the zoom buttons and, when the feed is not live, a status card.

use alloc::rc::Rc;
use alloc::vec::Vec;

use crate::ui::fonts::{FONT_18, FONT_20, FONT_24, FONT_28};
use twine::core::{Fx, Transform};
use twine::prelude::*;
use twine::render::{FillRule, GradExtend, GradStop};
use twine::vector::{Dash, FxPoint, LineCap, LineJoin, Paint, Path, Stops, Stroke, VectorDsc};

use super::blips::{Blip, CALLOUT_SIZE};
use super::{CenterSource, RANGES_NM, Store};
use crate::connectivity::WifiState;
use crate::map::Geometry;
use crate::map::format::{AirportKind, LineKind};
use crate::traffic::{Category, FeedState};
use crate::ui::palette;

/// The scope area's size (left of the side panel, below the title bar).
pub const SIZE: Size = Size::new(800 - super::panel::WIDTH, 440);
/// The receiver's position in the scope area.
pub const CENTER: Point = Point::new(SIZE.w / 2, 220);
/// The outer ring's radius (the range), pixels.
pub const RADIUS: i32 = 206;
/// Taps within this distance of a blip select it.
const HIT_RADIUS: i32 = 24;
/// The distance between range rings for a range (nautical miles): round numbers, 2 to 5 rings.
pub const fn ring_step(range_nm: u16) -> u16 {
    match range_nm {
        0..=5 => 1,
        6..=10 => 2,
        11..=25 => 5,
        26..=50 => 10,
        51..=100 => 25,
        _ => 50,
    }
}

/// The inner rings' radii (pixels) and distances (nm) at `range_nm`.
fn rings(range_nm: u16) -> impl Iterator<Item = (i32, u16)> {
    let step = ring_step(range_nm);
    (1..)
        .map(move |k| k * step)
        .take_while(move |&d| d < range_nm)
        .map(move |d| (RADIUS * i32::from(d) / i32::from(range_nm), d))
}

const fn stop(color: Color, frac: u8) -> GradStop {
    GradStop {
        color,
        opa: Opa::COVER,
        frac: Fraction::from_raw(frac),
    }
}

static DISC: [GradStop; 2] = [
    stop(palette::SCOPE_CENTER, 0),
    stop(palette::SCOPE_EDGE, 255),
];

/// The scope.
pub fn view(cx: Scope, open_settings: impl Fn() + Copy + 'static) -> impl View {
    let store = cx.expect_context::<Store>();
    let geometry = cx.memo(move || {
        let (Some(data), Some(center)) = (store.map.get(), store.center.get()) else {
            return Rc::new(Geometry::default());
        };
        let range = f32::from(store.range_nm());
        let g = Geometry::build(data, center, RADIUS as f32 / range, RADIUS);
        log::info!(
            "scope: map for {range} nm: {} lines, {} points, {} airports",
            g.lines.len(),
            g.points(),
            g.airports.len()
        );
        Rc::new(g)
    });

    column((
        background(store, geometry),
        traffic(store),
        labels(cx, store, geometry),
        receiver_pulse(cx),
        controls(store),
        status(store, open_settings),
    ))
    .layout(Layout::none())
    .size(SIZE.w, SIZE.h)
    .scrollable(false)
}

fn stroke(color: Color, width_tenths: i32) -> Option<(Paint, Stroke)> {
    Some((
        Paint::Solid(color),
        Stroke {
            width: Fx::from_ratio(width_tenths, 10),
            cap: LineCap::Round,
            join: LineJoin::Round,
            ..Stroke::default()
        },
    ))
}

fn fx(p: Point) -> FxPoint {
    FxPoint::from_int(p.x, p.y)
}

/// Layer 1: disc, map, rings, ticks.
fn background(store: Store, geometry: Memo<Rc<Geometry>>) -> impl View {
    vector_canvas(move |scene| {
        let c = fx(CENTER);
        if let Some((path, dsc)) = scene.path_mut() {
            path.circle(c, Fx::from_int(RADIUS + 6));
            *dsc = VectorDsc {
                fill: Some((
                    Paint::Radial {
                        center: c,
                        radius: Fx::from_int(RADIUS + 6),
                        focal: None,
                        stops: Stops::Static(&DISC),
                        extend: GradExtend::Pad,
                        transform: Transform::IDENTITY,
                    },
                    FillRule::NonZero,
                )),
                ..VectorDsc::default()
            };
        }
        geometry.with(|g| {
            for (kind, points) in &g.lines {
                let Some((path, dsc)) = scene.path_mut() else {
                    return;
                };
                polyline(path, points);
                let (color, width, opa) = match kind {
                    LineKind::Coast => (palette::COAST, 15, Opa::COVER),
                    LineKind::Lake => (palette::LAKE, 10, Opa::P80),
                    LineKind::Country => (palette::BORDER, 12, Opa::P80),
                    LineKind::State => (palette::STATE, 10, Opa::P70),
                };
                *dsc = VectorDsc {
                    stroke: stroke(color, width),
                    stroke_opa: opa,
                    transform: Transform::translate(Fx::from_int(CENTER.x), Fx::from_int(CENTER.y)),
                    ..VectorDsc::default()
                };
            }
            for &(kind, _, at) in &g.airports {
                let Some((path, dsc)) = scene.path_mut() else {
                    return;
                };
                let r = if kind == AirportKind::Large { 4 } else { 3 };
                path.circle(fx(at), Fx::from_int(r));
                *dsc = VectorDsc {
                    stroke: stroke(palette::AIRPORT, 15),
                    transform: Transform::translate(Fx::from_int(CENTER.x), Fx::from_int(CENTER.y)),
                    ..VectorDsc::default()
                };
            }
        });
        // Range rings: dashed inside, solid at the range.
        if let Some((path, dsc)) = scene.path_mut() {
            for (r, _) in rings(store.range_nm()) {
                path.circle(c, Fx::from_int(r));
            }
            let mut s = stroke(palette::RING, 10);
            if let Some((_, st)) = s.as_mut() {
                st.dash = Some(Dash::new(&[Fx::from_int(3), Fx::from_int(5)], Fx::ZERO));
            }
            *dsc = VectorDsc {
                stroke: s,
                stroke_opa: Opa::P70,
                ..VectorDsc::default()
            };
        }
        if let Some((path, dsc)) = scene.path_mut() {
            path.circle(c, Fx::from_int(RADIUS));
            *dsc = VectorDsc {
                stroke: stroke(palette::RING, 20),
                ..VectorDsc::default()
            };
        }
        // Compass ticks every 10°, longer every 30°.
        if let Some((path, dsc)) = scene.path_mut() {
            for deg in (0..360).step_by(10) {
                let len = if deg % 30 == 0 { 10 } else { 5 };
                let a = Angle::deg(deg);
                let (s, co) = (twine::core::math::sin_fx(a), twine::core::math::cos_fx(a));
                let at = |r: i32| {
                    FxPoint::new(c.x + s.mul(Fx::from_int(r)), c.y - co.mul(Fx::from_int(r)))
                };
                path.move_to(at(RADIUS)).line_to(at(RADIUS - len));
            }
            *dsc = VectorDsc {
                stroke: stroke(palette::RING, 15),
                ..VectorDsc::default()
            };
        }
    })
    .size(SIZE.w, SIZE.h)
    .clickable(false)
    .pos(0, 0)
}

fn polyline(path: &mut Path, points: &[Point]) {
    let mut it = points.iter();
    if let Some(&first) = it.next() {
        path.move_to(fx(first));
        for &p in it {
            path.line_to(fx(p));
        }
    }
}

/// The airplane glyph, nose up, in tenths of a pixel around its centre.
const PLANE: [(i32, i32); 19] = [
    (0, -95),
    (14, -70),
    (16, -25),
    (90, 15),
    (90, 32),
    (16, 12),
    (12, 55),
    (38, 75),
    (38, 88),
    (0, 78),
    (-38, 88),
    (-38, 75),
    (-12, 55),
    (-16, 12),
    (-90, 32),
    (-90, 15),
    (-16, -25),
    (-14, -70),
    (0, -95),
];

/// Layer 2: trails, glyphs, callout leaders, selection. Taps select.
fn traffic(store: Store) -> impl View {
    vector_canvas(move |scene| {
        store.blips.with(|blips| {
            for b in blips {
                draw_trail(scene, b);
            }
            for b in blips {
                if let Some(c) = b.callout
                    && let Some((path, dsc)) = scene.path_mut()
                {
                    let end = leader_end(b.at, c);
                    path.move_to(fx(b.at)).line_to(fx(end));
                    *dsc = VectorDsc {
                        stroke: stroke(b.color, 10),
                        stroke_opa: Opa::P60,
                        ..VectorDsc::default()
                    };
                }
            }
            for b in blips {
                draw_glyph(scene, b);
            }
        });
    })
    .size(SIZE.w, SIZE.h)
    .pos(0, 0)
    .on_event(EventCode::Clicked, move |ecx| {
        let Some(p) = ecx.point() else {
            return;
        };
        let o = twine::widgets_ext::vector_view::origin(ecx.engine(), ecx.node());
        let tap = Point::new(p.x - o.x, p.y - o.y);
        let hit = store.blips.with_untracked(|blips| {
            blips
                .iter()
                .map(|b| {
                    let (dx, dy) = (b.at.x - tap.x, b.at.y - tap.y);
                    (dx * dx + dy * dy, b.address)
                })
                .filter(|&(d, _)| d <= HIT_RADIUS * HIT_RADIUS)
                .min_by_key(|&(d, _)| d)
                .map(|(_, a)| a)
        });
        store.selected.set(hit);
    })
}

/// Where a callout's leader meets its box: the box corner nearest the blip.
fn leader_end(at: Point, callout: Point) -> Point {
    let (w, h) = (CALLOUT_SIZE.w, CALLOUT_SIZE.h);
    let x = if callout.x > at.x {
        callout.x
    } else {
        callout.x + w
    };
    let y = if callout.y > at.y {
        callout.y
    } else {
        callout.y + h
    };
    Point::new(x, y)
}

fn draw_trail(scene: &mut twine::vector::VectorScene, b: &Blip) {
    if b.trail.len() < 2 {
        return;
    }
    // Three segments, fading towards the oldest point.
    let n = b.trail.len();
    let parts = [
        (0, n / 3, Opa::P20),
        (n / 3, 2 * n / 3, Opa::P40),
        (2 * n / 3, n, Opa::P70),
    ];
    for (from, to, opa) in parts {
        let to = (to + 1).min(n);
        if to <= from + 1 {
            continue;
        }
        let Some((path, dsc)) = scene.path_mut() else {
            return;
        };
        polyline(path, &b.trail[from..to]);
        *dsc = VectorDsc {
            stroke: stroke(b.color, if b.selected { 25 } else { 15 }),
            stroke_opa: opa,
            ..VectorDsc::default()
        };
    }
}

fn draw_glyph(scene: &mut twine::vector::VectorScene, b: &Blip) {
    let at = fx(b.at);
    if (b.selected || b.emergency)
        && let Some((path, dsc)) = scene.path_mut()
    {
        path.circle(at, Fx::from_int(15));
        *dsc = VectorDsc {
            stroke: stroke(
                if b.emergency {
                    palette::ALERT
                } else {
                    palette::ACCENT
                },
                20,
            ),
            ..VectorDsc::default()
        };
    }
    let Some((path, dsc)) = scene.path_mut() else {
        return;
    };
    let Some(track) = b.track else {
        // No track: a dot.
        path.circle(at, Fx::from_int(4));
        *dsc = VectorDsc::fill(b.color);
        return;
    };
    let mut first = true;
    for &(x, y) in &PLANE {
        let p = FxPoint::new(Fx::from_ratio(x, 10), Fx::from_ratio(y, 10));
        if first {
            path.move_to(p);
            first = false;
        } else {
            path.line_to(p);
        }
    }
    path.close();
    let scale = match b.category {
        Some(Category::Heavy) => Fx::from_ratio(13, 10),
        Some(Category::Light | Category::Light2 | Category::Helicopter) => Fx::from_ratio(8, 10),
        _ => Fx::from_int(1),
    };
    *dsc = VectorDsc {
        fill: Some((Paint::Solid(b.color), FillRule::NonZero)),
        stroke: Some((
            Paint::Solid(palette::SKY),
            Stroke {
                width: Fx::from_int(1),
                join: LineJoin::Round,
                ..Stroke::default()
            },
        )),
        transform: Transform::scale(scale, scale)
            .then(Transform::rotate(Angle::deci_deg((track * 10.0) as i32)))
            .then(Transform::translate(at.x, at.y)),
        ..VectorDsc::default()
    };
}

/// A callout's contents.
#[derive(Clone, Debug, PartialEq)]
struct Callout {
    address: u32,
    at: Point,
    name: crate::traffic::Ident,
    altitude: heapless::String<12>,
    speed: Option<u16>,
    color: Color,
    /// The altitude colour for text.
    text: Color,
    selected: bool,
}

/// An airport label.
#[derive(Clone, Debug, PartialEq)]
struct AirportLabel {
    ident: [u8; 4],
    at: Point,
}

/// Layer 3: ring distances, compass points, airports, callouts.
fn labels(cx: Scope, store: Store, geometry: Memo<Rc<Geometry>>) -> impl View {
    let callouts = cx.memo(move || {
        store.blips.with(|blips| {
            blips
                .iter()
                .filter_map(|b| {
                    let at = b.callout?;
                    let mut altitude = heapless::String::new();
                    b.altitude_text(&mut altitude);
                    Some(Callout {
                        address: b.address,
                        at,
                        name: b.name,
                        altitude,
                        speed: b.speed_kt.map(|s| s as u16),
                        color: b.color,
                        text: crate::ui::palette::altitude_text(b.altitude),
                        selected: b.selected,
                    })
                })
                .collect::<Vec<_>>()
        })
    });
    let airports = cx.memo(move || {
        let range = store.range_nm();
        geometry.with(|g| {
            g.airports
                .iter()
                .filter(|(kind, _, _)| match kind {
                    AirportKind::Large => range <= 50,
                    AirportKind::Medium => range <= 25,
                })
                .take(24)
                .map(|&(_, ident, at)| AirportLabel {
                    ident,
                    at: Point::new(CENTER.x + at.x, CENTER.y + at.y),
                })
                .collect::<Vec<_>>()
        })
    });
    // On the 45° diagonal (north-east), just outside each ring.
    let ring_labels = cx.memo(move || {
        rings(store.range_nm())
            .map(|(r, nm)| {
                let d = r * 707 / 1000;
                (nm, Point::new(CENTER.x + d + 2, CENTER.y - d - 16))
            })
            .collect::<Vec<_>>()
    });
    let compass = |text: &'static str, x: i32, y: i32| {
        label(text)
            .font(&FONT_20)
            .text_color(palette::ACCENT)
            .pos(x, y)
    };
    column((
        for_each(
            ring_labels,
            |&(nm, _): &(u16, Point)| nm,
            |_, r| {
                label(text!("{}", r.with(|r| r.0)))
                    .font(&FONT_18)
                    .text_color(palette::RING_TEXT)
                    .pos(move || r.with(|r| r.1.x), move || r.with(|r| r.1.y))
            },
        ),
        compass("N", CENTER.x - 6, CENTER.y - RADIUS + 12),
        compass("S", CENTER.x - 5, CENTER.y + RADIUS - 30),
        compass("E", CENTER.x + RADIUS - 26, CENTER.y - 9),
        compass("W", CENTER.x - RADIUS + 12, CENTER.y - 9),
        for_each(
            airports,
            |a: &AirportLabel| a.ident,
            |_, a| {
                label(text!("{}", Ident4(a.with(|a| a.ident))))
                    .font(&FONT_18)
                    .text_color(palette::AIRPORT_TEXT)
                    .pos(
                        move || a.with(|a| a.at.x + 6),
                        move || a.with(|a| a.at.y - 8),
                    )
            },
        ),
        for_each(callouts, |c: &Callout| c.address, |_, c| callout(c)),
    ))
    .layout(Layout::none())
    .size(SIZE.w, SIZE.h)
    .pos(0, 0)
    .clickable(false)
    .scrollable(false)
}

fn callout(c: ReadSignal<Callout>) -> impl View {
    column((
        label(text!("{}", c.with(|c| c.name)))
            .font(&FONT_20)
            .text_color(palette::TEXT),
        label(text!(
            "{} {}",
            c.with(|c| c.altitude.clone()),
            SpeedText(c.with(|c| c.speed))
        ))
        .font(&FONT_18)
        .text_color(move || c.with(|c| c.text)),
    ))
    .gap(0)
    .padding_x(6)
    .padding_y(2)
    .size(CALLOUT_SIZE.w, CALLOUT_SIZE.h)
    .bg_color(palette::SURFACE)
    .bg_opacity(Opa::P80)
    .border(1, move || c.with(|c| c.color))
    .border_opacity(move || {
        if c.with(|c| c.selected) {
            Opa::COVER
        } else {
            Opa::P60
        }
    })
    .radius(6)
    .clickable(false)
    .scrollable(false)
    .pos(move || c.with(|c| c.at.x), move || c.with(|c| c.at.y))
}

/// `450 kt`, or nothing.
struct SpeedText(Option<u16>);

impl core::fmt::Display for SpeedText {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0 {
            Some(kt) => write!(f, "{kt}kt"),
            None => Ok(()),
        }
    }
}

/// How often the receiver's ring pulses when the tracker opens. Then it stays faded out:
/// every pulse frame redraws the map and rings under it (~200 ms), so a forever pulse kept the
/// CPU busy at ~4 fps.
const PULSES: u16 = 3;

/// The receiver: a dot with a ring pulsing outwards a few times.
fn receiver_pulse(cx: Scope) -> impl View {
    let spec = AnimSpec::new(Duration::ms(2400))
        .ease_out()
        .repeat(Repeat::Times(PULSES));
    let (scale, _) = cx.animation(Scale::pct(30), Scale::pct(100), spec);
    let (opa, _) = cx.animation(Opa::P70, Opa::TRANSP, spec);
    column((
        column(())
            .size(48, 48)
            .radius(Radius::Circle)
            .border(2, palette::ACCENT)
            .border_opacity(opa)
            .transform_scale(scale)
            .transform_pivot(Point::new(24, 24))
            .clickable(false)
            .pos(0, 0),
        column(())
            .size(8, 8)
            .radius(Radius::Circle)
            .bg(palette::ACCENT)
            .clickable(false)
            .pos(20, 20),
    ))
    .layout(Layout::none())
    .size(48, 48)
    .clickable(false)
    .scrollable(false)
    .pos(CENTER.x - 24, CENTER.y - 24)
}

fn round_button(icon: Symbol, on_click: impl Fn() + 'static) -> impl View {
    button((label(icon).font(&FONT_24),))
        .size(44, 44)
        .radius(Radius::Circle)
        .bg(palette::SURFACE_HI)
        .border(1, palette::DIVIDER)
        .text_color(palette::TEXT)
        .on_state(State::PRESSED, |s| s.bg(palette::DIVIDER))
        .on_click(on_click)
}

/// Layer 4: zoom and range.
fn controls(store: Store) -> impl View {
    column((
        round_button(Symbol::Plus, move || store.zoom(-1)),
        round_button(Symbol::Minus, move || store.zoom(1)),
        label(text!("{} nm", store.range_nm()))
            .font(&FONT_20)
            .text_color(palette::TEXT_MUTED),
    ))
    .gap(8)
    .align_items(CrossAlign::Center)
    .pos(12, SIZE.h - 160)
    .scrollable(false)
}

/// What the status card says.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Fine,
    Welcome,
    NotConfigured,
    Offline,
    Connecting,
    Unreachable,
    NoLocation,
}

/// Layer 4: the feed's state when it is not live, or a missing location.
fn status(store: Store, open_settings: impl Fn() + Copy + 'static) -> impl View {
    let status = move || match store.feed.get() {
        _ if store.wifi.get() == WifiState::NotConfigured => Status::Welcome,
        FeedState::NotConfigured => Status::NotConfigured,
        FeedState::Offline => Status::Offline,
        FeedState::Connecting => Status::Connecting,
        FeedState::Unreachable(_) if store.traffic.with(Option::is_none) => Status::Unreachable,
        FeedState::Unreachable(_) => Status::Unreachable,
        FeedState::Live if store.center_source.get() == CenterSource::Unknown => Status::NoLocation,
        FeedState::Live => Status::Fine,
    };
    when(
        move || status() != Status::Fine,
        move |_cx| {
            let (icon, color) = match status() {
                Status::Welcome => (Symbol::Wifi, palette::ACCENT),
                Status::NotConfigured | Status::NoLocation => (Symbol::Settings, palette::WARN),
                Status::Connecting => (Symbol::Refresh, palette::ACCENT),
                _ => (Symbol::Warning, palette::ALERT),
            };
            column((
                label(icon).font(&FONT_28).text_color(color),
                label(move || match status() {
                    Status::Welcome => "Welcome to nano",
                    Status::NotConfigured => "Feeder not set up",
                    Status::Offline => "Wi-Fi offline",
                    Status::Connecting => "Connecting to feeder...",
                    Status::Unreachable => "No connection to feeder",
                    Status::NoLocation => "Receiver location unknown",
                    Status::Fine => "",
                })
                .font(&FONT_24)
                .text_color(palette::TEXT),
                label(move || match status() {
                    Status::Welcome => "Connect Wi-Fi and your ADS-B feeder in Settings".into(),
                    Status::NotConfigured => "Enter its address in Settings".into(),
                    Status::NoLocation => "Set it in Settings to centre the scope".into(),
                    Status::Unreachable => store.settings.with(|s| {
                        let mut t = alloc::string::String::new();
                        let _ = core::fmt::Write::write_fmt(
                            &mut t,
                            format_args!("{}:{}  -  retrying", s.feeder_host, s.feeder_port),
                        );
                        t
                    }),
                    Status::Offline => "Waiting for the network".into(),
                    _ => alloc::string::String::new(),
                })
                .font(&FONT_20)
                .text_color(palette::TEXT_MUTED),
                when(
                    move || {
                        matches!(
                            status(),
                            Status::Welcome | Status::NotConfigured | Status::NoLocation
                        )
                    },
                    move |_| {
                        button((label("Open Settings").font(&FONT_20),))
                            .bg(palette::ACCENT)
                            .text_color(palette::SKY)
                            .radius(8)
                            .padding_x(16)
                            .padding_y(8)
                            .on_click(open_settings)
                    },
                ),
            ))
            .gap(6)
            .align_items(CrossAlign::Center)
            .padding(16)
            .bg_color(palette::SURFACE)
            .bg_opacity(Opa::COVER)
            .border(1, color)
            .radius(12)
            .scrollable(false)
            .align(Align::Center)
        },
    )
}

/// A space-padded 4-character ident.
struct Ident4([u8; 4]);

impl core::fmt::Display for Ident4 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for b in self.0.iter().filter(|b| **b != b' ') {
            core::fmt::Write::write_char(f, char::from(*b))?;
        }
        Ok(())
    }
}

/// The scope's range in nautical miles (for other parts).
#[allow(dead_code)]
pub fn range_nm(index: usize) -> u16 {
    RANGES_NM[index]
}
