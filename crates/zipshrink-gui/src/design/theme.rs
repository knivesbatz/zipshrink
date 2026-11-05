//! Appearance: picks light or dark, cross-fades the palette when it changes,
//! and maps the palette onto egui's own widgets (text fields, scroll bars,
//! tooltips, menus) so they match the custom components.

use super::motion;
use super::tokens::{self, corner, radius, Palette, DARK, LIGHT};
use eframe::egui::{self, Color32, Context, Id, Margin, Stroke, Theme, Visuals};

const PALETTE_ID: &str = "zs.palette";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub fn from_setting(v: u8) -> Self {
        match v {
            1 => Appearance::Light,
            2 => Appearance::Dark,
            _ => Appearance::System,
        }
    }
    pub fn to_setting(self) -> u8 {
        match self {
            Appearance::System => 0,
            Appearance::Light => 1,
            Appearance::Dark => 2,
        }
    }
}

/// The palette for this frame (set by [`frame`]).
pub fn pal(ctx: &Context) -> Palette {
    ctx.data(|d| d.get_temp(Id::unique(PALETTE_ID))).unwrap_or(LIGHT)
}

/// Run at the start of every frame: resolves the appearance, animates the
/// palette toward it and refreshes egui's visuals while it changes.
pub fn frame(ctx: &Context, appearance: Appearance) -> Palette {
    if cfg!(target_os = "macos") {
        // The window's own appearance (traffic lights, alerts, the open
        // panel) follows the app's choice.
        let id = Id::unique("zs.window-theme");
        if ctx.data(|d| d.get_temp::<Appearance>(id)) != Some(appearance) {
            ctx.data_mut(|d| d.insert_temp(id, appearance));
            ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(match appearance {
                Appearance::System => egui::SystemTheme::SystemDefault,
                Appearance::Light => egui::SystemTheme::Light,
                Appearance::Dark => egui::SystemTheme::Dark,
            }));
        }
    }
    let dark = match appearance {
        Appearance::Light => false,
        Appearance::Dark => true,
        // While the window's appearance is forced, winit stops reporting
        // system changes, so on macOS ask the system directly.
        #[cfg(target_os = "macos")]
        Appearance::System => crate::macos::system_dark(),
        #[cfg(not(target_os = "macos"))]
        Appearance::System => ctx.system_theme().unwrap_or(Theme::Light) == Theme::Dark,
    };
    let t = motion::fade(ctx, Id::unique("zs.appearance"), dark, tokens::motion::SLOW);
    let p = Palette::lerp(&LIGHT, &DARK, t);
    let prev: Option<Palette> = ctx.data(|d| d.get_temp(Id::unique(PALETTE_ID)));
    if prev != Some(p) {
        ctx.data_mut(|d| d.insert_temp(Id::unique(PALETTE_ID), p));
        let theme = if dark { Theme::Dark } else { Theme::Light };
        if ctx.options(|o| o.theme_preference) != theme.into() {
            ctx.set_theme(theme);
        }
        let v = visuals(&p, t > 0.5);
        ctx.set_visuals_of(Theme::Light, v.clone());
        ctx.set_visuals_of(Theme::Dark, v);
    }
    p
}

fn visuals(p: &Palette, dark: bool) -> Visuals {
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    // macOS draws text unhinted; hinted glyphs look out of place there.
    v.text_options.font_hinting = !cfg!(target_os = "macos");
    let ctl = corner(radius::CONTROL);
    let stroke = |c: Color32| Stroke::new(1.0, c);
    v.panel_fill = p.bg;
    v.window_fill = p.elevated;
    v.window_stroke = stroke(p.separator);
    v.window_corner_radius = corner(radius::SHEET);
    v.window_shadow = tokens::shadow_sheet(p);
    v.popup_shadow = tokens::shadow_popover(p);
    v.menu_corner_radius = corner(radius::CARD);
    v.extreme_bg_color = p.field;
    v.text_edit_bg_color = Some(p.field);
    v.faint_bg_color = p.fill;
    v.code_bg_color = p.fill;
    v.hyperlink_color = p.accent;
    v.warn_fg_color = p.orange;
    v.error_fg_color = p.red;
    v.override_text_color = None;
    v.selection.bg_fill = p.accent.gamma_multiply(0.32);
    v.selection.stroke = Stroke::new(1.0, p.on_accent);
    v.text_cursor.stroke = Stroke::new(1.6, p.accent);
    v.window_highlight_topmost = false;
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.surface;
    w.noninteractive.weak_bg_fill = p.surface;
    w.noninteractive.bg_stroke = stroke(p.separator);
    w.noninteractive.fg_stroke = stroke(p.text);
    w.noninteractive.corner_radius = ctl;
    for (wv, bg) in
        [(&mut w.inactive, p.fill), (&mut w.hovered, p.fill_hover), (&mut w.active, p.fill_pressed), (&mut w.open, p.fill_hover)]
    {
        wv.bg_fill = bg;
        wv.weak_bg_fill = bg;
        wv.bg_stroke = Stroke::NONE;
        wv.fg_stroke = stroke(p.text);
        wv.corner_radius = ctl;
        wv.expansion = 0.0;
    }
    w.hovered.bg_stroke = Stroke::NONE;
    v
}

/// Spacing and interaction defaults shared by every screen.
pub fn install_style(ctx: &Context) {
    ctx.all_styles_mut(|s| {
        let sp = &mut s.spacing;
        sp.item_spacing = egui::vec2(tokens::space::S, tokens::space::S);
        sp.button_padding = egui::vec2(12.0, 5.0);
        sp.interact_size = egui::vec2(28.0, tokens::space::CONTROL);
        sp.menu_margin = Margin::same(6);
        sp.window_margin = Margin::same(20);
        sp.tooltip_width = 300.0;
        sp.menu_spacing = 2.0;
        sp.scroll.bar_width = 8.0;
        sp.scroll.floating = true;
        s.interaction.tooltip_delay = 0.45;
        s.interaction.selectable_labels = false;
        s.animation_time = tokens::motion::BASE;
        s.visuals.text_cursor.blink = true;
    });
}
