//! Klondike solver. Depth-first search with safe automoves, a transposition
//! table and "macro" stock moves: the waste/stock pair is kept as a single
//! sequence plus a cursor, so every card reachable by clicking the stock
//! becomes one move instead of a chain of draws.

use crate::game::{self, Action, FOUND, STOCK, State, TAB, WASTE};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

#[derive(Clone, Debug)]
pub enum Verdict {
    Win(Vec<Action>),
    Lost,
    Unknown,
}

const MAXCOL: usize = 20;

#[derive(Clone, Copy)]
struct S {
    col: [[u8; MAXCOL]; 7],
    len: [u8; 7],
    down: [u8; 7],
    /// waste bottom..top, then stock top..bottom
    deck: [u8; 24],
    dlen: u8,
    w: u8,
    found: [u8; 4],
}

#[derive(Clone, Copy, Debug)]
enum M {
    Found(u8),
    Tab {
        from: u8,
        n: u8,
        to: u8,
    },
    /// Click the stock `clicks` times, then play the waste top (deck[p-1]).
    Deck {
        p: u8,
        clicks: u16,
        to: u8,
    }, // to < 7: column, 7: foundation
}

#[inline]
fn rank(c: u8) -> u8 {
    game::rank(c)
}
#[inline]
fn red(c: u8) -> bool {
    game::red(c)
}

impl S {
    fn from_state(st: &State) -> S {
        let mut s = S {
            col: [[0; MAXCOL]; 7],
            len: [0; 7],
            down: [0; 7],
            deck: [0; 24],
            dlen: 0,
            w: 0,
            found: [0; 4],
        };
        for i in 0..7 {
            let p = &st.piles[TAB + i];
            for (j, c) in p.iter().enumerate() {
                s.col[i][j] = c.id;
            }
            s.len[i] = p.len() as u8;
            s.down[i] = p.iter().take_while(|c| !c.up).count() as u8;
        }
        let mut n = 0;
        for c in st.piles[WASTE].iter().chain(st.piles[STOCK].iter().rev()) {
            s.deck[n] = c.id;
            n += 1;
        }
        s.dlen = n as u8;
        s.w = st.piles[WASTE].len() as u8;
        for f in FOUND..FOUND + 4 {
            if let Some(t) = st.piles[f].last() {
                s.found[game::suit(t.id) as usize] = rank(t.id);
            }
        }
        s
    }

    fn won(&self) -> bool {
        self.found.iter().all(|&h| h == 13)
    }

    fn fits_found(&self, c: u8) -> bool {
        self.found[game::suit(c) as usize] + 1 == rank(c)
    }

    fn fits_col(&self, c: u8, to: usize) -> bool {
        match self.len[to] {
            0 => rank(c) == 13,
            l => {
                let t = self.col[to][l as usize - 1];
                red(t) != red(c) && rank(t) == rank(c) + 1
            }
        }
    }

    fn safe(&self, c: u8) -> bool {
        let r = rank(c);
        if r <= 2 {
            return true;
        }
        let me = red(c);
        let opp = (0..4).filter(|&s| matches!(s, 1 | 2) != me);
        if opp.clone().all(|s| self.found[s] >= r - 1) {
            return true;
        }
        let other = (0..4)
            .find(|&s| matches!(s, 1 | 2) == me && s != game::suit(c) as usize)
            .unwrap();
        opp.clone().all(|s| self.found[s] >= r - 2) && self.found[other] >= r - 3
    }

    fn pop_col(&mut self, i: usize, n: u8) {
        self.len[i] -= n;
        if self.down[i] >= self.len[i] && self.len[i] > 0 {
            self.down[i] = self.len[i] - 1;
        }
        if self.len[i] == 0 {
            self.down[i] = 0;
        }
    }

    fn apply(&mut self, m: M) {
        match m {
            M::Found(i) => {
                let i = i as usize;
                let c = self.col[i][self.len[i] as usize - 1];
                self.found[game::suit(c) as usize] += 1;
                self.pop_col(i, 1);
            }
            M::Tab { from, n, to } => {
                let (f, t) = (from as usize, to as usize);
                let start = (self.len[f] - n) as usize;
                for k in 0..n as usize {
                    self.col[t][self.len[t] as usize + k] = self.col[f][start + k];
                }
                self.len[t] += n;
                self.pop_col(f, n);
            }
            M::Deck { p, to, .. } => {
                let p = p as usize;
                let c = self.deck[p - 1];
                self.deck.copy_within(p..self.dlen as usize, p - 1);
                self.dlen -= 1;
                self.w = (p - 1) as u8;
                if to == 7 {
                    self.found[game::suit(c) as usize] += 1;
                } else {
                    let t = to as usize;
                    self.col[t][self.len[t] as usize] = c;
                    self.len[t] += 1;
                }
            }
        }
    }

    /// Positions reachable by clicking the stock: (cursor, clicks).
    fn reach(&self, draw: u8) -> Vec<(u8, u16)> {
        let mut out = Vec::new();
        let mut seen = 0u32;
        let (mut p, mut clicks) = (self.w, 0u16);
        loop {
            if seen >> p & 1 != 0 {
                break;
            }
            seen |= 1 << p;
            if p > 0 {
                out.push((p, clicks));
            }
            if p == self.dlen {
                if p <= 1 {
                    break;
                }
                p = 0;
            } else {
                p = (p + draw).min(self.dlen);
            }
            clicks += 1;
        }
        out
    }

    fn hash(&self, draw_one: bool) -> u64 {
        #[inline]
        fn mix(h: u64, x: u64) -> u64 {
            (h.rotate_left(5) ^ x).wrapping_mul(0x517c_c1b7_2722_0a95)
        }
        // Columns are interchangeable, so hash them as a sorted set.
        let mut cols = [0u64; 7];
        for i in 0..7 {
            let mut h = mix(0, self.down[i] as u64 + 1);
            for &c in &self.col[i][..self.len[i] as usize] {
                h = mix(h, c as u64 + 1);
            }
            cols[i] = h;
        }
        cols.sort_unstable();
        let mut h = 0;
        for c in cols {
            h = mix(h, c);
        }
        for &c in &self.deck[..self.dlen as usize] {
            h = mix(h, c as u64 + 1);
        }
        // In draw 1 every stock card is reachable from any cursor position.
        if !draw_one {
            h = mix(h, self.w as u64 + 100);
        }
        mix(h, u32::from_le_bytes(self.found) as u64)
    }

    fn moves(&self, draw: u8, variant: u32, out: &mut Vec<(i32, M)>) {
        out.clear();
        let first_empty = (0..7).find(|&i| self.len[i] == 0);
        let king_waiting = (0..7)
            .any(|i| self.down[i] > 0 && rank(self.col[i][self.down[i] as usize]) == 13)
            || self.deck[..self.dlen as usize]
                .iter()
                .any(|&c| rank(c) == 13);
        for i in 0..7 {
            let len = self.len[i] as usize;
            if len == 0 {
                continue;
            }
            let top = self.col[i][len - 1];
            if self.fits_found(top) {
                let reveal = self.down[i] as i32 == len as i32 - 1 && len > 1;
                out.push((100 + 20 * reveal as i32, M::Found(i as u8)));
            }
            let down = self.down[i] as usize;
            for start in down..len {
                let c = self.col[i][start];
                let n = (len - start) as u8;
                let whole = start == down;
                for t in 0..7 {
                    if t == i {
                        continue;
                    }
                    if self.len[t] == 0 && Some(t) != first_empty {
                        continue;
                    }
                    if !self.fits_col(c, t) {
                        continue;
                    }
                    let pri = if whole {
                        if down > 0 {
                            80 + down as i32
                        } else if rank(c) == 13 {
                            continue; // king already at the bottom
                        } else if king_waiting {
                            30
                        } else {
                            continue;
                        }
                    } else {
                        // Splitting a run only helps if it frees a card for
                        // the foundation.
                        if !self.fits_found(self.col[i][start - 1]) {
                            continue;
                        }
                        70
                    };
                    out.push((
                        pri,
                        M::Tab {
                            from: i as u8,
                            n,
                            to: t as u8,
                        },
                    ));
                }
            }
        }
        for (p, clicks) in self.reach(draw) {
            let c = self.deck[p as usize - 1];
            let cost = clicks.min(40) as i32;
            if self.fits_found(c) {
                out.push((90 - cost, M::Deck { p, clicks, to: 7 }));
            }
            for t in 0..7 {
                if (self.len[t] > 0 || Some(t) == first_empty) && self.fits_col(c, t) {
                    out.push((
                        60 - cost,
                        M::Deck {
                            p,
                            clicks,
                            to: t as u8,
                        },
                    ));
                }
            }
        }
        match variant {
            0 => {}
            // Alternative orderings for restarts: DFS run times are heavy
            // tailed, so a different first guess often finishes quickly.
            1 => out
                .iter_mut()
                .filter(|m| matches!(m.1, M::Deck { .. }))
                .for_each(|m| m.0 += 35),
            v => {
                let mut x = (v as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ self.hash(true);
                for m in out.iter_mut() {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    m.0 += (x % 41) as i32 - 20;
                }
            }
        }
        out.sort_by_key(|m| -m.0);
    }
}

struct Search<'a> {
    draw: u8,
    variant: u32,
    seen: HashSet<u64, BuildNoHash>,
    path: Vec<M>,
    nodes: u64,
    limit: u64,
    aborted: bool,
    cancel: &'a AtomicBool,
    bufs: Vec<Vec<(i32, M)>>,
}

impl Search<'_> {
    fn dfs(&mut self, s0: &S) -> bool {
        self.nodes += 1;
        if self.nodes > self.limit
            || (self.nodes & 4095 == 0 && self.cancel.load(Ordering::Relaxed))
        {
            self.aborted = true;
            return false;
        }
        let mark = self.path.len();
        let mut s = *s0;
        // Safe automoves are forced, not branched on.
        'auto: loop {
            for i in 0..7 {
                let l = s.len[i] as usize;
                if l > 0 && s.fits_found(s.col[i][l - 1]) && s.safe(s.col[i][l - 1]) {
                    s.apply(M::Found(i as u8));
                    self.path.push(M::Found(i as u8));
                    continue 'auto;
                }
            }
            if s.w > 0 && (self.draw == 1 || s.w < 3) {
                let c = s.deck[s.w as usize - 1];
                if s.fits_found(c) && s.safe(c) {
                    let m = M::Deck {
                        p: s.w,
                        clicks: 0,
                        to: 7,
                    };
                    s.apply(m);
                    self.path.push(m);
                    continue 'auto;
                }
            }
            break;
        }
        if s.won() {
            return true;
        }
        if !self.seen.insert(s.hash(self.draw == 1)) {
            self.path.truncate(mark);
            return false;
        }
        let mut moves = self.bufs.pop().unwrap_or_default();
        s.moves(self.draw, self.variant, &mut moves);
        for &(_, m) in &moves {
            let mut s2 = s;
            s2.apply(m);
            self.path.push(m);
            if self.dfs(&s2) {
                return true;
            }
            self.path.pop();
            if self.aborted {
                break;
            }
        }
        self.bufs.push(moves);
        self.path.truncate(mark);
        false
    }
}

/// The hash is already well mixed; don't hash it again.
#[derive(Default, Clone, Copy)]
struct BuildNoHash;
struct NoHash(u64);
impl std::hash::BuildHasher for BuildNoHash {
    type Hasher = NoHash;
    fn build_hasher(&self) -> NoHash {
        NoHash(0)
    }
}
impl std::hash::Hasher for NoHash {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, _: &[u8]) {
        unreachable!()
    }
    fn write_u64(&mut self, v: u64) {
        self.0 = v;
    }
}

pub fn solve(st: &State, draw_one: bool, limit: u64, cancel: &AtomicBool) -> Verdict {
    let draw = if draw_one { 1 } else { 3 };
    let s = S::from_state(st);
    const ATTEMPTS: u32 = 4;
    let mut seen = HashSet::with_capacity_and_hasher(1 << 16, BuildNoHash);
    for variant in 0..ATTEMPTS {
        seen.clear();
        let mut search = Search {
            draw,
            variant,
            seen,
            path: Vec::new(),
            nodes: 0,
            limit: limit / ATTEMPTS as u64,
            aborted: false,
            cancel,
            bufs: Vec::new(),
        };
        if search.dfs(&s) {
            return Verdict::Win(translate(st, &search.path, draw_one));
        }
        // Every attempt prunes the same way, so one exhaustive run proves
        // there is no solution (within the moves the solver considers).
        if !search.aborted {
            return Verdict::Lost;
        }
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        seen = search.seen;
    }
    Verdict::Unknown
}

/// Turn solver moves into game actions by replaying them on a real state.
fn translate(st: &State, path: &[M], draw_one: bool) -> Vec<Action> {
    let n = if draw_one { 1 } else { 3 };
    let mut g = st.clone();
    let mut out = Vec::new();
    let push = |g: &mut State, a: Action, out: &mut Vec<Action>| -> bool {
        if !g.legal(a, draw_one) {
            return false;
        }
        g.apply(a, n);
        out.push(a);
        true
    };
    for &m in path {
        let ok = match m {
            M::Found(i) => {
                let from = TAB + i as usize;
                let idx = g.piles[from].len() - 1;
                let to = g.foundation_for(g.piles[from][idx]).unwrap_or(FOUND);
                push(&mut g, Action::Move { from, idx, to }, &mut out)
            }
            M::Tab { from, n: k, to } => {
                let from = TAB + from as usize;
                let idx = g.piles[from].len() - k as usize;
                push(
                    &mut g,
                    Action::Move {
                        from,
                        idx,
                        to: TAB + to as usize,
                    },
                    &mut out,
                )
            }
            M::Deck { clicks, to, .. } => {
                let mut ok = true;
                for _ in 0..clicks {
                    ok &= push(&mut g, Action::Draw, &mut out);
                }
                let idx = g.piles[WASTE].len().wrapping_sub(1);
                let to = if to == 7 {
                    g.piles[WASTE]
                        .last()
                        .and_then(|&c| g.foundation_for(c))
                        .unwrap_or(FOUND)
                } else {
                    TAB + to as usize
                };
                ok && push(
                    &mut g,
                    Action::Move {
                        from: WASTE,
                        idx,
                        to,
                    },
                    &mut out,
                )
            }
        };
        if !ok {
            debug_assert!(false, "solver/game disagreement at {m:?}");
            break;
        }
    }
    out
}

/// Runs the solver off the UI thread; newer requests cancel older ones.
pub struct Worker {
    tx: mpsc::Sender<(u64, Verdict)>,
    pub rx: mpsc::Receiver<(u64, Verdict)>,
    cancel: Arc<AtomicBool>,
    pub busy: bool,
}

impl Worker {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Worker {
            tx,
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
            busy: false,
        }
    }

    pub fn start(&mut self, generation: u64, st: State, draw_one: bool, limit: u64) {
        self.stop();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = cancel.clone();
        let tx = self.tx.clone();
        self.busy = true;
        let spawned = std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn(move || {
                let v = solve(&st, draw_one, limit, &cancel);
                if !cancel.load(Ordering::Relaxed) {
                    let _ = tx.send((generation, v));
                }
            });
        if spawned.is_err() {
            self.busy = false;
        }
    }

    pub fn stop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.busy = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solutions_replay_to_a_win() {
        let cancel = AtomicBool::new(false);
        let (mut won, mut lost, mut unknown) = (0, 0, 0);
        for draw_one in [true, false] {
            for deal in 1..=10u32 {
                let st = State::deal(deal);
                match solve(&st, draw_one, 100_000, &cancel) {
                    Verdict::Win(actions) => {
                        let mut g = st.clone();
                        for a in actions {
                            assert!(g.legal(a, draw_one), "deal {deal}: illegal {a:?}");
                            g.apply(a, if draw_one { 1 } else { 3 });
                        }
                        assert!(g.is_won(), "deal {deal} not won after replay");
                        won += 1;
                    }
                    Verdict::Lost => lost += 1,
                    Verdict::Unknown => unknown += 1,
                }
            }
        }
        eprintln!("won {won} lost {lost} unknown {unknown}");
        assert!(won >= 10);
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    #[test]
    #[ignore]
    fn stats() {
        let cancel = AtomicBool::new(false);
        let limit: u64 = std::env::var("LIMIT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(400_000);
        for draw_one in [true, false] {
            let (mut w, mut l, mut u) = (0, 0, 0);
            let t = std::time::Instant::now();
            for deal in 1..=100u32 {
                match solve(&State::deal(deal), draw_one, limit, &cancel) {
                    Verdict::Win(_) => w += 1,
                    Verdict::Lost => l += 1,
                    Verdict::Unknown => u += 1,
                }
            }
            eprintln!(
                "draw1={draw_one} limit={limit}: won {w} lost {l} unknown {u} in {:?}",
                t.elapsed()
            );
        }
    }
}
