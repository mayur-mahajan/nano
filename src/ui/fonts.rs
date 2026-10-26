//! The UI's fonts: twine's Montserrat **smooth** sizes, unhinted with linear coverage (as LVGL
//! renders them), weighted per panel by the display's text gamma (Settings → Display).
//!
//! On this 4.3" 800 × 480 panel (~217 px per inch) 14 px text has capitals ~1.1 mm tall: too
//! small at desk distance. Captions are 18 px, body text 20 px at least.

pub use twine::fonts::{
    MONTSERRAT_18_SMOOTH as FONT_18, MONTSERRAT_20_SMOOTH as FONT_20,
    MONTSERRAT_24_SMOOTH as FONT_24, MONTSERRAT_28_SMOOTH as FONT_28,
    MONTSERRAT_48_SMOOTH as FONT_48,
};
