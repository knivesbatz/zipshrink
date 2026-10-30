//! Millennium-era "futuristic" chrome, as utilities and media players drew it
//! around 1999-2001: glowing LCD/VFD readouts with unlit ghost segments, a
//! Winamp-style segmented level meter, LED activity lights, and a chrome
//! wordmark over a perspective grid with an orbit swoosh.
//!
//! Everything shown is a live reading of a real operation; nothing animates
//! on its own.

use crate::theme::{self, bevel, Bevel};
use eframe::egui::{self, pos2, vec2, Color32, Painter, Pos2, Rect, Stroke};
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use zipshrink_core::progress::{Phase, Stats};
use zipshrink_core::util::human_bytes;
use zipshrink_core::EntryStatus;

pub const LCD_BG: Color32 = Color32::from_rgb(4, 20, 16);
pub const LCD_ON: Color32 = Color32::from_rgb(80, 255, 190);
pub const LCD_OFF: Color32 = Color32::from_rgb(14, 48, 38);
pub const LCD_DIM: Color32 = Color32::from_rgb(40, 150, 112);

/// Live readings sampled from the running job.
pub struct Telemetry {
    /// Bytes per second, one sample every [`SAMPLE`].
    pub speeds: VecDeque<f32>,
    last: (Instant, u64),
    /// When each activity was last seen: compress, verify, commit, delete.
    pub activity: [Option<Instant>; 4],
}

const SAMPLE: Duration = Duration::from_millis(250);
const BARS: usize = 28;

impl Telemetry {
    pub fn new() -> Self {
        Telemetry { speeds: VecDeque::new(), last: (Instant::now(), 0), activity: [None; 4] }
    }

    /// Takes a throughput sample from real progress counters.
    pub fn sample(&mut self, s: &Stats) {
        let now = Instant::now();
        let dt = now.duration_since(self.last.0);
        if dt < SAMPLE {
            return;
        }
        let bytes = s.bytes_processed.max(s.verify_done);
        let v = bytes.saturating_sub(self.last.1) as f32 / dt.as_secs_f32();
        self.last = (now, bytes);
        self.speeds.push_back(v);
        while self.speeds.len() > BARS {
            self.speeds.pop_front();
        }
    }

    pub fn on_status(&mut self, st: &EntryStatus) {
        let i = match st {
            EntryStatus::Compressing => 0,
            EntryStatus::Verifying => 1,
            EntryStatus::Archived => 2,
            EntryStatus::Deleted => 3,
            _ => return,
        };
        self.activity[i] = Some(Instant::now());
    }

    fn lit(&self, i: usize, steady: bool) -> bool {
        steady || self.activity[i].is_some_and(|t| t.elapsed() < Duration::from_millis(220))
    }
}

/// Segment layout of a 7-segment digit: a, b, c, d, e, f, g.
fn segments(c: char) -> [bool; 7] {
    let m: u8 = match c {
        '0' => 0b0111111,
        '1' => 0b0000110,
        '2' => 0b1011011,
        '3' => 0b1001111,
        '4' => 0b1100110,
        '5' => 0b1101101,
        '6' => 0b1111101,
        '7' => 0b0000111,
        '8' => 0b1111111,
        '9' => 0b1101111,
        '-' => 0b1000000,
        _ => 0,
    };
    std::array::from_fn(|i| m & (1 << i) != 0)
}

/// Draws one 7-segment digit `w` wide and `h` tall, unlit segments faintly
/// visible like a real LCD.
pub fn seven_seg(p: &Painter, at: Pos2, w: f32, h: f32, c: char, on: Color32, off: Color32) {
    let t = 3.0;
    let half = ((h - t) / 2.0).floor();
    let segs = segments(c);
    let r = |x: f32, y: f32, ww: f32, hh: f32| Rect::from_min_size(at + vec2(x, y), vec2(ww, hh));
    let rects = [
        r(t, 0.0, w - 2.0 * t, t),                 // a
        r(w - t, t, t, half - t),                  // b
        r(w - t, half + t, t, h - half - 2.0 * t), // c
        r(t, h - t, w - 2.0 * t, t),               // d
        r(0.0, half + t, t, h - half - 2.0 * t),   // e
        r(0.0, t, t, half - t),                    // f
        r(t, half, w - 2.0 * t, t),                // g
    ];
    for (i, rect) in rects.iter().enumerate() {
        p.rect_filled(*rect, 0.0, if segs[i] { on } else { off });
    }
}

fn lcd_text(p: &Painter, at: Pos2, text: &str, color: Color32) {
    p.text(at.round(), egui::Align2::LEFT_TOP, text, theme::regular(), color);
}

/// The control-panel strip of the progress screen.
pub fn console(ui: &mut egui::Ui, s: &Stats, tel: &Telemetry, stopping: bool) {
    let h = 78.0;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), egui::Sense::hover());
    let p = ui.painter();
    let lcd_w = 250.0f32.min(rect.width() * 0.4);
    let led_w = 112.0;
    let lcd = Rect::from_min_size(rect.min, vec2(lcd_w, h));
    let meter = Rect::from_min_max(pos2(lcd.right() + 8.0, rect.top()), pos2(rect.right() - led_w - 8.0, rect.bottom()));
    let leds = Rect::from_min_max(pos2(rect.right() - led_w, rect.top()), rect.max);

    // LCD readout: big percentage plus three live values.
    p.rect_filled(lcd, 0.0, LCD_BG);
    bevel(p, lcd, Bevel::Field);
    let pct = s.percent().clamp(0.0, 100.0);
    let digits = format!("{:>3}", pct.floor() as u32);
    let mut x = lcd.left() + 10.0;
    for c in digits.chars() {
        seven_seg(
            p,
            pos2(x, lcd.top() + 9.0),
            18.0,
            34.0,
            if c == ' ' { '8' } else { c },
            if c == ' ' { LCD_OFF } else { LCD_ON },
            LCD_OFF,
        );
        x += 23.0;
    }
    // Decimal point, tenths digit, then the percent sign.
    p.rect_filled(Rect::from_min_size(pos2(x - 2.0, lcd.top() + 40.0), vec2(3.0, 3.0)), 0.0, LCD_ON);
    let tenth = char::from_digit((pct * 10.0) as u32 % 10, 10).unwrap_or('0');
    seven_seg(p, pos2(x + 4.0, lcd.top() + 25.0), 9.0, 18.0, tenth, LCD_ON, LCD_OFF);
    lcd_text(p, pos2(x + 4.0, lcd.top() + 8.0), "%", LCD_ON);
    let rx = lcd.left() + 118.0;
    let speed = tel.speeds.back().copied().unwrap_or(0.0);
    let rows = [
        ("RATE", if speed > 0.0 { format!("{}/s", human_bytes(speed as u64)) } else { "--".into() }),
        ("FREED", if s.delete_originals { human_bytes(s.reclaimed) } else { "OFF".into() }),
        ("FILES", format!("{}/{}", s.files_archived, s.total_files)),
    ];
    for (i, (k, v)) in rows.iter().enumerate() {
        let y = lcd.top() + 8.0 + i as f32 * 16.0;
        lcd_text(p, pos2(rx, y), k, LCD_DIM);
        lcd_text(p, pos2(rx + 42.0, y), v, LCD_ON);
    }
    let status = if stopping {
        "PAUSING"
    } else {
        match s.phase {
            Phase::Scanning => "SCAN",
            Phase::Recovering => "RECOVER",
            Phase::Compressing => "ONLINE",
            Phase::Finalizing => "SEAL",
            Phase::VerifyingArchive => "VERIFY",
            Phase::Cleanup => "CLEANUP",
            Phase::Done => "DONE",
        }
    };
    lcd_text(p, pos2(lcd.left() + 10.0, lcd.bottom() - 19.0), &format!(">> {status}"), LCD_DIM);

    // Segmented throughput meter in the style of a media player's level
    // display; one bar per quarter second, newest on the right.
    p.rect_filled(meter, 0.0, Color32::BLACK);
    bevel(p, meter, Bevel::Field);
    let label_h = 15.0;
    let inner = Rect::from_min_max(meter.min + vec2(5.0, 5.0 + label_h), meter.max - vec2(5.0, 5.0));
    let max = tel.speeds.iter().copied().fold(1.0f32, f32::max);
    let n = BARS as f32;
    let bw = ((inner.width() - (n - 1.0) * 2.0) / n).floor().max(2.0);
    let rows_n = ((inner.height() + 1.0) / 4.0).floor() as usize;
    let pad = BARS - tel.speeds.len().min(BARS);
    for i in 0..BARS {
        let v = if i < pad { 0.0 } else { tel.speeds[i - pad] / max };
        let lit = (v * rows_n as f32).round() as usize;
        let bx = inner.left() + i as f32 * (bw + 2.0);
        for r in 0..rows_n {
            let y = inner.bottom() - (r as f32 + 1.0) * 4.0 + 1.0;
            let f = r as f32 / rows_n as f32;
            let col = if f > 0.8 {
                Color32::from_rgb(255, 60, 40)
            } else if f > 0.55 {
                Color32::from_rgb(255, 210, 0)
            } else {
                Color32::from_rgb(40, 230, 70)
            };
            let c = if r < lit { col } else { Color32::from_rgb(18, 26, 22) };
            p.rect_filled(Rect::from_min_size(pos2(bx, y), vec2(bw, 3.0)), 0.0, c);
        }
    }
    p.text(
        pos2(meter.left() + 6.0, meter.top() + 4.0),
        egui::Align2::LEFT_TOP,
        "THROUGHPUT",
        theme::regular(),
        Color32::from_rgb(90, 140, 255),
    );
    if max > 1.0 {
        let peak = format!("PEAK {}/s", human_bytes(max as u64));
        p.text(
            pos2(meter.right() - 6.0, meter.top() + 4.0),
            egui::Align2::RIGHT_TOP,
            peak,
            theme::regular(),
            Color32::from_rgb(90, 140, 255),
        );
    }

    // LED activity lights, lit by the real events of the engine.
    p.rect_filled(leds, 0.0, theme::FACE);
    bevel(p, leds, Bevel::Thin);
    let names = ["COMPRESS", "VERIFY", "COMMIT", "DELETE"];
    let colors =
        [Color32::from_rgb(60, 255, 90), Color32::from_rgb(255, 200, 0), Color32::from_rgb(60, 200, 255), Color32::from_rgb(255, 60, 60)];
    let steady = [
        s.current.as_ref().is_some_and(|c| c.done > 0 && s.phase == Phase::Compressing),
        s.phase == Phase::VerifyingArchive,
        s.phase == Phase::Finalizing,
        false,
    ];
    for i in 0..4 {
        let y = leds.top() + 7.0 + i as f32 * 17.0;
        let c = pos2(leds.left() + 12.0, y + 6.0);
        let on = !stopping && tel.lit(i, steady[i]) && !(i == 3 && !s.delete_originals);
        let dark = Color32::from_rgb(colors[i].r() / 5, colors[i].g() / 5, colors[i].b() / 5);
        p.circle_filled(c, 5.0, Color32::from_rgb(40, 40, 40));
        p.circle_filled(c, 4.0, if on { colors[i] } else { dark });
        if on {
            p.rect_filled(Rect::from_min_size(c + vec2(-2.0, -2.0), vec2(2.0, 2.0)), 0.0, Color32::WHITE);
        }
        p.text(
            pos2(leds.left() + 23.0, y),
            egui::Align2::LEFT_TOP,
            names[i],
            theme::regular(),
            if i == 3 && !s.delete_originals { theme::GRAY_TEXT } else { theme::TEXT },
        );
    }
}

/// Chrome wordmark over a perspective grid with an orbit swoosh and a
/// "Year 2000 Compliant" seal, the way boxed software announced itself.
pub fn banner(ui: &mut egui::Ui, subtitle: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 58.0), egui::Sense::hover());
    let p = ui.painter().with_clip_rect(rect);
    // Night-sky gradient.
    let mut mesh = egui::Mesh::default();
    let top = Color32::from_rgb(8, 6, 40);
    let bottom = Color32::from_rgb(70, 10, 110);
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), Color32::from_rgb(0, 40, 90));
    mesh.colored_vertex(rect.right_bottom(), Color32::from_rgb(0, 90, 140));
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    p.add(egui::Shape::mesh(mesh));
    // Perspective grid floor; lines start just under the horizon so they
    // do not merge into a solid band.
    let horizon = rect.top() + 30.0;
    let vanish = pos2(rect.center().x + 120.0, horizon);
    let grid = Stroke::new(1.0, Color32::from_rgb(170, 40, 210));
    for k in -12..=12 {
        let end = pos2(vanish.x + k as f32 * 70.0, rect.bottom());
        let t = 5.0 / (end.y - vanish.y);
        let start = pos2(vanish.x + (end.x - vanish.x) * t, horizon + 5.0);
        p.line_segment([start, end], grid);
    }
    let mut y = horizon + 4.0;
    let mut step = 3.0;
    while y < rect.bottom() {
        p.line_segment([pos2(rect.left(), y.round()), pos2(rect.right(), y.round())], grid);
        step *= 1.7;
        y += step;
    }
    // Stars.
    for (i, (sx, sy)) in
        [(0.08, 0.2), (0.21, 0.55), (0.33, 0.15), (0.47, 0.35), (0.62, 0.12), (0.71, 0.4), (0.86, 0.22), (0.94, 0.5)].iter().enumerate()
    {
        let c = pos2((rect.left() + rect.width() * sx).round(), (rect.top() + 30.0 * sy).round());
        p.rect_filled(
            Rect::from_min_size(c, vec2(1.0, 1.0)),
            0.0,
            if i % 3 == 0 { Color32::from_rgb(160, 220, 255) } else { Color32::WHITE },
        );
    }
    // Orbit swoosh.
    let o = pos2(rect.left() + 100.0, rect.top() + 26.0);
    let mut prev: Option<Pos2> = None;
    for i in 0..=60 {
        let a = std::f32::consts::PI * (0.15 + 1.45 * i as f32 / 60.0);
        let q = pos2(o.x + 92.0 * a.cos(), o.y + 15.0 * a.sin() - 4.0 * (a * 0.5).sin());
        if let Some(pr) = prev {
            let w = 1.0 + 2.0 * (i as f32 / 60.0);
            p.line_segment([pr, q], Stroke::new(w, Color32::from_rgb(255, 170, 30)));
        }
        prev = Some(q);
    }
    // Chrome wordmark: dark drop shadow, then the letters in two bands.
    let word = "ZipShrink";
    let at = pos2(rect.left() + 26.0, rect.top() + 12.0);
    let font = egui::FontId::new(theme::PX * 2.0, egui::FontFamily::Name("bold".into()));
    p.text(at + vec2(2.0, 2.0), egui::Align2::LEFT_TOP, word, font.clone(), Color32::from_rgb(10, 0, 30));
    p.text(at + vec2(-1.0, 0.0), egui::Align2::LEFT_TOP, word, font.clone(), Color32::from_rgb(0, 200, 255));
    let galley = p.layout_no_wrap(word.to_string(), font.clone(), Color32::WHITE);
    let r = Rect::from_min_size(at, galley.size());
    let split = (r.top() + r.height() * 0.55).round();
    p.with_clip_rect(Rect::from_min_max(r.min, pos2(r.right(), split))).text(
        at,
        egui::Align2::LEFT_TOP,
        word,
        font.clone(),
        Color32::from_rgb(250, 252, 255),
    );
    p.with_clip_rect(Rect::from_min_max(pos2(r.left(), split), r.max)).text(
        at,
        egui::Align2::LEFT_TOP,
        word,
        font,
        Color32::from_rgb(150, 170, 200),
    );
    // A lens-flare sparkle on the last letter.
    let flare = pos2(r.right() - 3.0, r.top() + 3.0);
    p.line_segment([flare - vec2(9.0, 0.0), flare + vec2(9.0, 0.0)], Stroke::new(1.0, Color32::WHITE));
    p.line_segment([flare - vec2(0.0, 6.0), flare + vec2(0.0, 6.0)], Stroke::new(1.0, Color32::WHITE));
    let sub = pos2(r.left() + 2.0, r.bottom() + 1.0);
    p.text(sub + vec2(1.0, 1.0), egui::Align2::LEFT_TOP, subtitle, theme::regular(), Color32::from_rgb(10, 0, 30));
    p.text(sub, egui::Align2::LEFT_TOP, subtitle, theme::regular(), Color32::from_rgb(190, 240, 255));
    // "Year 2000 Compliant" seal.
    let seal = Rect::from_min_size(pos2(rect.right() - 92.0, rect.top() + 9.0), vec2(82.0, 40.0));
    p.rect_filled(seal, 0.0, Color32::from_rgb(255, 210, 40));
    theme::edge(&p, seal, Color32::from_rgb(255, 245, 180), Color32::from_rgb(140, 90, 0));
    p.text(pos2(seal.center().x, seal.top() + 4.0), egui::Align2::CENTER_TOP, "YEAR 2000", theme::bold(), Color32::from_rgb(90, 20, 0));
    p.text(pos2(seal.center().x, seal.top() + 20.0), egui::Align2::CENTER_TOP, "COMPLIANT", theme::bold(), Color32::from_rgb(90, 20, 0));
    bevel(ui.painter(), rect, Bevel::Field);
}
