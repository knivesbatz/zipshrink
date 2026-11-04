//! Self-updating: a weekly check of GitHub Releases, a background download
//! verified against the release's SHA-256, then an unobtrusive banner that
//! offers to restart into the new version. An update that is not installed
//! right away installs when ZipShrink closes. Copies that cannot install
//! updates themselves (portable, or on macOS not movable where they are, see
//! [`Method::Blocked`]) point to the release page and say why.

use super::toast::Toasts;
use crate::design::{self, motion, pal, space, typ, Tone};
use crate::worker;
use eframe::egui::{self, pos2, vec2, Context, Id, Rect, UiBuilder};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;
use zipshrink_update::{Blocker, Method, Release, Settings};

enum State {
    Idle,
    Checking { rx: Receiver<Result<Option<Release>, String>>, manual: bool },
    Downloading { rx: Receiver<Result<PathBuf, String>>, done: Arc<AtomicU64>, total: Arc<AtomicU64>, release: Release, manual: bool },
}

struct Ready {
    /// The installer (Windows) or the unpacked and checked app (macOS).
    path: PathBuf,
    release: Release,
    launched: bool,
}

pub struct Updates {
    state: State,
    status: String,
    ready: Option<Ready>,
    /// A newer release for copies that cannot install it themselves.
    page_only: Option<Release>,
    /// Why this copy cannot install `page_only` itself, when it could elsewhere.
    blocked: Option<Blocker>,
    /// An install that failed after the banner's Restart, shown as a toast by
    /// the next `poll` (the banner has no access to the app's toasts).
    failed: Option<(&'static str, String)>,
    banner: bool,
    next_check: f64,
}

/// First automatic check, seconds after start-up.
const FIRST_CHECK: f64 = 3.0;
/// How often an open window looks whether a week has passed.
const RECHECK: f64 = 3600.0;

impl Updates {
    pub fn new(_ctx: &Context) -> Self {
        Updates {
            state: State::Idle,
            status: String::new(),
            ready: None,
            page_only: None,
            blocked: None,
            failed: None,
            banner: false,
            next_check: FIRST_CHECK,
        }
    }

    pub fn busy(&self) -> bool {
        !matches!(self.state, State::Idle)
    }

    pub fn check(&mut self, ctx: &Context, manual: bool, settings: &mut Settings) {
        if self.busy() {
            return;
        }
        if let Some(r) = &self.ready {
            if manual {
                self.status = format!("ZipShrink {} is downloaded and ready to install.", r.release.version);
                self.banner = true;
            }
            return;
        }
        settings.last_check = zipshrink_update::now();
        settings.save();
        self.status = "Checking for updates…".into();
        self.state = State::Checking { rx: worker::check_update(ctx.clone()), manual };
    }

    pub fn status_line(&self) -> String {
        match &self.state {
            State::Checking { .. } => "Checking for updates…".into(),
            State::Downloading { done, total, release, .. } => {
                let version = &release.version;
                let (d, t) = (done.load(Ordering::Relaxed), total.load(Ordering::Relaxed));
                if t > 0 {
                    format!("Downloading ZipShrink {version}… {:.0}%", 100.0 * d as f64 / t as f64)
                } else {
                    format!("Downloading ZipShrink {version}…")
                }
            }
            State::Idle if !self.status.is_empty() => self.status.clone(),
            State::Idle => "Not checked yet in this session.".into(),
        }
    }

    pub fn sidebar_note(&self) -> Option<(String, String)> {
        if self.banner {
            return None;
        }
        if let Some(r) = &self.ready {
            return Some((format!("ZipShrink {}", r.release.version), "Installs when you quit".into()));
        }
        let detail = match self.blocked {
            Some(Blocker::Translocated | Blocker::ReadOnlyVolume) => "Move to Applications to update",
            _ => "Available to download",
        };
        self.page_only.as_ref().map(|r| (format!("ZipShrink {}", r.version), detail.into()))
    }

    pub fn show_banner(&mut self) {
        self.banner = true;
    }

    pub fn poll(&mut self, ctx: &Context, busy: bool, toasts: &mut Toasts, settings: &mut Settings) {
        if let Some((title, e)) = self.failed.take() {
            toasts.show(Tone::Error, title, &e);
        }
        let now = ctx.input(|i| i.time);
        if now >= self.next_check {
            self.next_check = now + RECHECK;
            if settings.check_due() && self.ready.is_none() && self.page_only.is_none() {
                self.check(ctx, false, settings);
            }
        }
        ctx.request_repaint_after(Duration::from_secs_f64((self.next_check - now).max(1.0)));
        match &self.state {
            State::Idle => {}
            State::Checking { rx, manual } => {
                let manual = *manual;
                if let Ok(r) = rx.try_recv() {
                    self.state = State::Idle;
                    let method = if matches!(r, Ok(Some(_))) { zipshrink_update::method() } else { Method::ReleasePage };
                    match r {
                        Ok(Some(rel)) if method.installs_itself() => {
                            self.status = format!("Downloading ZipShrink {}…", rel.version);
                            let done = Arc::new(AtomicU64::new(0));
                            let total = Arc::new(AtomicU64::new(0));
                            self.state = State::Downloading {
                                rx: worker::download_update(ctx.clone(), rel.clone(), done.clone(), total.clone()),
                                done,
                                total,
                                release: rel,
                                manual,
                            };
                        }
                        Ok(Some(rel)) => {
                            self.blocked = match method {
                                Method::Blocked(b) => Some(b),
                                _ => None,
                            };
                            self.status = match method {
                                // Preferences shows the whole reason.
                                Method::Blocked(_) => format!("ZipShrink {} is available. {}", rel.version, method.explain()),
                                _ => format!("ZipShrink {} is available.", rel.version),
                            };
                            self.page_only = Some(rel);
                            self.banner = true;
                        }
                        Ok(None) => {
                            self.status = "ZipShrink is up to date.".into();
                            if manual {
                                toasts.show(
                                    Tone::Success,
                                    "You're up to date",
                                    &format!("ZipShrink {} is the newest version.", zipshrink_update::CURRENT),
                                );
                            }
                        }
                        Err(e) => {
                            self.status = format!("Couldn't check for updates: {e}");
                            if manual {
                                toasts.show(Tone::Error, "Couldn't check for updates", &e);
                            }
                        }
                    }
                }
            }
            State::Downloading { rx, manual, .. } => {
                let manual = *manual;
                if let Ok(r) = rx.try_recv() {
                    let State::Downloading { release, .. } = std::mem::replace(&mut self.state, State::Idle) else { return };
                    match r {
                        Ok(path) => {
                            self.status = format!("ZipShrink {} is downloaded and verified.", release.version);
                            self.ready = Some(Ready { path, release, launched: false });
                            self.banner = true;
                        }
                        Err(e) => {
                            self.status = format!("The update couldn't be downloaded: {e}");
                            if manual {
                                toasts.show(Tone::Error, "The update couldn't be downloaded", &e);
                            }
                        }
                    }
                } else {
                    ctx.request_repaint_after(Duration::from_millis(200));
                }
            }
        }
        let _ = busy;
    }

    fn install_now(&mut self, ctx: &Context) {
        let Some(path) = self.ready.as_ref().map(|r| r.path.clone()) else { return };
        match zipshrink_update::launch_installer(&path, true) {
            Ok(()) => {
                if let Some(r) = &mut self.ready {
                    r.launched = true;
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) if cfg!(target_os = "macos") => {
                // The new app could not be put in place (for example macOS's
                // App Management said no): offer the release page instead.
                if let Some(r) = self.ready.take() {
                    self.status = format!("ZipShrink {} couldn't be installed: {e}", r.release.version);
                    self.page_only = Some(r.release);
                    self.banner = true;
                }
                self.failed = Some(("The update couldn't be installed", e.to_string()));
            }
            Err(e) => self.failed = Some(("The update couldn't start", e.to_string())),
        }
    }

    /// Called as the window closes: installs a downloaded update silently.
    pub fn install_on_close(&mut self) {
        if let Some(r) = &mut self.ready {
            if !r.launched {
                r.launched = zipshrink_update::launch_installer(&r.path, false).is_ok();
            }
        }
    }

    /// The update notice: a Big Sur alert-style notification at the top
    /// right, with its actions stacked on the right edge.
    pub fn banner(&mut self, ctx: &Context, area: Rect, busy: bool) {
        let visible = self.banner && (self.ready.is_some() || self.page_only.is_some());
        let t = motion::fade(ctx, Id::unique("zs.update-banner"), visible, design::tokens::motion::SLOW);
        if t <= 0.0 {
            return;
        }
        let p = pal(ctx);
        let w = 344.0_f32.min(area.width() - 2.0 * space::M);
        let h = 74.0;
        let dx = motion::travel(ctx, t, 40.0);
        let rect = Rect::from_min_size(pos2(area.right() - w - space::M + dx, area.top() + space::TOOLBAR + space::S), vec2(w, h));
        let mut later = false;
        let mut install = false;
        let mut open_page = None;
        let (title, body, action) = match (&self.ready, &self.page_only) {
            (Some(r), _) => (
                format!("ZipShrink {} Is Ready", r.release.version),
                if busy {
                    "It installs when you quit, after the current job.".to_string()
                } else {
                    "Restart now to update, or it installs when you quit.".to_string()
                },
                if busy { None } else { Some("Restart") },
            ),
            (None, Some(r)) => {
                let body = self.blocked.map_or("Download it from the release page.", |b| b.advice());
                (format!("ZipShrink {} Is Available", r.version), body.to_string(), Some("Download"))
            }
            _ => return,
        };
        egui::Area::new(Id::unique("zs.update-area")).order(egui::Order::Tooltip).fixed_pos(rect.min).interactable(t > 0.5).show(
            ctx,
            |ui| {
                ui.multiply_opacity(t);
                let (r, _) = ui.allocate_exact_size(rect.size(), egui::Sense::hover());
                let edge = if design::is_dark(&p) { egui::Color32::from_white_alpha(24) } else { egui::Color32::from_black_alpha(22) };
                ui.painter().add(design::tokens::shadow_popover(&p).as_shape(r, 13.0));
                ui.painter().rect_filled(r, 13.0, p.elevated.gamma_multiply(0.97));
                ui.painter().rect_stroke(r, 13.0, egui::Stroke::new(1.0, edge), egui::StrokeKind::Inside);
                let actions_w = 86.0;
                let split = r.right() - actions_w;
                ui.painter().line_segment([pos2(split, r.top()), pos2(split, r.bottom())], egui::Stroke::new(1.0, edge));
                let icon = Rect::from_min_size(pos2(r.left() + 12.0, r.center().y - 18.0), vec2(36.0, 36.0));
                design::chrome::paint_app_icon(ui, icon);
                let x = icon.right() + 10.0;
                let title = design::elide_end(ui, &title, typ::headline(), split - 10.0 - x);
                ui.painter().text(pos2(x, r.top() + 20.0), egui::Align2::LEFT_CENTER, title, typ::headline(), p.text);
                let mut text_ui = ui.new_child(
                    UiBuilder::new().max_rect(Rect::from_min_max(pos2(x, r.top() + 30.0), pos2(split - 10.0, r.bottom() - 6.0))),
                );
                design::para(&mut text_ui, body, typ::callout(), p.text);
                // Actions: one or two halves of the right edge.
                let halves: Vec<(Rect, &str)> = match action {
                    Some(a) => vec![
                        (Rect::from_min_max(pos2(split, r.top()), pos2(r.right(), r.center().y)), a),
                        (Rect::from_min_max(pos2(split, r.center().y), r.max), "Later"),
                    ],
                    None => vec![(Rect::from_min_max(pos2(split, r.top()), r.max), "OK")],
                };
                if halves.len() == 2 {
                    ui.painter().line_segment([pos2(split, r.center().y), pos2(r.right(), r.center().y)], egui::Stroke::new(1.0, edge));
                }
                for (k, (hr, label)) in halves.iter().enumerate() {
                    let resp = ui.interact(*hr, Id::unique(("zs.update-action", k)), egui::Sense::click());
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, *label));
                    if resp.hovered() || resp.is_pointer_button_down_on() {
                        let a = if resp.is_pointer_button_down_on() { 0.16 } else { 0.08 };
                        ui.painter().with_clip_rect(*hr).rect_filled(r, 13.0, p.text.gamma_multiply(a));
                    }
                    ui.painter().text(hr.center(), egui::Align2::CENTER_CENTER, *label, typ::body_medium(), p.text);
                    if resp.has_focus() {
                        design::focus_ring(ui, hr.shrink(3.0), 6.0);
                    }
                    if resp.clicked() {
                        match *label {
                            "Restart" => install = true,
                            "Download" => open_page = self.page_only.as_ref().map(|r| r.page.clone()),
                            _ => later = true,
                        }
                    }
                }
            },
        );
        if later {
            self.banner = false;
        }
        if let Some(u) = open_page {
            zipshrink_update::open_url(&u);
            self.banner = false;
        }
        if install {
            self.install_now(ctx);
        }
    }
}
