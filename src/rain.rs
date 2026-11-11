//! Windows 95 style win animation: foundation cards launch one at a time,
//! fall, bounce off the bottom and leave never-erased trails.
//!
//! Unlike the Qt version, physics run on a fixed 16 ms step driven by real
//! elapsed time, so a slow or busy frame doesn't slow the rain down or
//! change the trail spacing.

use crate::gfx::{Image, Rect};

const REFERENCE_CARD_WIDTH: f32 = 71.0;
const GRAVITY: f32 = 0.6;
const BOUNCE: f32 = -0.75;
const STEP: f32 = 0.016;
const MAX_STEPS_PER_CARD: u32 = 700;
const MAX_STEPS_PER_FRAME: u32 = 8;

pub struct Rng(u64);
impl Rng {
    pub fn new() -> Rng {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64);
        Rng(t | 1)
    }
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

struct Flying {
    card: u8,
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    steps: u32,
}

pub struct Rain {
    pub buf: Image,
    queue: Vec<(u8, f32, f32)>, // card and launch position
    next: usize,
    cur: Option<Flying>,
    pub finished: bool,
    acc: f32,
    rng: Rng,
}

impl Rain {
    /// `snapshot` is the finished board; `queue` lists cards and where
    /// they currently sit.
    pub fn new(snapshot: Image, queue: Vec<(u8, f32, f32)>) -> Rain {
        Rain {
            buf: snapshot,
            queue,
            next: 0,
            cur: None,
            finished: false,
            acc: 0.0,
            rng: Rng::new(),
        }
    }

    /// The board was re-laid out (resize/zoom): new backdrop and launch
    /// positions. The card in flight keeps going from where it is.
    pub fn regrab(&mut self, snapshot: Image, queue: Vec<(u8, f32, f32)>) {
        self.buf = snapshot;
        self.queue = queue;
    }

    fn launch(&mut self, cw: i32) {
        let Some(&(card, x, y)) = self.queue.get(self.next) else {
            self.cur = None;
            self.finished = true;
            return;
        };
        self.next += 1;
        let scale = cw as f32 / REFERENCE_CARD_WIDTH;
        let mut vx = (3 + self.rng.below(5)) as f32 * scale;
        if self.rng.below(2) == 1 {
            vx = -vx;
        }
        let vy = -(self.rng.below(5) as f32) * scale;
        self.cur = Some(Flying {
            card,
            x,
            y,
            vx,
            vy,
            steps: 0,
        });
    }

    /// Advance by `dt` seconds, stamping cards into the buffer. Returns the
    /// area that changed.
    pub fn update(&mut self, dt: f32, cards: &[Image]) -> Rect {
        let mut dirty = Rect::default();
        if self.finished {
            return dirty;
        }
        let Some(img) = cards.first() else {
            return dirty;
        };
        let (cw, ch) = (img.w, img.h);
        self.acc = (self.acc + dt).min(STEP * MAX_STEPS_PER_FRAME as f32);
        let scale = cw as f32 / REFERENCE_CARD_WIDTH;
        let (w, h) = (self.buf.w as f32, self.buf.h as f32);
        while self.acc >= STEP && !self.finished {
            self.acc -= STEP;
            if self.cur.is_none() {
                self.launch(cw);
                continue;
            }
            let f = self.cur.as_mut().unwrap();
            f.x += f.vx;
            f.y += f.vy;
            f.vy += GRAVITY * scale;
            if f.y + ch as f32 >= h {
                f.y = h - ch as f32;
                f.vy *= BOUNCE;
            }
            let (x, y) = (f.x.round() as i32, f.y.round() as i32);
            let r = Rect::new(x, y, cw, ch);
            self.buf
                .blit(&cards[f.card as usize], x, y, self.buf.rect(), 0);
            dirty = dirty.union(&r);
            f.steps += 1;
            if f.x > w || f.x + (cw as f32) < 0.0 || f.steps > MAX_STEPS_PER_CARD {
                self.launch(cw);
            }
        }
        dirty
    }
}
