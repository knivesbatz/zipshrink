//! Design tokens: the only place colours, type sizes, spacing, radii,
//! elevation and motion timings are defined. Everything else asks for these.

use eframe::egui::{Color32, CornerRadius, FontFamily, FontId, Shadow};

/// Colours of one appearance. Light and dark are separate, hand-tuned sets
/// (not inversions); the app cross-fades between them when the system
/// appearance changes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// Window background behind the content.
    pub bg: Color32,
    /// Sidebar surface.
    pub sidebar: Color32,
    /// Grouped cards and list surfaces.
    pub surface: Color32,
    /// Raised surfaces: sheets, popovers, toasts.
    pub elevated: Color32,
    /// Hairline separators and card outlines.
    pub separator: Color32,
    /// Outline of text fields and the drop zone.
    pub outline: Color32,
    pub text: Color32,
    pub text_secondary: Color32,
    pub text_tertiary: Color32,
    /// Neutral fill of secondary buttons, segmented tracks, switches (off).
    pub fill: Color32,
    pub fill_hover: Color32,
    pub fill_pressed: Color32,
    /// Selected sidebar row (unemphasised, as in Big Sur).
    pub selection: Color32,
    /// Alternate rows of tables.
    pub stripe: Color32,
    /// Text field background.
    pub field: Color32,
    /// Segmented-control thumb and switch knob.
    pub thumb: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_pressed: Color32,
    /// Text and icons drawn on the accent colour.
    pub on_accent: Color32,
    pub green: Color32,
    pub orange: Color32,
    pub red: Color32,
    pub red_hover: Color32,
    /// Push buttons and pop-up buttons (white in light mode, like macOS).
    pub control: Color32,
    pub control_pressed: Color32,
    /// Edge of push buttons, pop-ups and fields.
    pub control_edge: Color32,
    /// Icon tiles in the sidebar.
    pub tile_blue: Color32,
    pub tile_green: Color32,
    pub tile_orange: Color32,
    pub tile_gray: Color32,
    /// Backdrop behind sheets.
    pub scrim: Color32,
    pub shadow: Color32,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied_const(r, g, b, a)
}

/// Big Sur light appearance: white content, a soft gray sidebar, gray
/// group boxes, labels at 85% and 50%-ish black.
pub const LIGHT: Palette = Palette {
    bg: rgb(0xFF, 0xFF, 0xFF),
    sidebar: rgb(0xE9, 0xE9, 0xEA),
    surface: rgb(0xF5, 0xF5, 0xF5),
    elevated: rgb(0xF2, 0xF2, 0xF2),
    separator: rgb(0xE3, 0xE3, 0xE3),
    outline: rgb(0xC8, 0xC8, 0xC8),
    text: rgb(0x26, 0x26, 0x26),
    text_secondary: rgb(0x6E, 0x6E, 0x6E),
    text_tertiary: rgb(0xAE, 0xAE, 0xAE),
    fill: rgb(0xE4, 0xE4, 0xE4),
    fill_hover: rgb(0xDA, 0xDA, 0xDA),
    fill_pressed: rgb(0xCE, 0xCE, 0xCE),
    selection: rgb(0xD4, 0xD4, 0xD5),
    stripe: rgb(0xF4, 0xF5, 0xF5),
    field: rgb(0xFF, 0xFF, 0xFF),
    thumb: rgb(0xFF, 0xFF, 0xFF),
    accent: rgb(0x00, 0x7A, 0xFF),
    accent_hover: rgb(0x0A, 0x6F, 0xEB),
    accent_pressed: rgb(0x00, 0x60, 0xD6),
    on_accent: rgb(0xFF, 0xFF, 0xFF),
    control: rgb(0xFF, 0xFF, 0xFF),
    control_pressed: rgb(0xE6, 0xE6, 0xE6),
    control_edge: rgb(0xC9, 0xC9, 0xC9),
    tile_blue: rgb(0x0A, 0x7A, 0xFF),
    tile_green: rgb(0x28, 0xCD, 0x41),
    tile_orange: rgb(0xFF, 0x95, 0x00),
    tile_gray: rgb(0x8E, 0x8E, 0x93),
    green: rgb(0x1F, 0xA8, 0x3D),
    orange: rgb(0xE8, 0x87, 0x00),
    red: rgb(0xFF, 0x3B, 0x30),
    red_hover: rgb(0xE6, 0x2E, 0x24),
    scrim: rgba(0, 0, 0, 20),
    shadow: rgba(0, 0, 0, 70),
};

/// Big Sur dark appearance: charcoal content, a graphite sidebar, layered
/// surfaces separated by light hairlines.
pub const DARK: Palette = Palette {
    bg: rgb(0x1E, 0x1E, 0x1E),
    sidebar: rgb(0x2C, 0x2C, 0x2D),
    surface: rgb(0x28, 0x28, 0x28),
    elevated: rgb(0x33, 0x33, 0x34),
    separator: rgb(0x38, 0x38, 0x38),
    outline: rgb(0x4C, 0x4C, 0x4C),
    text: rgb(0xE4, 0xE4, 0xE4),
    text_secondary: rgb(0x9A, 0x9A, 0x9A),
    text_tertiary: rgb(0x5E, 0x5E, 0x5E),
    fill: rgb(0x3A, 0x3A, 0x3A),
    fill_hover: rgb(0x45, 0x45, 0x45),
    fill_pressed: rgb(0x50, 0x50, 0x50),
    selection: rgb(0x45, 0x45, 0x46),
    stripe: rgb(0x25, 0x25, 0x25),
    field: rgb(0x2B, 0x2B, 0x2B),
    thumb: rgb(0x65, 0x65, 0x67),
    accent: rgb(0x0A, 0x84, 0xFF),
    accent_hover: rgb(0x33, 0x96, 0xFF),
    accent_pressed: rgb(0x00, 0x70, 0xE0),
    on_accent: rgb(0xFF, 0xFF, 0xFF),
    control: rgb(0x5C, 0x5C, 0x5E),
    control_pressed: rgb(0x70, 0x70, 0x72),
    control_edge: rgb(0x48, 0x48, 0x48),
    tile_blue: rgb(0x0A, 0x84, 0xFF),
    tile_green: rgb(0x32, 0xD7, 0x4B),
    tile_orange: rgb(0xFF, 0x9F, 0x0A),
    tile_gray: rgb(0x8E, 0x8E, 0x93),
    green: rgb(0x32, 0xD7, 0x4B),
    orange: rgb(0xFF, 0x9F, 0x0A),
    red: rgb(0xFF, 0x45, 0x3A),
    red_hover: rgb(0xFF, 0x63, 0x5A),
    scrim: rgba(0, 0, 0, 60),
    shadow: rgba(0, 0, 0, 150),
};

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()), f(a.a(), b.a()))
}

impl Palette {
    /// Blends two appearances; `t` = 0 is `a`, 1 is `b`.
    pub fn lerp(a: &Palette, b: &Palette, t: f32) -> Palette {
        if t <= 0.0 {
            return *a;
        }
        if t >= 1.0 {
            return *b;
        }
        macro_rules! m {
            ($($f:ident),*) => { Palette { $($f: mix(a.$f, b.$f, t)),* } };
        }
        m!(
            bg,
            sidebar,
            surface,
            elevated,
            separator,
            outline,
            text,
            text_secondary,
            text_tertiary,
            fill,
            fill_hover,
            fill_pressed,
            selection,
            stripe,
            field,
            thumb,
            accent,
            accent_hover,
            accent_pressed,
            on_accent,
            control,
            control_pressed,
            control_edge,
            tile_blue,
            tile_green,
            tile_orange,
            tile_gray,
            green,
            orange,
            red,
            red_hover,
            scrim,
            shadow
        )
    }
}

/// Spacing scale (points). Layouts use these steps only.
pub mod space {
    pub const XS: f32 = 4.0;
    pub const S: f32 = 8.0;
    pub const M: f32 = 12.0;
    pub const L: f32 = 16.0;
    pub const XL: f32 = 24.0;
    pub const XXL: f32 = 32.0;
    /// Height of buttons, fields, segmented controls and pop-up buttons.
    pub const CONTROL: f32 = 24.0;
    /// Widest the page content grows before it is centred.
    pub const CONTENT_MAX: f32 = 680.0;
    /// Width of the right-aligned label column in forms.
    pub const LABEL: f32 = 150.0;
    pub const SIDEBAR: f32 = 220.0;
    /// Height of the unified title bar and toolbar.
    pub const TOOLBAR: f32 = 52.0;
    pub const SIDEBAR_COMPACT: f32 = 76.0;
}

/// Corner radii.
pub mod radius {
    pub const CONTROL: u8 = 5;
    pub const CARD: u8 = 7;
    pub const SHEET: u8 = 11;
}

pub fn corner(r: u8) -> CornerRadius {
    CornerRadius::same(r)
}

/// Elevation: shadows for surfaces that float above the window.
pub fn shadow_popover(p: &Palette) -> Shadow {
    Shadow { offset: [0, 6], blur: 20, spread: 0, color: p.shadow.gamma_multiply(0.8) }
}

pub fn shadow_sheet(p: &Palette) -> Shadow {
    Shadow { offset: [0, 16], blur: 40, spread: 0, color: p.shadow }
}

/// Type scale. Sizes are points; weights are separate font families.
pub mod typ {
    use super::*;

    pub const REGULAR: &str = "regular";
    pub const MEDIUM: &str = "medium";
    pub const SEMIBOLD: &str = "semibold";
    /// Headings of 20 points and more (SF Pro Display's role).
    pub const DISPLAY: &str = "display";

    fn f(size: f32, weight: &'static str) -> FontId {
        FontId::new(size, FontFamily::Name(weight.into()))
    }
    /// Page titles.
    pub fn title() -> FontId {
        f(22.0, DISPLAY)
    }
    /// Sheet titles, the result headline.
    pub fn title2() -> FontId {
        f(17.0, SEMIBOLD)
    }
    /// Emphasised body text, row titles that need weight.
    pub fn headline() -> FontId {
        f(13.0, SEMIBOLD)
    }
    pub fn body() -> FontId {
        f(13.0, REGULAR)
    }
    pub fn body_medium() -> FontId {
        f(13.0, MEDIUM)
    }
    /// Descriptions under row titles, secondary information.
    pub fn callout() -> FontId {
        f(11.5, REGULAR)
    }
    pub fn caption() -> FontId {
        f(11.0, REGULAR)
    }
    pub fn caption_medium() -> FontId {
        f(11.0, MEDIUM)
    }
    /// The page title in the toolbar.
    pub fn toolbar() -> FontId {
        f(15.0, SEMIBOLD)
    }
    /// Tracking for larger sizes (Inter and SF read better slightly tighter).
    pub fn tracking(size: f32) -> f32 {
        if size >= 20.0 {
            -0.45
        } else if size >= 16.0 {
            -0.25
        } else {
            0.0
        }
    }
}

/// Motion: durations (seconds) and easing curves. Movement is skipped and
/// fades shortened when reduced motion is on (see `motion.rs`).
pub mod motion {
    /// Hover and press feedback.
    pub const FAST: f32 = 0.12;
    /// Switches, segmented controls, reveals, selection.
    pub const BASE: f32 = 0.22;
    /// Page transitions, sheets, appearance changes.
    pub const SLOW: f32 = 0.32;

    /// Decelerating curve for things entering or settling.
    pub fn ease_out(t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        1.0 - (1.0 - t).powi(3)
    }

    /// Symmetric curve for state changes in place.
    pub fn ease_in_out(t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        if t < 0.5 {
            4.0 * t * t * t
        } else {
            1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
        }
    }
}
