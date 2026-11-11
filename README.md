# solitaire-rs

Windows 95 style Klondike solitaire, written in Rust as a single static binary (about
3.3 MB). It is a native Wayland client and nothing else: no X11,
no XWayland. It doesn't load any shared libraries either: no Qt, no KDE Frameworks, no
libwayland, no libxkbcommon, no libc.so. The Wayland protocol, keyboard layouts (XKB) and SVG
rendering are all pure Rust inside the binary.

## The game

- Klondike rules as [KPatience](https://apps.kde.org/kpat/) plays them: draw 1 / draw 3, unlimited redeals, taking cards
  back off the foundations in draw 1 only, the same automatic drops (Raymond's rule), and the
  same deal numbers. Deal 1234 here is deal 1234 in KPat, because it uses the same
  Windows FreeCell shuffle.
- **KDE card decks**: anything in `carddecks/*/index.desktop` under `$XDG_DATA_HOME` or
  `$XDG_DATA_DIRS`, so system decks and decks from "Get New Card Decks" both show up. Plain
  `.svg` and compressed `.svgz` both work, and so do decks that keep their cards in `<defs>`
  (Tigullio, XSkat German).
- **KPat background themes**: anything in `kpat/themes/*.desktop`, using the `background`,
  `pile` and `pile_selected` elements.
- Hints, demo, solver ("This game is winnable / lost"), undo and redo, peeking at a covered
  card with right-click, double-clicking a card to send it to the foundation.
- On first run it reads your deck, background, card size and draw mode from KPat's `kpatrc`,
  if there is one. After that it keeps its own settings in `~/.config/solitaire-rsrc` and
  never writes to KPat's file.

## Windows 95

- **Menus, dialogs and status bar** drawn by hand in the Windows 95 style, in a bitmap
  Helvetica (the X11 one), which is about as close as a free font gets to MS Sans Serif.
- **Draw 3 as Windows plays it.** The waste fans out only the cards of the last draw, so the
  fan shrinks as you play them, and the game never draws for you.
- **Raining cards when you win**, Windows 95 style. Click to dismiss it; double-click to deal
  again. The rain carries on if you resize the window or change the card size.
- **Card size is yours, not the window's.** Ctrl++ and Ctrl+- grow and shrink the cards in
  15% steps, and so does Ctrl+mouse wheel. The size is saved. Ctrl+0 fits the cards to the
  window again.

## Details

- Rain physics run on a fixed 16 ms step tied to real time, so a busy or slow frame doesn't
  slow the rain down or bunch up the trails.
- Cards zoomed wider than the window line up against the left edge instead of being cut off
  on both sides.
- Clicking while cards are moving snaps them into place and takes the click, instead of
  ignoring it.
- One undo takes back your move together with the automatic drops it set off.
- Once every card is face up and the stock is empty, the remaining cards go up by themselves
  (only when autodrop is on).
- Startup and zooming are quick because the deck renders on all cores. While you resize the
  window the old background is stretched, then re-rendered in a background thread once you stop.
- Only the parts of the window that changed are redrawn and sent to the compositor, and it
  uses no CPU while idle. Fractional scaling (125%, 150%...) renders at full resolution instead
  of being blurred by the compositor.
- The solver searches the stock as a single sequence plus a cursor, so it never has to step
  through draws one at a time, and it restarts with different move orders when a search goes
  nowhere.
- A built-in Windows 95 style deck and the Clean Green background are compiled in, so the
  binary works with nothing else installed.

## Keys

| Key | Action |
| --- | --- |
| Ctrl+N, F2 | New deal |
| Ctrl+D | Numbered deal |
| F5 | Restart deal |
| Ctrl+Z / Ctrl+Shift+Z, Ctrl+Y | Undo / redo |
| H | Hint |
| D | Demo (on/off) |
| Tab | Draw |
| P | Drop |
| Ctrl++ / Ctrl+- / Ctrl+0 | Bigger / smaller / fit cards |
| Ctrl+PgDn / Ctrl+PgUp | Next / previous deal |
| F10, Menu key, right-click on the table | Menu (draw mode, deck, background and more) |
| F11, Ctrl+Shift+F | Full screen |
| F1 | Key list |
| Ctrl+Q | Quit |

`solitaire-rs --deal N` starts a given deal. `--draw1` and `--draw3` pick the draw mode.

## Building

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release
# -> target/x86_64-unknown-linux-musl/release/solitaire-rs
```

`.cargo/config.toml` targets static musl by default. `cargo test` checks that
the solver's solutions replay to a win.

It needs a compositor with xdg-shell (every desktop compositor has it). Title bars come from
the compositor (KDE Plasma, sway, Hyprland and most others draw them). GNOME doesn't offer
server-side title bars, so there the window has none; use the keyboard or the right-click menu.

## Not included

KPat's other games, statistics, sounds, keyboard-only card play, and saving or loading games.

## License

GPL-3.0-or-later; see `COPYING`. The game logic follows KPat (GPL-2.0-or-later), the
built-in Clean Green background comes from KPat (by Parker Coates), and the UI font is Adobe
Helvetica from the X11 bitmap fonts. `NOTICE` has the details.
