use crate::engine::EngineMessage;
use std::collections::HashMap;
use tokio::sync::{mpsc, oneshot};
use tracing::info;
use zbus::object_server::SignalEmitter;
use zvariant::OwnedValue;

pub struct NotificationsInterface {
    engine_tx: mpsc::Sender<EngineMessage>,
}

impl NotificationsInterface {
    pub fn new(engine_tx: mpsc::Sender<EngineMessage>) -> Self {
        Self { engine_tx }
    }
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl NotificationsInterface {
    /// Notify method according to org.freedesktop.Notifications v1.2 spec
    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> zbus::fdo::Result<u32> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let msg = EngineMessage::Notify {
            app_name,
            replaces_id,
            app_icon,
            summary,
            body,
            actions,
            hints,
            expire_timeout,
            reply: reply_tx,
        };

        if self.engine_tx.send(msg).await.is_err() {
            return Err(zbus::fdo::Error::Failed("Engine actor unavailable".into()));
        }

        reply_rx
            .await
            .map_err(|_| zbus::fdo::Error::Failed("Engine dropped reply channel".into()))
    }

    /// CloseNotification method
    async fn close_notification(&self, id: u32) -> zbus::fdo::Result<()> {
        let msg = EngineMessage::CloseNotification { id, reason: 3 }; // 3 = CloseNotification call
        self.engine_tx
            .send(msg)
            .await
            .map_err(|_| zbus::fdo::Error::Failed("Engine actor unavailable".into()))?;
        Ok(())
    }

    /// GetCapabilities method
    async fn get_capabilities(&self) -> zbus::fdo::Result<Vec<String>> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let msg = EngineMessage::GetCapabilities { reply: reply_tx };
        if self.engine_tx.send(msg).await.is_err() {
            return Err(zbus::fdo::Error::Failed("Engine actor unavailable".into()));
        }
        reply_rx
            .await
            .map_err(|_| zbus::fdo::Error::Failed("Engine dropped reply".into()))
    }

    /// GetServerInformation method
    async fn get_server_information(&self) -> zbus::fdo::Result<(String, String, String, String)> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let msg = EngineMessage::GetServerInformation { reply: reply_tx };
        if self.engine_tx.send(msg).await.is_err() {
            return Err(zbus::fdo::Error::Failed("Engine actor unavailable".into()));
        }
        reply_rx
            .await
            .map_err(|_| zbus::fdo::Error::Failed("Engine dropped reply".into()))
    }

    /// DumpNotifications method (compatibility with i3-notifier CLI)
    async fn dump_notifications(&self) -> zbus::fdo::Result<String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let msg = EngineMessage::DumpNotifications { reply: reply_tx };
        if self.engine_tx.send(msg).await.is_err() {
            return Err(zbus::fdo::Error::Failed("Engine actor unavailable".into()));
        }
        reply_rx
            .await
            .map_err(|_| zbus::fdo::Error::Failed("Engine dropped reply".into()))
    }

    /// ShowNotificationCount method: returns (count, urgency)
    async fn show_notification_count(&self) -> zbus::fdo::Result<(u32, u32)> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let msg = EngineMessage::ShowNotificationCount { reply: reply_tx };
        if self.engine_tx.send(msg).await.is_err() {
            return Err(zbus::fdo::Error::Failed("Engine actor unavailable".into()));
        }
        reply_rx
            .await
            .map_err(|_| zbus::fdo::Error::Failed("Engine dropped reply".into()))
    }

    /// ShowNotifications method (interactive UI toggle)
    async fn show_notifications(&self) -> zbus::fdo::Result<()> {
        self.engine_tx
            .send(EngineMessage::ShowNotifications)
            .await
            .map_err(|_| zbus::fdo::Error::Failed("Engine actor unavailable".into()))?;
        Ok(())
    }

    /// SignalNotificationCount method (called by py3notifier on bar startup)
    async fn signal_notification_count(&self) -> zbus::fdo::Result<()> {
        self.engine_tx
            .send(EngineMessage::SignalNotificationCount)
            .await
            .map_err(|_| zbus::fdo::Error::Failed("Engine actor unavailable".into()))?;
        Ok(())
    }

    /// Quit method
    async fn quit(&self) -> zbus::fdo::Result<()> {
        info!("D-Bus Quit invoked");
        self.engine_tx
            .send(EngineMessage::Quit)
            .await
            .map_err(|_| zbus::fdo::Error::Failed("Engine actor unavailable".into()))?;
        Ok(())
    }

    // Signals
    #[zbus(signal)]
    pub async fn notification_closed(
        emitter: &SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn action_invoked(
        emitter: &SignalEmitter<'_>,
        id: u32,
        action_key: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn notifications_updated(
        emitter: &SignalEmitter<'_>,
        mode: u32,
        num: u32,
        urgency: u32,
        single_line: &str,
    ) -> zbus::Result<()>;
}
