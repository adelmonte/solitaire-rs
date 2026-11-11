//! Card layout (a port of KCardScene's relayout for the Klondike piles) and
//! the animated card sprites.

use crate::game::{FOUND, NPILES, STOCK, State, TAB, WASTE};
use crate::gfx::Rect;
use std::time::Instant;

const HSPACING: f32 = 1.0 / 6.0 + 0.02;
const MARGIN: f32 = 0.15;
const SPREAD: f32 = 0.33;
const TAB_Y: f32 = 1.25;
const USED_W: f32 = 6.0 * (1.0 + HSPACING) + 1.0;
const USED_H: f32 = TAB_Y + 1.0 + SPREAD * 5.0;

#[derive(Clone, Copy, Default)]
pub struct Layout {
    pub cw: i32,
    pub ch: i32,
    pub pile: [(f32, f32); NPILES],
    /// Height in card units below the tableau row's first card.
    tab_room: f32,
}

fn layout_pos(p: usize) -> (f32, f32) {
    match p {
        STOCK => (0.0, 0.0),
        WASTE => (1.0 + HSPACING, 0.0),
        _ if p < TAB => ((3 + p - FOUND) as f32 * (1.0 + HSPACING), 0.0),
        _ => ((p - TAB) as f32 * (1.0 + HSPACING), TAB_Y),
    }
}

/// Card width that fits the whole layout into the view (KCardScene's
/// auto-fit, used when the user hasn't picked a size).
pub fn fit_width(vw: i32, vh: i32, aspect: f32) -> i32 {
    let by_w = vw as f32 / (USED_W + 2.0 * MARGIN);
    let by_h = vh as f32 * aspect / (USED_H + 2.0 * MARGIN);
    (by_w.min(by_h) as i32).clamp(20, 2000)
}

impl Layout {
    pub fn new(vw: i32, vh: i32, cw: i32, ch: i32) -> Layout {
        let (cwf, chf) = (cw as f32, ch as f32);
        let used = USED_W * cwf;
        // Centre horizontally; when the cards are zoomed wider than the
        // window, pin to the left edge instead of clipping both sides.
        let xoff = ((vw as f32 - used) / 2.0).max(0.0);
        let yoff = MARGIN * chf;
        let content_h = (vh as f32 - 2.0 * MARGIN * chf) / chf;
        let mut pile = [(0.0, 0.0); NPILES];
        for (p, slot) in pile.iter_mut().enumerate() {
            let (lx, ly) = layout_pos(p);
            *slot = ((xoff + lx * cwf).round(), (yoff + ly * chf).round());
        }
        Layout {
            cw,
            ch,
            pile,
            tab_room: content_h - (TAB_Y + 1.0),
        }
    }

    pub fn pile_rect(&self, p: usize) -> Rect {
        Rect::new(
            self.pile[p].0 as i32,
            self.pile[p].1 as i32,
            self.cw,
            self.ch,
        )
    }

    /// Where every card of `p` sits: (x, y) per card.
    pub fn positions(&self, st: &State, p: usize, draw_n: usize, out: &mut Vec<(f32, f32)>) {
        out.clear();
        let cards = &st.piles[p];
        let (px, py) = self.pile[p];
        if p == WASTE {
            let n = cards.len();
            let mut x = 0.0;
            for i in 0..n {
                out.push((px + x * self.cw as f32, py));
                if i + draw_n >= n {
                    x += SPREAD;
                }
            }
        } else if p >= TAB {
            let mut y = 0.0;
            let mut ys = Vec::with_capacity(cards.len());
            for c in cards {
                ys.push(y);
                y += if c.up { SPREAD } else { SPREAD * 0.6 };
            }
            let max_y = ys.last().copied().unwrap_or(0.0);
            let scale = if max_y > 0.0 {
                (self.tab_room / max_y).clamp(0.0, 1.0)
            } else {
                1.0
            };
            for y in ys {
                out.push((px, py + (y * scale * self.ch as f32).round()));
            }
        } else {
            out.extend(std::iter::repeat_n((px, py), cards.len()));
        }
    }
}

#[derive(Clone, Copy)]
pub struct Anim {
    from: (f32, f32),
    flip_from: f32,
    start: Instant,
    dur: f32,
    flip_factor: f32,
    pub raised: bool,
}

#[derive(Clone, Copy)]
pub struct Sprite {
    /// Resting place and face.
    pub tx: f32,
    pub ty: f32,
    pub up: bool,
    pub z: i32,
    pub pile: usize,
    pub anim: Option<Anim>,
    /// Current displayed position and flippedness (0 = back, 1 = face).
    pub x: f32,
    pub y: f32,
    pub flip: f32,
}

impl Default for Sprite {
    fn default() -> Self {
        Sprite {
            tx: 0.0,
            ty: 0.0,
            up: false,
            z: 0,
            pile: STOCK,
            anim: None,
            x: 0.0,
            y: 0.0,
            flip: 0.0,
        }
    }
}

fn ease(t: f32) -> f32 {
    // Gentle ease-out; reads smoother than KPat's linear slide.
    1.0 - (1.0 - t) * (1.0 - t)
}

impl Sprite {
    /// Move towards a new resting place over `ms` milliseconds, beginning
    /// at `start` (the sprite waits where it is until then).
    pub fn go(
        &mut self,
        tx: f32,
        ty: f32,
        up: bool,
        ms: f32,
        raised: bool,
        start: Instant,
        cw: i32,
        ch: i32,
    ) {
        self.tx = tx;
        self.ty = ty;
        self.up = up;
        let target_flip = if up { 1.0 } else { 0.0 };
        let moving =
            (tx - self.x).abs() > 1.0 || (ty - self.y).abs() > 1.0 || self.flip != target_flip;
        if ms <= 0.0 || !moving {
            self.x = tx;
            self.y = ty;
            self.flip = target_flip;
            self.anim = None;
            return;
        }
        let (dx, dy) = (tx - self.x, ty - self.y);
        let diag2 = (cw * cw + ch * ch) as f32;
        self.anim = Some(Anim {
            from: (self.x, self.y),
            flip_from: self.flip,
            start,
            dur: ms / 1000.0,
            flip_factor: ((dx * dx + dy * dy) / diag2).sqrt().max(1.0),
            raised: raised || self.anim.is_some_and(|a| a.raised),
        });
    }

    /// Advance; returns true while still animating.
    pub fn tick(&mut self, now: Instant) -> bool {
        let Some(a) = self.anim else { return false };
        let t = (now.saturating_duration_since(a.start).as_secs_f32() / a.dur).min(1.0);
        let e = ease(t);
        self.x = a.from.0 + (self.tx - a.from.0) * e;
        self.y = a.from.1 + (self.ty - a.from.1) * e;
        let target = if self.up { 1.0 } else { 0.0 };
        let ft = (e * a.flip_factor).min(1.0);
        self.flip = a.flip_from + (target - a.flip_from) * ft;
        if t >= 1.0 {
            self.finish();
            return false;
        }
        true
    }

    pub fn finish(&mut self) {
        self.x = self.tx;
        self.y = self.ty;
        self.flip = if self.up { 1.0 } else { 0.0 };
        self.anim = None;
    }

    pub fn raised(&self) -> bool {
        self.anim.is_some_and(|a| a.raised)
    }
}
