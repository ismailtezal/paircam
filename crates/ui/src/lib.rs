//! The PairCam look: the generated theme and tokens, the bundled fonts, and the components every
//! screen shares. The design direction is `docs/DESIGN.md`.

use std::borrow::Cow;

use anyhow::{Context as _, Result};
use gpui_kit::component::{Theme, ThemeMode, ThemeRegistry};
use gpui_kit::{App, Hsla, Rgba, rgba};

mod components;
pub mod tokens;

pub use components::*;
pub use tokens::Palette;

const THEME_FILE: &str = include_str!("../assets/themes/paircam.json");
const DARK_THEME: &str = "PairCam Dark";
const LIGHT_THEME: &str = "PairCam Light";

const FONTS: [&[u8]; 5] = [
    include_bytes!("../assets/fonts/Geist-Regular.ttf"),
    include_bytes!("../assets/fonts/Geist-Medium.ttf"),
    include_bytes!("../assets/fonts/Geist-SemiBold.ttf"),
    include_bytes!("../assets/fonts/GeistMono-Regular.ttf"),
    include_bytes!("../assets/fonts/GeistMono-Medium.ttf"),
];

/// Sets up GPUI Kit, the bundled fonts and the PairCam theme. Call once, before opening a window.
pub fn init(cx: &mut App) -> Result<()> {
    gpui_kit::init(cx);

    cx.text_system()
        .add_fonts(FONTS.iter().map(|font| Cow::Borrowed(*font)).collect())
        .context("loading the bundled fonts")?;

    ThemeRegistry::global_mut(cx)
        .load_themes_from_str(THEME_FILE)
        .context("parsing the PairCam theme")?;
    let themes = ThemeRegistry::global(cx).themes();
    let light = themes
        .get(LIGHT_THEME)
        .cloned()
        .context("the PairCam light theme is missing")?;
    let dark = themes
        .get(DARK_THEME)
        .cloned()
        .context("the PairCam dark theme is missing")?;

    // Register both so switching modes stays inside the PairCam theme. Dark is the default.
    Theme::update(cx, |theme| {
        theme.apply_config(&light);
        theme.apply_config(&dark);
    });
    Ok(())
}

/// Switches between the dark and light PairCam themes.
pub fn set_mode(mode: ThemeMode, cx: &mut App) {
    Theme::change(mode, None, cx);
}

/// The token palette for the active mode. Use it for colours the kit theme does not carry,
/// such as `live` and `material`.
pub fn palette(cx: &App) -> &'static Palette {
    if Theme::global(cx).is_dark() {
        &tokens::DARK
    } else {
        &tokens::LIGHT
    }
}

/// Converts a token colour (`0xRRGGBBAA`) to a GPUI colour.
pub fn color(token: u32) -> Hsla {
    let color: Rgba = rgba(token);
    color.into()
}
