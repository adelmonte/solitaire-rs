//! Built-in fallbacks so the binary runs with nothing else installed: a
//! Windows 95 flavoured card deck generated as SVG, and KPat's Clean Green
//! background theme.

use std::fmt::Write;

pub const THEME_SVG: &[u8] = include_bytes!("../assets/cleangreen.svg");

const W: f32 = 71.0;
const H: f32 = 96.0;
const RED: &str = "#d00000";
const BLACK: &str = "#000000";

const HEART: &str = "M50 96C22 72 0 54 0 29C0 11 13 0 28 0C40 0 47 8 50 17C53 8 60 0 72 0C87 0 100 11 100 29C100 54 78 72 50 96Z";
const DIAMOND: &str = "M50 0L86 50L50 100L14 50Z";
const SPADE: &str = "M50 0C62 22 100 40 100 63C100 79 89 88 75 88C65 88 57 83 53 76C54 88 58 95 67 100L33 100C42 95 46 88 47 76C43 83 35 88 25 88C11 88 0 79 0 63C0 40 38 22 50 0Z";
const CLUB: &str = "M50 2A23 23 0 1 1 49.9 2ZM24 36A23 23 0 1 1 23.9 36ZM76 36A23 23 0 1 1 75.9 36ZM40 44L60 44L60 66L40 66ZM47 62C46 80 41 92 32 100L68 100C59 92 54 80 53 62Z";

/// Stroke glyphs in a 10 x 14 box.
fn rank_glyph(rank: u8) -> &'static str {
    match rank {
        1 => "M0.5 14L5 0.5L9.5 14M2.2 9.3L7.8 9.3",
        2 => "M0.8 3.8C0.8 -0.4 9.4 -0.4 9.4 3.8C9.4 7.4 0.6 10 0.6 13.4L9.6 13.4",
        3 => "M1 0.8L9 0.8L4.4 5.8C10.4 5.4 10.6 13.6 5 13.6C2.6 13.6 1.2 12.8 0.6 11.6",
        4 => "M7 13.6L7 0.6L0.6 9.8L9.8 9.8",
        5 => "M9 0.8L1.6 0.8L1 6.4C3.6 4.8 9.6 5.2 9.6 9.4C9.6 14.6 2.4 14.4 0.6 12",
        6 => "M8.8 1.6C4.2 -1 0.6 2.8 0.6 8.6C0.6 15 9.6 14.8 9.6 9.4C9.6 4.6 1.6 4.6 0.8 9",
        7 => "M0.6 0.8L9.6 0.8L4 13.6",
        8 => {
            "M5 6.6C0.8 6.6 1 0.6 5 0.6C9 0.6 9.2 6.6 5 6.6C0.2 6.6 0.2 13.6 5 13.6C9.8 13.6 9.8 6.6 5 6.6Z"
        }
        9 => "M1.2 12.4C5.8 15 9.4 11.2 9.4 5.4C9.4 -0.6 0.6 -0.4 0.6 4.6C0.6 9.4 8.4 9.4 9.2 5",
        10 => {
            "M0 2.6L2.4 0.6L2.4 13.6M7.6 0.6C3.6 0.6 3.6 13.6 7.6 13.6C11.6 13.6 11.6 0.6 7.6 0.6Z"
        }
        11 => "M3 0.8L9.6 0.8M7.2 0.8L7.2 9.8C7.2 14.8 0.8 14.6 0.8 10.4",
        12 => "M5 0.6C-0.8 0.6 -0.8 13.4 5 13.4C10.8 13.4 10.8 0.6 5 0.6ZM6 9.2L9.8 13.8",
        _ => "M1 0.6L1 13.6M9.6 0.6L1 9M3.8 6.4L9.6 13.6",
    }
}

fn suit_path(suit: u8) -> &'static str {
    [CLUB, DIAMOND, HEART, SPADE][suit as usize]
}

/// A suit symbol of `size` centred on (cx, cy), optionally upside down.
fn pip(s: &mut String, suit: u8, cx: f32, cy: f32, size: f32, flip: bool) {
    let k = size / 100.0;
    let rot = if flip {
        format!(" rotate(180 {cx} {cy})")
    } else {
        String::new()
    };
    let _ = write!(
        s,
        r#"<path transform="{rot} translate({} {}) scale({k})" d="{}"/>"#,
        cx - size / 2.0,
        cy - size / 2.0,
        suit_path(suit)
    );
}

fn glyph(s: &mut String, rank: u8, x: f32, y: f32, k: f32, color: &str) {
    let _ = write!(
        s,
        r#"<path transform="translate({x} {y}) scale({k})" d="{}" fill="none" stroke="{color}" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round"/>"#,
        rank_glyph(rank)
    );
}

fn pip_layout(rank: u8) -> Vec<(f32, f32)> {
    let (l, c, r) = (21.0, 35.5, 50.0);
    let (t, m, b) = (21.0, 48.0, 75.0);
    let q = |f: f32| t + (b - t) * f;
    match rank {
        2 => vec![(c, t), (c, b)],
        3 => vec![(c, t), (c, m), (c, b)],
        4 => vec![(l, t), (r, t), (l, b), (r, b)],
        5 => vec![(l, t), (r, t), (c, m), (l, b), (r, b)],
        6 => vec![(l, t), (r, t), (l, m), (r, m), (l, b), (r, b)],
        7 => vec![(l, t), (r, t), (c, q(0.25)), (l, m), (r, m), (l, b), (r, b)],
        8 => vec![
            (l, t),
            (r, t),
            (c, q(0.25)),
            (l, m),
            (r, m),
            (c, q(0.75)),
            (l, b),
            (r, b),
        ],
        9 => vec![
            (l, t),
            (r, t),
            (l, q(1.0 / 3.0)),
            (r, q(1.0 / 3.0)),
            (c, m),
            (l, q(2.0 / 3.0)),
            (r, q(2.0 / 3.0)),
            (l, b),
            (r, b),
        ],
        _ => vec![
            (l, t),
            (r, t),
            (c, q(1.0 / 6.0)),
            (l, q(1.0 / 3.0)),
            (r, q(1.0 / 3.0)),
            (l, q(2.0 / 3.0)),
            (r, q(2.0 / 3.0)),
            (c, q(5.0 / 6.0)),
            (l, b),
            (r, b),
        ],
    }
}

fn card(s: &mut String, id: u8) {
    let rank = id % 13 + 1;
    let suit = id / 13;
    let color = if suit == 1 || suit == 2 { RED } else { BLACK };
    let _ = write!(s, r#"<g id="{}">"#, crate::assets::element_name(id));
    let _ = write!(
        s,
        r##"<rect x="0.5" y="0.5" width="{}" height="{}" rx="4" fill="#fff" stroke="#000"/>"##,
        W - 1.0,
        H - 1.0
    );
    let _ = write!(s, r#"<g fill="{color}">"#);
    match rank {
        1 => pip(
            s,
            suit,
            W / 2.0,
            H / 2.0,
            if suit == 3 { 34.0 } else { 26.0 },
            false,
        ),
        11..=13 => {
            let _ = write!(
                s,
                r##"<rect x="13" y="12" width="45" height="72" fill="#f6e7a6" stroke="{color}" stroke-width="1"/>"##
            );
            let _ = write!(
                s,
                r##"<rect x="15.5" y="14.5" width="40" height="67" fill="none" stroke="#3050b0" stroke-width="0.8"/>"##
            );
            glyph(s, rank, 26.0, 20.0, 1.9, color);
            pip(s, suit, W / 2.0, 64.0, 18.0, false);
        }
        _ => {
            for (x, y) in pip_layout(rank) {
                pip(s, suit, x, y, 13.0, y > H / 2.0 + 1.0);
            }
        }
    }
    // Corner indices, the second one rotated into the opposite corner.
    for rot in [false, true] {
        if rot {
            let _ = write!(s, r#"<g transform="rotate(180 {} {})">"#, W / 2.0, H / 2.0);
        }
        glyph(s, rank, if rank == 10 { 2.2 } else { 3.0 }, 3.5, 0.6, color);
        pip(s, suit, 6.0, 16.5, 7.0, false);
        if rot {
            s.push_str("</g>");
        }
    }
    s.push_str("</g></g>");
}

pub fn deck_svg() -> String {
    let mut s = String::with_capacity(96 * 1024);
    let _ = write!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="0 0 {} {}">"#,
        W * 13.0,
        H * 5.0,
        W * 13.0,
        H * 5.0
    );
    s.push_str(
        r##"<defs><pattern id="weave" width="6" height="6" patternUnits="userSpaceOnUse"><rect width="6" height="6" fill="#0030a0"/><path d="M0 0L6 6M6 0L0 6" stroke="#6fa0ff" stroke-width="0.9"/><circle cx="3" cy="3" r="0.9" fill="#ffffff"/></pattern></defs>"##,
    );
    for id in 0..52u8 {
        let _ = write!(
            s,
            r#"<g transform="translate({} {})">"#,
            (id % 13) as f32 * W,
            (id / 13) as f32 * H
        );
        card(&mut s, id);
        s.push_str("</g>");
    }
    let _ = write!(
        s,
        r##"<g transform="translate(0 {})"><g id="back"><rect x="0.5" y="0.5" width="{}" height="{}" rx="4" fill="#fff" stroke="#000"/><rect x="4" y="4" width="{}" height="{}" fill="url(#weave)" stroke="#001060" stroke-width="0.8"/></g></g>"##,
        H * 4.0,
        W - 1.0,
        H - 1.0,
        W - 8.0,
        H - 8.0
    );
    s.push_str("</svg>");
    s
}
