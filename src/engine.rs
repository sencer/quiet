use crate::config::Config;
use crate::data::cluster::{ClusterNode, NotificationCluster};
use crate::data::notification::Notification;
use crate::data::persistence::{load_dump, save_dump};
use crate::data::urgency::extract_urgency;
use crate::dbus::NotificationsInterface;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::StreamExt;
use tokio_util::time::delay_queue::Key as DelayKey;
use tokio_util::time::DelayQueue;
use tracing::{debug, info, warn};
use zbus::object_server::SignalEmitter;
use zvariant::OwnedValue;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiItem {
    pub id: Option<u32>,
    pub is_group: bool,
    pub key: String,
    pub title: String,
    pub subtitle: String,
    pub summary: String,
    pub app_name: String,
    pub body: String,
    pub time_str: String,
    pub urgency: u8,
    pub count: usize,
    pub icon: String,
    pub actions: Vec<String>,
    pub created_at: u64,
}

#[derive(Debug, Clone)]
pub enum UiCommand {
    Open {
        items: Vec<UiItem>,
        context: Vec<String>,
    },
    UpdateItems {
        items: Vec<UiItem>,
        context: Vec<String>,
    },
    Close,
}

#[derive(Clone)]
pub enum UiSender {
    Tokio(mpsc::Sender<UiCommand>),
    Calloop(calloop::channel::Sender<UiCommand>),
}

impl UiSender {
    pub fn send_command(&self, cmd: UiCommand) {
        match self {
            UiSender::Tokio(tx) => {
                let _ = tx.try_send(cmd);
            }
            UiSender::Calloop(tx) => {
                let _ = tx.send(cmd);
            }
        }
    }
}

impl From<mpsc::Sender<UiCommand>> for UiSender {
    fn from(tx: mpsc::Sender<UiCommand>) -> Self {
        UiSender::Tokio(tx)
    }
}

impl From<calloop::channel::Sender<UiCommand>> for UiSender {
    fn from(tx: calloop::channel::Sender<UiCommand>) -> Self {
        UiSender::Calloop(tx)
    }
}

#[derive(Debug, Clone)]
pub enum UiEvent {
    EnterGroup(String),
    LeaveGroup,
    ActionInvoked { id: u32, action_key: String },
    DeleteItem(u32),
    DeleteGroup(String),
    ClearAll,
    CloseUi,
}

pub enum EngineMessage {
    Notify {
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
        reply: oneshot::Sender<u32>,
    },
    CloseNotification {
        id: u32,
        reason: u32,
    },
    GetCapabilities {
        reply: oneshot::Sender<Vec<String>>,
    },
    GetServerInformation {
        reply: oneshot::Sender<(String, String, String, String)>,
    },
    DumpNotifications {
        reply: oneshot::Sender<String>,
    },
    ShowNotificationCount {
        reply: oneshot::Sender<(u32, u32)>,
    },
    ShowNotifications,
    SignalNotificationCount,
    Quit,
    UiEvent(UiEvent),
}

pub struct Engine {
    tree: NotificationCluster,
    map: HashMap<u32, Vec<String>>,
    last: Option<Notification>,
    id_counter: u32,
    config: Config,
    dump_path: PathBuf,
    dbus_conn: Option<zbus::Connection>,
    ui_tx: UiSender,
    ui_open: bool,
    current_context: Vec<String>,
    delay_queue: DelayQueue<u32>,
    delay_keys: HashMap<u32, DelayKey>,
    rx: mpsc::Receiver<EngineMessage>,
    dump_dirty: bool,
    last_dump_time: Instant,
    icon_cache: Arc<crate::icon::IconCache>,
}

fn unwrap_variant<'a>(mut val: &'a zvariant::Value<'a>) -> &'a zvariant::Value<'a> {
    while let zvariant::Value::Value(inner) = val {
        val = inner;
    }
    val
}

fn parse_image_data_hint(
    hints: &HashMap<String, OwnedValue>,
    target_size: u32,
) -> Option<tiny_skia::Pixmap> {
    let owned = hints
        .get("image-data")
        .or_else(|| hints.get("image_data"))
        .or_else(|| hints.get("icon_data"))?;

    let val = unwrap_variant(owned);
    if let zvariant::Value::Structure(ref st) = val {
        let fields = st.fields();
        if fields.len() >= 7 {
            let width = match unwrap_variant(&fields[0]) {
                zvariant::Value::I32(v) => *v,
                _ => return None,
            };
            let height = match unwrap_variant(&fields[1]) {
                zvariant::Value::I32(v) => *v,
                _ => return None,
            };
            let rowstride = match unwrap_variant(&fields[2]) {
                zvariant::Value::I32(v) => *v,
                _ => return None,
            };
            let has_alpha = match unwrap_variant(&fields[3]) {
                zvariant::Value::Bool(v) => *v,
                _ => return None,
            };
            let bits_per_sample = match unwrap_variant(&fields[4]) {
                zvariant::Value::I32(v) => *v,
                _ => return None,
            };
            let channels = match unwrap_variant(&fields[5]) {
                zvariant::Value::I32(v) => *v,
                _ => return None,
            };
            let data: Vec<u8> = match unwrap_variant(&fields[6]) {
                zvariant::Value::Array(arr) => arr
                    .iter()
                    .filter_map(|v| match unwrap_variant(v) {
                        zvariant::Value::U8(b) => Some(*b),
                        _ => None,
                    })
                    .collect(),
                _ => return None,
            };
            return crate::icon::pixmap_from_image_data(
                width,
                height,
                rowstride,
                has_alpha,
                bits_per_sample,
                channels,
                &data,
                target_size,
            );
        }
    }
    None
}

impl Engine {
    pub fn new(
        config: Config,
        dump_path: PathBuf,
        ui_tx: impl Into<UiSender>,
        rx: mpsc::Receiver<EngineMessage>,
        icon_cache: Arc<crate::icon::IconCache>,
    ) -> Self {
        let mut engine = Self {
            tree: NotificationCluster::new(),
            map: HashMap::new(),
            last: None,
            id_counter: 1,
            config,
            dump_path,
            dbus_conn: None,
            ui_tx: ui_tx.into(),
            ui_open: false,
            current_context: Vec::new(),
            delay_queue: DelayQueue::new(),
            delay_keys: HashMap::new(),
            rx,
            dump_dirty: false,
            last_dump_time: Instant::now(),
            icon_cache,
        };

        engine.restore_dump();
        engine
    }

    pub fn set_dbus_connection(&mut self, conn: zbus::Connection) {
        self.dbus_conn = Some(conn);
    }

    fn restore_dump(&mut self) {
        let restored = load_dump(&self.dump_path);
        let mut max_id = 0;
        let now_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;

        for mut notif in restored {
            if notif.id > max_id {
                max_id = notif.id;
            }
            let keys = if let Some(k) = notif.keys.take() {
                k
            } else {
                self.config.apply_and_get_keys(&mut notif)
            };
            let id = notif.id;

            if notif.expires {
                if let Some(exp_ns) = notif.expires_at {
                    if exp_ns <= now_ns {
                        continue;
                    }
                    let remaining_ms = (exp_ns - now_ns) / 1_000_000;
                    let key = self
                        .delay_queue
                        .insert(id, Duration::from_millis(remaining_ms));
                    self.delay_keys.insert(id, key);
                }
            }

            self.map.insert(id, keys.clone());
            self.tree.insert(&keys, notif.clone());
            self.last = Some(notif);
        }

        self.id_counter = max_id + 1;
    }

    pub async fn run(&mut self) {
        info!("Quiet Engine actor running");

        // Emit initial notification count on startup
        self.emit_notifications_updated(2).await;

        let mut dump_sleep: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = None;

        loop {
            tokio::select! {
                Some(msg) = self.rx.recv() => {
                    if self.handle_message(msg).await {
                        break; // Quit requested
                    }
                    if self.dump_dirty && dump_sleep.is_none() {
                        dump_sleep = Some(Box::pin(tokio::time::sleep(Duration::from_millis(500))));
                    }
                }
                Some(expired) = self.delay_queue.next() => {
                    let id = expired.into_inner();
                    self.delay_keys.remove(&id);
                    self.handle_expiration(id).await;
                    if self.dump_dirty && dump_sleep.is_none() {
                        dump_sleep = Some(Box::pin(tokio::time::sleep(Duration::from_millis(500))));
                    }
                }
                _ = async {
                    match dump_sleep.as_mut() {
                        Some(sleep) => sleep.await,
                        None => std::future::pending::<()>().await,
                    }
                } => {
                    dump_sleep = None;
                    if self.dump_dirty {
                        self.flush_dump();
                    }
                }
                else => break,
            }
        }

        info!("Quiet Engine shutting down; flushing dump to disk");
        self.flush_dump();
    }

    async fn handle_message(&mut self, msg: EngineMessage) -> bool {
        match msg {
            EngineMessage::Notify {
                app_name,
                replaces_id,
                app_icon,
                summary,
                body,
                actions,
                hints,
                expire_timeout,
                reply,
            } => {
                let urgency = extract_urgency(&hints);
                let id = if replaces_id > 0 {
                    replaces_id
                } else {
                    while self.id_counter == 0 || self.map.contains_key(&self.id_counter) {
                        self.id_counter = self.id_counter.wrapping_add(1);
                    }
                    let new_id = self.id_counter;
                    self.id_counter = self.id_counter.wrapping_add(1);
                    new_id
                };

                let mut resolved_app = app_name;
                let mut resolved_icon = app_icon;

                // 1. Resolve raw image data hint (image-data, image_data, icon_data)
                if let Some(pixmap) = parse_image_data_hint(&hints, self.icon_cache.target_size()) {
                    let raw_key = format!("raw-image:{}", id);
                    self.icon_cache.insert_pixmap(raw_key.clone(), pixmap);
                    resolved_icon = raw_key;
                }

                // 2. Resolve via desktop-entry hint if present
                if resolved_icon.is_empty() || resolved_app.is_empty() {
                    if let Some(entry_val) = hints.get("desktop-entry") {
                        let s_opt = match unwrap_variant(entry_val) {
                            zvariant::Value::Str(s) => Some(s.as_str()),
                            _ => None,
                        };
                        if let Some(s) = s_opt {
                            if let Some(entry) = crate::icon::desktop::lookup_desktop_entry(s) {
                                if resolved_app.is_empty() {
                                    resolved_app = entry.name;
                                }
                                if resolved_icon.is_empty() {
                                    resolved_icon = entry.icon;
                                }
                            }
                        }
                    }
                }

                // 3. Resolve via app_name if icon is missing
                if resolved_icon.is_empty() && !resolved_app.is_empty() {
                    if let Some(entry) = crate::icon::desktop::lookup_desktop_entry(&resolved_app) {
                        resolved_icon = entry.icon;
                    }
                }

                // 4. Resolve via image-path / image_path hint if icon is missing
                if resolved_icon.is_empty() {
                    if let Some(path_val) =
                        hints.get("image-path").or_else(|| hints.get("image_path"))
                    {
                        if let zvariant::Value::Str(s) = unwrap_variant(path_val) {
                            resolved_icon = s.to_string();
                        }
                    }
                }

                // Parse standard Freedesktop spec 1.2 hints
                let transient = hints
                    .get("transient")
                    .map(|v| match unwrap_variant(v) {
                        zvariant::Value::Bool(b) => *b,
                        _ => false,
                    })
                    .unwrap_or(false);

                let resident = hints
                    .get("resident")
                    .map(|v| match unwrap_variant(v) {
                        zvariant::Value::Bool(b) => *b,
                        _ => false,
                    })
                    .unwrap_or(false);

                let action_icons = hints
                    .get("action-icons")
                    .map(|v| match unwrap_variant(v) {
                        zvariant::Value::Bool(b) => *b,
                        _ => false,
                    })
                    .unwrap_or(false);

                let category = hints.get("category").and_then(|v| match unwrap_variant(v) {
                    zvariant::Value::Str(s) => Some(s.to_string()),
                    _ => None,
                });

                let sound_file = hints
                    .get("sound-file")
                    .or_else(|| hints.get("sound-name"))
                    .and_then(|v| match unwrap_variant(v) {
                        zvariant::Value::Str(s) => Some(s.to_string()),
                        _ => None,
                    });

                let suppress_sound = hints
                    .get("suppress-sound")
                    .map(|v| match unwrap_variant(v) {
                        zvariant::Value::Bool(b) => *b,
                        _ => false,
                    })
                    .unwrap_or(false);

                let mut notif = Notification::new(
                    id,
                    resolved_app,
                    resolved_icon,
                    summary,
                    body,
                    actions,
                    urgency,
                    expire_timeout,
                );

                notif.transient = transient;
                notif.resident = resident;
                notif.action_icons = action_icons;
                notif.category = category;
                notif.sound_file = sound_file;
                notif.suppress_sound = suppress_sound;

                // If replacing an existing notification, remove it first
                if self.map.contains_key(&id) {
                    self.remove_notification_internal(id, false);
                }

                // Apply user rules
                let keys = self.config.apply_and_get_keys(&mut notif);

                // Asynchronously pre-warm shared icon cache if icon is present
                if !notif.app_icon.is_empty() && self.config.ui.show_icons {
                    let icon_to_preload = notif.app_icon.clone();
                    let cache = self.icon_cache.clone();
                    tokio::task::spawn_blocking(move || {
                        cache.preload(&icon_to_preload);
                    });
                }

                // If marked to expire but no expiration time was computed, assign default timeout
                if notif.expires && notif.expires_at.is_none() {
                    let default_ms = self.config.behavior.default_expire_timeout_ms;
                    if default_ms > 0 {
                        let now_ns = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos() as u64;
                        notif.expires_at = Some(now_ns.saturating_add(default_ms * 1_000_000));
                    }
                }

                // Schedule expiration if required
                if notif.expires {
                    if let Some(exp_ns) = notif.expires_at {
                        let now_ns = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos() as u64;

                        if exp_ns > now_ns {
                            let duration = Duration::from_millis((exp_ns - now_ns) / 1_000_000);
                            let delay_key = self.delay_queue.insert(id, duration);
                            self.delay_keys.insert(id, delay_key);
                        }
                    }
                }

                self.last = Some(notif.clone());
                self.map.insert(id, keys.clone());
                self.tree.insert(&keys, notif);

                // Enforce max notification history to prevent unbounded memory growth
                let max_notifs = self.config.behavior.max_notifications;
                if max_notifs > 0 && self.tree.len() > max_notifs {
                    let mut leafs = self.tree.leafs();
                    // Evict low/normal urgency first, oldest first
                    leafs.sort_by(|a, b| {
                        a.urgency
                            .cmp(&b.urgency)
                            .then_with(|| a.created_at.cmp(&b.created_at))
                    });
                    let to_remove_count = self.tree.len().saturating_sub(max_notifs);
                    for notif_to_remove in leafs.into_iter().take(to_remove_count) {
                        if self.remove_notification_internal(notif_to_remove.id, true) {
                            self.emit_notification_closed(notif_to_remove.id, 4).await;
                        }
                    }
                }

                self.dump_dirty = true;
                self.last_dump_time = Instant::now();

                let _ = reply.send(id);

                // Emit NotificationsUpdated signal (mode = 0: Added)
                self.emit_notifications_updated(0).await;

                // Update UI if open
                if self.ui_open {
                    self.refresh_ui();
                }
            }
            EngineMessage::CloseNotification { id, reason } => {
                if reason == 3 {
                    if let Some(keys) = self.map.get(&id) {
                        let ctx = self.tree.get_context(keys, false);
                        if let Some(ClusterNode::Leaf(notif)) = ctx.children.get(&id.to_string()) {
                            if notif.ignore_close {
                                info!(
                                    "Ignoring CloseNotification for id {} per rule configuration",
                                    id
                                );
                                return false;
                            }
                        }
                    }
                }

                if self.remove_notification_internal(id, true) {
                    self.emit_notification_closed(id, reason).await;
                    self.emit_notifications_updated(1).await;
                    if self.ui_open {
                        self.refresh_ui();
                    }
                }
            }
            EngineMessage::GetCapabilities { reply } => {
                let caps = vec![
                    "actions".to_string(),
                    "body".to_string(),
                    "body-markup".to_string(),
                    "icon-static".to_string(),
                    "persistence".to_string(),
                ];
                let _ = reply.send(caps);
            }
            EngineMessage::GetServerInformation { reply } => {
                let info = (
                    "quiet".to_string(),
                    "quiet-project".to_string(),
                    env!("CARGO_PKG_VERSION").to_string(),
                    "1.2".to_string(),
                );
                let _ = reply.send(info);
            }
            EngineMessage::DumpNotifications { reply } => {
                self.flush_dump();
                let non_transient: Vec<_> = self
                    .tree
                    .leafs()
                    .into_iter()
                    .filter(|n| !n.transient)
                    .map(|mut n| {
                        if let Some(keys) = self.map.get(&n.id) {
                            n.keys = Some(keys.clone());
                        }
                        n
                    })
                    .collect();
                let json =
                    serde_json::to_string_pretty(&non_transient).unwrap_or_else(|_| "[]".into());
                let _ = reply.send(json);
            }
            EngineMessage::ShowNotificationCount { reply } => {
                let count = self.tree.len() as u32;
                let urgency = self.tree.urgency() as u32;
                let _ = reply.send((count, urgency));
            }
            EngineMessage::ShowNotifications => {
                if self.ui_open {
                    self.ui_open = false;
                    self.ui_tx.send_command(UiCommand::Close);
                    self.emit_notifications_updated(2).await;
                } else {
                    if self.tree.is_empty() || self.tree.len() == 0 {
                        return false;
                    }
                    self.ui_open = true;
                    self.current_context.clear();

                    // If there is only ONE group of notifications, default to expanding them.
                    // Backspace will pop the context up so the group can be deleted at once.
                    if self.tree.children.len() == 1 {
                        let mut curr = &self.tree;
                        while curr.children.len() == 1 {
                            if let Some((first_key, first_node)) = curr.children.iter().next() {
                                if let ClusterNode::Cluster(sub) = first_node {
                                    if sub.len() > 1 {
                                        self.current_context.push(first_key.clone());
                                        curr = sub;
                                    } else {
                                        break;
                                    }
                                } else {
                                    break;
                                }
                            } else {
                                break;
                            }
                        }
                    }

                    let (resolved_ctx, _) = self.tree.resolve_context(&self.current_context, true);
                    let items = self.generate_ui_items(&self.current_context);
                    self.ui_tx.send_command(UiCommand::Open {
                        items,
                        context: resolved_ctx,
                    });

                    // Clear any transient notification content in py3status on open
                    self.emit_notifications_updated(2).await;
                }
            }
            EngineMessage::SignalNotificationCount => {
                self.emit_notifications_updated(2).await;
            }
            EngineMessage::Quit => {
                return true;
            }
            EngineMessage::UiEvent(event) => {
                self.handle_ui_event(event).await;
            }
        }
        false
    }

    async fn handle_ui_event(&mut self, event: UiEvent) {
        match event {
            UiEvent::EnterGroup(key) => {
                let (mut resolved, _) = self.tree.resolve_context(&self.current_context, true);
                resolved.push(key);
                self.current_context = resolved;
                self.refresh_ui();
            }
            UiEvent::LeaveGroup => {
                if !self.current_context.is_empty() {
                    self.current_context.pop();
                    self.refresh_ui();
                } else {
                    self.ui_open = false;
                    self.ui_tx.send_command(UiCommand::Close);
                    self.emit_notifications_updated(2).await;
                }
            }
            UiEvent::ActionInvoked { id, action_key } => {
                let (is_resident, hints) = if let Some(keys) = self.map.get(&id) {
                    let ctx = self.tree.get_context(keys, false);
                    if let Some(ClusterNode::Leaf(notif)) = ctx.children.get(&id.to_string()) {
                        (
                            notif.resident,
                            crate::ipc::sway::FocusTargetHints {
                                app_name: notif.app_name.clone(),
                                summary: notif.summary.clone(),
                                body: notif.body.clone(),
                            },
                        )
                    } else {
                        (false, crate::ipc::sway::FocusTargetHints::default())
                    }
                } else {
                    (false, crate::ipc::sway::FocusTargetHints::default())
                };

                if !is_resident {
                    self.remove_notification_internal(id, true);
                    self.emit_notifications_updated(1).await;
                }
                self.ui_open = false;
                self.ui_tx.send_command(UiCommand::Close);

                let dbus_conn = self.dbus_conn.clone();
                let action_key_clone = action_key.clone();

                if self.config.behavior.focus_on_action {
                    let timeout = Duration::from_millis(self.config.behavior.urgent_timeout_ms);
                    crate::ipc::sway::spawn_race_free_action_focus(timeout, hints, move || async move {
                        if let Some(ref conn) = dbus_conn {
                            if let Ok(emitter) =
                                SignalEmitter::new(conn, "/org/freedesktop/Notifications")
                            {
                                let _ = NotificationsInterface::action_invoked(
                                    &emitter,
                                    id,
                                    &action_key_clone,
                                )
                                .await;
                                if !is_resident {
                                    let _ = NotificationsInterface::notification_closed(
                                        &emitter, id, 2,
                                    )
                                    .await;
                                }
                            }
                        }
                    });
                } else {
                    self.emit_action_invoked(id, &action_key).await;
                    if !is_resident {
                        self.emit_notification_closed(id, 2).await;
                    }
                }
            }
            UiEvent::DeleteItem(id) => {
                if self.remove_notification_internal(id, true) {
                    self.emit_notification_closed(id, 2).await;
                    self.emit_notifications_updated(1).await;
                    if self.tree.is_empty() {
                        self.ui_open = false;
                        self.ui_tx.send_command(UiCommand::Close);
                    } else {
                        while !self.current_context.is_empty()
                            && !self.tree.has_context(&self.current_context)
                        {
                            self.current_context.pop();
                        }
                        self.refresh_ui();
                    }
                }
            }
            UiEvent::DeleteGroup(key) => {
                let (mut path, _) = self.tree.resolve_context(&self.current_context, true);
                path.push(key);

                let removed_leafs = self.tree.remove_branch_path(&path);
                for leaf in &removed_leafs {
                    self.map.remove(&leaf.id);
                    self.icon_cache.remove(&format!("raw-image:{}", leaf.id));
                    if let Some(k) = self.delay_keys.remove(&leaf.id) {
                        self.delay_queue.remove(&k);
                    }
                    self.emit_notification_closed(leaf.id, 2).await;
                }

                let last_removed = self
                    .last
                    .as_ref()
                    .map(|l| removed_leafs.iter().any(|r| r.id == l.id))
                    .unwrap_or(false);
                if last_removed {
                    self.last = self.tree.best().cloned();
                }

                self.dump_dirty = true;
                self.last_dump_time = Instant::now();
                self.emit_notifications_updated(1).await;

                if self.tree.is_empty() {
                    self.ui_open = false;
                    self.ui_tx.send_command(UiCommand::Close);
                } else {
                    while !self.current_context.is_empty()
                        && !self.tree.has_context(&self.current_context)
                    {
                        self.current_context.pop();
                    }
                    self.refresh_ui();
                }
            }
            UiEvent::ClearAll => {
                let all_leafs = self.tree.leafs();
                for leaf in all_leafs {
                    self.map.remove(&leaf.id);
                    self.icon_cache.remove(&format!("raw-image:{}", leaf.id));
                    if let Some(k) = self.delay_keys.remove(&leaf.id) {
                        self.delay_queue.remove(&k);
                    }
                    self.emit_notification_closed(leaf.id, 2).await;
                }
                self.tree = NotificationCluster::new();
                self.last = None;
                self.current_context.clear();
                self.dump_dirty = true;
                self.last_dump_time = Instant::now();
                self.emit_notifications_updated(1).await;
                self.ui_open = false;
                self.ui_tx.send_command(UiCommand::Close);
            }
            UiEvent::CloseUi => {
                self.ui_open = false;
                self.emit_notifications_updated(2).await;
            }
        }
    }

    async fn handle_expiration(&mut self, id: u32) {
        if self.remove_notification_internal(id, true) {
            self.emit_notification_closed(id, 1).await; // 1 = Expired
            self.emit_notifications_updated(1).await;
            if self.ui_open {
                if self.tree.is_empty() {
                    self.ui_open = false;
                    self.ui_tx.send_command(UiCommand::Close);
                } else {
                    while !self.current_context.is_empty()
                        && !self.tree.has_context(&self.current_context)
                    {
                        self.current_context.pop();
                    }
                    self.refresh_ui();
                }
            }
        }
    }

    fn remove_notification_internal(&mut self, id: u32, update_last: bool) -> bool {
        self.icon_cache.remove(&format!("raw-image:{}", id));
        if let Some(keys) = self.map.remove(&id) {
            if let Some(k) = self.delay_keys.remove(&id) {
                self.delay_queue.remove(&k);
            }
            self.tree.remove(&keys, id);
            self.dump_dirty = true;
            self.last_dump_time = Instant::now();

            if update_last {
                let last_removed = self.last.as_ref().map(|l| l.id == id).unwrap_or(false);
                if last_removed {
                    self.last = self.tree.best().cloned();
                }
            }
            true
        } else {
            false
        }
    }

    fn refresh_ui(&mut self) {
        if self.ui_open {
            let (resolved_ctx, _) = self.tree.resolve_context(&self.current_context, true);
            let items = self.generate_ui_items(&self.current_context);
            self.ui_tx.send_command(UiCommand::UpdateItems {
                items,
                context: resolved_ctx,
            });
        }
    }

    pub fn generate_ui_items(&self, context: &[String]) -> Vec<UiItem> {
        let (resolved_ctx, cluster) = self.tree.resolve_context(context, true);
        let mut items = Vec::new();

        for (key, node) in &cluster.children {
            match node {
                ClusterNode::Cluster(sub) => {
                    let best = sub.best();
                    let best_id = best.map(|n| n.id);
                    let best_actions = best.map(|n| n.actions.clone()).unwrap_or_default();
                    let is_multi = sub.len() > 1;
                    let title = if is_multi {
                        format!("{} ({})", key, sub.len())
                    } else {
                        key.clone()
                    };
                    let raw_subtitle = best.map(|n| n.single_line()).unwrap_or_default();
                    let subtitle = crate::data::notification::strip_markup(&raw_subtitle);
                    let raw_summary = best.map(|n| n.summary.clone()).unwrap_or_else(|| key.clone());
                    let summary = crate::data::notification::strip_markup(&raw_summary);
                    let raw_app = best.map(|n| n.app_name.clone()).unwrap_or_else(|| key.clone());
                    let app_name = if is_multi {
                        format!("{} ({})", crate::data::notification::strip_markup(&raw_app), sub.len())
                    } else {
                        crate::data::notification::strip_markup(&raw_app)
                    };
                    let raw_body = best.map(|n| n.body.clone()).unwrap_or_default();
                    let body = crate::data::notification::strip_markup(&raw_body);
                    let time_str = best.map(|n| n.time_str()).unwrap_or_default();
                    let icon = best.map(|n| n.app_icon.clone()).unwrap_or_default();
                    let urgency = sub.urgency();
                    let created_at = best.map(|n| n.created_at).unwrap_or(0);

                    items.push(UiItem {
                        id: best_id,
                        is_group: is_multi,
                        key: key.clone(),
                        title,
                        subtitle,
                        summary,
                        app_name,
                        body,
                        time_str,
                        urgency,
                        count: sub.len(),
                        icon,
                        actions: best_actions,
                        created_at,
                    });
                }
                ClusterNode::Leaf(notif) => {
                    let raw_title = if notif.summary.is_empty() {
                        notif.app_name.clone()
                    } else {
                        notif.summary.clone()
                    };
                    let title = crate::data::notification::strip_markup(&raw_title);
                    let subtitle = crate::data::notification::strip_markup(&notif.body);
                    let summary = title.clone();
                    let app_name = crate::data::notification::strip_markup(&notif.app_name);
                    let body = subtitle.clone();

                    items.push(UiItem {
                        id: Some(notif.id),
                        is_group: false,
                        key: key.clone(),
                        title,
                        subtitle,
                        summary,
                        app_name,
                        body,
                        time_str: notif.time_str(),
                        urgency: notif.urgency,
                        count: 1,
                        icon: notif.app_icon.clone(),
                        actions: notif.actions.clone(),
                        created_at: notif.created_at,
                    });
                }
            }
        }

        // Determine sorting order for the current view:
        // - Root view follows behavior.sort_order (default: NewestToOldest)
        // - Intermediate group view follows group_sort_order (default: NewestToOldest)
        // - Leaf notifications view follows leaf sort_order (e.g. OldestToNewest for WhatsApp)
        let sort_order = if resolved_ctx.is_empty() {
            self.config.behavior.sort_order
        } else {
            let has_sub_clusters = cluster
                .children
                .values()
                .any(|node| matches!(node, ClusterNode::Cluster(_)));
            if has_sub_clusters {
                cluster
                    .best()
                    .and_then(|n| n.group_sort_order)
                    .unwrap_or(self.config.behavior.group_sort_order)
            } else {
                cluster
                    .best()
                    .map(|n| n.sort_order)
                    .unwrap_or(self.config.behavior.sort_order)
            }
        };

        match sort_order {
            crate::data::notification::SortOrder::NewestToOldest => {
                items.sort_by(|a, b| {
                    b.urgency
                        .cmp(&a.urgency)
                        .then_with(|| b.created_at.cmp(&a.created_at))
                        .then_with(|| b.id.cmp(&a.id))
                });
            }
            crate::data::notification::SortOrder::OldestToNewest => {
                items.sort_by(|a, b| {
                    b.urgency
                        .cmp(&a.urgency)
                        .then_with(|| a.created_at.cmp(&b.created_at))
                        .then_with(|| a.id.cmp(&b.id))
                });
            }
        }

        items
    }

    fn flush_dump(&mut self) {
        if !self.dump_dirty {
            return;
        }
        let non_transient: Vec<_> = self
            .tree
            .leafs()
            .into_iter()
            .filter(|n| !n.transient)
            .map(|mut n| {
                if let Some(keys) = self.map.get(&n.id) {
                    n.keys = Some(keys.clone());
                }
                n
            })
            .collect();
        if let Err(e) = save_dump(&self.dump_path, &non_transient) {
            warn!("Failed to save notification dump: {}", e);
        } else {
            self.dump_dirty = false;
            debug!("Dump successfully persisted to {:?}", self.dump_path);
        }
    }

    pub fn compute_single_line(&self, mode: u32) -> String {
        let num = self.tree.len();
        if mode == 0 && num > 0 {
            // The content of notifications should ONLY appear in py3status when
            // the notification first arrives (mode = 0: Added).
            if let Some(ref l) = self.last {
                l.single_line()
            } else if let Some(best) = self.tree.best() {
                best.single_line()
            } else {
                String::new()
            }
        } else {
            // When notifications are opened (mode = 2) or deleted (mode = 1),
            // do not send notification content; send empty string so py3status
            // immediately displays only the notification count.
            String::new()
        }
    }

    async fn emit_notifications_updated(&self, mode: u32) {
        if let Some(ref conn) = self.dbus_conn {
            let num = self.tree.len() as u32;
            let urgency = self.tree.urgency() as u32;
            let single_line = self.compute_single_line(mode);

            if let Ok(emitter) = SignalEmitter::new(conn, "/org/freedesktop/Notifications") {
                if let Err(e) = NotificationsInterface::notifications_updated(
                    &emitter,
                    mode,
                    num,
                    urgency,
                    &single_line,
                )
                .await
                {
                    warn!("Failed to emit NotificationsUpdated signal: {}", e);
                }
            }
        }
    }

    async fn emit_notification_closed(&self, id: u32, reason: u32) {
        if let Some(ref conn) = self.dbus_conn {
            if let Ok(emitter) = SignalEmitter::new(conn, "/org/freedesktop/Notifications") {
                let _ = NotificationsInterface::notification_closed(&emitter, id, reason).await;
            }
        }
    }

    async fn emit_action_invoked(&self, id: u32, action_key: &str) {
        if let Some(ref conn) = self.dbus_conn {
            if let Ok(emitter) = SignalEmitter::new(conn, "/org/freedesktop/Notifications") {
                let _ = NotificationsInterface::action_invoked(&emitter, id, action_key).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::data::Notification;
    use tempfile::tempdir;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_py3status_single_line_only_on_mode_0_arrival() {
        let dir = tempdir().unwrap();
        let dump_path = dir.path().join("test_dump.json");
        let config = Config::default();
        let (ui_tx, _ui_rx) = mpsc::channel(32);
        let (_engine_tx, engine_rx) = mpsc::channel(32);
        let icon_cache = std::sync::Arc::new(crate::icon::IconCache::new(None, 32));

        let mut engine = Engine::new(config, dump_path, ui_tx, engine_rx, icon_cache);

        // Initially 0 notifications
        assert_eq!(engine.compute_single_line(0), "");
        assert_eq!(engine.compute_single_line(1), "");
        assert_eq!(engine.compute_single_line(2), "");

        // Add a notification
        let notif = Notification::new(1, "Chrome".into(), "".into(), "Alice".into(), "Hello world".into(), vec![], 1, -1);
        engine.tree.insert(&["Chrome".into()], notif.clone());
        engine.last = Some(notif.clone());

        // Mode 0 (Arrived): must return formatted content
        assert_eq!(engine.compute_single_line(0), "Alice : Hello world");

        // Mode 1 (Deleted): MUST return empty string so py3status only displays count
        assert_eq!(engine.compute_single_line(1), "");

        // Mode 2 (Manual / Opened): MUST return empty string so py3status clears transient message
        assert_eq!(engine.compute_single_line(2), "");
    }
}

