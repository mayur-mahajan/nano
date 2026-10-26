//! Images compiled into flash.

use twine::image::{ColorFormat, Image, ImageFlags, ImageHeader};
use twine::prelude::ImageSource;

/// The Twine logo (the Rope mark: red, green and blue strands on transparent) at 128 × 128,
/// RGB565 + 8-bit alpha, drawn 1:1. Converted from twine's `assets/logo/twine-logo-color-128.png`
/// (commit 6e19547) with `twine image --format rgb565a8`; its flat colours need no dithering.
/// Draw it unscaled: magnifying a bitmap shows its pixels.
pub static TWINE_LOGO: Image = Image::new_static(
    ImageHeader {
        format: ColorFormat::Rgb565A8,
        w: 128,
        h: 128,
        stride: 256,
        flags: ImageFlags::from_bits_retain(0x00),
    },
    include_bytes!("../../assets/twine_logo_color_128_rgb565a8.bin"),
);

/// A side view of an airplane flying right (64 × 32 SVG, drawn as vectors).
pub const AIRPLANE: ImageSource = ImageSource::Svg(include_bytes!("../../assets/airplane.svg"));

/// An airplane seen from above, nose up (24 × 24 SVG, the scope's glyph): the title bar icon.
pub const PLANE_TOP: ImageSource = ImageSource::Svg(include_bytes!("../../assets/plane_top.svg"));
