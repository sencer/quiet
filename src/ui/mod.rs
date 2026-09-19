pub mod render;

use crate::config::Config;
use crate::engine::{EngineMessage, UiCommand, UiEvent, UiItem};
use crate::ui::render::Renderer;
use calloop::channel::{channel, Event as ChannelEvent};
use calloop::EventLoop;
use calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::reexports::client::{
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm::Format, wl_surface},
    Connection, QueueHandle,
};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_pointer,
    delegate_registry, delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    seat::{
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers},
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use std::sync::Arc;
use tiny_skia::PixmapMut;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

pub struct AppState {
    pub registry_state: RegistryState,
    pub seat_state: SeatState,
    pub output_state: OutputState,
    pub compositor_state: CompositorState,
    pub shm: Shm,
    pub layer_shell: LayerShell,
    pub pool: SlotPool,
    pub qh: QueueHandle<AppState>,
    pub layer_surface: Option<LayerSurface>,
    pub renderer: Renderer,
    pub items: Vec<UiItem>,
    pub context: Vec<String>,
    pub selected_index: usize,
    pub scroll_offset: usize,
    pub width: u32,
    pub height: u32,
    pub configured: bool,
    pub engine_tx: mpsc::Sender<EngineMessage>,
    pub config: Config,
    pub exit: bool,
    pub modifiers: Modifiers,
    pub scale_factor: i32,
    pub scroll_accumulator: f64,
}

delegate_compositor!(AppState);
delegate_output!(AppState);
delegate_shm!(AppState);
delegate_seat!(AppState);
delegate_keyboard!(AppState);
delegate_pointer!(AppState);
delegate_layer!(AppState);
delegate_registry!(AppState);

impl ProvidesRegistryState for AppState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    smithay_client_toolkit::registry_handlers![OutputState, SeatState];
}

impl CompositorHandler for AppState {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        let factor = new_factor.max(1);
        if self.scale_factor != factor {
            self.scale_factor = factor;
            surface.set_buffer_scale(factor);
            if self.configured {
                self.render_frame();
            }
        }
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for AppState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }
    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }
    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }
}

impl ShmHandler for AppState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl SeatHandler for AppState {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {}
    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard {
            let _ = self.seat_state.get_keyboard(qh, &seat, None);
        }
        if capability == Capability::Pointer {
            let _ = self.seat_state.get_pointer(qh, &seat);
        }
    }
    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
        _capability: Capability,
    ) {
    }
    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {
    }
}

impl PointerHandler for AppState {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            match event.kind {
                PointerEventKind::Press { button, .. } => {
                    // BTN_LEFT is 0x110 (272)
                    if button == 0x110 {
                        self.handle_click_at_y(event.position.1, false);
                    } else if button == 0x111 {
                        // BTN_RIGHT is 0x111 (273): dismiss item
                        self.handle_click_at_y(event.position.1, true);
                    }
                }
                PointerEventKind::Axis { vertical, .. } => {
                    let mut step = 0i32;
                    if vertical.discrete != 0 {
                        step = vertical.discrete;
                    } else if vertical.absolute != 0.0 {
                        self.scroll_accumulator += vertical.absolute;
                        if self.scroll_accumulator >= 15.0 {
                            step = 1;
                            self.scroll_accumulator = 0.0;
                        } else if self.scroll_accumulator <= -15.0 {
                            step = -1;
                            self.scroll_accumulator = 0.0;
                        }
                    }

                    if step > 0 && !self.items.is_empty() {
                        self.selected_index = (self.selected_index + 1).min(self.items.len().saturating_sub(1));
                        self.adjust_scroll();
                        self.render_frame();
                    } else if step < 0 && !self.items.is_empty() {
                        self.selected_index = self.selected_index.saturating_sub(1);
                        self.adjust_scroll();
                        self.render_frame();
                    }
                }
                _ => {}
            }
        }
    }
}

impl KeyboardHandler for AppState {
    fn enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _surface: &wl_surface::WlSurface,
        _serial: u32,
        _raw_keys: &[u32],
        _keysyms: &[Keysym],
    ) {
    }

    fn leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _surface: &wl_surface::WlSurface,
        _serial: u32,
    ) {
        let _ = self
            .engine_tx
            .try_send(EngineMessage::UiEvent(UiEvent::CloseUi));
        self.close_ui();
    }

    fn press_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        match event.keysym {
            Keysym::n if self.modifiers.ctrl => {
                if !self.items.is_empty() {
                    self.selected_index = (self.selected_index + 1) % self.items.len();
                    self.adjust_scroll();
                    self.render_frame();
                }
            }
            Keysym::p if self.modifiers.ctrl => {
                if !self.items.is_empty() {
                    self.selected_index = if self.selected_index == 0 {
                        self.items.len().saturating_sub(1)
                    } else {
                        self.selected_index - 1
                    };
                    self.adjust_scroll();
                    self.render_frame();
                }
            }
            Keysym::j | Keysym::Down | Keysym::Tab => {
                if !self.items.is_empty() {
                    self.selected_index = (self.selected_index + 1) % self.items.len();
                    self.adjust_scroll();
                    self.render_frame();
                }
            }
            Keysym::k | Keysym::Up | Keysym::ISO_Left_Tab => {
                if !self.items.is_empty() {
                    self.selected_index = if self.selected_index == 0 {
                        self.items.len().saturating_sub(1)
                    } else {
                        self.selected_index - 1
                    };
                    self.adjust_scroll();
                    self.render_frame();
                }
            }
            Keysym::Home => {
                if !self.items.is_empty() {
                    self.selected_index = 0;
                    self.adjust_scroll();
                    self.render_frame();
                }
            }
            Keysym::End => {
                if !self.items.is_empty() {
                    self.selected_index = self.items.len().saturating_sub(1);
                    self.adjust_scroll();
                    self.render_frame();
                }
            }
            Keysym::Page_Up => {
                if !self.items.is_empty() {
                    let page = self.max_visible_cards().max(1);
                    self.selected_index = self.selected_index.saturating_sub(page);
                    self.adjust_scroll();
                    self.render_frame();
                }
            }
            Keysym::Page_Down => {
                if !self.items.is_empty() {
                    let page = self.max_visible_cards().max(1);
                    self.selected_index = (self.selected_index + page).min(self.items.len() - 1);
                    self.adjust_scroll();
                    self.render_frame();
                }
            }
            Keysym::d | Keysym::Delete => {
                if let Some(item) = self.items.get(self.selected_index) {
                    if self.modifiers.shift || self.modifiers.ctrl {
                        if let Some(id) = item.id {
                            let _ = self
                                .engine_tx
                                .try_send(EngineMessage::UiEvent(UiEvent::DeleteItem(id)));
                        }
                    } else if item.is_group {
                        let _ =
                            self.engine_tx
                                .try_send(EngineMessage::UiEvent(UiEvent::DeleteGroup(
                                    item.key.clone(),
                                )));
                    } else if let Some(id) = item.id {
                        let _ = self
                            .engine_tx
                            .try_send(EngineMessage::UiEvent(UiEvent::DeleteItem(id)));
                    }
                }
            }
            Keysym::D => {
                // Caps Lock Safety: Only clear all if Shift is explicitly held!
                if self.modifiers.shift {
                    let _ = self
                        .engine_tx
                        .try_send(EngineMessage::UiEvent(UiEvent::ClearAll));
                } else if let Some(item) = self.items.get(self.selected_index) {
                    if item.is_group {
                        let _ =
                            self.engine_tx
                                .try_send(EngineMessage::UiEvent(UiEvent::DeleteGroup(
                                    item.key.clone(),
                                )));
                    } else if let Some(id) = item.id {
                        let _ = self
                            .engine_tx
                            .try_send(EngineMessage::UiEvent(UiEvent::DeleteItem(id)));
                    }
                }
            }
            Keysym::m if self.modifiers.ctrl => {
                self.activate_selected();
            }
            Keysym::Return | Keysym::KP_Enter | Keysym::space => {
                if self.modifiers.shift {
                    if let Some(item) = self.items.get(self.selected_index) {
                        if let Some(id) = item.id {
                            let action_key = item
                                .actions
                                .first()
                                .cloned()
                                .unwrap_or_else(|| "default".into());
                            let _ = self.engine_tx.try_send(EngineMessage::UiEvent(
                                UiEvent::ActionInvoked { id, action_key },
                            ));
                        }
                    }
                } else {
                    self.activate_selected();
                }
            }
            Keysym::grave => {
                let _ = self
                    .engine_tx
                    .try_send(EngineMessage::UiEvent(UiEvent::CloseUi));
                self.close_ui();
            }
            Keysym::BackSpace => {
                if self.modifiers.ctrl || self.modifiers.shift {
                    let _ = self
                        .engine_tx
                        .try_send(EngineMessage::UiEvent(UiEvent::CloseUi));
                    self.close_ui();
                } else {
                    let _ = self
                        .engine_tx
                        .try_send(EngineMessage::UiEvent(UiEvent::LeaveGroup));
                }
            }
            Keysym::Escape | Keysym::bracketleft if event.keysym == Keysym::Escape || self.modifiers.ctrl => {
                if self.context.is_empty() {
                    let _ = self
                        .engine_tx
                        .try_send(EngineMessage::UiEvent(UiEvent::CloseUi));
                    self.close_ui();
                } else {
                    let _ = self
                        .engine_tx
                        .try_send(EngineMessage::UiEvent(UiEvent::LeaveGroup));
                }
            }
            Keysym::h | Keysym::Left => {
                let _ = self
                    .engine_tx
                    .try_send(EngineMessage::UiEvent(UiEvent::LeaveGroup));
            }
            Keysym::q => {
                let _ = self
                    .engine_tx
                    .try_send(EngineMessage::UiEvent(UiEvent::CloseUi));
                self.close_ui();
            }
            _ => {}
        }
    }

    fn release_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        _event: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        modifiers: Modifiers,
        _layout: u32,
    ) {
        self.modifiers = modifiers;
    }
}

impl LayerShellHandler for AppState {
    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let (w, h) = configure.new_size;
        self.width = if w > 0 { w } else { self.config.ui.width };
        let target_h = self.compute_target_height();
        self.height = if h > 0 { h } else { target_h };
        self.configured = true;
        self.render_frame();
    }

    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {
        self.layer_surface = None;
        self.configured = false;
        let _ = self
            .engine_tx
            .try_send(EngineMessage::UiEvent(UiEvent::CloseUi));
    }
}

impl AppState {
    pub fn compute_target_height(&self) -> u32 {
        let card_height = 98.0;
        let card_spacing = 10.0;
        let padding = 10.0;
        let count = self.items.len().max(1);
        let max_visible = 8;
        let visible_count = count.min(max_visible);
        let h = padding * 2.0 + (visible_count as f32) * card_height
            + ((visible_count - 1) as f32) * card_spacing;
        h.round() as u32
    }

    pub fn max_visible_cards(&self) -> usize {
        let card_height = 98.0;
        let card_spacing = 10.0;
        let padding = 10.0;
        let available = (self.height as f32 - padding * 2.0).max(card_height);
        (((available + card_spacing) / (card_height + card_spacing)).floor() as usize).max(1)
    }

    pub fn adjust_scroll(&mut self) {
        if self.items.is_empty() {
            self.selected_index = 0;
            self.scroll_offset = 0;
            return;
        }
        if self.selected_index >= self.items.len() {
            self.selected_index = self.items.len().saturating_sub(1);
        }
        let max_vis = self.max_visible_cards();
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        } else if self.selected_index >= self.scroll_offset + max_vis {
            self.scroll_offset = self.selected_index - max_vis + 1;
        }
    }

    pub fn open_ui(&mut self, items: Vec<UiItem>, context: Vec<String>) {
        self.items = items;
        self.context = context;
        self.selected_index = 0;
        self.scroll_offset = 0;

        if self.config.ui.show_icons {
            for item in &self.items {
                if !item.icon.is_empty() {
                    self.renderer.icon_cache.preload(&item.icon);
                }
            }
        }

        let target_h = self.compute_target_height();
        self.height = target_h;
        self.width = self.config.ui.width;

        if self.layer_surface.is_none() {
            let surface = self.compositor_state.create_surface(&self.qh);
            surface.set_buffer_scale(self.scale_factor.max(1));

            let layer = self.layer_shell.create_layer_surface(
                &self.qh,
                surface,
                Layer::Overlay,
                Some("quiet"),
                None,
            );

            // Drop down from top center:
            layer.set_anchor(Anchor::TOP);
            layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
            layer.set_size(self.width, target_h);
            layer.set_margin(
                self.config.ui.margin_top,
                0,
                0,
                0,
            );
            layer.set_exclusive_zone(0);
            layer.wl_surface().commit();

            self.layer_surface = Some(layer);
            self.configured = false;
        } else {
            if let Some(ref layer) = self.layer_surface {
                layer.set_size(self.width, target_h);
                layer.wl_surface().commit();
            }
            if self.configured {
                self.render_frame();
            }
        }
    }

    pub fn update_items(&mut self, items: Vec<UiItem>, context: Vec<String>) {
        self.items = items;
        self.context = context;
        self.adjust_scroll();

        if self.config.ui.show_icons {
            for item in &self.items {
                if !item.icon.is_empty() {
                    self.renderer.icon_cache.preload(&item.icon);
                }
            }
        }

        let target_h = self.compute_target_height();
        self.height = target_h;
        if let Some(ref layer) = self.layer_surface {
            layer.set_size(self.config.ui.width, target_h);
            layer.wl_surface().commit();
        }

        if self.configured {
            self.render_frame();
        }
    }

    pub fn handle_click_at_y(&mut self, y: f64, is_right_click: bool) {
        let padding_y = 10.0;
        let card_height = 98.0;
        let card_spacing = 10.0;
        let y_f32 = y as f32;

        if y_f32 < padding_y || y_f32 > self.height as f32 - padding_y || self.items.is_empty() {
            let _ = self
                .engine_tx
                .try_send(EngineMessage::UiEvent(UiEvent::CloseUi));
            self.close_ui();
            return;
        }

        let mut curr_y = padding_y;
        let mut clicked_card = false;

        for idx in self.scroll_offset..self.items.len() {
            if curr_y + card_height > self.height as f32 + 1.0 {
                break;
            }
            if y_f32 >= curr_y && y_f32 <= curr_y + card_height {
                clicked_card = true;
                self.selected_index = idx;
                self.adjust_scroll();
                if is_right_click {
                    if let Some(item) = self.items.get(idx) {
                        if item.is_group {
                            let _ = self.engine_tx.try_send(EngineMessage::UiEvent(
                                UiEvent::DeleteGroup(item.key.clone()),
                            ));
                        } else if let Some(id) = item.id {
                            let _ = self
                                .engine_tx
                                .try_send(EngineMessage::UiEvent(UiEvent::DeleteItem(id)));
                        }
                    }
                } else {
                    self.activate_selected();
                }
                break;
            }
            curr_y += card_height + card_spacing;
        }

        if !clicked_card {
            let _ = self
                .engine_tx
                .try_send(EngineMessage::UiEvent(UiEvent::CloseUi));
            self.close_ui();
        }
    }

    pub fn activate_selected(&mut self) {
        if let Some(item) = self.items.get(self.selected_index) {
            if item.is_group && item.count > 1 {
                let _ = self
                    .engine_tx
                    .try_send(EngineMessage::UiEvent(UiEvent::EnterGroup(
                        item.key.clone(),
                    )));
            } else if let Some(id) = item.id {
                let action_key = item
                    .actions
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "default".into());
                let _ = self
                    .engine_tx
                    .try_send(EngineMessage::UiEvent(UiEvent::ActionInvoked {
                        id,
                        action_key,
                    }));
            }
        }
    }

    pub fn close_ui(&mut self) {
        if let Some(layer) = self.layer_surface.take() {
            drop(layer);
        }
        self.configured = false;
    }

    pub fn render_frame(&mut self) {
        if let Some(ref layer) = self.layer_surface {
            if !self.configured || self.width == 0 || self.height == 0 {
                return;
            }

            let scale = self.scale_factor.max(1);
            let phys_width = self.width.saturating_mul(scale as u32);
            let phys_height = self.height.saturating_mul(scale as u32);
            let stride = (phys_width * 4) as i32;
            let (buffer, canvas) = match self.pool.create_buffer(
                phys_width as i32,
                phys_height as i32,
                stride,
                Format::Argb8888,
            ) {
                Ok(res) => res,
                Err(e) => {
                    warn!("Failed to create shm buffer: {}", e);
                    return;
                }
            };

            if let Some(mut pixmap) = PixmapMut::from_bytes(canvas, phys_width, phys_height) {
                self.renderer.draw(
                    &mut pixmap,
                    &self.items,
                    self.selected_index,
                    self.scroll_offset,
                    &self.context,
                    &self.config,
                    scale as f32,
                );
            }

            // Swap R (byte 0) and B (byte 2) channels for Wayland ARGB8888 (which expects BGRA in memory on little-endian)
            for pixel in canvas.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }

            layer.wl_surface().attach(Some(buffer.wl_buffer()), 0, 0);
            layer
                .wl_surface()
                .damage_buffer(0, 0, phys_width as i32, phys_height as i32);
            layer.wl_surface().commit();
        }
    }
}

pub fn spawn_ui_worker(
    config: Config,
    engine_tx: mpsc::Sender<EngineMessage>,
    icon_cache: Arc<crate::icon::IconCache>,
) -> calloop::channel::Sender<UiCommand> {
    let (calloop_tx, calloop_rx) = channel::<UiCommand>();
    let ret_tx = calloop_tx.clone();

    let spawn_res = std::thread::Builder::new()
        .name("quiet-ui".into())
        .spawn(move || {
            let conn = match Connection::connect_to_env() {
                Ok(c) => c,
                Err(e) => {
                    warn!(
                        "Could not connect to Wayland display ({}); running in headless mode",
                        e
                    );
                    return;
                }
            };

            let (globals, event_queue) =
                match smithay_client_toolkit::reexports::client::globals::registry_queue_init(&conn)
                {
                    Ok(res) => res,
                    Err(e) => {
                        error!("Failed to initialize Wayland registry queue: {}", e);
                        return;
                    }
                };

            let qh = event_queue.handle();
            let compositor_state = match CompositorState::bind(&globals, &qh) {
                Ok(cs) => cs,
                Err(e) => {
                    error!("wl_compositor not available: {}", e);
                    return;
                }
            };
            let layer_shell = match LayerShell::bind(&globals, &qh) {
                Ok(ls) => ls,
                Err(e) => {
                    error!("zwlr_layer_shell_v1 not available: {}", e);
                    return;
                }
            };
            let shm = match Shm::bind(&globals, &qh) {
                Ok(s) => s,
                Err(e) => {
                    error!("wl_shm not available: {}", e);
                    return;
                }
            };

            let pool_size =
                ((config.ui.width.max(600) * 2160 * 4 * 2) as usize).max(840 * 2160 * 4 * 2);
            let pool = match SlotPool::new(pool_size, &shm) {
                Ok(p) => p,
                Err(e) => {
                    error!("Failed to create SlotPool: {}", e);
                    return;
                }
            };

            let mut app_state = AppState {
                registry_state: RegistryState::new(&globals),
                seat_state: SeatState::new(&globals, &qh),
                output_state: OutputState::new(&globals, &qh),
                compositor_state,
                shm,
                layer_shell,
                pool,
                qh,
                layer_surface: None,
                renderer: Renderer::new(icon_cache),
                items: Vec::new(),
                context: Vec::new(),
                selected_index: 0,
                scroll_offset: 0,
                width: config.ui.width,
                height: 1080,
                configured: false,
                engine_tx,
                config,
                exit: false,
                modifiers: Modifiers::default(),
                scale_factor: 1,
                scroll_accumulator: 0.0,
            };

            let mut event_loop: EventLoop<'static, AppState> = match EventLoop::try_new() {
                Ok(el) => el,
                Err(e) => {
                    error!("Failed to create calloop EventLoop: {}", e);
                    return;
                }
            };

            let wayland_source = WaylandSource::new(conn.clone(), event_queue);
            if let Err(e) = wayland_source.insert(event_loop.handle()) {
                error!("Failed to insert WaylandSource: {}", e);
                return;
            }

            if let Err(e) = event_loop
                .handle()
                .insert_source(calloop_rx, |event, _, state| match event {
                    ChannelEvent::Msg(cmd) => match cmd {
                        UiCommand::Open { items, context } => {
                            state.open_ui(items, context);
                        }
                        UiCommand::UpdateItems { items, context } => {
                            state.update_items(items, context);
                        }
                        UiCommand::Close => {
                            state.close_ui();
                        }
                    },
                    ChannelEvent::Closed => {
                        state.exit = true;
                    }
                })
            {
                error!("Failed to insert calloop channel: {}", e);
                return;
            }

            info!("Wayland UI event loop initialized successfully");

            while !app_state.exit {
                if let Err(e) = event_loop.dispatch(None, &mut app_state) {
                    error!("Error in Wayland event loop: {}", e);
                    break;
                }
            }

            info!("Wayland UI thread finished");
        });

    if let Err(e) = spawn_res {
        error!("Failed to spawn UI thread: {}", e);
    }

    ret_tx
}
