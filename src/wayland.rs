//! Native Wayland window: pure-Rust protocol implementation (no
//! libwayland), keymaps parsed with kbvm (no libxkbcommon), shared-memory
//! buffers, fractional scaling via wp_fractional_scale + wp_viewporter.

use crate::gfx::{Image, Rect};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    dispatch2::Dispatch2,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        pointer::{
            CursorIcon, PointerEvent, PointerEventKind, PointerHandler, ThemeSpec, ThemedPointer,
        },
    },
    shell::{
        WaylandSurface,
        xdg::{
            XdgShell,
            window::{Window as XdgWindow, WindowConfigure, WindowDecorations, WindowHandler},
        },
    },
    shm::{
        Shm, ShmHandler,
        slot::{Buffer, SlotPool},
    },
};
use std::collections::VecDeque;
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileExt;
use wayland_client::{
    Connection, EventQueue, Proxy, QueueHandle, WEnum,
    globals::{GlobalList, registry_queue_init},
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
    wp_fractional_scale_v1::{self, WpFractionalScaleV1},
};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};

pub const SHIFT: u16 = 1;
pub const CTRL: u16 = 4;
pub const ALT: u16 = 8;

#[derive(Debug)]
pub enum Ev {
    Expose,
    Resize,
    Press {
        button: u8,
        x: i32,
        y: i32,
        time: u32,
        mods: u16,
    },
    Release {
        button: u8,
        x: i32,
        y: i32,
    },
    Motion {
        x: i32,
        y: i32,
    },
    Key {
        sym: u32,
        mods: u16,
    },
    Leave,
    Close,
}

type Res<T> = Result<T, Box<dyn std::error::Error>>;

/// User data for objects whose events we handle ourselves.
struct Kb;
struct FracScale;
struct NoEvents;

struct State {
    registry: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor: CompositorState,
    shm: Shm,
    window: XdgWindow,
    pointer: Option<ThemedPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    keymap: Option<kbvm::lookup::LookupTable>,
    mods: kbvm::ModifierMask,
    group: kbvm::GroupIndex,
    events: VecDeque<Ev>,
    configured: bool,
    /// Logical (surface) size.
    lw: i32,
    lh: i32,
    /// Integer scale from wl_surface, and fractional scale * 120 if the
    /// compositor offers it.
    int_scale: i32,
    frac120: Option<u32>,
    fullscreen: bool,
    pointer_pos: (f64, f64),
    scroll_acc: f64,
}

impl State {
    fn scale(&self) -> f64 {
        match self.frac120 {
            Some(f) => f as f64 / 120.0,
            None => self.int_scale as f64,
        }
    }

    fn px(&self, x: f64, y: f64) -> (i32, i32) {
        let s = self.scale();
        ((x * s).floor() as i32, (y * s).floor() as i32)
    }

    fn key_mods(&self) -> u16 {
        self.mods.0 as u16
    }
}

struct Buf {
    buffer: Buffer,
    pending: Rect,
}

pub struct Window {
    conn: Connection,
    queue: EventQueue<State>,
    st: State,
    pool: SlotPool,
    bufs: Vec<Buf>,
    viewport: Option<WpViewport>,
    _frac: Option<WpFractionalScaleV1>,
    /// Buffer size in pixels.
    pub w: i32,
    pub h: i32,
    pub dpi_scale: i32,
    pub fullscreen: bool,
}

impl Window {
    pub fn open(title: &str, w: i32, h: i32) -> Res<Window> {
        let conn =
            Connection::connect_to_env().map_err(|e| format!("no Wayland compositor ({e})"))?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
        let qh = queue.handle();
        let compositor = CompositorState::bind(&globals, &qh)?;
        let xdg = XdgShell::bind(&globals, &qh)?;
        let shm = Shm::bind(&globals, &qh)?;
        let surface = compositor.create_surface(&qh);
        let window = xdg.create_window(surface, WindowDecorations::RequestServer, &qh);
        window.set_title(title);
        window.set_app_id("solitaire-rs");
        window.set_min_size(Some((240, 180)));

        let (viewport, frac) = bind_fractional(&globals, &qh, window.wl_surface());
        window.commit();

        let pool = SlotPool::new((w * h * 4).max(4096) as usize, &shm)?;
        let mut st = State {
            registry: RegistryState::new(&globals),
            seat_state: SeatState::new(&globals, &qh),
            output_state: OutputState::new(&globals, &qh),
            compositor,
            shm,
            window,
            pointer: None,
            keyboard: None,
            keymap: None,
            mods: Default::default(),
            group: Default::default(),
            events: VecDeque::new(),
            configured: false,
            lw: w,
            lh: h,
            int_scale: 1,
            frac120: None,
            fullscreen: false,
            pointer_pos: (0.0, 0.0),
            scroll_acc: 0.0,
        };
        while !st.configured {
            queue.blocking_dispatch(&mut st)?;
        }
        let mut me = Window {
            conn,
            queue,

            st,
            pool,
            bufs: Vec::new(),
            viewport,
            _frac: frac,
            w: 0,
            h: 0,
            dpi_scale: 0,
            fullscreen: false,
        };
        me.update_size();
        Ok(me)
    }

    /// Recompute the buffer size from the logical size and scale.
    fn update_size(&mut self) -> bool {
        let s = self.st.scale();
        let w = ((self.st.lw as f64 * s).round() as i32).max(1);
        let h = ((self.st.lh as f64 * s).round() as i32).max(1);
        let ui = (s.round() as i32).max(1);
        let ui_changed = ui != self.dpi_scale;
        self.dpi_scale = ui;
        self.fullscreen = self.st.fullscreen;
        let surface = self.st.window.wl_surface();
        match (&self.viewport, self.st.frac120) {
            (Some(vp), Some(_)) => {
                surface.set_buffer_scale(1);
                vp.set_destination(self.st.lw, self.st.lh);
            }
            _ => surface.set_buffer_scale(self.st.int_scale),
        }
        let changed = w != self.w || h != self.h || ui_changed;
        self.w = w;
        self.h = h;
        changed
    }

    pub fn set_title(&self, t: &str) -> Res<()> {
        self.st.window.set_title(t);
        Ok(())
    }

    pub fn toggle_fullscreen(&mut self) -> Res<()> {
        if self.st.fullscreen {
            self.st.window.unset_fullscreen();
        } else {
            self.st.window.set_fullscreen(None);
        }
        self.conn.flush()?;
        Ok(())
    }

    /// Next event without blocking.
    pub fn poll(&mut self) -> Res<Option<Ev>> {
        if self.st.events.is_empty() {
            self.queue.dispatch_pending(&mut self.st)?;
        }
        match self.st.events.pop_front() {
            Some(Ev::Resize) => {
                // Coalesce: several configures/scale changes may be queued.
                self.st.events.retain(|e| !matches!(e, Ev::Resize));
                if self.update_size() {
                    Ok(Some(Ev::Resize))
                } else {
                    Ok(Some(Ev::Expose))
                }
            }
            e => Ok(e),
        }
    }

    /// Block until the compositor sends something or `timeout_ms` passes.
    pub fn wait(&mut self, timeout_ms: i32) {
        let _ = self.conn.flush();
        if !self.st.events.is_empty() {
            return;
        }
        if let Some(guard) = self.queue.prepare_read() {
            let mut pfd = libc::pollfd {
                fd: guard.connection_fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let n = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
            if n > 0 && pfd.revents & libc::POLLIN != 0 {
                let _ = guard.read();
            }
        }
    }

    /// Show region `r` of `img` (window sized).
    pub fn present(&mut self, img: &Image, r: Rect) -> Res<()> {
        let r = r.intersect(&img.rect());
        if r.is_empty() {
            return Ok(());
        }
        let full = img.rect();
        // Buffers of a stale size are useless.
        self.bufs
            .retain(|b| b.buffer.height() == img.h && b.buffer.stride() == img.w * 4);
        for b in &mut self.bufs {
            b.pending = b.pending.union(&r);
        }
        // Reuse a buffer the compositor has released, else make one (at
        // most three in flight).
        let idx = match (0..self.bufs.len())
            .find(|&i| self.bufs[i].buffer.canvas(&mut self.pool).is_some())
        {
            Some(i) => i,
            None => {
                if self.bufs.len() >= 3 {
                    self.bufs.remove(0);
                }
                let (buffer, _) =
                    self.pool
                        .create_buffer(img.w, img.h, img.w * 4, wl_shm::Format::Xrgb8888)?;
                self.bufs.push(Buf {
                    buffer,
                    pending: full,
                });
                self.bufs.len() - 1
            }
        };
        let b = &mut self.bufs[idx];
        let p = std::mem::take(&mut b.pending).intersect(&full);
        let canvas = b.buffer.canvas(&mut self.pool).ok_or("buffer busy")?;
        let (pre, px, _) = unsafe { canvas.align_to_mut::<u32>() };
        if !pre.is_empty() {
            return Err("misaligned shm buffer".into());
        }
        for y in p.y..p.bottom() {
            let o = (y * img.w + p.x) as usize;
            px[o..o + p.w as usize].copy_from_slice(&img.px[o..o + p.w as usize]);
        }
        let surface = self.st.window.wl_surface();
        surface.damage_buffer(r.x, r.y, r.w, r.h);
        b.buffer.attach_to(surface)?;
        surface.commit();
        self.conn.flush()?;
        Ok(())
    }
}

fn bind_fractional(
    globals: &GlobalList,
    qh: &QueueHandle<State>,
    surface: &wl_surface::WlSurface,
) -> (Option<WpViewport>, Option<WpFractionalScaleV1>) {
    let vp: Option<WpViewporter> = globals.bind(qh, 1..=1, NoEvents).ok();
    let fm: Option<WpFractionalScaleManagerV1> = globals.bind(qh, 1..=1, NoEvents).ok();
    match (vp, fm) {
        (Some(vp), Some(fm)) => (
            Some(vp.get_viewport(surface, qh, NoEvents)),
            Some(fm.get_fractional_scale(surface, qh, FracScale)),
        ),
        _ => (None, None),
    }
}

impl<I: Proxy> Dispatch2<I, State> for NoEvents {
    fn event(&self, _: &mut State, _: &I, _: I::Event, _: &Connection, _: &QueueHandle<State>) {}
}

impl Dispatch2<WpFractionalScaleV1, State> for FracScale {
    fn event(
        &self,
        st: &mut State,
        _: &WpFractionalScaleV1,
        ev: wp_fractional_scale_v1::Event,
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = ev {
            if st.frac120 != Some(scale) {
                st.frac120 = Some(scale);
                st.events.push_back(Ev::Resize);
            }
        }
    }
}

impl Dispatch2<wl_keyboard::WlKeyboard, State> for Kb {
    fn event(
        &self,
        st: &mut State,
        _: &wl_keyboard::WlKeyboard,
        ev: wl_keyboard::Event,
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        use wl_keyboard::Event;
        match ev {
            Event::Keymap {
                format: WEnum::Value(wl_keyboard::KeymapFormat::XkbV1),
                fd,
                size,
            } => {
                let file = std::fs::File::from(fd);
                let mut buf = vec![0u8; size as usize];
                if file.read_exact_at(&mut buf, 0).is_ok() {
                    while buf.last() == Some(&0) {
                        buf.pop();
                    }
                    let ctx = kbvm::xkb::Context::default();
                    let mut diags: Vec<kbvm::xkb::diagnostic::Diagnostic> = Vec::new();
                    match ctx.keymap_from_bytes(&mut diags, None, &buf) {
                        Ok(map) => st.keymap = Some(map.to_builder().build_lookup_table()),
                        Err(e) => eprintln!("solitaire-rs: cannot parse keymap: {e}"),
                    }
                }
            }
            Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                st.mods = kbvm::ModifierMask(mods_depressed | mods_latched | mods_locked);
                st.group = kbvm::GroupIndex(group);
            }
            Event::Leave { .. } => st.mods = Default::default(),
            Event::Key {
                key,
                state: WEnum::Value(wl_keyboard::KeyState::Pressed),
                ..
            } => {
                let Some(map) = &st.keymap else { return };
                let lookup = map.lookup(st.group, st.mods, kbvm::Keycode::from_evdev(key));
                if let Some(p) = lookup.into_iter().next() {
                    let mut sym = p.keysym().0;
                    // Letters: report lower case; Shift is in the modifiers.
                    if (0x41..=0x5a).contains(&sym) {
                        sym += 0x20;
                    }
                    let mods = st.key_mods();
                    st.events.push_back(Ev::Key { sym, mods });
                }
            }
            _ => {}
        }
    }
}

impl CompositorHandler for State {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        f: i32,
    ) {
        if f != self.int_scale {
            self.int_scale = f.max(1);
            if self.frac120.is_none() {
                self.events.push_back(Ev::Resize);
            }
        }
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl WindowHandler for State {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &XdgWindow) {
        self.events.push_back(Ev::Close);
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &XdgWindow,
        c: WindowConfigure,
        _: u32,
    ) {
        if let Some(w) = c.new_size.0 {
            self.lw = w.get() as i32;
        }
        if let Some(h) = c.new_size.1 {
            self.lh = h.get() as i32;
        }
        self.fullscreen = c.is_fullscreen();
        self.configured = true;
        self.events.push_back(Ev::Resize);
    }
}

impl SeatHandler for State {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        cap: Capability,
    ) {
        if cap == Capability::Keyboard && self.keyboard.is_none() {
            self.keyboard = Some(seat.get_keyboard(qh, Kb));
        }
        if cap == Capability::Pointer && self.pointer.is_none() {
            let surface = self.compositor.create_surface(qh);
            self.pointer = self
                .seat_state
                .get_pointer_with_theme::<_, ()>(
                    qh,
                    &seat,
                    self.shm.wl_shm(),
                    surface,
                    ThemeSpec::default(),
                )
                .ok();
        }
    }
    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        cap: Capability,
    ) {
        if cap == Capability::Keyboard {
            if let Some(k) = self.keyboard.take() {
                k.release();
            }
        }
        if cap == Capability::Pointer {
            if let Some(p) = self.pointer.take() {
                p.pointer().release();
            }
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

fn map_button(b: u32) -> u8 {
    match b {
        0x110 => 1, // BTN_LEFT
        0x112 => 2, // BTN_MIDDLE
        0x111 => 3, // BTN_RIGHT
        _ => 0,
    }
}

impl PointerHandler for State {
    fn pointer_frame(
        &mut self,
        conn: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for e in events {
            if &e.surface != self.window.wl_surface() {
                continue;
            }
            self.pointer_pos = e.position;
            let (x, y) = self.px(e.position.0, e.position.1);
            let mods = self.key_mods();
            match e.kind {
                PointerEventKind::Enter { .. } => {
                    if let Some(p) = &self.pointer {
                        let _ = p.set_cursor(conn, CursorIcon::Default);
                    }
                    self.events.push_back(Ev::Motion { x, y });
                }
                PointerEventKind::Leave { .. } => self.events.push_back(Ev::Leave),
                PointerEventKind::Motion { .. } => self.events.push_back(Ev::Motion { x, y }),
                PointerEventKind::Press { button, time, .. } => {
                    let button = map_button(button);
                    if button != 0 {
                        self.events.push_back(Ev::Press {
                            button,
                            x,
                            y,
                            time,
                            mods,
                        });
                    }
                }
                PointerEventKind::Release { button, .. } => {
                    let button = map_button(button);
                    if button != 0 {
                        self.events.push_back(Ev::Release { button, x, y });
                    }
                }
                PointerEventKind::Axis { vertical, time, .. } => {
                    // Wheel notches, or every ~20px of touchpad scrolling.
                    let delta = if vertical.value120 != 0 {
                        vertical.value120 as f64 / 120.0
                    } else if vertical.discrete != 0 {
                        vertical.discrete as f64
                    } else {
                        vertical.absolute / 20.0
                    };
                    self.scroll_acc += delta;
                    let steps = self.scroll_acc.trunc();
                    self.scroll_acc -= steps;
                    for _ in 0..(steps.abs() as i32).min(5) {
                        let button = if steps < 0.0 { 4 } else { 5 };
                        self.events.push_back(Ev::Press {
                            button,
                            x,
                            y,
                            time,
                            mods,
                        });
                    }
                }
            }
        }
    }
}

impl ShmHandler for State {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

smithay_client_toolkit::delegate_registry!(State);

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}

smithay_client_toolkit::delegate_dispatch2!(State);

pub mod keys {
    pub const BACKSPACE: u32 = 0xff08;
    pub const TAB: u32 = 0xff09;
    pub const RETURN: u32 = 0xff0d;
    pub const ESCAPE: u32 = 0xff1b;
    pub const PAGE_UP: u32 = 0xff55;
    pub const PAGE_DOWN: u32 = 0xff56;
    pub const MENU: u32 = 0xff67;
    pub const KP_ENTER: u32 = 0xff8d;
    pub const KP_ADD: u32 = 0xffab;
    pub const KP_SUB: u32 = 0xffad;
    pub const KP_0: u32 = 0xffb0;
    pub const KP_9: u32 = 0xffb9;
    pub const F1: u32 = 0xffbe;
    pub const F2: u32 = 0xffbf;
    pub const F5: u32 = 0xffc2;
    pub const F10: u32 = 0xffc7;
    pub const F11: u32 = 0xffc8;
    pub const ISO_LEFT_TAB: u32 = 0xfe20;
}
