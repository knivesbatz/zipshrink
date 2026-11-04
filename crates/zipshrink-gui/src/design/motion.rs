//! Animation helpers. Every animation in the app goes through these, so
//! reduced motion is honoured everywhere: movement becomes instant and fades
//! become quick cross-fades.

use super::tokens::motion::{ease_in_out, ease_out, SLOW};
use eframe::egui::{Context, Id};

const REDUCED_ID: &str = "zs.reduce_motion";
const REDUCED_FADE: f32 = 0.08;

pub fn set_reduced(ctx: &Context, reduced: bool) {
    ctx.data_mut(|d| d.insert_temp(Id::unique(REDUCED_ID), reduced));
}

pub fn reduced(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp(Id::unique(REDUCED_ID))).unwrap_or(false)
}

/// Opacity-style transition of a boolean state (0 = off, 1 = on), eased.
pub fn fade(ctx: &Context, id: Id, on: bool, secs: f32) -> f32 {
    let secs = if reduced(ctx) { secs.min(REDUCED_FADE) } else { secs };
    ctx.animate_bool_with_time_and_easing(id, on, secs, ease_in_out)
}

/// A transition that moves something (a thumb, a knob, a highlight).
/// Instant when motion is reduced.
pub fn slide(ctx: &Context, id: Id, on: bool, secs: f32) -> f32 {
    let secs = if reduced(ctx) { 0.0 } else { secs };
    ctx.animate_bool_with_time_and_easing(id, on, secs, ease_in_out)
}

/// Follows a moving target value (positions, progress), easing toward it.
pub fn follow(ctx: &Context, id: Id, target: f32, secs: f32) -> f32 {
    let secs = if reduced(ctx) { 0.0 } else { secs };
    ctx.animate_value_with_time(id, target, secs)
}

/// Entrance of something identified by `key` (a page, a sheet, a result):
/// returns 0 -> 1 over `secs` after `key` last changed for `id`.
pub fn appear(ctx: &Context, id: Id, key: u64, secs: f32) -> f32 {
    let now = ctx.input(|i| i.time);
    let start = ctx.data_mut(|d| {
        let slot = d.get_temp_mut_or_insert_with(id, || (key, now));
        if slot.0 != key {
            *slot = (key, now);
        }
        slot.1
    });
    let secs = if reduced(ctx) { REDUCED_FADE } else { secs };
    let t = ((now - start) as f32 / secs).clamp(0.0, 1.0);
    if t < 1.0 {
        ctx.request_repaint();
    }
    ease_out(t)
}

/// Offset (points) to apply alongside an `appear` value; zero when motion
/// is reduced, so things only fade.
pub fn travel(ctx: &Context, t: f32, distance: f32) -> f32 {
    if reduced(ctx) {
        0.0
    } else {
        (1.0 - t) * distance
    }
}

/// Default page-transition duration.
pub const PAGE: f32 = SLOW;

/// Whether Windows' "Animation effects" setting is off, or macOS's
/// "Reduce motion" is on.
pub fn system_prefers_reduced() -> bool {
    #[cfg(target_os = "macos")]
    {
        crate::macos::reduce_motion()
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION};
        let mut on: i32 = 1;
        // SAFETY: SPI_GETCLIENTAREAANIMATION writes a BOOL to the pointer.
        let ok = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&mut on as *mut i32).cast(), 0) };
        ok != 0 && on == 0
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        false
    }
}
