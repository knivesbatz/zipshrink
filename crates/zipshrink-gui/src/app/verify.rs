//! The Verify page: read back ZIP archives and check every entry.

use super::compress::{column, shortcut};
use super::fmt;
use super::App;
use crate::design::{self, icons, pal, space, typ, Button, Icon, Tone};
use crate::worker;
use eframe::egui::{self, vec2, Align, Color32, Id, Layout, Sense, Ui};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use zipshrink_core::verify::ArchiveCheck;

type Results = Vec<(PathBuf, Result<ArchiveCheck, String>)>;

#[derive(Default)]
pub struct Verify {
    rx: Option<Receiver<Results>>,
    progress: Arc<AtomicU64>,
    total: u64,
    current: Vec<PathBuf>,
    results: Results,
    run: u64,
}

fn uncompressed_total(path: &Path) -> u64 {
    std::fs::File::open(path)
        .ok()
        .and_then(|f| zipshrink_core::format::read_central_directory(&f).ok())
        .map(|cd| cd.iter().map(|e| e.uncompressed).sum())
        .unwrap_or(0)
}

impl Verify {
    pub fn running(&self) -> bool {
        self.rx.is_some()
    }

    pub fn start(&mut self, ctx: &egui::Context, paths: Vec<PathBuf>) {
        if self.running() || paths.is_empty() {
            return;
        }
        let progress = Arc::new(AtomicU64::new(0));
        self.total = paths.iter().map(|p| uncompressed_total(p)).sum();
        self.current = paths.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx2 = ctx.clone();
        let pr = progress.clone();
        std::thread::spawn(move || {
            let mut out = Vec::new();
            for p in paths {
                let rx = worker::verify(ctx2.clone(), p.clone(), pr.clone());
                out.push((p, rx.recv().unwrap_or_else(|_| Err("verification stopped".into()))));
            }
            let _ = tx.send(out);
            ctx2.request_repaint();
        });
        self.progress = progress;
        self.rx = Some(rx);
        self.run += 1;
    }

    pub fn pick(&mut self, ctx: &egui::Context) {
        if let Some(ps) = rfd::FileDialog::new().set_title("Verify Archives").add_filter("ZIP archive", &["zip"]).pick_files() {
            self.start(ctx, ps);
        }
    }

    pub fn poll(&mut self) {
        if let Some(rx) = &self.rx {
            if let Ok(r) = rx.try_recv() {
                self.results = r;
                self.rx = None;
            }
        }
    }
}

pub fn page(app: &mut App, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let p = pal(&ctx);
    let v = &mut app.verify;
    let mut pick = false;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(space::XL);
        column(ui, |ui| {
            if v.running() {
                ui.horizontal_top(|ui| {
                    let (ir, _) = ui.allocate_exact_size(vec2(48.0, 48.0), Sense::hover());
                    design::chrome::paint_app_icon(ui, ir);
                    ui.add_space(space::S);
                    ui.vertical(|ui| {
                        let done = v.progress.load(Ordering::Relaxed);
                        let names: Vec<String> = v.current.iter().map(|p| fmt::file_name(p)).collect();
                        ui.add_space(2.0);
                        design::truncated(ui, format!("Verifying “{}”", names.join("”, “")), typ::body_medium(), p.text);
                        ui.add_space(6.0);
                        let f = if v.total > 0 { Some(done as f32 / v.total as f32) } else { None };
                        design::progress_bar(ui, Id::unique("verify-bar").with(v.run), f, 6.0);
                        ui.add_space(6.0);
                        design::secondary(ui, format!("{} of {} decompressed and checked", fmt::size(done), fmt::size(v.total)));
                    });
                });
                ctx.request_repaint_after(std::time::Duration::from_millis(150));
            } else if v.results.is_empty() {
                let h = (ui.available_height() - space::XL).clamp(280.0, 420.0);
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
                super::dashed_rect(ui.painter(), rect, 12.0, egui::Stroke::new(1.5, p.outline));
                let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(space::XL)).layout(Layout::top_down(Align::Center)));
                inner.add_space((h - 260.0).max(0.0) / 2.0 + 20.0);
                design::empty_state(&mut inner, Icon::Shield, "Drop ZIP Archives Here", "Every entry is read back and decompressed, and its checksums compared. Any ZIP file can be checked. Nothing is changed.", |ui| {
                    design::centered_row(ui, 150.0, |ui| {
                        if Button::secondary("Choose Archives…").show(ui).on_hover_text(shortcut(ui, "Ctrl+O")).clicked() {
                            pick = true;
                        }
                    });
                });
                if resp.double_clicked() {
                    pick = true;
                }
            }
            if !v.results.is_empty() && !v.running() {
                let ok = v.results.iter().filter(|(_, r)| matches!(r, Ok(c) if c.ok())).count();
                let (tone, title) = if ok == v.results.len() {
                    (Tone::Success, if ok == 1 { "The archive is intact.".to_string() } else { format!("All {ok} archives are intact.") })
                } else {
                    (Tone::Error, format!("{} of {} archives have problems.", v.results.len() - ok, v.results.len()))
                };
                design::notice(ui, tone, &title, None);
                ui.add_space(space::M);
                for (path, r) in &v.results {
                    result_row(ui, path, r);
                    design::separator(ui);
                }
                ui.horizontal(|ui| {
                    if Button::secondary("Verify Other Archives…").show(ui).clicked() {
                        pick = true;
                    }
                });
            }
            ui.add_space(space::XXL);
        });
    });
    if pick {
        v.pick(&ctx);
    }
}

fn result_row(ui: &mut Ui, path: &Path, r: &Result<ArchiveCheck, String>) {
    let p = pal(ui.ctx());
    ui.horizontal_top(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
        icons::paint(ui.painter(), rect, Icon::Archive, p.text_secondary);
        let (color, icon) = match r {
            Ok(c) if c.ok() => (p.green, Icon::Check),
            _ => (p.red, Icon::Close),
        };
        icons::badge(ui.painter(), rect.right_bottom() - vec2(3.0, 3.0), 7.5, color, icon, Color32::WHITE);
        ui.add_space(space::S);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            design::text(ui, fmt::file_name(path), typ::body_medium(), p.text);
            design::truncated(ui, fmt::parent(path), typ::caption(), p.text_secondary);
            ui.add_space(4.0);
            match r {
                Ok(c) if c.ok() => {
                    let crc = if c.crc_only > 0 { format!(", {} CRC-32 only", fmt::group(c.crc_only)) } else { String::new() };
                    design::secondary(
                        ui,
                        format!(
                            "Intact: {}, {}. {} matched SHA-256 and CRC-32{crc}.",
                            fmt::count(c.entries, "entry", "entries"),
                            fmt::size(c.bytes),
                            fmt::count(c.sha_checked, "file", "files")
                        ),
                    );
                }
                Ok(c) => {
                    design::para(
                        ui,
                        format!("{} found:", fmt::count(c.problems.len() as u64, "problem", "problems")),
                        typ::callout(),
                        p.red,
                    );
                    for pr in c.problems.iter().take(10) {
                        design::secondary(ui, format!("• {pr}"));
                    }
                    if c.problems.len() > 10 {
                        design::secondary(ui, format!("…and {} more.", c.problems.len() - 10));
                    }
                }
                Err(e) => {
                    design::para(ui, e, typ::callout(), p.red);
                }
            }
        });
    });
}
