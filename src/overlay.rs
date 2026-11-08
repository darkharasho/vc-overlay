//! The overlay surface: a wlr-layer-shell surface on the Overlay layer, which KWin
//! (and other wlroots-style compositors) draw above fullscreen games.
//! Click-through when locked (empty input region); draggable in edit mode.

use crate::render::{Hits, Painter, contains};
use crate::state::{Shared, Status};
use crate::tray::Tray;
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData, Region},
    delegate_registry,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{self, EventLoop, LoopSignal},
        calloop_wayland_source::WaylandSource,
        client::{
            Connection, QueueHandle,
            globals::registry_queue_init,
            protocol::{wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
        },
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
    },
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
        },
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tiny_skia::Pixmap;

const BTN_LEFT: u32 = 0x110;
const MIN_W: u32 = 140;
const MIN_H: u32 = 100;

enum Drag {
    Move { press: (f64, f64) },
    Resize { press: (f64, f64), start: (u32, u32) },
}

struct App {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    pool: SlotPool,

    shared: Arc<Shared>,
    tray: Option<ksni::blocking::Handle<Tray>>,
    painter: Painter,
    hits: Hits,

    layer: Option<LayerSurface>,
    /// Output name the current surface was created for.
    layer_output: Option<String>,
    configured: bool,
    width: u32,
    height: u32,
    scale: i32,
    frame_pending: bool,
    dirty: bool,
    editing: bool,

    pointer: Option<wl_pointer::WlPointer>,
    drag: Option<Drag>,
    tray_snapshot: (Status, Option<String>, bool, usize),
    signal: LoopSignal,
}

pub fn run(shared: Arc<Shared>, tray: Option<ksni::blocking::Handle<Tray>>) -> Result<(), String> {
    let conn = Connection::connect_to_env().map_err(|e| format!("no Wayland session: {e}"))?;
    let (globals, event_queue) = registry_queue_init::<App>(&conn).map_err(|e| e.to_string())?;
    let qh = event_queue.handle();

    let mut event_loop: EventLoop<App> = EventLoop::try_new().map_err(|e| e.to_string())?;
    WaylandSource::new(conn.clone(), event_queue)
        .insert(event_loop.handle())
        .map_err(|e| e.to_string())?;

    // Other threads poke this to redraw / re-sync.
    let (ping, ping_source) = calloop::ping::make_ping().map_err(|e| e.to_string())?;
    {
        let qh = qh.clone();
        event_loop
            .handle()
            .insert_source(ping_source, move |_, _, app: &mut App| app.sync(&qh))
            .map_err(|e| e.to_string())?;
    }
    shared.set_waker(move || ping.ping());
    let wake: Arc<dyn Fn() + Send + Sync> = {
        let shared = shared.clone();
        Arc::new(move || shared.repaint())
    };

    let compositor = CompositorState::bind(&globals, &qh).map_err(|e| format!("wl_compositor: {e}"))?;
    let layer_shell = LayerShell::bind(&globals, &qh)
        .map_err(|_| "this compositor has no layer-shell support (needs KDE Plasma, Sway, Hyprland…)".to_string())?;
    let shm = Shm::bind(&globals, &qh).map_err(|e| format!("wl_shm: {e}"))?;
    let pool = SlotPool::new(256 * 512 * 4, &shm).map_err(|e| e.to_string())?;

    let cfg = shared.config().clone();
    let mut app = App {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        compositor,
        layer_shell,
        shm,
        pool,
        painter: Painter::new(wake),
        hits: Hits::default(),
        tray,
        layer: None,
        layer_output: None,
        configured: false,
        width: cfg.width.max(MIN_W),
        height: cfg.height.max(MIN_H),
        scale: 1,
        frame_pending: false,
        dirty: false,
        editing: shared.is_editing(),
        pointer: None,
        drag: None,
        tray_snapshot: (Status::NoDiscord, None, false, usize::MAX),
        signal: event_loop.get_signal(),
        shared,
    };

    // Learn the outputs before picking one.
    event_loop.dispatch(std::time::Duration::from_millis(200), &mut app).map_err(|e| e.to_string())?;
    app.refresh_outputs();
    app.create_layer(&qh);

    event_loop.run(None, &mut app, |_| {}).map_err(|e| e.to_string())
}

impl App {
    fn create_layer(&mut self, qh: &QueueHandle<Self>) {
        self.layer = None; // dropping destroys the old surface
        self.configured = false;
        self.frame_pending = false;

        let cfg = self.shared.config().clone();
        let output = cfg.output.as_ref().and_then(|want| {
            self.output_state
                .outputs()
                .find(|o| self.output_state.info(o).and_then(|i| i.name).as_ref() == Some(want))
        });
        self.layer_output = cfg.output.clone();

        let surface = self.compositor.create_surface(qh);
        let layer = self.layer_shell.create_layer_surface(qh, surface, Layer::Overlay, Some("vc-overlay"), output.as_ref());
        layer.set_anchor(Anchor::TOP | Anchor::LEFT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_size(self.width, self.height);
        layer.set_margin(cfg.y.max(0), 0, 0, cfg.x.max(0));
        self.layer = Some(layer);
        self.apply_input_region();
        if let Some(l) = &self.layer {
            l.commit();
        }
    }

    /// Locked: empty input region so clicks go to the game. Editing: whole surface.
    fn apply_input_region(&self) {
        let Some(layer) = &self.layer else { return };
        if self.editing {
            layer.set_input_region(None);
        } else if let Ok(empty) = Region::new(&self.compositor) {
            layer.set_input_region(Some(empty.wl_region()));
        }
    }

    fn refresh_outputs(&mut self) {
        let list: Vec<(String, String)> = self
            .output_state
            .outputs()
            .filter_map(|o| self.output_state.info(&o))
            .filter_map(|i| {
                let name = i.name.clone()?;
                let desc = format!("{name} — {} {}", i.make, i.model).trim().to_string();
                Some((name, desc))
            })
            .collect();
        *self.shared.outputs.lock().unwrap_or_else(|e| e.into_inner()) = list;
        self.shared.repaint();
    }

    /// Called whenever shared state changes.
    fn sync(&mut self, qh: &QueueHandle<Self>) {
        if self.shared.quit.load(Ordering::Relaxed) {
            self.signal.stop();
            return;
        }

        let want_output = self.shared.config().output.clone();
        if want_output != self.layer_output {
            self.create_layer(qh);
        }

        let editing = self.shared.is_editing();
        if editing != self.editing {
            self.editing = editing;
            self.drag = None;
            self.apply_input_region();
            if let Some(l) = &self.layer {
                l.commit();
            }
        }

        self.sync_tray();
        self.request_draw(qh);
    }

    fn sync_tray(&mut self) {
        let snap = {
            let v = self.shared.voice();
            let outputs = self.shared.outputs.lock().map(|o| o.len()).unwrap_or(0);
            (v.status.clone(), v.channel_name.clone(), self.editing, outputs)
        };
        if snap != self.tray_snapshot {
            self.tray_snapshot = snap;
            if let Some(t) = &self.tray {
                t.update(|_| ());
            }
        }
    }

    fn request_draw(&mut self, qh: &QueueHandle<Self>) {
        if !self.configured {
            return;
        }
        if self.frame_pending {
            self.dirty = true;
            return;
        }
        self.draw(qh);
    }

    fn draw(&mut self, qh: &QueueHandle<Self>) {
        let Some(layer) = &self.layer else { return };
        let scale = self.scale.max(1);
        let (w, h) = (self.width * scale as u32, self.height * scale as u32);
        let Some(mut pm) = Pixmap::new(w, h) else { return };

        let voice = self.shared.voice().clone();
        let cfg = self.shared.config().clone();
        self.hits = self.painter.draw(&mut pm, scale as f32, &voice, &cfg, self.editing);

        let Ok((buffer, canvas)) =
            self.pool.create_buffer(w as i32, h as i32, w as i32 * 4, wl_shm::Format::Argb8888)
        else {
            return;
        };
        // tiny-skia is premultiplied RGBA; ARGB8888 little-endian is B,G,R,A in memory.
        for (dst, src) in canvas.chunks_exact_mut(4).zip(pm.data().chunks_exact(4)) {
            dst.copy_from_slice(&[src[2], src[1], src[0], src[3]]);
        }

        let surface = layer.wl_surface();
        surface.set_buffer_scale(scale);
        surface.damage_buffer(0, 0, w as i32, h as i32);
        surface.frame(qh, FrameCallbackData(surface.clone()));
        if buffer.attach_to(surface).is_ok() {
            layer.commit();
            self.frame_pending = true;
            self.dirty = false;
        }
    }

    fn on_pointer(&mut self, ev: &PointerEvent) {
        if !self.editing {
            return;
        }
        let (px, py) = ev.position;
        match ev.kind {
            PointerEventKind::Press { button: BTN_LEFT, .. } => {
                if contains(self.hits.done, px as f32, py as f32) {
                    self.shared.editing.store(false, Ordering::Relaxed);
                    self.shared.repaint();
                } else if contains(self.hits.grip, px as f32, py as f32) {
                    self.drag = Some(Drag::Resize { press: (px, py), start: (self.width, self.height) });
                } else {
                    self.drag = Some(Drag::Move { press: (px, py) });
                }
            }
            PointerEventKind::Motion { .. } => match self.drag {
                Some(Drag::Move { press }) => {
                    let (dx, dy) = ((px - press.0).round() as i32, (py - press.1).round() as i32);
                    if dx == 0 && dy == 0 {
                        return;
                    }
                    let (x, y) = {
                        let mut c = self.shared.config();
                        c.x = (c.x + dx).max(0);
                        c.y = (c.y + dy).max(0);
                        (c.x, c.y)
                    };
                    if let Some(l) = &self.layer {
                        l.set_margin(y, 0, 0, x);
                        l.commit();
                    }
                }
                Some(Drag::Resize { press, start }) => {
                    let w = (start.0 as f64 + px - press.0).max(MIN_W as f64) as u32;
                    let h = (start.1 as f64 + py - press.1).max(MIN_H as f64) as u32;
                    if (w, h) != (self.width, self.height) {
                        {
                            let mut c = self.shared.config();
                            c.width = w;
                            c.height = h;
                        }
                        if let Some(l) = &self.layer {
                            l.set_size(w, h);
                            l.commit();
                        }
                    }
                }
                None => {}
            },
            PointerEventKind::Release { button: BTN_LEFT, .. } => {
                if self.drag.take().is_some() {
                    self.shared.update_config(|_| ()); // persist
                }
            }
            _ => {}
        }
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(&mut self, _: &Connection, qh: &QueueHandle<Self>, _: &wl_surface::WlSurface, factor: i32) {
        if factor != self.scale {
            self.scale = factor;
            self.request_draw(qh);
        }
    }
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}
    fn frame(&mut self, _: &Connection, qh: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {
        self.frame_pending = false;
        if self.dirty {
            self.draw(qh);
        }
    }
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.refresh_outputs();
    }
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.refresh_outputs();
    }
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.refresh_outputs();
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, qh: &QueueHandle<Self>, _: &LayerSurface) {
        // Output went away; recreate on whatever the compositor picks.
        self.create_layer(qh);
    }

    fn configure(&mut self, _: &Connection, qh: &QueueHandle<Self>, _: &LayerSurface, cfg: LayerSurfaceConfigure, _: u32) {
        if cfg.new_size.0 > 0 {
            self.width = cfg.new_size.0;
        }
        if cfg.new_size.1 > 0 {
            self.height = cfg.new_size.1;
        }
        self.configured = true;
        self.frame_pending = false;
        self.draw(qh);
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat, cap: Capability) {
        if cap == Capability::Pointer && self.pointer.is_none() {
            self.pointer = self.seat_state.get_pointer(qh, &seat).ok();
        }
    }
    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat, cap: Capability) {
        if cap == Capability::Pointer {
            if let Some(p) = self.pointer.take() {
                p.release();
            }
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for App {
    fn pointer_frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_pointer::WlPointer, events: &[PointerEvent]) {
        for ev in events {
            if self.layer.as_ref().is_some_and(|l| &ev.surface == l.wl_surface()) {
                self.on_pointer(ev);
            }
        }
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

delegate_registry!(App);
smithay_client_toolkit::delegate_dispatch2!(App);
