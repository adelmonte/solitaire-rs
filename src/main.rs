//! solitaire-rs: Windows 95 style Klondike solitaire, one static binary.
//! Reads KDE card decks and KPat background themes.

mod assets;
mod board;
mod builtin;
mod config;
mod font;
mod game;
mod gfx;
mod rain;
mod solver;
mod ui;
mod wayland;

use assets::{Deck, Entry, Theme};
use board::{Layout, Sprite};
use config::Settings;
use game::{Action, FOUND, History, NPILES, STOCK, State, TAB, WASTE};
use gfx::{Image, Rect};
use rain::{Rain, Rng};
use solver::{Verdict, Worker};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use ui::{Cmd, Dialog, Item, Menu};
use wayland::{ALT, CTRL, Ev, SHIFT, Window, keys};

// KPat's timings (speeds.h), in ms.
const DURATION_MOVE: f32 = 230.0;
const DURATION_DEAL: f32 = 1500.0;
const DURATION_DEMO: f32 = 300.0;
const DURATION_FANCYSHOW: f32 = 350.0;
const DURATION_AUTODROP: f32 = 250.0;
const DURATION_AUTODROP_MIN: f32 = 80.0;
const AUTODROP_SPEEDUP: f32 = 0.90;
const TIME_BETWEEN_MOVES: f32 = 250.0;
// Cards drawn together leave the stock one after another.
const DRAW_STAGGER: f32 = 60.0;
const DOUBLE_CLICK_MS: u32 = 400;
const SOLVER_LIMIT: u64 = 400_000;
const FRAME_MS: f32 = 1000.0 / 60.0;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Now,
    Ms(f32),
    Deal,
}

#[derive(Clone, Copy, PartialEq)]
enum Auto {
    Idle,
    Drop,
    Finish,
    Demo,
}

#[derive(PartialEq)]
enum DialogKind {
    Help,
    DealNumber,
}

struct Drag {
    pile: usize,
    idx: usize,
    press: (i32, i32),
    started: bool,
    base: Vec<(f32, f32)>,
    target: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Default)]
struct Drawn {
    rect: Rect,
    img: u8,
    dim: u8,
    squash: i32,
    layer: u8,
}

struct App {
    win: Window,
    set: Settings,
    decks: Vec<Entry>,
    themes: Vec<Entry>,
    deck: Deck,
    theme: Theme,
    cards: Vec<Image>,
    cw: i32,
    ch: i32,
    pile_img: Option<Image>,
    pile_sel: Option<Image>,
    bg: Image,
    bg_due: Option<Instant>,
    bg_gen: u64,
    bg_tx: mpsc::Sender<(u64, Image)>,
    bg_rx: mpsc::Receiver<(u64, Image)>,
    fb: Image,
    ui: i32,

    st: State,
    deal: u32,
    hist: History,
    sprites: [Sprite; 52],
    lay: Layout,
    drag: Option<Drag>,
    stock_press: bool,
    peek: Option<u8>,
    hl: u64,
    hl_pile: Option<usize>,

    solver: Worker,
    generation: u64,
    verdict: Option<Verdict>,
    need_solve: bool,
    hint_queued: bool,
    status: String,

    auto: Auto,
    next_step: Instant,
    drop_speed: f32,
    demo_draws: usize,

    won: bool,
    rain: Option<Rain>,
    rain_dismissed: bool,
    rain_regrab: Option<Instant>,
    last_frame: Instant,

    menu: Option<Menu>,
    dialog: Option<(DialogKind, Dialog)>,
    dirty: Rect,
    drawn: [Drawn; 52],
    last_press: (u32, i32, i32),
    mouse: (i32, i32),
    rng: Rng,
    quit: bool,
}

fn zbase(p: usize) -> i32 {
    if p == STOCK { -100 } else { 0 }
}

fn find_index(list: &[Entry], id: &str) -> Option<usize> {
    list.iter().position(|e| e.id == id)
}

impl App {
    fn view(&self) -> Rect {
        let sb = if self.set.status_bar {
            ui::status_height(self.ui)
        } else {
            0
        };
        Rect::new(0, 0, self.win.w, (self.win.h - sb).max(1))
    }

    fn draw_n(&self) -> usize {
        if self.set.draw_one { 1 } else { 3 }
    }

    fn invalidate(&mut self, r: Rect) {
        self.dirty = self.dirty.union(&r);
    }

    fn invalidate_all(&mut self) {
        self.dirty = self.fb.rect();
    }

    // ---------------------------------------------------------------- assets

    fn render_cards(&mut self) {
        self.ch = ((self.cw as f32 / self.deck.aspect).round() as i32).max(1);
        self.cards = self.deck.render_all(self.cw, self.ch);
        let svg = &self.theme.svg;
        self.pile_img = svg.render("pile", self.cw, self.ch);
        self.pile_sel = svg.render("pile_selected", self.cw, self.ch);
    }

    fn render_bg_now(&mut self) {
        let v = self.view();
        self.bg = self
            .theme
            .svg
            .render("background", v.w, v.h)
            .unwrap_or_else(|| {
                let mut i = Image::new(v.w, v.h);
                i.fill(i.rect(), 0xff008000);
                i
            });
        self.bg_due = None;
        self.bg_gen += 1;
    }

    /// Stretch the old background now, render the real one in the
    /// background once resizing settles.
    fn bg_resized(&mut self) {
        let v = self.view();
        if self.bg.w != v.w || self.bg.h != v.h {
            self.bg = self.bg.scaled(v.w, v.h);
            self.bg_due = Some(Instant::now() + Duration::from_millis(120));
            self.bg_gen += 1;
        }
    }

    fn start_bg_job(&mut self) {
        self.bg_due = None;
        let v = self.view();
        let svg = Arc::clone(&self.theme.svg);
        let tx = self.bg_tx.clone();
        let g = self.bg_gen;
        std::thread::spawn(move || {
            if let Some(img) = svg.render("background", v.w, v.h) {
                let _ = tx.send((g, img));
            }
        });
    }

    fn card_width_for_view(&self) -> i32 {
        let v = self.view();
        if self.set.card_width > 0 {
            self.set.card_width
        } else {
            board::fit_width(v.w, v.h, self.deck.aspect)
        }
    }

    fn relayout(&mut self, mode: Mode) {
        // A drag can't survive the cards moving under it.
        if self.drag.take().is_some_and(|d| d.started) {
            self.hl = 0;
            self.hl_pile = None;
        }
        let v = self.view();
        self.lay = Layout::new(v.w, v.h, self.cw, self.ch);
        self.sync(mode);
        self.invalidate_all();
        if self.rain.is_some() {
            self.rain_regrab = Some(Instant::now() + Duration::from_millis(200));
        }
    }

    fn set_card_width(&mut self, w: i32) {
        let w = w.clamp(20, 2000);
        if w != self.cw {
            self.cw = w;
            self.render_cards();
        }
        self.relayout(Mode::Now);
    }

    fn zoom(&mut self, factor: f32) {
        let mut w = (self.cw as f32 * factor).round() as i32;
        if w == self.cw {
            w += if factor > 1.0 { 1 } else { -1 };
        }
        let w = w.clamp(20, 2000);
        self.set.card_width = w;
        self.set.save();
        self.set_card_width(w);
    }

    fn load_deck(&mut self, i: usize) {
        match Deck::load(&self.decks[i]) {
            Ok(d) => {
                self.deck = d;
                self.set.deck = self.decks[i].id.clone();
                self.set.save();
                self.ch = 0;
                self.render_cards();
                self.relayout(Mode::Now);
            }
            Err(e) => self.set_status(format!("Could not load card deck: {e}")),
        }
    }

    fn load_theme(&mut self, i: usize) {
        match Theme::load(&self.themes[i]) {
            Ok(t) => {
                self.theme = t;
                self.set.theme = self.themes[i].id.clone();
                self.set.save();
                self.render_bg_now();
                self.render_cards();
                self.invalidate_all();
                if self.rain.is_some() {
                    self.rain_regrab = Some(Instant::now());
                }
            }
            Err(e) => self.set_status(format!("Could not load theme: {e}")),
        }
    }

    // ----------------------------------------------------------------- game

    fn title(&self) {
        let mode = if self.set.draw_one {
            "Draw 1"
        } else {
            "Draw 3"
        };
        let _ = self.win.set_title(&format!(
            "Klondike ({mode}) - Deal {} - Solitaire",
            self.deal
        ));
    }

    fn set_status(&mut self, s: String) {
        if self.status != s {
            self.status = s;
            self.invalidate_status();
        }
    }

    fn invalidate_status(&mut self) {
        if self.set.status_bar {
            let v = self.view();
            self.invalidate(Rect::new(
                0,
                v.bottom(),
                self.win.w,
                self.win.h - v.bottom(),
            ));
        }
    }

    fn stop_auto(&mut self) {
        if self.auto != Auto::Idle {
            self.auto = Auto::Idle;
            self.drop_speed = 1.0;
        }
    }

    fn clear_hint(&mut self) {
        self.hint_queued = false;
        if self.hl != 0 || self.hl_pile.is_some() {
            self.hl = 0;
            self.hl_pile = None;
            self.invalidate_all();
        }
    }

    fn snap(&mut self) {
        for s in self.sprites.iter_mut() {
            if s.anim.is_some() {
                s.finish();
            }
        }
    }

    fn animating(&self) -> bool {
        self.sprites.iter().any(|s| s.anim.is_some())
    }

    fn start_new(&mut self, deal: u32) {
        self.stop_auto();
        self.clear_hint();
        self.drag = None;
        self.peek = None;
        self.rain = None;
        self.rain_dismissed = false;
        self.won = false;
        self.deal = deal.clamp(1, i32::MAX as u32);
        self.st = State::deal(self.deal);
        self.hist = History::new();
        let (sx, sy) = self.lay.pile[STOCK];
        for s in self.sprites.iter_mut() {
            *s = Sprite {
                x: sx,
                y: sy,
                tx: sx,
                ty: sy,
                ..Sprite::default()
            };
        }
        self.sync(Mode::Deal);
        self.invalidate_all();
        self.title();
        self.state_changed(None, false);
    }

    /// Bring every sprite to where the state says it belongs.
    fn sync(&mut self, mode: Mode) {
        let now = Instant::now();
        let v = self.view();
        let deal_speed = ((v.w * v.w + v.h * v.h) as f32).sqrt() / DURATION_DEAL;
        let mut pos = Vec::with_capacity(24);
        let mut drawn = 0;
        let drawing = self.st.piles[WASTE]
            .iter()
            .any(|c| self.sprites[c.id as usize].pile == STOCK);
        for p in 0..NPILES {
            self.lay.positions(&self.st, p, &mut pos);
            for (i, c) in self.st.piles[p].iter().enumerate() {
                let s = &mut self.sprites[c.id as usize];
                let (tx, ty) = pos[i];
                let new_pile = s.pile != p;
                let from_stock = p == WASTE && s.pile == STOCK;
                let to_stock = p == STOCK && s.pile == WASTE;
                s.z = zbase(p) + i as i32;
                s.pile = p;
                if s.tx == tx && s.ty == ty && s.up == c.up && mode != Mode::Now {
                    continue;
                }
                let ms = match mode {
                    Mode::Now => 0.0,
                    // The old fan closes up at once under the new cards.
                    Mode::Ms(_) if drawing && p == WASTE && !new_pile => 0.0,
                    Mode::Ms(ms) => ms,
                    Mode::Deal => ((tx - s.x).hypot(ty - s.y) / deal_speed).round(),
                };
                let mut start = now;
                if from_stock && matches!(mode, Mode::Ms(_)) {
                    start += Duration::from_millis((drawn as f32 * DRAW_STAGGER) as u64);
                    drawn += 1;
                }
                s.go(
                    tx,
                    ty,
                    c.up,
                    ms,
                    new_pile && mode != Mode::Deal,
                    start,
                    self.cw,
                    self.ch,
                );
                if from_stock || to_stock {
                    s.turn_at_start();
                }
            }
        }
        if self.peek.is_some() {
            self.peek = None;
        }
    }

    fn act(&mut self, a: Action, mode: Mode, record: bool) {
        if !self.st.legal(a, self.set.draw_one) {
            return;
        }
        if record {
            self.hist.undo.push(self.st.clone());
            self.hist.redo.clear();
        }
        self.st.apply(a, self.draw_n());
        self.sync(mode);
        self.state_changed(Some(a), record);
    }

    /// Bookkeeping after any change of the game state.
    fn state_changed(&mut self, a: Option<Action>, user_move: bool) {
        self.generation += 1;
        self.verdict = match (self.verdict.take(), a) {
            // Following the known solution keeps it valid.
            (Some(Verdict::Win(v)), Some(a)) if v.first() == Some(&a) => {
                Some(Verdict::Win(v[1..].to_vec()))
            }
            _ => None,
        };
        if self.verdict.is_none() {
            self.need_solve = self.set.solver;
            self.solver.stop();
        }
        if self.st.is_won() {
            self.won = true;
            self.auto = Auto::Idle;
            self.need_solve = false;
            self.set_status(String::new());
        } else if self.auto != Auto::Demo
            && (user_move || a.is_none())
            && self.hist.redo.is_empty()
            && self.set.autodrop
        {
            self.auto = Auto::Drop;
            self.next_step = Instant::now();
        }
        if !self.set.solver {
            self.set_status(String::new());
        }
        self.invalidate_status();
    }

    fn undo(&mut self) {
        self.interrupt();
        if let Some(prev) = self.hist.undo.pop() {
            self.hist.redo.push(std::mem::replace(&mut self.st, prev));
            self.after_history();
        }
    }

    fn redo(&mut self) {
        self.interrupt();
        if let Some(next) = self.hist.redo.pop() {
            self.hist.undo.push(std::mem::replace(&mut self.st, next));
            self.after_history();
        }
    }

    fn after_history(&mut self) {
        self.won = false;
        self.rain = None;
        self.rain_dismissed = false;
        self.sync(Mode::Ms(DURATION_MOVE));
        self.invalidate_all();
        self.state_changed(None, false);
        self.auto = Auto::Idle;
        if self.st.is_won() {
            self.won = true;
        }
    }

    /// What a user click does to running automation (KPat's stop()).
    fn interrupt(&mut self) {
        self.stop_auto();
        self.clear_hint();
        self.snap();
    }

    fn draw_cmd(&mut self) {
        self.interrupt();
        self.act(Action::Draw, Mode::Ms(DURATION_MOVE), true);
    }

    fn pile_of(&self, id: u8) -> (usize, usize) {
        for p in 0..NPILES {
            if let Some(i) = self.st.piles[p].iter().position(|c| c.id == id) {
                return (p, i);
            }
        }
        (STOCK, 0)
    }

    /// Heuristic hint, roughly KPat's bruteForceHints().
    fn heuristic(&self) -> Option<Action> {
        let st = &self.st;
        let mut best: Option<(i32, Action)> = None;
        let mut consider = |pri: i32, a: Action| {
            if best.is_none_or(|b| pri > b.0) {
                best = Some((pri, a));
            }
        };
        for src in std::iter::once(WASTE).chain(TAB..TAB + 7) {
            let pile = &st.piles[src];
            for idx in 0..pile.len() {
                if !st.can_remove(src, idx, self.set.draw_one) {
                    continue;
                }
                let cards = &pile[idx..];
                if idx == pile.len() - 1
                    && let Some(f) = st.foundation_for(cards[0])
                {
                    consider(
                        127,
                        Action::Move {
                            from: src,
                            idx,
                            to: f,
                        },
                    );
                }
                for t in TAB..TAB + 7 {
                    if t == src || !st.can_add(t, cards) {
                        continue;
                    }
                    let pri = if src == WASTE {
                        30
                    } else if idx == 0 {
                        if st.piles[t].is_empty() { continue } else { 5 }
                    } else if !pile[idx - 1].up {
                        50 + idx as i32
                    } else if st.foundation_for(pile[idx - 1]).is_some() {
                        40
                    } else {
                        continue;
                    };
                    consider(
                        pri,
                        Action::Move {
                            from: src,
                            idx,
                            to: t,
                        },
                    );
                }
            }
        }
        if best.is_none() && st.can_draw() {
            return Some(Action::Draw);
        }
        best.map(|b| b.1)
    }

    fn next_move(&self) -> Option<Action> {
        match &self.verdict {
            Some(Verdict::Win(v)) if !v.is_empty() => Some(v[0]),
            _ => self.heuristic(),
        }
    }

    fn hint(&mut self) {
        self.stop_auto();
        self.snap();
        if self.won {
            return;
        }
        if self.set.solver && self.verdict.is_none() {
            // Wait for the solver; it is usually done in milliseconds.
            self.need_solve = true;
            self.hint_queued = true;
            return;
        }
        self.show_hint();
    }

    fn show_hint(&mut self) {
        self.hint_queued = false;
        self.hl = 0;
        self.hl_pile = None;
        match self.next_move() {
            Some(Action::Draw) => match self.st.piles[STOCK].last() {
                Some(c) => self.hl |= 1 << c.id,
                None => self.hl_pile = Some(STOCK),
            },
            Some(Action::Move { from, idx, to }) => {
                self.hl |= 1 << self.st.piles[from][idx].id;
                match self.st.piles[to].last() {
                    Some(c) => self.hl |= 1 << c.id,
                    None => self.hl_pile = Some(to),
                }
            }
            None => self.set_status("No hint available.".into()),
        }
        self.invalidate_all();
    }

    fn start_demo(&mut self) {
        if self.won {
            return;
        }
        self.interrupt();
        self.auto = Auto::Demo;
        self.demo_draws = 0;
        self.next_step = Instant::now();
        if self.set.solver && self.verdict.is_none() {
            self.need_solve = true;
        }
    }

    fn try_auto_move(&mut self, id: u8) {
        let (p, i) = self.pile_of(id);
        if i + 1 != self.st.piles[p].len()
            || !self.st.can_remove(p, i, self.set.draw_one)
            || game::is_found(p)
        {
            return;
        }
        if let Some(f) = self.st.foundation_for(self.st.piles[p][i]) {
            self.act(
                Action::Move {
                    from: p,
                    idx: i,
                    to: f,
                },
                Mode::Ms(DURATION_MOVE),
                true,
            );
        }
    }

    fn speed_up(&self, ms: f32) -> f32 {
        if ms < DURATION_AUTODROP_MIN {
            ms
        } else {
            (ms * self.drop_speed).max(DURATION_AUTODROP_MIN)
        }
    }

    /// One step of autodrop / finishing sweep / demo.
    fn step(&mut self) {
        match self.auto {
            Auto::Idle => {}
            Auto::Drop | Auto::Finish => {
                let a = if self.auto == Auto::Finish {
                    self.st.finish_move()
                } else {
                    self.st.autodrop_move(self.set.draw_one)
                };
                if let Some(a) = a {
                    let ms = self.speed_up(DURATION_AUTODROP);
                    // Grouped with the move that triggered it: one undo
                    // step takes back the move and its autodrops.
                    self.act(a, Mode::Ms(ms), false);
                    self.drop_speed *= AUTODROP_SPEEDUP;
                    self.next_step = Instant::now()
                        + Duration::from_millis(self.speed_up(TIME_BETWEEN_MOVES) as u64);
                } else if self.auto == Auto::Drop && self.st.trivially_won() && !self.won {
                    self.auto = Auto::Finish;
                } else {
                    self.stop_auto();
                }
            }
            Auto::Demo => {
                if self.set.solver && self.verdict.is_none() {
                    self.need_solve = true;
                    return; // wait for the solver
                }
                let a = self.next_move();
                let budget = self.st.piles[STOCK].len() + self.st.piles[WASTE].len() + 3;
                match a {
                    // Cycling the stock without playing anything: give up.
                    Some(Action::Draw) if self.demo_draws > budget => {}
                    _ if self.hist.undo.len() > 3000 => {}
                    Some(a) => {
                        self.demo_draws = if a == Action::Draw {
                            self.demo_draws + 1
                        } else {
                            0
                        };
                        self.act(a, Mode::Ms(DURATION_DEMO), true);
                        if !self.won {
                            self.auto = Auto::Demo;
                            self.next_step =
                                Instant::now() + Duration::from_millis(TIME_BETWEEN_MOVES as u64);
                        }
                        return;
                    }
                    None => {}
                }
                self.stop_auto();
                self.set_status("Demo: no more useful moves.".into());
            }
        }
    }

    // ------------------------------------------------------------ win rain

    fn rain_queue(&self) -> Vec<(u8, f32, f32)> {
        let found: Vec<&Vec<game::C>> = (FOUND..FOUND + 4)
            .map(|p| &self.st.piles[p])
            .filter(|p| !p.is_empty())
            .collect();
        let depth = found.iter().map(|p| p.len()).max().unwrap_or(0);
        let mut q = Vec::new();
        // Top cards first, cycling through the piles.
        for d in 0..depth {
            for p in &found {
                if d < p.len() {
                    let c = p[p.len() - 1 - d].id;
                    let s = &self.sprites[c as usize];
                    q.push((c, s.tx, s.ty));
                }
            }
        }
        q
    }

    fn board_snapshot(&mut self) -> Image {
        let v = self.view();
        let saved = self.rain.take();
        let (hl, hp) = (self.hl, self.hl_pile);
        self.hl = 0;
        self.hl_pile = None;
        self.compose_board(v);
        self.hl = hl;
        self.hl_pile = hp;
        self.rain = saved;
        let mut snap = Image::new(v.w, v.h);
        snap.copy_from(&self.fb, 0, 0, v);
        snap
    }

    fn start_rain(&mut self) {
        let snap = self.board_snapshot();
        let q = self.rain_queue();
        self.rain = Some(Rain::new(snap, q));
        self.last_frame = Instant::now();
        self.invalidate_all();
    }

    // ---------------------------------------------------------------- input

    fn card_rect(&self, id: usize) -> Rect {
        let s = &self.sprites[id];
        Rect::new(s.x.round() as i32, s.y.round() as i32, self.cw, self.ch)
    }

    /// Cards in paint order (bottom first).
    fn paint_order(&self) -> Vec<(u8, i32, u8)> {
        let dragged = self.dragged_mask();
        let mut v: Vec<(u8, i32, u8)> = (0..52u8)
            .map(|c| {
                let s = &self.sprites[c as usize];
                let layer = if dragged >> c & 1 != 0 {
                    2
                } else {
                    s.raised() as u8
                };
                (layer, s.z, c)
            })
            .collect();
        v.sort_unstable();
        v
    }

    fn dragged_mask(&self) -> u64 {
        match &self.drag {
            Some(d) if d.started => self.st.piles[d.pile][d.idx..]
                .iter()
                .fold(0, |m, c| m | 1 << c.id),
            _ => 0,
        }
    }

    fn card_at(&self, x: i32, y: i32) -> Option<u8> {
        self.paint_order()
            .iter()
            .rev()
            .find(|&&(_, _, c)| self.card_rect(c as usize).contains(x, y))
            .map(|t| t.2)
    }

    fn pile_at(&self, x: i32, y: i32) -> Option<usize> {
        (0..NPILES).find(|&p| self.lay.pile_rect(p).contains(x, y))
    }

    fn drop_target(&self, d: &Drag) -> Option<usize> {
        let cards = &self.st.piles[d.pile][d.idx..];
        let first = self.card_rect(cards[0].id as usize);
        let mut best = None;
        let mut best_area = 1;
        for p in 0..NPILES {
            if p == d.pile || !self.st.can_add(p, cards) {
                continue;
            }
            let r = self.st.piles[p].iter().fold(self.lay.pile_rect(p), |r, c| {
                r.union(&self.card_rect(c.id as usize))
            });
            let i = r.intersect(&first);
            if i.w * i.h > best_area {
                best_area = i.w * i.h;
                best = Some(p);
            }
        }
        best
    }

    fn menu_items(&self) -> Vec<Item> {
        let can_undo = !self.hist.undo.is_empty();
        let can_redo = !self.hist.redo.is_empty();
        let decks = self
            .decks
            .iter()
            .enumerate()
            .map(|(i, e)| Item::check(&e.name, "", Cmd::Deck(i), e.id == self.set.deck))
            .collect();
        let themes = self
            .themes
            .iter()
            .enumerate()
            .map(|(i, e)| Item::check(&e.name, "", Cmd::Theme(i), e.id == self.set.theme))
            .collect();
        vec![
            Item::cmd("New Deal", "Ctrl+N", Cmd::NewDeal),
            Item::cmd("Numbered Deal...", "Ctrl+D", Cmd::NumberedDeal),
            Item::cmd("Restart Deal", "F5", Cmd::Restart),
            Item::Sep,
            Item::cmd("Undo", "Ctrl+Z", Cmd::Undo).enabled(can_undo),
            Item::cmd("Redo", "Ctrl+Shift+Z", Cmd::Redo).enabled(can_redo),
            Item::cmd("Hint", "H", Cmd::Hint).enabled(!self.won),
            Item::check("Demo", "D", Cmd::Demo, self.auto == Auto::Demo).enabled(!self.won),
            Item::cmd("Draw", "Tab", Cmd::Draw).enabled(self.st.can_draw()),
            Item::cmd("Drop", "P", Cmd::Drop).enabled(!self.won),
            Item::Sep,
            Item::check("Draw 1", "", Cmd::DrawMode(true), self.set.draw_one),
            Item::check("Draw 3", "", Cmd::DrawMode(false), !self.set.draw_one),
            Item::check("Autodrop", "", Cmd::ToggleAutodrop, self.set.autodrop),
            Item::check("Solver", "", Cmd::ToggleSolver, self.set.solver),
            Item::check("Status Bar", "", Cmd::ToggleStatus, self.set.status_bar),
            Item::Sub {
                label: "Card Deck".into(),
                items: decks,
            },
            Item::Sub {
                label: "Background".into(),
                items: themes,
            },
            Item::Sep,
            Item::cmd("Bigger Cards", "Ctrl++", Cmd::ZoomIn),
            Item::cmd("Smaller Cards", "Ctrl+-", Cmd::ZoomOut),
            Item::cmd("Fit Cards to Window", "Ctrl+0", Cmd::ZoomFit),
            Item::cmd("Full Screen", "F11", Cmd::Fullscreen),
            Item::Sep,
            Item::cmd("Keys...", "F1", Cmd::Help),
            Item::cmd("Quit", "Ctrl+Q", Cmd::Quit),
        ]
    }

    fn open_menu(&mut self, x: i32, y: i32) {
        self.interrupt();
        let m = Menu::open(self.menu_items(), x, y, self.ui, self.fb.rect());
        self.invalidate(m.bounds());
        self.menu = Some(m);
    }

    fn close_menu(&mut self) {
        if let Some(m) = self.menu.take() {
            self.invalidate(m.bounds());
        }
    }

    fn run(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::NewDeal => {
                let d = 1 + self.rng.below(i32::MAX as u64 - 1) as u32;
                self.start_new(d);
            }
            Cmd::NumberedDeal => {
                self.interrupt();
                let d = Dialog {
                    title: "New Numbered Deal".into(),
                    lines: vec!["Deal number (1 - 2147483647):".into()],
                    input: Some(String::new()),
                };
                self.dialog = Some((DialogKind::DealNumber, d));
                self.invalidate_all();
            }
            Cmd::Restart => self.start_new(self.deal),
            Cmd::Undo => self.undo(),
            Cmd::Redo => self.redo(),
            Cmd::Hint => self.hint(),
            Cmd::Demo => {
                if self.auto == Auto::Demo {
                    self.stop_auto();
                } else {
                    self.start_demo();
                }
            }
            Cmd::Draw => self.draw_cmd(),
            Cmd::Drop => {
                self.interrupt();
                self.auto = Auto::Drop;
                self.next_step = Instant::now();
            }
            Cmd::DrawMode(one) => {
                if one != self.set.draw_one {
                    self.set.draw_one = one;
                    self.set.save();
                    self.start_new(self.deal);
                }
            }
            Cmd::ToggleAutodrop => {
                self.set.autodrop = !self.set.autodrop;
                self.set.save();
            }
            Cmd::ToggleSolver => {
                self.set.solver = !self.set.solver;
                self.set.save();
                self.verdict = None;
                self.solver.stop();
                self.need_solve = self.set.solver;
                self.set_status(String::new());
            }
            Cmd::ToggleStatus => {
                self.set.status_bar = !self.set.status_bar;
                self.set.save();
                self.bg_resized();
                self.relayout(Mode::Now);
            }
            Cmd::Deck(i) => self.load_deck(i),
            Cmd::Theme(i) => self.load_theme(i),
            Cmd::ZoomIn => self.zoom(1.15),
            Cmd::ZoomOut => self.zoom(1.0 / 1.15),
            Cmd::ZoomFit => {
                self.set.card_width = 0;
                self.set.save();
                let w = self.card_width_for_view();
                self.set_card_width(w);
            }
            Cmd::Fullscreen => {
                let _ = self.win.toggle_fullscreen();
            }
            Cmd::Help => {
                let lines = [
                    "Drag cards to move them; double-click sends a card to the",
                    "foundation. Right-click a covered card to peek at it, or",
                    "empty space for the menu.",
                    "",
                    "Ctrl+N, F2\tNew deal",
                    "Ctrl+D\tNumbered deal",
                    "F5\tRestart deal",
                    "Ctrl+Z\tUndo",
                    "Ctrl+Shift+Z, Ctrl+Y\tRedo",
                    "H\tHint",
                    "D\tDemo",
                    "Tab\tDraw",
                    "P\tDrop",
                    "Ctrl++, Ctrl+-\tBigger / smaller cards",
                    "Ctrl+0\tFit cards to window",
                    "Ctrl+PgUp, Ctrl+PgDn\tPrevious / next deal",
                    "F10, Menu\tMenu",
                    "F11\tFull screen",
                    "Ctrl+Q\tQuit",
                ];
                let d = Dialog {
                    title: "Keys".into(),
                    lines: lines.iter().map(|s| s.to_string()).collect(),
                    input: None,
                };
                self.dialog = Some((DialogKind::Help, d));
                self.invalidate_all();
            }
            Cmd::Quit => self.quit = true,
        }
    }

    fn on_press(&mut self, button: u8, x: i32, y: i32, time: u32, mods: u16) {
        let dbl = button == 1
            && time.wrapping_sub(self.last_press.0) < DOUBLE_CLICK_MS
            && (x - self.last_press.1).abs() < 6
            && (y - self.last_press.2).abs() < 6;
        if button == 1 {
            // A double-click consumes the pair, so a third click starts over.
            self.last_press = if dbl {
                (time.wrapping_sub(DOUBLE_CLICK_MS), x, y)
            } else {
                (time, x, y)
            };
        }

        if let Some((kind, _)) = &self.dialog {
            if *kind == DialogKind::Help {
                self.dialog = None;
                self.invalidate_all();
            }
            return;
        }
        if let Some(menu) = &mut self.menu {
            if button == 1 || button == 3 {
                let r = menu.click(x, y, self.fb.rect());
                match r {
                    Some(Some(cmd)) => {
                        self.close_menu();
                        self.run(cmd);
                    }
                    Some(None) => self.invalidate(self.menu.as_ref().unwrap().bounds()),
                    None => self.close_menu(),
                }
            }
            return;
        }
        if (button == 4 || button == 5) && mods & CTRL != 0 {
            self.zoom(if button == 4 { 1.15 } else { 1.0 / 1.15 });
            return;
        }
        if self.won {
            if button == 1 && dbl {
                self.run(Cmd::NewDeal);
            } else if button == 1 && self.rain.is_some() {
                self.rain = None;
                self.rain_dismissed = true;
                self.invalidate_all();
            } else if button == 3 {
                self.open_menu(x, y);
            }
            return;
        }
        match button {
            1 => {
                self.interrupt();
                let card = self.card_at(x, y);
                let stock_hit = card.map_or(self.pile_at(x, y) == Some(STOCK), |c| {
                    self.pile_of(c).0 == STOCK
                });
                if stock_hit {
                    self.stock_press = true;
                    return;
                }
                let Some(c) = card else { return };
                if dbl {
                    self.try_auto_move(c);
                    return;
                }
                let (p, i) = self.pile_of(c);
                if self.st.can_remove(p, i, self.set.draw_one) {
                    let base = self.st.piles[p][i..]
                        .iter()
                        .map(|c| (self.sprites[c.id as usize].x, self.sprites[c.id as usize].y))
                        .collect();
                    self.drag = Some(Drag {
                        pile: p,
                        idx: i,
                        press: (x, y),
                        started: false,
                        base,
                        target: None,
                    });
                }
            }
            3 => {
                let peekable = self.card_at(x, y).filter(|&c| {
                    let (p, i) = self.pile_of(c);
                    i + 1 < self.st.piles[p].len()
                });
                match peekable {
                    Some(c) if self.drag.is_none() => {
                        self.interrupt();
                        let s = &mut self.sprites[c as usize];
                        let (tx, ty, up) = (s.tx, s.ty, s.up);
                        s.go(
                            tx + self.cw as f32 / 3.0,
                            ty - self.ch as f32 / 3.0,
                            up,
                            DURATION_FANCYSHOW,
                            false,
                            Instant::now(),
                            self.cw,
                            self.ch,
                        );
                        self.peek = Some(c);
                    }
                    Some(_) => {}
                    None => self.open_menu(x, y),
                }
            }
            _ => {}
        }
    }

    fn on_release(&mut self, button: u8, x: i32, y: i32) {
        if button == 3 {
            if let Some(c) = self.peek.take() {
                // Re-sync puts the peeked card back where it belongs.
                self.sprites[c as usize].tx = f32::NAN;
                self.sync(Mode::Ms(DURATION_FANCYSHOW));
            }
            return;
        }
        if button != 1 {
            return;
        }
        if std::mem::take(&mut self.stock_press) {
            let hit = self
                .card_at(x, y)
                .map_or(self.pile_at(x, y) == Some(STOCK), |c| {
                    self.pile_of(c).0 == STOCK
                });
            if hit {
                self.draw_cmd();
            }
            return;
        }
        let Some(d) = self.drag.take() else { return };
        self.hl_pile = None;
        self.hl = 0;
        if d.started {
            match d.target {
                Some(t) => self.act(
                    Action::Move {
                        from: d.pile,
                        idx: d.idx,
                        to: t,
                    },
                    Mode::Ms(DURATION_MOVE),
                    true,
                ),
                None => self.return_dragged(&d),
            }
            self.invalidate_all();
        }
    }

    fn return_dragged(&mut self, d: &Drag) {
        // Force the dragged cards to animate home.
        for c in &self.st.piles[d.pile][d.idx..] {
            self.sprites[c.id as usize].tx = f32::NAN;
        }
        self.sync(Mode::Ms(DURATION_MOVE));
    }

    fn on_motion(&mut self, x: i32, y: i32) {
        self.mouse = (x, y);
        if let Some(m) = &mut self.menu {
            let before = m.bounds();
            if m.hover(x, y, self.fb.rect()) {
                let after = self.menu.as_ref().unwrap().bounds();
                self.invalidate(before.union(&after));
            }
            return;
        }
        let Some(mut d) = self.drag.take() else {
            return;
        };
        let (dx, dy) = (x - d.press.0, y - d.press.1);
        if !d.started {
            let first = self.card_rect(self.st.piles[d.pile][d.idx].id as usize);
            if dx * dx + dy * dy > 16 || !first.contains(x, y) {
                d.started = true;
                // Continue from here if still over the card: smoother.
                if first.contains(x, y) {
                    d.press = (x, y);
                    d.base = self.st.piles[d.pile][d.idx..]
                        .iter()
                        .map(|c| (self.sprites[c.id as usize].x, self.sprites[c.id as usize].y))
                        .collect();
                }
            }
        }
        if d.started {
            let (dx, dy) = ((x - d.press.0) as f32, (y - d.press.1) as f32);
            for (k, c) in self.st.piles[d.pile][d.idx..].iter().enumerate() {
                let s = &mut self.sprites[c.id as usize];
                s.anim = None;
                s.x = d.base[k].0 + dx;
                s.y = d.base[k].1 + dy;
            }
            let t = self.drop_target(&d);
            if t != d.target {
                d.target = t;
                self.hl = 0;
                self.hl_pile = None;
                if let Some(p) = t {
                    match self.st.piles[p].last() {
                        Some(c) => self.hl = 1 << c.id,
                        None => self.hl_pile = Some(p),
                    }
                    self.invalidate(self.lay.pile_rect(p));
                }
                self.invalidate_all();
            }
        }
        self.drag = Some(d);
    }

    fn on_key(&mut self, sym: u32, mods: u16) {
        let ctrl = mods & CTRL != 0;
        let shift = mods & SHIFT != 0;
        if let Some((kind, d)) = &mut self.dialog {
            if *kind == DialogKind::DealNumber {
                let input = d.input.as_mut().unwrap();
                match sym {
                    keys::ESCAPE => self.dialog = None,
                    keys::RETURN | keys::KP_ENTER => {
                        let n = input
                            .parse::<u32>()
                            .ok()
                            .filter(|&n| n >= 1 && n <= i32::MAX as u32);
                        self.dialog = None;
                        if let Some(n) = n {
                            self.start_new(n);
                        }
                    }
                    keys::BACKSPACE => {
                        input.pop();
                    }
                    0x30..=0x39 if input.len() < 10 => input.push(char::from_u32(sym).unwrap()),
                    keys::KP_0..=keys::KP_9 if input.len() < 10 => {
                        input.push(char::from(b'0' + (sym - keys::KP_0) as u8))
                    }
                    _ => {}
                }
            } else {
                self.dialog = None;
            }
            self.invalidate_all();
            return;
        }
        if self.menu.is_some() {
            if sym == keys::ESCAPE || sym == keys::F10 || sym == keys::MENU {
                self.close_menu();
            }
            return;
        }
        let k = |c: char| sym == c as u32;
        let cmd = match () {
            _ if ctrl && k('q') => Some(Cmd::Quit),
            _ if ctrl && k('n') || sym == keys::F2 => Some(Cmd::NewDeal),
            _ if ctrl && k('d') => Some(Cmd::NumberedDeal),
            _ if sym == keys::F5 => Some(Cmd::Restart),
            _ if ctrl && (k('y') || (k('z') && shift)) => Some(Cmd::Redo),
            _ if ctrl && k('z') => Some(Cmd::Undo),
            _ if ctrl && (k('+') || k('=') || sym == keys::KP_ADD) => Some(Cmd::ZoomIn),
            _ if ctrl && (k('-') || k('_') || sym == keys::KP_SUB) => Some(Cmd::ZoomOut),
            _ if ctrl && (k('0') || k(')') || sym == keys::KP_0) => Some(Cmd::ZoomFit),
            _ if sym == keys::F11 || (ctrl && shift && k('f')) => Some(Cmd::Fullscreen),
            _ if sym == keys::F1 => Some(Cmd::Help),
            _ if ctrl || mods & ALT != 0 => None,
            _ if k('h') => Some(Cmd::Hint),
            _ if k('d') => Some(Cmd::Demo),
            _ if k('p') => Some(Cmd::Drop),
            _ if sym == keys::TAB || sym == keys::ISO_LEFT_TAB => Some(Cmd::Draw),
            _ => None,
        };
        if let Some(c) = cmd {
            self.run(c);
            return;
        }
        match sym {
            keys::F10 | keys::MENU => {
                let (x, y) = self.mouse;
                self.open_menu(x, y);
            }
            keys::ESCAPE => {
                if self.rain.is_some() {
                    self.rain = None;
                    self.rain_dismissed = true;
                    self.invalidate_all();
                }
                self.interrupt();
            }
            // Previous/next deal. The Qt fork put these on Ctrl+Shift+±,
            // which collides with Ctrl++ (zoom) on layouts where + needs
            // Shift; PageUp/PageDown don't.
            keys::PAGE_UP if ctrl => self.start_new(self.deal.saturating_sub(1).max(1)),
            keys::PAGE_DOWN if ctrl => self.start_new(self.deal.saturating_add(1)),
            _ => {}
        }
    }

    fn on_resize(&mut self) {
        self.fb = Image::new(self.win.w, self.win.h);
        self.ui = self.win.dpi_scale;
        self.close_menu();
        self.bg_resized();
        self.snap();
        self.relayout(Mode::Now);
        if self.rain.as_ref().is_some_and(|r| r.finished) {
            // Nothing left to rain over the re-laid-out board.
            self.rain = None;
            self.rain_dismissed = true;
        }
        if !self.win.fullscreen {
            self.set.win_w = self.win.w;
            self.set.win_h = self.win.h;
        }
    }

    // -------------------------------------------------------------- drawing

    fn compose_board(&mut self, clip: Rect) {
        let v = self.view();
        let clip = clip.intersect(&v);
        if clip.is_empty() {
            return;
        }
        self.fb.copy_from(&self.bg, 0, 0, clip);
        for p in 0..NPILES {
            let img = if self.hl_pile == Some(p) {
                self.pile_sel.as_ref().or(self.pile_img.as_ref())
            } else {
                self.pile_img.as_ref()
            };
            if let Some(img) = img {
                let r = self.lay.pile_rect(p);
                if !r.intersect(&clip).is_empty() {
                    self.fb.blit(img, r.x, r.y, clip, 0);
                }
            }
        }
        for (_, _, c) in self.paint_order() {
            let d = self.drawn_for(c as usize);
            if d.rect.intersect(&clip).is_empty() || d.squash <= 0 {
                continue;
            }
            let img = &self.cards[d.img as usize];
            self.fb
                .blit_squashed(img, d.rect.x, d.rect.y, d.squash, clip, d.dim);
        }
    }

    fn drawn_for(&self, c: usize) -> Drawn {
        let s = &self.sprites[c];
        let img = if s.flip >= 0.5 { c as u8 } else { 52 };
        let squash = ((2.0_f32 * s.flip - 1.0).abs() * self.cw as f32).round() as i32;
        let dim = if self.hl >> c & 1 != 0 { 128 } else { 0 };
        let layer = if self.dragged_mask() >> c & 1 != 0 {
            2
        } else {
            s.raised() as u8
        };
        Drawn {
            rect: self.card_rect(c),
            img,
            dim,
            squash: squash.max(1),
            layer,
        }
    }

    fn render(&mut self) {
        let v = self.view();
        if self.rain.is_none() {
            for c in 0..52 {
                let d = self.drawn_for(c);
                if d != self.drawn[c] {
                    self.dirty = self.dirty.union(&self.drawn[c].rect).union(&d.rect);
                    self.drawn[c] = d;
                }
            }
        }
        let dirty = self.dirty.intersect(&self.fb.rect());
        self.dirty = Rect::default();
        if dirty.is_empty() {
            return;
        }
        if let Some(r) = &self.rain {
            self.fb.copy_from(&r.buf, 0, 0, dirty.intersect(&v));
        } else {
            self.compose_board(dirty);
        }
        if self.set.status_bar {
            let sb = Rect::new(0, v.bottom(), self.win.w, self.win.h - v.bottom());
            if !sb.intersect(&dirty).is_empty() {
                let right = format!("Deal {}   Moves {}", self.deal, self.hist.undo.len());
                ui::status_bar(&mut self.fb, sb, self.ui, &self.status, &right, dirty);
            }
        }
        if let Some(m) = &self.menu {
            m.draw(&mut self.fb, dirty);
        }
        if let Some((_, d)) = &self.dialog {
            d.draw(&mut self.fb, self.ui, v, dirty);
        }
        if let Err(e) = self.win.present(&self.fb, dirty) {
            eprintln!("solitaire-rs: present failed: {e}");
            self.quit = true;
        }
    }

    // ------------------------------------------------------------ main loop

    fn tick(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;
        let mut anim = false;
        for s in self.sprites.iter_mut() {
            anim |= s.tick(now);
        }

        while let Ok((g, img)) = self.bg_rx.try_recv() {
            if g == self.bg_gen && img.w == self.view().w && img.h == self.view().h {
                self.bg = img;
                self.invalidate_all();
                if self.rain.is_some() {
                    self.rain_regrab = Some(now);
                }
            }
        }
        if self.bg_due.is_some_and(|t| now >= t) {
            self.start_bg_job();
        }

        while let Ok((g, v)) = self.solver.rx.try_recv() {
            if g != self.generation {
                continue;
            }
            self.solver.busy = false;
            let msg = match &v {
                Verdict::Win(_) => "Solver: This game is winnable.",
                Verdict::Lost => "Solver: This game is lost.",
                Verdict::Unknown => "Solver: Unable to determine if this game is winnable.",
            };
            self.verdict = Some(v);
            self.set_status(msg.into());
            if self.hint_queued {
                self.show_hint();
            }
        }

        if !anim {
            if self.auto != Auto::Idle && now >= self.next_step {
                self.step();
                anim = self.animating();
            }
            // Solve once things have settled, so we don't solve every
            // intermediate autodrop state.
            if self.need_solve && !anim && matches!(self.auto, Auto::Idle | Auto::Demo) && !self.won
            {
                self.need_solve = false;
                if self.set.solver {
                    self.solver.start(
                        self.generation,
                        self.st.clone(),
                        self.set.draw_one,
                        SOLVER_LIMIT,
                    );
                }
            }
            if self.hint_queued && !self.set.solver {
                self.show_hint();
            }
            if self.won && self.rain.is_none() && !self.rain_dismissed && !anim {
                self.start_rain();
            }
        }

        if self.rain_regrab.is_some_and(|t| now >= t) {
            self.rain_regrab = None;
            let finished = self.rain.as_ref().is_some_and(|r| r.finished);
            if finished {
                self.rain = None;
                self.rain_dismissed = true;
            } else if self.rain.is_some() {
                let snap = self.board_snapshot();
                let q = self.rain_queue();
                self.rain.as_mut().unwrap().regrab(snap, q);
            }
            self.invalidate_all();
        }
        if self.rain_regrab.is_none()
            && let Some(r) = &mut self.rain
        {
            let d = r.update(dt, &self.cards);
            self.dirty = self.dirty.union(&d);
        }
    }

    fn timeout_ms(&self) -> i32 {
        let busy = self.animating() || self.rain.as_ref().is_some_and(|r| !r.finished);
        if busy {
            return FRAME_MS as i32;
        }
        let now = Instant::now();
        let mut t = i32::MAX;
        let mut due =
            |at: Instant| t = t.min(at.saturating_duration_since(now).as_millis() as i32 + 1);
        if self.auto != Auto::Idle {
            due(self.next_step);
        }
        if let Some(b) = self.bg_due {
            due(b);
        }
        if let Some(r) = self.rain_regrab {
            due(r);
        }
        if self.solver.busy || self.need_solve {
            t = t.min(30);
        }
        if t == i32::MAX { -1 } else { t }
    }
}

fn usage() -> ! {
    println!(
        "solitaire-rs {} - Klondike solitaire\n\nUsage: solitaire-rs [--deal N] [--draw1 | --draw3]\n\nCard decks are read from $XDG_DATA_DIRS/carddecks (KDE decks), backgrounds\nfrom $XDG_DATA_DIRS/kpat/themes. Settings live in ~/.config/solitaire-rsrc.",
        env!("CARGO_PKG_VERSION")
    );
    std::process::exit(0)
}

fn main() {
    let mut set = Settings::load();
    let mut deal = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--deal" | "-d" => {
                deal = args
                    .next()
                    .and_then(|n| n.parse::<u32>().ok())
                    .filter(|&n| n >= 1 && n <= i32::MAX as u32)
            }
            "--draw1" => set.draw_one = true,
            "--draw3" => set.draw_one = false,
            "--version" | "-V" => {
                println!("solitaire-rs {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            _ => usage(),
        }
    }

    let decks = assets::find_decks();
    let themes = assets::find_themes();
    // Requested deck/theme, falling back to built-ins if they won't load.
    let mut deck_err = None;
    let deck = find_index(&decks, &set.deck)
        .and_then(|i| Deck::load(&decks[i]).map_err(|e| deck_err = Some(e)).ok())
        .unwrap_or_else(|| {
            set.deck = assets::BUILTIN.into();
            Deck::load(&decks[find_index(&decks, assets::BUILTIN).unwrap()]).expect("built-in deck")
        });
    let theme = find_index(&themes, &set.theme)
        .and_then(|i| Theme::load(&themes[i]).ok())
        .or_else(|| find_index(&themes, "greenblaze").and_then(|i| Theme::load(&themes[i]).ok()))
        .unwrap_or_else(|| {
            Theme::load(&themes[find_index(&themes, assets::BUILTIN).unwrap()])
                .expect("built-in theme")
        });

    let (w, h) = if set.win_w > 0 && set.win_h > 0 {
        (set.win_w, set.win_h)
    } else {
        (1000, 720)
    };
    let win = match Window::open("Solitaire", w, h) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("solitaire-rs: cannot open window: {e}");
            std::process::exit(1);
        }
    };
    let (bg_tx, bg_rx) = mpsc::channel();
    let ui_scale = win.dpi_scale;
    let mut app = App {
        fb: Image::new(win.w, win.h),
        win,
        set,
        decks,
        themes,
        deck,
        theme,
        cards: Vec::new(),
        cw: 0,
        ch: 0,
        pile_img: None,
        pile_sel: None,
        bg: Image::default(),
        bg_due: None,
        bg_gen: 0,
        bg_tx,
        bg_rx,
        ui: ui_scale,
        st: State::default(),
        deal: 1,
        hist: History::new(),
        sprites: [Sprite::default(); 52],
        lay: Layout::default(),
        drag: None,
        stock_press: false,
        peek: None,
        hl: 0,
        hl_pile: None,
        solver: Worker::new(),
        generation: 0,
        verdict: None,
        need_solve: false,
        hint_queued: false,
        status: String::new(),
        auto: Auto::Idle,
        next_step: Instant::now(),
        drop_speed: 1.0,
        demo_draws: 0,
        won: false,
        rain: None,
        rain_dismissed: false,
        rain_regrab: None,
        last_frame: Instant::now(),
        menu: None,
        dialog: None,
        dirty: Rect::default(),
        drawn: [Drawn::default(); 52],
        last_press: (0, -100, -100),
        mouse: (0, 0),
        rng: Rng::new(),
        quit: false,
    };
    // Match the theme setting to what actually loaded.
    if find_index(&app.themes, &app.set.theme).is_none() {
        app.set.theme = if find_index(&app.themes, "greenblaze").is_some() {
            "greenblaze".into()
        } else {
            assets::BUILTIN.into()
        };
    }
    if let Some(e) = deck_err {
        app.status = format!("Card deck failed to load ({e}); using the built-in deck.");
    }

    app.cw = app.card_width_for_view();
    app.render_cards();
    app.render_bg_now();
    let v = app.view();
    app.lay = Layout::new(v.w, v.h, app.cw, app.ch);
    let d = deal.unwrap_or_else(|| 1 + app.rng.below(i32::MAX as u64 - 1) as u32);
    app.start_new(d);

    while !app.quit {
        loop {
            match app.win.poll() {
                Ok(Some(ev)) => match ev {
                    Ev::Expose => app.invalidate_all(),
                    Ev::Resize => app.on_resize(),
                    Ev::Press {
                        button,
                        x,
                        y,
                        time,
                        mods,
                    } => app.on_press(button, x, y, time, mods),
                    Ev::Release { button, x, y } => app.on_release(button, x, y),
                    Ev::Motion { x, y } => app.on_motion(x, y),
                    Ev::Key { sym, mods } => app.on_key(sym, mods),
                    Ev::Leave => {}
                    Ev::Close => app.quit = true,
                },
                Ok(None) => break,
                Err(e) => {
                    eprintln!("solitaire-rs: X connection lost: {e}");
                    app.quit = true;
                    break;
                }
            }
        }
        if app.quit {
            break;
        }
        app.tick();
        app.render();
        let t = app.timeout_ms();
        app.win.wait(t);
    }
    app.solver.stop();
    if !app.win.fullscreen {
        app.set.win_w = app.win.w;
        app.set.win_h = app.win.h;
    }
    app.set.save();
}
