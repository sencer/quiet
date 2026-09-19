pub mod render;

use crate::config::{Config, UiConfig};
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
    pub last_action_time: std::time::Instant,
    pub context_history: std::collections::HashMap<Vec<String>, (usize, usize)>,
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

/// Pure navigation and layout state tracker, decoupled from Wayland types for testability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavState {
    pub selected_index: usize,
    pub scroll_offset: usize,
    pub height: u32,
    pub context: Vec<String>,
    pub context_history: std::collections::HashMap<Vec<String>, (usize, usize)>,
}

impl NavState {
    pub fn new(config: &UiConfig) -> Self {
        Self {
            selected_index: 0,
            scroll_offset: 0,
            height: Self::calc_target_height(0, config),
            context: Vec::new(),
            context_history: std::collections::HashMap::new(),
        }
    }

    pub fn calc_target_height(count: usize, config: &UiConfig) -> u32 {
        let card_height = config.card_height;
        let card_spacing = config.card_spacing;
        let padding = config.padding;
        let count = count.max(1);
        let max_visible = config.max_visible_cards;
        let visible_count = count.min(max_visible);
        let h = padding * 2.0 + (visible_count as f32) * card_height
            + ((visible_count - 1) as f32) * card_spacing;
        h.round() as u32
    }

    pub fn calc_max_visible_cards(height: u32, config: &UiConfig) -> usize {
        let card_height = config.card_height;
        let card_spacing = config.card_spacing;
        let padding = config.padding;
        let available = (height as f32 - padding * 2.0).max(card_height);
        (((available + card_spacing) / (card_height + card_spacing)).floor() as usize).max(1)
    }

    pub fn adjust_scroll(&mut self, items_len: usize, config: &UiConfig) {
        if items_len == 0 {
            self.selected_index = 0;
            self.scroll_offset = 0;
            return;
        }
        if self.selected_index >= items_len {
            self.selected_index = items_len.saturating_sub(1);
        }
        let max_vis = Self::calc_max_visible_cards(self.height, config);
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        } else if self.selected_index >= self.scroll_offset + max_vis {
            self.scroll_offset = self.selected_index - max_vis + 1;
        }
    }

    pub fn open_ui(&mut self, items_len: usize, context: Vec<String>, config: &UiConfig) {
        self.context = context;
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.context_history.clear();
        self.height = Self::calc_target_height(items_len, config);
    }

    pub fn update_items(&mut self, items_len: usize, new_context: Vec<String>, config: &UiConfig) {
        let old_context = std::mem::replace(&mut self.context, new_context);
        let context_changed = old_context != self.context;

        let target_h = Self::calc_target_height(items_len, config);
        self.height = target_h;

        if context_changed {
            self.context_history
                .insert(old_context, (self.selected_index, self.scroll_offset));
            if let Some(&(idx, offset)) = self.context_history.get(&self.context) {
                self.selected_index = idx.min(items_len.saturating_sub(1));
                self.scroll_offset = offset;
            } else {
                self.selected_index = 0;
                self.scroll_offset = 0;
            }
        }

        self.adjust_scroll(items_len, config);
    }

    pub fn handle_configure(&mut self, items_len: usize, _stale_h: u32, config: &UiConfig) {
        self.height = Self::calc_target_height(items_len, config);
        self.adjust_scroll(items_len, config);
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
        let (w, _h) = configure.new_size;
        self.width = if w > 0 { w } else { self.config.ui.width };
        let mut nav = self.to_nav_state();
        nav.handle_configure(self.items.len(), _h, &self.config.ui);
        self.apply_nav_state(nav);
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
    pub fn to_nav_state(&self) -> NavState {
        NavState {
            selected_index: self.selected_index,
            scroll_offset: self.scroll_offset,
            height: self.height,
            context: self.context.clone(),
            context_history: self.context_history.clone(),
        }
    }

    pub fn apply_nav_state(&mut self, nav: NavState) {
        self.selected_index = nav.selected_index;
        self.scroll_offset = nav.scroll_offset;
        self.height = nav.height;
        self.context = nav.context;
        self.context_history = nav.context_history;
    }

    pub fn compute_target_height(&self) -> u32 {
        NavState::calc_target_height(self.items.len(), &self.config.ui)
    }

    pub fn max_visible_cards(&self) -> usize {
        NavState::calc_max_visible_cards(self.height, &self.config.ui)
    }

    pub fn adjust_scroll(&mut self) {
        let mut nav = self.to_nav_state();
        nav.adjust_scroll(self.items.len(), &self.config.ui);
        self.selected_index = nav.selected_index;
        self.scroll_offset = nav.scroll_offset;
    }

    pub fn open_ui(&mut self, items: Vec<UiItem>, context: Vec<String>) {
        self.items = items;
        let mut nav = self.to_nav_state();
        nav.open_ui(self.items.len(), context, &self.config.ui);
        self.apply_nav_state(nav);
        self.last_action_time = std::time::Instant::now();

        if self.config.ui.show_icons {
            for item in &self.items {
                if !item.icon.is_empty() {
                    self.renderer.icon_cache.preload(&item.icon);
                }
            }
        }

        let target_h = self.height;
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
        let mut nav = self.to_nav_state();
        nav.update_items(self.items.len(), context, &self.config.ui);
        self.apply_nav_state(nav);

        let target_h = self.height;

        if self.config.ui.show_icons {
            for item in &self.items {
                if !item.icon.is_empty() {
                    self.renderer.icon_cache.preload(&item.icon);
                }
            }
        }

        if let Some(ref layer) = self.layer_surface {
            layer.set_size(self.config.ui.width, target_h);
            layer.wl_surface().commit();
        }

        if self.configured {
            self.render_frame();
        }
    }

    pub fn handle_click_at_y(&mut self, y: f64, is_right_click: bool) {
        let padding_y = self.config.ui.padding;
        let card_height = self.config.ui.card_height;
        let card_spacing = self.config.ui.card_spacing;
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
        if self.last_action_time.elapsed() < std::time::Duration::from_millis(150) {
            return;
        }
        self.last_action_time = std::time::Instant::now();

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
                last_action_time: std::time::Instant::now(),
                context_history: std::collections::HashMap::new(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::UiConfig;

    #[test]
    fn test_nav_selection_reset_on_descend_to_child_group() {
        let config = UiConfig::default();
        let mut nav = NavState::new(&config);

        // Root view with 5 items: Spotify (0), WhatsApp (1), Gmail (2), Slack (3), System (4)
        nav.open_ui(5, vec![], &config);
        assert_eq!(nav.selected_index, 0);

        // User navigates cursor down to WhatsApp at index 1
        nav.selected_index = 1;
        assert_eq!(nav.selected_index, 1);

        // Enter WhatsApp group (contains 2 subclusters: [0] Dev Team, [1] Mom)
        nav.update_items(2, vec!["WhatsApp".to_string()], &config);

        // REGRESSION TEST: Selection must reset to 0 (Dev Team), NOT inherit index 1 (Mom)!
        assert_eq!(
            nav.selected_index, 0,
            "Entering a child group must select the top item (Dev Team at 0), not inherit index 1 (Mom)"
        );
        assert_eq!(nav.scroll_offset, 0);

        // User navigates inside WhatsApp to Mom at index 1
        nav.selected_index = 1;

        // Enter Mom's conversation (contains 3 messages: [0] oldest, [1] middle, [2] newest)
        nav.update_items(
            3,
            vec!["WhatsApp".to_string(), "Mom".to_string()],
            &config,
        );

        // REGRESSION TEST: Selection must reset to 0 (oldest/first message)
        assert_eq!(
            nav.selected_index, 0,
            "Entering Mom's conversation must select the first message (index 0)"
        );
    }

    #[test]
    fn test_nav_selection_restore_on_ascend() {
        let config = UiConfig::default();
        let mut nav = NavState::new(&config);

        // Root view with 5 items
        nav.open_ui(5, vec![], &config);
        // Select Gmail at index 2
        nav.selected_index = 2;

        // Descend into Gmail (3 messages)
        nav.update_items(3, vec!["Gmail".to_string()], &config);
        assert_eq!(nav.selected_index, 0, "Descend starts at 0");

        // Inside Gmail, move to message 2
        nav.selected_index = 2;

        // Press Backspace to ascend back to root []
        nav.update_items(5, vec![], &config);

        // REGRESSION TEST: Selected index must be restored to 2 (Gmail) in root context
        assert_eq!(
            nav.selected_index, 2,
            "Ascending back to root must restore the previously selected Gmail card at index 2"
        );
    }

    #[test]
    fn test_nav_height_recalculated_and_scroll_offset_zero_on_ascend() {
        let config = UiConfig::default();
        let mut nav = NavState::new(&config);

        // Root view with 5 items
        nav.open_ui(5, vec![], &config);
        // Expected height: padding(10)*2 + 5*card_height(98) + 4*spacing(10) = 20 + 490 + 40 = 550
        assert_eq!(nav.height, 550);
        assert_eq!(NavState::calc_max_visible_cards(nav.height, &config), 5);
        assert_eq!(nav.scroll_offset, 0);

        // Select Gmail at index 2
        nav.selected_index = 2;
        nav.adjust_scroll(5, &config);
        assert_eq!(nav.scroll_offset, 0);

        // Enter Gmail (3 items)
        nav.update_items(3, vec!["Gmail".to_string()], &config);
        // Expected height: 20 + 3*98 + 2*10 = 334
        assert_eq!(nav.height, 334);
        assert_eq!(NavState::calc_max_visible_cards(nav.height, &config), 3);
        assert_eq!(nav.selected_index, 0);
        assert_eq!(nav.scroll_offset, 0);

        // Compositor sends configure for layer shell while in 3-item sub-group
        nav.handle_configure(3, 334, &config);
        assert_eq!(nav.height, 334);

        // User presses Backspace to ascend back to root (5 items)
        nav.update_items(5, vec![], &config);

        // REGRESSION TEST:
        // 1. Height must be 550 (not stuck at 334)
        assert_eq!(
            nav.height, 550,
            "Returning to 5 root items must recompute height to 550, not remain stuck at 334"
        );
        // 2. Selected index restored to 2
        assert_eq!(nav.selected_index, 2);
        // 3. Scroll offset must be 0, NOT scrolled down hiding top 2 rows
        assert_eq!(
            nav.scroll_offset, 0,
            "Returning to root must keep scroll_offset at 0 so all 5 items are visible without scrolling"
        );
        assert!(
            nav.scroll_offset + NavState::calc_max_visible_cards(nav.height, &config) >= 5,
            "All 5 items must be completely visible in viewport"
        );

        // REGRESSION TEST: Compositor race condition
        // Sway may send a configure event with stale height 334 from before the resize
        nav.handle_configure(5, 334, &config);
        assert_eq!(
            nav.height, 550,
            "Stale configure height (334) must NOT overwrite target height (550)"
        );
        assert_eq!(nav.scroll_offset, 0);
    }

    #[test]
    fn test_nav_scroll_adjustment_when_scrolling_past_viewport() {
        let config = UiConfig::default(); // max_visible_cards = 8
        let mut nav = NavState::new(&config);

        // 10 items
        nav.open_ui(10, vec![], &config);
        assert_eq!(NavState::calc_max_visible_cards(nav.height, &config), 8);
        assert_eq!(nav.scroll_offset, 0);

        // Navigate to index 7 (fits in first page 0..7)
        nav.selected_index = 7;
        nav.adjust_scroll(10, &config);
        assert_eq!(nav.scroll_offset, 0);

        // Navigate to index 8 (scrolls viewport by 1)
        nav.selected_index = 8;
        nav.adjust_scroll(10, &config);
        assert_eq!(nav.scroll_offset, 1);

        // Navigate to index 9 (scrolls viewport by 2)
        nav.selected_index = 9;
        nav.adjust_scroll(10, &config);
        assert_eq!(nav.scroll_offset, 2);

        // Navigate back up to index 1 (scrolls offset back to 1)
        nav.selected_index = 1;
        nav.adjust_scroll(10, &config);
        assert_eq!(nav.scroll_offset, 1);

        // Navigate back up to index 0 (scrolls offset back to 0)
        nav.selected_index = 0;
        nav.adjust_scroll(10, &config);
        assert_eq!(nav.scroll_offset, 0);
    }
}
