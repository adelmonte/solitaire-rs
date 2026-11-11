//! Klondike rules, matching KPat's: deal numbering, draw 1 / draw 3,
//! unlimited redeals, foundation take-backs in draw 1 only.

pub const STOCK: usize = 0;
pub const WASTE: usize = 1;
pub const FOUND: usize = 2; // 2..6
pub const TAB: usize = 6; // 6..13
pub const NPILES: usize = 13;

/// Card id = suit * 13 + (rank - 1); suits are KCardDeck's order
/// (clubs, diamonds, hearts, spades).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct C {
    pub id: u8,
    pub up: bool,
}

#[inline]
pub fn rank(id: u8) -> u8 {
    id % 13 + 1
}
#[inline]
pub fn suit(id: u8) -> u8 {
    id / 13
}
#[inline]
pub fn red(id: u8) -> bool {
    matches!(suit(id), 1 | 2)
}

pub fn is_found(p: usize) -> bool {
    (FOUND..FOUND + 4).contains(&p)
}
pub fn is_tab(p: usize) -> bool {
    (TAB..TAB + 7).contains(&p)
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct State {
    pub piles: [Vec<C>; NPILES],
    /// Cards the player took down from a foundation; autodrop leaves them be.
    pub taken_down: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Click on the stock: draw, or turn the waste over.
    Draw,
    /// Move the cards from `idx` up out of `from` onto `to`.
    Move { from: usize, idx: usize, to: usize },
}

/// Windows FreeCell's LCG, so deal numbers match KPat (and MS FreeCell).
pub fn shuffled(deal: u32) -> Vec<u8> {
    // KPat builds its deck rank-major: A♣ A♦ A♥ A♠ 2♣ ...
    let mut cards: Vec<u8> = (0..52u8).map(|i| (i % 4) * 13 + i / 4).collect();
    let mut seed = deal;
    for i in (2..=cards.len()).rev() {
        seed = seed.wrapping_mul(214013).wrapping_add(2531011);
        let r = ((seed >> 16) & 0x7fff) as usize;
        cards.swap(i - 1, r % i);
    }
    cards
}

impl State {
    pub fn deal(deal: u32) -> State {
        let mut list = shuffled(deal);
        let mut s = State::default();
        for round in 0..7 {
            for i in round..7 {
                let id = list.pop().unwrap();
                s.piles[TAB + i].push(C { id, up: i == round });
            }
        }
        s.piles[STOCK] = list.into_iter().map(|id| C { id, up: false }).collect();
        s
    }

    pub fn is_won(&self) -> bool {
        (0..NPILES).all(|p| is_found(p) || self.piles[p].is_empty())
    }

    pub fn can_draw(&self) -> bool {
        !self.piles[STOCK].is_empty() || self.piles[WASTE].len() > 1
    }

    pub fn draw(&mut self, n: usize) -> bool {
        let (stock, waste) = (STOCK, WASTE);
        if self.piles[stock].is_empty() {
            if self.piles[waste].len() <= 1 {
                return false;
            }
            let w = std::mem::take(&mut self.piles[waste]);
            self.piles[stock] = w.into_iter().rev().map(|c| C { up: false, ..c }).collect();
        } else {
            for _ in 0..n {
                let Some(c) = self.piles[stock].pop() else {
                    break;
                };
                self.piles[waste].push(C { up: true, ..c });
            }
        }
        true
    }

    /// Can the cards from `idx` up be picked up?
    pub fn can_remove(&self, p: usize, idx: usize, draw_one: bool) -> bool {
        let pile = &self.piles[p];
        if idx >= pile.len() || !pile[idx].up {
            return false;
        }
        let top = idx == pile.len() - 1;
        match p {
            WASTE => top,
            _ if is_found(p) => draw_one && top,
            _ if is_tab(p) => alt_desc(&pile[idx..]),
            _ => false,
        }
    }

    pub fn can_add(&self, p: usize, cards: &[C]) -> bool {
        let Some(first) = cards.first() else {
            return false;
        };
        let pile = &self.piles[p];
        if let Some(t) = pile.last()
            && !t.up
        {
            return false;
        }
        if is_tab(p) {
            alt_desc(cards)
                && match pile.last() {
                    None => rank(first.id) == 13,
                    Some(t) => red(t.id) != red(first.id) && rank(t.id) == rank(first.id) + 1,
                }
        } else if is_found(p) {
            cards
                .windows(2)
                .all(|w| suit(w[0].id) == suit(w[1].id) && rank(w[1].id) == rank(w[0].id) + 1)
                && match pile.last() {
                    None => rank(first.id) == 1,
                    Some(t) => suit(t.id) == suit(first.id) && rank(first.id) == rank(t.id) + 1,
                }
        } else {
            false
        }
    }

    pub fn legal(&self, a: Action, draw_one: bool) -> bool {
        match a {
            Action::Draw => self.can_draw(),
            Action::Move { from, idx, to } => {
                from != to
                    && self.can_remove(from, idx, draw_one)
                    && self.can_add(to, &self.piles[from][idx..])
            }
        }
    }

    /// Apply a move without checking legality (callers check), turning up
    /// the newly exposed tableau card like KPat's autoTurnTop.
    pub fn apply(&mut self, a: Action, draw_n: usize) {
        match a {
            Action::Draw => {
                self.draw(draw_n);
            }
            Action::Move { from, idx, to } => {
                let moved: Vec<C> = self.piles[from].drain(idx..).collect();
                for c in &moved {
                    let bit = 1u64 << c.id;
                    if is_found(from) && !is_found(to) {
                        self.taken_down |= bit;
                    } else {
                        self.taken_down &= !bit;
                    }
                }
                self.piles[to].extend(moved);
                if is_tab(from)
                    && let Some(t) = self.piles[from].last_mut()
                {
                    t.up = true;
                }
            }
        }
    }

    /// Foundation pile that would take `card`, if any.
    pub fn foundation_for(&self, card: C) -> Option<usize> {
        (FOUND..FOUND + 4).find(|&f| self.can_add(f, &[card]))
    }

    /// KPat's safe automove test (Raymond's rule): a card can go up if it
    /// can no longer be needed to hold a card of the opposite colour.
    pub fn safe_to_found(&self, id: u8) -> bool {
        let r = rank(id);
        if r <= 2 {
            return true;
        }
        let mut height = [0u8; 4];
        for f in FOUND..FOUND + 4 {
            if let Some(t) = self.piles[f].last() {
                height[suit(t.id) as usize] = rank(t.id);
            }
        }
        let me = red(id);
        let opp: Vec<u8> = (0..4u8)
            .filter(|&s| matches!(s, 1 | 2) != me)
            .map(|s| height[s as usize])
            .collect();
        if opp.iter().all(|&h| h >= r - 1) {
            return true;
        }
        let same_other = (0..4u8)
            .find(|&s| matches!(s, 1 | 2) == me && s != suit(id))
            .unwrap();
        opp.iter().all(|&h| h >= r - 2) && height[same_other as usize] >= r.saturating_sub(3)
    }

    /// The next automatic foundation move, as KPat's drop() would pick it.
    pub fn autodrop_move(&self, draw_one: bool) -> Option<Action> {
        let mut sources: Vec<usize> = (TAB..TAB + 7).collect();
        // Waste drops in draw 3 can break the deal's rhythm; KPat only does
        // them when the waste is nearly empty.
        if draw_one || self.piles[WASTE].len() < 3 {
            sources.push(WASTE);
        }
        for p in sources {
            let Some(&c) = self.piles[p].last() else {
                continue;
            };
            if !c.up || self.taken_down & (1 << c.id) != 0 || !self.safe_to_found(c.id) {
                continue;
            }
            if let Some(f) = self.foundation_for(c) {
                return Some(Action::Move {
                    from: p,
                    idx: self.piles[p].len() - 1,
                    to: f,
                });
            }
        }
        None
    }

    /// Everything is face up and the stock is gone: the rest is mechanical.
    pub fn trivially_won(&self) -> bool {
        self.piles[STOCK].is_empty()
            && self.piles[WASTE].is_empty()
            && (TAB..TAB + 7).all(|p| self.piles[p].iter().all(|c| c.up))
    }

    /// Any foundation move for the finishing sweep (lowest rank first).
    pub fn finish_move(&self) -> Option<Action> {
        (TAB..TAB + 7)
            .filter_map(|p| {
                let c = *self.piles[p].last()?;
                self.foundation_for(c).map(|f| {
                    (
                        rank(c.id),
                        Action::Move {
                            from: p,
                            idx: self.piles[p].len() - 1,
                            to: f,
                        },
                    )
                })
            })
            .min_by_key(|x| x.0)
            .map(|x| x.1)
    }
}

pub fn alt_desc(cards: &[C]) -> bool {
    cards
        .windows(2)
        .all(|w| w[1].up && red(w[0].id) != red(w[1].id) && rank(w[0].id) == rank(w[1].id) + 1)
}

/// Undo history: whole-state snapshots (a deal is tiny).
pub struct History {
    pub undo: Vec<State>,
    pub redo: Vec<State>,
}

impl History {
    pub fn new() -> Self {
        History {
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }
}
