//! 16x16 pixel-art icons, painted one rectangle per pixel.

use eframe::egui::{self, pos2, vec2, Color32, Pos2, Rect};

pub type Art = [&'static str; 16];

fn color(c: u8) -> Option<Color32> {
    Some(match c {
        b'K' => Color32::BLACK,
        b'W' => Color32::WHITE,
        b'G' => Color32::from_rgb(192, 192, 192),
        b'L' => Color32::from_rgb(223, 223, 223),
        b'D' => Color32::from_rgb(128, 128, 128),
        b'Y' => Color32::from_rgb(255, 255, 0),
        b'O' => Color32::from_rgb(128, 128, 0),
        b'R' => Color32::from_rgb(255, 0, 0),
        b'M' => Color32::from_rgb(128, 0, 0),
        b'B' => Color32::from_rgb(0, 0, 128),
        b'C' => Color32::from_rgb(0, 128, 128),
        b'T' => Color32::from_rgb(0, 255, 255),
        _ => return None,
    })
}

pub fn paint(p: &egui::Painter, at: Pos2, art: Art, scale: f32) {
    let at = at.round();
    for (y, row) in art.iter().enumerate() {
        let bytes = row.as_bytes();
        let mut x = 0;
        while x < bytes.len() {
            let Some(c) = color(bytes[x]) else {
                x += 1;
                continue;
            };
            let start = x;
            while x < bytes.len() && bytes[x] == bytes[start] {
                x += 1;
            }
            let r =
                Rect::from_min_size(pos2(at.x + start as f32 * scale, at.y + y as f32 * scale), vec2((x - start) as f32 * scale, scale));
            p.rect_filled(r, 0.0, c);
        }
    }
}

pub const APP: Art = [
    "................",
    ".KKKKKKKKKKKKKK.",
    ".KWWWWWWWWWWWDK.",
    ".KWGGGGKKGGGGDK.",
    ".KWGGGGKYGGGGDK.",
    ".KWGGGGYKGGGGDK.",
    ".KWGGGGKYGGGGDK.",
    ".KWGGGGYKGGGGDK.",
    ".KWGGGKKKKGGGDK.",
    ".KWGGGKRRKGGGDK.",
    ".KWGGGKKKKGGGDK.",
    ".KWGGGGGGGGGGDK.",
    ".KWGGGGGGGGGGDK.",
    ".KDDDDDDDDDDDDK.",
    ".KKKKKKKKKKKKKK.",
    "................",
];

pub const FOLDER: Art = [
    "................",
    "................",
    "..KKKKK.........",
    ".KYWYYYK........",
    "KYWYYYYYKKKKKKK.",
    "KWYYYYYYYYYYYYKD",
    "KYYYYYYYYYYYYYKD",
    "KYYYYYYYYYYYYYKD",
    "KYYYYYYYYYYYYYKD",
    "KYYYYYYYYYYYYYKD",
    "KYYYYYYYYYYYYYKD",
    "KYYYYYYYYYYYYYKD",
    "KOOOOOOOOOOOOOKD",
    "KKKKKKKKKKKKKKKD",
    ".DDDDDDDDDDDDDDD",
    "................",
];

pub const WARNING: Art = [
    ".......KK.......",
    "......KYYK......",
    "......KYYK......",
    ".....KYYYYK.....",
    ".....KYKKYK.....",
    "....KYYKKYYK....",
    "....KYYKKYYK....",
    "...KYYYKKYYYK...",
    "...KYYYKKYYYK...",
    "..KYYYYKKYYYYK..",
    "..KYYYYYYYYYYK..",
    ".KYYYYYKKYYYYYK.",
    ".KYYYYYKKYYYYYK.",
    "KYYYYYYYYYYYYYYK",
    "KKKKKKKKKKKKKKKK",
    "................",
];

pub const ERROR: Art = [
    ".....KKKKKK.....",
    "...KKRRRRRRKK...",
    "..KRRRRRRRRRRK..",
    ".KRRWRRRRRRWRRK.",
    ".KRWWWRRRRWWWRK.",
    "KRRRWWWRRWWWRRRK",
    "KRRRRWWWWWWRRRRK",
    "KRRRRRWWWWRRRRRK",
    "KRRRRRWWWWRRRRRK",
    "KRRRRWWWWWWRRRRK",
    "KRRRWWWRRWWWRRRK",
    ".KRWWWRRRRWWWRK.",
    ".KRRWRRRRRRWRRK.",
    "..KRRRRRRRRRRK..",
    "...KKRRRRRRKK...",
    ".....KKKKKK.....",
];

pub const INFO: Art = [
    ".....KKKKKK.....",
    "...KKWWWWWWKK...",
    "..KWWWWWWWWWWK..",
    ".KWWWWWBBWWWWWK.",
    ".KWWWWWBBWWWWWK.",
    "KWWWWWWWWWWWWWWK",
    "KWWWWWBBBWWWWWWK",
    "KWWWWWWBBWWWWWWK",
    "KWWWWWWBBWWWWWWK",
    "KWWWWWWBBWWWWWWK",
    "KWWWWWWBBWWWWWWK",
    ".KWWWWBBBBWWWWK.",
    ".KWWWWWWWWWWWWK.",
    "..KWWWWWWWWWWK..",
    "...KKWWWWWWKK...",
    ".....KKKKKK.....",
];

pub const CHECK: Art = [
    "................",
    "................",
    "..............K.",
    ".............KK.",
    "............KKK.",
    "...........KKK..",
    "..K.......KKK...",
    "..KK.....KKK....",
    "..KKK...KKK.....",
    "...KKK.KKK......",
    "....KKKKK.......",
    ".....KKK........",
    "......K.........",
    "................",
    "................",
    "................",
];
