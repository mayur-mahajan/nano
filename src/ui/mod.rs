//! The application UI: platform-neutral (only `twine`), so it also runs in twine's simulator
//! and in host tests.
//!
//! With a saved Wi-Fi network the boot screen shows until it is up (or for at most
//! [`boot::MAX_WAIT`]) and its airplane has landed; then the tracker replaces it (the boot
//! screen's scope, timers and animations are disposed). Without one nano starts on the tracker,
//! which asks for the settings.

mod assets;
mod boot;
pub mod fonts;
mod palette;
mod settings;
mod tracker;

use twine::prelude::*;

use crate::settings::TextWeight;

/// Logical screen size the layouts are designed for (the 8048S043 panel).
pub const SCREEN_WIDTH: u16 = 800;
/// See [`SCREEN_WIDTH`].
pub const SCREEN_HEIGHT: u16 = 480;

/// The LVGL theme in dark mode only (the other modes' tables are not linked).
static THEME: Lvgl = Lvgl::new().modes(&[(ThemeMode::Dark, &DARK)]);

/// The LVGL theme's dark table with nano's fonts (also for widgets on the top layer, such as a
/// dropdown's list) and accent.
static DARK: twine::style::design::ElementTable = Lvgl::SET
    .primary(palette::ACCENT)
    .fonts(twine::theme::FontScale {
        small: &fonts::FONT_18,
        normal: &fonts::FONT_20,
        large: &fonts::FONT_24,
    })
    .table(ThemeMode::Dark);

/// The app configuration: the LVGL theme in dark mode.
pub fn config() -> AppConfig {
    AppConfig::new().theme(&THEME).mode(ThemeMode::Dark)
}

/// [`TextWeight::Medium`]: a mild lift of the linear coverage (γ 1.1 dark-on-light, 1.4
/// light-on-dark), between LVGL's look and twine's default.
static MEDIUM: twine::text::TextGamma = twine::text::TextGamma {
    dark_on_light: twine::text::GammaRamp::new([
        0, 22, 41, 59, 77, 94, 111, 128, 144, 160, 176, 192, 208, 224, 239, 255,
    ]),
    light_on_dark: twine::text::GammaRamp::new([
        0, 37, 60, 81, 99, 116, 133, 148, 163, 177, 191, 204, 217, 230, 243, 255,
    ]),
};

/// The display's text gamma of a weight (`None`: the fonts' own linear coverage).
const fn text_gamma(weight: TextWeight) -> Option<&'static twine::text::TextGamma> {
    match weight {
        TextWeight::Light => None,
        TextWeight::Medium => Some(&MEDIUM),
        TextWeight::Strong => Some(&twine::text::gamma::DEFAULT),
    }
}

/// Draws all text on the display with `weight` (redraws it when it changes).
pub(crate) fn apply_text_weight(cx: Scope, weight: TextWeight) {
    twine::view::EngineAccess::with(cx, |e| {
        if let Some(d) = e.default_display() {
            e.set_text_gamma(d, text_gamma(weight));
        }
    });
}

/// The root view.
pub fn app(cx: Scope) -> impl View {
    // The saved text weight, applied whenever the settings change.
    let settings = cx.watch(&crate::settings::SETTINGS);
    cx.effect(move || apply_text_weight(cx, settings.with(|s| s.text_weight)));
    let booted = cx.signal(!crate::settings::SETTINGS.get().has_wifi());
    let open_settings = move || settings::open(cx);
    when(
        move || booted.get(),
        move |cx| tracker::screen(cx, open_settings),
    )
    .otherwise(move |cx| boot::screen(cx, move || booted.set(true)))
}
