//! Line icons in the spirit of SF Symbols, drawn as vectors on a 20x20 grid
//! so they stay sharp at every display scale and take the theme's colours.

use eframe::egui::{epaint::PathShape, pos2, vec2, Color32, Painter, Pos2, Rect, Shape, Stroke};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Archive,
    Shield,
    Resume,
    Gear,
    Folder,
    Doc,
    Plus,
    Close,
    Check,
    Warning,
    Info,
    Pause,
    Download,
}

struct Pen<'a> {
    p: &'a Painter,
    o: Pos2,
    s: f32,
    stroke: Stroke,
    color: Color32,
}

impl Pen<'_> {
    fn pt(&self, x: f32, y: f32) -> Pos2 {
        self.o + vec2(x, y) * self.s
    }
    fn line(&self, pts: &[(f32, f32)]) {
        let pts: Vec<Pos2> = pts.iter().map(|&(x, y)| self.pt(x, y)).collect();
        self.p.add(Shape::Path(PathShape::line(pts, self.stroke)));
    }
    fn closed(&self, pts: &[(f32, f32)]) {
        let pts: Vec<Pos2> = pts.iter().map(|&(x, y)| self.pt(x, y)).collect();
        self.p.add(Shape::Path(PathShape::closed_line(pts, self.stroke)));
    }
    fn circle(&self, x: f32, y: f32, r: f32) {
        self.p.circle_stroke(self.pt(x, y), r * self.s, self.stroke);
    }
    fn dot(&self, x: f32, y: f32, r: f32) {
        self.p.circle_filled(self.pt(x, y), r * self.s, self.color);
    }
    fn arc(&self, cx: f32, cy: f32, r: f32, from: f32, to: f32) -> Vec<(f32, f32)> {
        let n = 24;
        (0..=n)
            .map(|i| {
                let a = from + (to - from) * i as f32 / n as f32;
                (cx + r * a.cos(), cy + r * a.sin())
            })
            .collect()
    }
    fn rounded_rect(&self, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) {
        let rect = Rect::from_min_max(self.pt(x0, y0), self.pt(x1, y1));
        self.p.rect_stroke(rect, r * self.s, self.stroke, eframe::egui::StrokeKind::Middle);
    }
}

/// Draws `icon` centred in `rect` (square or not) in `color`.
pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let side = rect.width().min(rect.height());
    let s = side / 20.0;
    let o = rect.center() - vec2(10.0, 10.0) * s;
    let width = (1.5 * s).clamp(1.1, 2.4);
    let pen = Pen { p: painter, o, s, stroke: Stroke::new(width, color), color };
    use Icon::*;
    match icon {
        Archive => {
            pen.rounded_rect(2.5, 3.5, 17.5, 7.5, 1.2);
            pen.line(&[(3.5, 7.5), (3.5, 15.5), (4.8, 16.8), (15.2, 16.8), (16.5, 15.5), (16.5, 7.5)]);
            pen.line(&[(8.0, 11.0), (12.0, 11.0)]);
        }
        Shield => {
            pen.closed(&[(10.0, 2.2), (16.5, 4.6), (16.5, 9.6), (14.6, 14.0), (10.0, 17.6), (5.4, 14.0), (3.5, 9.6), (3.5, 4.6)]);
            pen.line(&[(7.0, 9.9), (9.2, 12.1), (13.2, 7.9)]);
        }
        Resume => {
            // Open at the top, with the head travelling clockwise.
            let (a0, a1) = (-50f32.to_radians(), 250f32.to_radians());
            let a = pen.arc(10.0, 10.6, 6.4, a0, a1);
            pen.line(&a);
            let (ex, ey) = *a.last().unwrap_or(&(10.0, 4.0));
            let dir = vec2(-a1.sin(), a1.cos());
            let back = |deg: f32| {
                let r = deg.to_radians();
                let d = vec2(dir.x * r.cos() - dir.y * r.sin(), dir.x * r.sin() + dir.y * r.cos());
                (ex - d.x * 3.6, ey - d.y * 3.6)
            };
            pen.line(&[back(42.0), (ex + dir.x * 0.6, ey + dir.y * 0.6), back(-42.0)]);
        }
        Gear => {
            // gearshape: eight flat-topped teeth around a ring.
            let mut pts = Vec::new();
            let teeth = 8;
            for k in 0..teeth {
                let base = k as f32 * std::f32::consts::TAU / teeth as f32;
                let step = std::f32::consts::TAU / teeth as f32;
                for (da, r) in [(-0.30, 5.9), (-0.17, 7.7), (0.17, 7.7), (0.30, 5.9)] {
                    let a = base + da * step * 1.6;
                    pts.push((10.0 + r * a.cos(), 10.0 + r * a.sin()));
                }
            }
            pen.closed(&pts);
            pen.circle(10.0, 10.0, 2.6);
        }
        Folder => {
            pen.closed(&[
                (2.5, 5.0),
                (2.5, 15.2),
                (3.6, 16.3),
                (16.4, 16.3),
                (17.5, 15.2),
                (17.5, 7.8),
                (16.4, 6.7),
                (9.6, 6.7),
                (8.0, 4.3),
                (3.6, 4.3),
            ]);
            pen.line(&[(2.5, 8.8), (17.5, 8.8)]);
        }
        Doc => {
            pen.closed(&[(4.5, 3.0), (4.5, 17.0), (15.5, 17.0), (15.5, 7.2), (11.3, 3.0)]);
            pen.line(&[(11.3, 3.0), (11.3, 7.2), (15.5, 7.2)]);
        }
        Plus => {
            pen.line(&[(10.0, 4.0), (10.0, 16.0)]);
            pen.line(&[(4.0, 10.0), (16.0, 10.0)]);
        }
        Close => {
            pen.line(&[(5.5, 5.5), (14.5, 14.5)]);
            pen.line(&[(14.5, 5.5), (5.5, 14.5)]);
        }
        Check => pen.line(&[(4.5, 10.5), (8.3, 14.2), (15.5, 6.2)]),
        Warning => {
            pen.closed(&[(10.0, 2.8), (18.0, 16.6), (2.0, 16.6)]);
            pen.line(&[(10.0, 7.6), (10.0, 11.6)]);
            pen.dot(10.0, 14.0, 0.95);
        }
        Info => {
            pen.circle(10.0, 10.0, 7.5);
            pen.dot(10.0, 6.6, 0.95);
            pen.line(&[(10.0, 9.2), (10.0, 14.0)]);
        }
        Pause => {
            let r = |x0: f32| {
                painter.rect_filled(Rect::from_min_max(pen.pt(x0, 4.5), pen.pt(x0 + 3.0, 15.5)), 1.0 * s, color);
            };
            r(5.5);
            r(11.5);
        }
        Download => {
            pen.line(&[(10.0, 3.0), (10.0, 12.0)]);
            pen.line(&[(6.5, 8.6), (10.0, 12.1), (13.5, 8.6)]);
            pen.line(&[(3.5, 12.5), (3.5, 16.5), (16.5, 16.5), (16.5, 12.5)]);
        }
    }
}

/// A filled circular badge with a white glyph: success, warning and error
/// marks next to results.
pub fn badge(painter: &Painter, center: Pos2, radius: f32, fill: Color32, icon: Icon, glyph: Color32) {
    painter.circle_filled(center, radius, fill);
    let r = Rect::from_center_size(center, vec2(radius * 1.25, radius * 1.25));
    let s = r.width() / 20.0;
    let pen = Pen { p: painter, o: r.center() - vec2(10.0, 10.0) * s, s, stroke: Stroke::new((2.2 * s).max(1.4), glyph), color: glyph };
    match icon {
        Icon::Check => pen.line(&[(4.0, 10.5), (8.3, 14.6), (16.0, 6.2)]),
        Icon::Close => {
            pen.line(&[(5.5, 5.5), (14.5, 14.5)]);
            pen.line(&[(14.5, 5.5), (5.5, 14.5)]);
        }
        Icon::Warning | Icon::Info => {
            let (y0, y1, d) = if icon == Icon::Warning { (4.0, 11.5, 15.5) } else { (8.5, 16.0, 4.5) };
            pen.line(&[(10.0, y0), (10.0, y1)]);
            pen.dot(10.0, d, 1.3);
        }
        Icon::Pause => {
            pen.line(&[(7.0, 5.0), (7.0, 15.0)]);
            pen.line(&[(13.0, 5.0), (13.0, 15.0)]);
        }
        other => paint(painter, r, other, glyph),
    }
}

/// An animated check mark: `t` from 0 to 1 draws the stroke on.
pub fn check_stroke(painter: &Painter, rect: Rect, color: Color32, width: f32, t: f32) {
    let s = rect.width() / 20.0;
    let o = rect.center() - vec2(10.0, 10.0) * s;
    let pts = [pos2(5.0, 10.6), pos2(8.6, 14.0), pos2(15.2, 6.6)].map(|p| o + p.to_vec2() * s);
    let l1 = (pts[1] - pts[0]).length();
    let l2 = (pts[2] - pts[1]).length();
    let mut len = (l1 + l2) * t.clamp(0.0, 1.0);
    let mut out = vec![pts[0]];
    for w in pts.windows(2) {
        let seg = (w[1] - w[0]).length();
        if len >= seg {
            out.push(w[1]);
            len -= seg;
        } else {
            out.push(w[0] + (w[1] - w[0]) * (len / seg));
            break;
        }
    }
    if out.len() > 1 {
        painter.add(Shape::Path(PathShape::line(out, Stroke::new(width, color))));
    }
}
