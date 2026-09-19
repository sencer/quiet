use quiet::config::Config;
use quiet::engine::{Engine, EngineMessage, UiCommand};
use std::collections::HashMap;
use std::time::Duration;
use tempfile::tempdir;
use tokio::sync::{mpsc, oneshot};
use zvariant::OwnedValue;

#[tokio::test]
async fn test_engine_full_lifecycle() {
    let dir = tempdir().unwrap();
    let dump_path = dir.path().join("test_dump.json");
    let config = Config::default();

    let (ui_tx, _ui_rx) = mpsc::channel::<UiCommand>(32);
    let (engine_tx, engine_rx) = mpsc::channel::<EngineMessage>(32);

    let icon_cache = std::sync::Arc::new(quiet::icon::IconCache::new(None, 32));
    let mut engine = Engine::new(config, dump_path, ui_tx, engine_rx, icon_cache);

    // Spawn engine in background task
    let engine_handle = tokio::spawn(async move {
        engine.run().await;
    });

    // Test 1: GetServerInformation
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        engine_tx
            .send(EngineMessage::GetServerInformation { reply: reply_tx })
            .await
            .unwrap();
        let (name, vendor, version, spec) = reply_rx.await.unwrap();
        assert_eq!(name, "quiet");
        assert_eq!(vendor, "quiet-project");
        assert_eq!(spec, "1.2");
        assert!(!version.is_empty());
    }

    // Test 2: GetCapabilities
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        engine_tx
            .send(EngineMessage::GetCapabilities { reply: reply_tx })
            .await
            .unwrap();
        let caps = reply_rx.await.unwrap();
        assert!(caps.contains(&"actions".to_string()));
        assert!(caps.contains(&"body".to_string()));
        assert!(caps.contains(&"persistence".to_string()));
    }

    // Test 3: Notify (Normal urgency)
    let notif1_id = {
        let (reply_tx, reply_rx) = oneshot::channel();
        let mut hints = HashMap::new();
        hints.insert("urgency".to_string(), OwnedValue::from(1u8)); // Normal

        engine_tx
            .send(EngineMessage::Notify {
                app_name: "slack".into(),
                replaces_id: 0,
                app_icon: "".into(),
                summary: "Alice".into(),
                body: "Hello from Slack".into(),
                actions: vec!["default".into(), "Open".into()],
                hints,
                expire_timeout: -1,
                reply: reply_tx,
            })
            .await
            .unwrap();

        reply_rx.await.unwrap()
    };
    assert_eq!(notif1_id, 1);

    // Verify count is 1, urgency is 1
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        engine_tx
            .send(EngineMessage::ShowNotificationCount { reply: reply_tx })
            .await
            .unwrap();
        let (count, urgency) = reply_rx.await.unwrap();
        assert_eq!(count, 1);
        assert_eq!(urgency, 1);
    }

    // Test 4: Notify Critical Urgency
    let notif2_id = {
        let (reply_tx, reply_rx) = oneshot::channel();
        let mut hints = HashMap::new();
        hints.insert("urgency".to_string(), OwnedValue::from(2u8)); // Critical

        engine_tx
            .send(EngineMessage::Notify {
                app_name: "monitor".into(),
                replaces_id: 0,
                app_icon: "".into(),
                summary: "High CPU".into(),
                body: "CPU reached 99%".into(),
                actions: vec![],
                hints,
                expire_timeout: -1,
                reply: reply_tx,
            })
            .await
            .unwrap();

        reply_rx.await.unwrap()
    };
    assert_eq!(notif2_id, 2);

    // Verify count is 2, highest urgency escalated to 2
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        engine_tx
            .send(EngineMessage::ShowNotificationCount { reply: reply_tx })
            .await
            .unwrap();
        let (count, urgency) = reply_rx.await.unwrap();
        assert_eq!(count, 2);
        assert_eq!(urgency, 2);
    }

    // Test 5: Gmail rule matching
    let notif3_id = {
        let (reply_tx, reply_rx) = oneshot::channel();
        let hints = HashMap::new();

        engine_tx
            .send(EngineMessage::Notify {
                app_name: "Google Chrome".into(),
                replaces_id: 0,
                app_icon: "".into(),
                summary: "Meeting".into(),
                body: "mail.google.com\nCalendar invite".into(),
                actions: vec![],
                hints,
                expire_timeout: -1,
                reply: reply_tx,
            })
            .await
            .unwrap();

        reply_rx.await.unwrap()
    };
    assert_eq!(notif3_id, 3);

    // Test 6: DumpNotifications JSON
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        engine_tx
            .send(EngineMessage::DumpNotifications { reply: reply_tx })
            .await
            .unwrap();
        let json_dump = reply_rx.await.unwrap();
        assert!(json_dump.contains("Hello from Slack"));
        assert!(json_dump.contains("CPU reached 99%"));
        assert!(json_dump.contains("Gmail")); // transformed by rule
    }

    // Test 7: CloseNotification (critical one)
    {
        engine_tx
            .send(EngineMessage::CloseNotification {
                id: notif2_id,
                reason: 3,
            })
            .await
            .unwrap();
    }

    // Verify count dropped to 2, urgency lowered back to 1
    tokio::time::sleep(Duration::from_millis(50)).await;
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        engine_tx
            .send(EngineMessage::ShowNotificationCount { reply: reply_tx })
            .await
            .unwrap();
        let (count, urgency) = reply_rx.await.unwrap();
        assert_eq!(count, 2);
        assert_eq!(urgency, 1);
    }

    // Test 8: Timed Notification Expiration
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        let hints = HashMap::new();

        // Expire in 150ms
        engine_tx
            .send(EngineMessage::Notify {
                app_name: "notify-send".into(),
                replaces_id: 0,
                app_icon: "".into(),
                summary: "Transient".into(),
                body: "Will expire quickly".into(),
                actions: vec![],
                hints,
                expire_timeout: 150,
                reply: reply_tx,
            })
            .await
            .unwrap();

        let timed_id = reply_rx.await.unwrap();
        assert_eq!(timed_id, 4);

        // Before expiration: count is 3
        {
            let (reply_tx, reply_rx) = oneshot::channel();
            engine_tx
                .send(EngineMessage::ShowNotificationCount { reply: reply_tx })
                .await
                .unwrap();
            let (count, _) = reply_rx.await.unwrap();
            assert_eq!(count, 3);
        }

        // Wait for expiration
        tokio::time::sleep(Duration::from_millis(350)).await;

        // After expiration: count should automatically be 2!
        {
            let (reply_tx, reply_rx) = oneshot::channel();
            engine_tx
                .send(EngineMessage::ShowNotificationCount { reply: reply_tx })
                .await
                .unwrap();
            let (count, _) = reply_rx.await.unwrap();
            assert_eq!(count, 2);
        }
    }

    // Test 9: Desktop Entry Resolution
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        let mut hints = HashMap::new();
        hints.insert(
            "desktop-entry".to_string(),
            zvariant::Value::from("slack").try_to_owned().unwrap(),
        );

        engine_tx
            .send(EngineMessage::Notify {
                app_name: "".into(),
                replaces_id: 0,
                app_icon: "".into(),
                summary: "Direct Mention".into(),
                body: "Hello from Slack thread".into(),
                actions: vec![],
                hints,
                expire_timeout: -1,
                reply: reply_tx,
            })
            .await
            .unwrap();

        let notif_id = reply_rx.await.unwrap();
        assert_eq!(notif_id, 5);
    }

    // Clean shutdown
    engine_tx.send(EngineMessage::Quit).await.unwrap();
    let _ = engine_handle.await;
}

#[tokio::test]
async fn test_replaces_id_preservation_and_update() {
    let dir = tempdir().unwrap();
    let dump_path = dir.path().join("test_dump_replaces.json");
    let config = Config::default();

    let (ui_tx, _ui_rx) = mpsc::channel::<UiCommand>(32);
    let (engine_tx, engine_rx) = mpsc::channel::<EngineMessage>(32);
    let icon_cache = std::sync::Arc::new(quiet::icon::IconCache::new(None, 32));
    let mut engine = Engine::new(config, dump_path, ui_tx, engine_rx, icon_cache);

    let engine_handle = tokio::spawn(async move {
        engine.run().await;
    });

    // Notify with replaces_id: 100
    let id1 = {
        let (reply_tx, reply_rx) = oneshot::channel();
        engine_tx
            .send(EngineMessage::Notify {
                app_name: "custom".into(),
                replaces_id: 100,
                app_icon: "".into(),
                summary: "Initial".into(),
                body: "Initial body".into(),
                actions: vec![],
                hints: HashMap::new(),
                expire_timeout: -1,
                reply: reply_tx,
            })
            .await
            .unwrap();
        reply_rx.await.unwrap()
    };
    assert_eq!(id1, 100);

    // Update with same replaces_id: 100
    let id2 = {
        let (reply_tx, reply_rx) = oneshot::channel();
        engine_tx
            .send(EngineMessage::Notify {
                app_name: "custom".into(),
                replaces_id: 100,
                app_icon: "".into(),
                summary: "Updated".into(),
                body: "Updated body".into(),
                actions: vec![],
                hints: HashMap::new(),
                expire_timeout: -1,
                reply: reply_tx,
            })
            .await
            .unwrap();
        reply_rx.await.unwrap()
    };
    assert_eq!(id2, 100);

    // Count should be 1, dump should contain "Updated body"
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        engine_tx
            .send(EngineMessage::ShowNotificationCount { reply: reply_tx })
            .await
            .unwrap();
        let (count, _) = reply_rx.await.unwrap();
        assert_eq!(count, 1);

        let (reply_tx, reply_rx) = oneshot::channel();
        engine_tx
            .send(EngineMessage::DumpNotifications { reply: reply_tx })
            .await
            .unwrap();
        let dump = reply_rx.await.unwrap();
        assert!(dump.contains("Updated body"));
        assert!(!dump.contains("Initial body"));
    }

    // New notification with replaces_id: 0
    let id3 = {
        let (reply_tx, reply_rx) = oneshot::channel();
        engine_tx
            .send(EngineMessage::Notify {
                app_name: "custom".into(),
                replaces_id: 0,
                app_icon: "".into(),
                summary: "Another".into(),
                body: "Another body".into(),
                actions: vec![],
                hints: HashMap::new(),
                expire_timeout: -1,
                reply: reply_tx,
            })
            .await
            .unwrap();
        reply_rx.await.unwrap()
    };
    assert_eq!(id3, 1); // Generates 1, doesn't collide with 100

    // Count should be 2
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        engine_tx
            .send(EngineMessage::ShowNotificationCount { reply: reply_tx })
            .await
            .unwrap();
        let (count, _) = reply_rx.await.unwrap();
        assert_eq!(count, 2);
    }

    engine_tx.send(EngineMessage::Quit).await.unwrap();
    let _ = engine_handle.await;
}

#[tokio::test]
async fn test_ignore_close_client_dismiss() {
    let dir = tempdir().unwrap();
    let dump_path = dir.path().join("test_dump_ignore_close.json");
    let config = Config::default();

    let (ui_tx, _ui_rx) = mpsc::channel::<UiCommand>(32);
    let (engine_tx, engine_rx) = mpsc::channel::<EngineMessage>(32);
    let icon_cache = std::sync::Arc::new(quiet::icon::IconCache::new(None, 32));
    let mut engine = Engine::new(config, dump_path, ui_tx, engine_rx, icon_cache);

    let engine_handle = tokio::spawn(async move {
        engine.run().await;
    });

    // Post notification matching Gmail rule (which has ignore_close: true)
    let notif_id = {
        let (reply_tx, reply_rx) = oneshot::channel();
        engine_tx
            .send(EngineMessage::Notify {
                app_name: "Google Chrome".into(),
                replaces_id: 0,
                app_icon: "".into(),
                summary: "Gmail Notification".into(),
                body: "mail.google.com\nNew message received".into(),
                actions: vec![],
                hints: HashMap::new(),
                expire_timeout: -1,
                reply: reply_tx,
            })
            .await
            .unwrap();
        reply_rx.await.unwrap()
    };

    // Verify count is 1
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        engine_tx
            .send(EngineMessage::ShowNotificationCount { reply: reply_tx })
            .await
            .unwrap();
        let (count, _) = reply_rx.await.unwrap();
        assert_eq!(count, 1);
    }

    // Try to close with reason 3 (client app dismissed - e.g. Chrome timeout)
    engine_tx
        .send(EngineMessage::CloseNotification {
            id: notif_id,
            reason: 3,
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Verify notification was NOT closed
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        engine_tx
            .send(EngineMessage::ShowNotificationCount { reply: reply_tx })
            .await
            .unwrap();
        let (count, _) = reply_rx.await.unwrap();
        assert_eq!(
            count, 1,
            "Notification with ignore_close should not be closed with reason 3"
        );
    }

    // Now close with reason 2 (user dismissed)
    engine_tx
        .send(EngineMessage::CloseNotification {
            id: notif_id,
            reason: 2,
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Verify notification was closed
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        engine_tx
            .send(EngineMessage::ShowNotificationCount { reply: reply_tx })
            .await
            .unwrap();
        let (count, _) = reply_rx.await.unwrap();
        assert_eq!(
            count, 0,
            "Notification should be closed when reason is not 3"
        );
    }

    engine_tx.send(EngineMessage::Quit).await.unwrap();
    let _ = engine_handle.await;
}

#[tokio::test]
async fn test_delete_group_auto_descended() {
    let dir = tempdir().unwrap();
    let dump_path = dir.path().join("test_dump_delete_group.json");
    let config = Config::default();

    let (ui_tx, _ui_rx) = mpsc::channel::<UiCommand>(32);
    let (engine_tx, engine_rx) = mpsc::channel::<EngineMessage>(32);
    let icon_cache = std::sync::Arc::new(quiet::icon::IconCache::new(None, 32));
    let mut engine = Engine::new(config, dump_path, ui_tx, engine_rx, icon_cache);

    let engine_handle = tokio::spawn(async move {
        engine.run().await;
    });

    // Notify with slack -> general (2 items)
    for i in 1..=2 {
        let (reply_tx, reply_rx) = oneshot::channel();
        engine_tx
            .send(EngineMessage::Notify {
                app_name: "slack".into(),
                replaces_id: 0,
                app_icon: "".into(),
                summary: format!("msg {i}"),
                body: "body".into(),
                actions: vec![],
                hints: HashMap::new(),
                expire_timeout: -1,
                reply: reply_tx,
            })
            .await
            .unwrap();
        let _ = reply_rx.await.unwrap();
    }

    // Verify count is 2
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        engine_tx
            .send(EngineMessage::ShowNotificationCount { reply: reply_tx })
            .await
            .unwrap();
        let (count, _) = reply_rx.await.unwrap();
        assert_eq!(count, 2);
    }

    // Delete group "slack"
    use quiet::engine::UiEvent;
    engine_tx
        .send(EngineMessage::UiEvent(UiEvent::DeleteGroup("slack".into())))
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Verify count is now 0
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        engine_tx
            .send(EngineMessage::ShowNotificationCount { reply: reply_tx })
            .await
            .unwrap();
        let (count, _) = reply_rx.await.unwrap();
        assert_eq!(
            count, 0,
            "Deleting group should remove all items in the auto-descended branch"
        );
    }

    engine_tx.send(EngineMessage::Quit).await.unwrap();
    let _ = engine_handle.await;
}

#[tokio::test]
async fn test_image_data_hint_decoding() {
    let dir = tempdir().unwrap();
    let dump_path = dir.path().join("test_dump_image.json");
    let config = Config::default();

    let (ui_tx, _ui_rx) = mpsc::channel::<UiCommand>(32);
    let (engine_tx, engine_rx) = mpsc::channel::<EngineMessage>(32);
    let icon_cache = std::sync::Arc::new(quiet::icon::IconCache::new(None, 32));
    let mut engine = Engine::new(config, dump_path, ui_tx, engine_rx, icon_cache.clone());

    let engine_handle = tokio::spawn(async move {
        engine.run().await;
    });

    // Construct 2x2 RGBA image-data tuple: (i32, i32, i32, bool, i32, i32, Vec<u8>)
    let raw_pixels: Vec<u8> = vec![
        255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
    ];
    let tuple = (2i32, 2i32, 8i32, true, 8i32, 4i32, raw_pixels);
    let owned = OwnedValue::try_from(zvariant::Value::from(tuple)).unwrap();

    let mut hints = HashMap::new();
    hints.insert("image-data".to_string(), owned);

    let (reply_tx, reply_rx) = oneshot::channel();
    engine_tx
        .send(EngineMessage::Notify {
            app_name: "test-app".into(),
            replaces_id: 0,
            app_icon: "".into(),
            summary: "Raw Image Test".into(),
            body: "Has custom image-data".into(),
            actions: vec![],
            hints,
            expire_timeout: -1,
            reply: reply_tx,
        })
        .await
        .unwrap();
    let notif_id = reply_rx.await.unwrap();

    // Verify raw icon is cached in icon_cache under "raw-image:{notif_id}"
    let cached = icon_cache.get(&format!("raw-image:{}", notif_id));
    assert!(
        cached.is_some(),
        "Raw image should be decoded and inserted into icon cache"
    );
    let pixmap = cached.unwrap();
    assert_eq!(pixmap.width(), 32);
    assert_eq!(pixmap.height(), 32);

    engine_tx.send(EngineMessage::Quit).await.unwrap();
    let _ = engine_handle.await;
}

#[tokio::test]
async fn test_resident_hint_preserves_notification() {
    let dir = tempdir().unwrap();
    let dump_path = dir.path().join("test_dump_resident.json");
    let config = Config::default();

    let (ui_tx, _ui_rx) = mpsc::channel::<UiCommand>(32);
    let (engine_tx, engine_rx) = mpsc::channel::<EngineMessage>(32);
    let icon_cache = std::sync::Arc::new(quiet::icon::IconCache::new(None, 32));
    let mut engine = Engine::new(config, dump_path, ui_tx, engine_rx, icon_cache);

    let engine_handle = tokio::spawn(async move {
        engine.run().await;
    });

    let mut hints = HashMap::new();
    hints.insert(
        "resident".to_string(),
        OwnedValue::try_from(zvariant::Value::Bool(true)).unwrap(),
    );

    let (reply_tx, reply_rx) = oneshot::channel();
    engine_tx
        .send(EngineMessage::Notify {
            app_name: "player".into(),
            replaces_id: 0,
            app_icon: "".into(),
            summary: "Now Playing".into(),
            body: "Song title".into(),
            actions: vec!["pause".into(), "Pause".into()],
            hints,
            expire_timeout: -1,
            reply: reply_tx,
        })
        .await
        .unwrap();
    let id = reply_rx.await.unwrap();

    // Verify initial count is 1
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        engine_tx
            .send(EngineMessage::ShowNotificationCount { reply: reply_tx })
            .await
            .unwrap();
        let (count, _) = reply_rx.await.unwrap();
        assert_eq!(count, 1);
    }

    // Invoke action on resident notification
    use quiet::engine::UiEvent;
    engine_tx
        .send(EngineMessage::UiEvent(UiEvent::ActionInvoked {
            id,
            action_key: "pause".into(),
        }))
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Verify notification is STILL present (count is 1) because resident = true
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        engine_tx
            .send(EngineMessage::ShowNotificationCount { reply: reply_tx })
            .await
            .unwrap();
        let (count, _) = reply_rx.await.unwrap();
        assert_eq!(
            count, 1,
            "Resident notification must not be closed on action invocation"
        );
    }

    // Now explicitly close it with reason 2 (DismissedByUser)
    engine_tx
        .send(EngineMessage::CloseNotification { id, reason: 2 })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Verify count is now 0
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        engine_tx
            .send(EngineMessage::ShowNotificationCount { reply: reply_tx })
            .await
            .unwrap();
        let (count, _) = reply_rx.await.unwrap();
        assert_eq!(count, 0);
    }

    engine_tx.send(EngineMessage::Quit).await.unwrap();
    let _ = engine_handle.await;
}

#[tokio::test]
async fn test_transient_hint_dump_exclusion() {
    let dir = tempdir().unwrap();
    let dump_path = dir.path().join("test_dump_transient.json");
    let config = Config::default();

    let (ui_tx, _ui_rx) = mpsc::channel::<UiCommand>(32);
    let (engine_tx, engine_rx) = mpsc::channel::<EngineMessage>(32);
    let icon_cache = std::sync::Arc::new(quiet::icon::IconCache::new(None, 32));
    let mut engine = Engine::new(config, dump_path, ui_tx, engine_rx, icon_cache);

    let engine_handle = tokio::spawn(async move {
        engine.run().await;
    });

    // 1. Send normal notification
    let (reply_tx, reply_rx) = oneshot::channel();
    engine_tx
        .send(EngineMessage::Notify {
            app_name: "normal-app".into(),
            replaces_id: 0,
            app_icon: "".into(),
            summary: "Persistent Notification".into(),
            body: "Should be saved to dump".into(),
            actions: vec![],
            hints: HashMap::new(),
            expire_timeout: -1,
            reply: reply_tx,
        })
        .await
        .unwrap();
    let _ = reply_rx.await.unwrap();

    // 2. Send transient notification
    let mut transient_hints = HashMap::new();
    transient_hints.insert(
        "transient".to_string(),
        OwnedValue::try_from(zvariant::Value::Bool(true)).unwrap(),
    );

    let (reply_tx, reply_rx) = oneshot::channel();
    engine_tx
        .send(EngineMessage::Notify {
            app_name: "transient-app".into(),
            replaces_id: 0,
            app_icon: "".into(),
            summary: "Ephemeral Alert".into(),
            body: "Must not be saved to disk".into(),
            actions: vec![],
            hints: transient_hints,
            expire_timeout: -1,
            reply: reply_tx,
        })
        .await
        .unwrap();
    let _ = reply_rx.await.unwrap();

    // 3. Request dump
    let (reply_tx, reply_rx) = oneshot::channel();
    engine_tx
        .send(EngineMessage::DumpNotifications { reply: reply_tx })
        .await
        .unwrap();
    let dump_json = reply_rx.await.unwrap();

    assert!(
        dump_json.contains("Persistent Notification"),
        "Persistent notification must be in dump"
    );
    assert!(
        !dump_json.contains("Ephemeral Alert"),
        "Transient notification must be excluded from dump"
    );

    engine_tx.send(EngineMessage::Quit).await.unwrap();
    let _ = engine_handle.await;
}

#[tokio::test]
async fn test_max_notifications_eviction() {
    let dir = tempdir().unwrap();
    let dump_path = dir.path().join("test_dump_max.json");
    let mut config = Config::default();
    config.behavior.max_notifications = 5;

    let (ui_tx, _ui_rx) = mpsc::channel::<UiCommand>(32);
    let (engine_tx, engine_rx) = mpsc::channel::<EngineMessage>(32);
    let icon_cache = std::sync::Arc::new(quiet::icon::IconCache::new(None, 32));
    let mut engine = Engine::new(config, dump_path, ui_tx, engine_rx, icon_cache);

    let engine_handle = tokio::spawn(async move {
        engine.run().await;
    });

    // Send 7 notifications
    for i in 1..=7 {
        let (reply_tx, reply_rx) = oneshot::channel();
        engine_tx
            .send(EngineMessage::Notify {
                app_name: "burst".into(),
                replaces_id: 0,
                app_icon: "".into(),
                summary: format!("Notification {}", i),
                body: format!("Body {}", i),
                actions: vec![],
                hints: HashMap::new(),
                expire_timeout: -1,
                reply: reply_tx,
            })
            .await
            .unwrap();
        let _ = reply_rx.await.unwrap();
    }

    // Verify count is capped at 5
    let (reply_tx, reply_rx) = oneshot::channel();
    engine_tx
        .send(EngineMessage::ShowNotificationCount { reply: reply_tx })
        .await
        .unwrap();
    let (count, _) = reply_rx.await.unwrap();
    assert_eq!(
        count, 5,
        "Notification count must be bounded by max_notifications (5)"
    );

    // Verify oldest notifications (1 and 2) were evicted
    let (reply_tx, reply_rx) = oneshot::channel();
    engine_tx
        .send(EngineMessage::DumpNotifications { reply: reply_tx })
        .await
        .unwrap();
    let dump_json = reply_rx.await.unwrap();
    assert!(
        !dump_json.contains("Notification 1"),
        "Oldest notification 1 should be evicted"
    );
    assert!(
        !dump_json.contains("Notification 2"),
        "Oldest notification 2 should be evicted"
    );
    assert!(
        dump_json.contains("Notification 7"),
        "Latest notification 7 should be present"
    );

    engine_tx.send(EngineMessage::Quit).await.unwrap();
    let _ = engine_handle.await;
}

#[tokio::test]
async fn test_populate_gmail_whatsapp_ungrouped() {
    let dir = tempdir().unwrap();
    let dump_path = dir.path().join("test_dump_populate.json");
    let config = Config::default();

    let (ui_tx, mut ui_rx) = mpsc::channel::<UiCommand>(32);
    let (engine_tx, engine_rx) = mpsc::channel::<EngineMessage>(32);
    let icon_cache = std::sync::Arc::new(quiet::icon::IconCache::new(None, 32));
    let mut engine = Engine::new(config, dump_path, ui_tx, engine_rx, icon_cache);

    let engine_handle = tokio::spawn(async move {
        engine.run().await;
    });

    let test_notifications = vec![
        // 3 Gmail (via Chrome rule)
        (
            "Google Chrome",
            "gmail",
            "Alice Smith",
            "mail.google.com\nHey, let's review the Q4 architecture plan.",
            1u8,
        ),
        (
            "Google Chrome",
            "gmail",
            "Bob Jones",
            "mail.google.com\nTeam sync tomorrow at 10:00 AM",
            1u8,
        ),
        (
            "Google Chrome",
            "gmail",
            "GitHub Notifications",
            "mail.google.com\n[quiet] 3 new comments on PR #42",
            1u8,
        ),
        // 3 WhatsApp (via Chrome rule)
        (
            "Google Chrome",
            "whatsapp",
            "Mom",
            "web.whatsapp.com\nAre you coming over for dinner on Sunday?",
            1u8,
        ),
        (
            "Google Chrome",
            "whatsapp",
            "Mom",
            "web.whatsapp.com\nLet me know so I can cook your favorites!",
            1u8,
        ),
        (
            "Google Chrome",
            "whatsapp",
            "Dev Team",
            "web.whatsapp.com\nDeployment to production is complete",
            1u8,
        ),
        // 3 Ungrouped
        (
            "Spotify",
            "spotify",
            "Daft Punk",
            "Get Lucky (feat. Pharrell Williams)",
            1u8,
        ),
        (
            "Alacritty",
            "utilities-terminal",
            "Cargo Build",
            "Finished release target(s)",
            1u8,
        ),
        (
            "Power Manager",
            "battery-caution",
            "Battery Low",
            "Battery level is at 18%",
            2u8,
        ),
    ];

    for (app, icon, summary, body, urgency) in test_notifications {
        let mut hints = HashMap::new();
        hints.insert("urgency".to_string(), OwnedValue::from(urgency));
        let (reply_tx, reply_rx) = oneshot::channel();
        engine_tx
            .send(EngineMessage::Notify {
                app_name: app.into(),
                replaces_id: 0,
                app_icon: icon.into(),
                summary: summary.into(),
                body: body.into(),
                actions: vec![],
                hints,
                expire_timeout: -1,
                reply: reply_tx,
            })
            .await
            .unwrap();
        let _ = reply_rx.await.unwrap();
    }

    // Verify notification count is 9 and urgency is 2 (due to Power Manager)
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        engine_tx
            .send(EngineMessage::ShowNotificationCount { reply: reply_tx })
            .await
            .unwrap();
        let (count, urgency) = reply_rx.await.unwrap();
        assert_eq!(count, 9, "Expected 9 total notifications");
        assert_eq!(urgency, 2, "Expected highest urgency = 2 (critical)");
    }

    // Toggle UI to open it and receive UiCommand::Open
    engine_tx
        .send(EngineMessage::ShowNotifications)
        .await
        .unwrap();

    let open_cmd = ui_rx.recv().await.expect("Expected UiCommand::Open");
    if let UiCommand::Open { items, context } = open_cmd {
        assert!(context.is_empty(), "Initial context should be root");

        let gmail_group = items.iter().find(|i| i.app_name.contains("Gmail"));
        assert!(gmail_group.is_some(), "Gmail group must exist at root");
        let gmail = gmail_group.unwrap();
        assert!(gmail.is_group, "Gmail should be displayed as group");
        assert_eq!(gmail.count, 3, "Gmail group must have count 3");

        let whatsapp_group = items.iter().find(|i| i.app_name.contains("WhatsApp"));
        assert!(whatsapp_group.is_some(), "WhatsApp group must exist at root");
        let whatsapp = whatsapp_group.unwrap();
        assert!(whatsapp.is_group, "WhatsApp should be displayed as group");
        assert_eq!(whatsapp.count, 3, "WhatsApp group must have count 3");

        let spotify_item = items.iter().find(|i| i.app_name == "Spotify");
        assert!(spotify_item.is_some(), "Spotify leaf must exist at root");
        let spotify = spotify_item.unwrap();
        assert!(!spotify.is_group, "Spotify should NOT be a group");
        assert_eq!(spotify.count, 1);

        let alacritty_item = items.iter().find(|i| i.app_name == "Alacritty");
        assert!(alacritty_item.is_some(), "Alacritty leaf must exist at root");
        assert!(!alacritty_item.unwrap().is_group);

        let power_item = items.iter().find(|i| i.app_name == "Power Manager");
        assert!(power_item.is_some(), "Power Manager leaf must exist at root");
        let power = power_item.unwrap();
        assert!(!power.is_group);
        assert_eq!(
            power.urgency, 2,
            "Power Manager notification must have urgency 2"
        );
    } else {
        panic!("Expected UiCommand::Open, got {:?}", open_cmd);
    }

    // Now descend into the Gmail group
    use quiet::engine::UiEvent;
    engine_tx
        .send(EngineMessage::UiEvent(UiEvent::EnterGroup("Gmail".into())))
        .await
        .unwrap();

    let update_cmd = ui_rx.recv().await.expect("Expected UiCommand::UpdateItems");
    if let UiCommand::UpdateItems { items, context } = update_cmd {
        assert_eq!(context, vec!["Gmail".to_string()]);
        assert_eq!(
            items.len(),
            3,
            "Descending into Gmail should show all 3 emails"
        );
        for item in &items {
            assert_eq!(item.app_name, "Gmail");
        }
    } else {
        panic!("Expected UiCommand::UpdateItems, got {:?}", update_cmd);
    }

    // Clean up
    engine_tx.send(EngineMessage::Quit).await.unwrap();
    let _ = engine_handle.await;
}

#[tokio::test]
async fn test_single_group_auto_expand_and_backspace_to_delete() {
    let dir = tempdir().unwrap();
    let dump_path = dir.path().join("test_dump_single_group.json");
    let config = Config::default();

    let (ui_tx, mut ui_rx) = mpsc::channel::<UiCommand>(32);
    let (engine_tx, engine_rx) = mpsc::channel::<EngineMessage>(32);
    let icon_cache = std::sync::Arc::new(quiet::icon::IconCache::new(None, 32));
    let mut engine = Engine::new(config, dump_path, ui_tx, engine_rx, icon_cache);

    let engine_handle = tokio::spawn(async move {
        engine.run().await;
    });

    // Send 3 Gmail notifications (single group at root)
    for i in 1..=3 {
        let (reply_tx, reply_rx) = oneshot::channel();
        engine_tx
            .send(EngineMessage::Notify {
                app_name: "Google Chrome".into(),
                replaces_id: 0,
                app_icon: "gmail".into(),
                summary: format!("Sender {}", i),
                body: format!("mail.google.com\nEmail body {}", i),
                actions: vec![],
                hints: HashMap::new(),
                expire_timeout: -1,
                reply: reply_tx,
            })
            .await
            .unwrap();
        let _ = reply_rx.await.unwrap();
    }

    // When ShowNotifications is triggered, it should default to expanding the single group!
    engine_tx
        .send(EngineMessage::ShowNotifications)
        .await
        .unwrap();

    let open_cmd = ui_rx.recv().await.expect("Expected UiCommand::Open");
    if let UiCommand::Open { items, context } = open_cmd {
        // Must be auto-expanded into Gmail!
        assert_eq!(
            context,
            vec!["Gmail".to_string()],
            "Single group must auto-expand on open"
        );
        assert_eq!(
            items.len(),
            3,
            "Opened view must show the 3 individual notifications inside the group"
        );
        for item in &items {
            assert_eq!(item.app_name, "Gmail");
            assert!(!item.is_group);
        }
    } else {
        panic!("Expected UiCommand::Open, got {:?}", open_cmd);
    }

    // Now press Backspace (LeaveGroup): it should come a level up to root []
    use quiet::engine::UiEvent;
    engine_tx
        .send(EngineMessage::UiEvent(UiEvent::LeaveGroup))
        .await
        .unwrap();

    let update_cmd = ui_rx.recv().await.expect("Expected UiCommand::UpdateItems");
    if let UiCommand::UpdateItems { items, context } = update_cmd {
        assert!(context.is_empty(), "Backspace must come a level up to root");
        assert_eq!(items.len(), 1, "Root should display the single group");
        assert!(items[0].is_group, "Root item must be a group");
        assert_eq!(items[0].key, "Gmail");
        assert_eq!(items[0].count, 3);
        assert!(items[0].app_name.contains("Gmail (3)"));
    } else {
        panic!("Expected UiCommand::UpdateItems, got {:?}", update_cmd);
    }

    // Now delete the group at once while at root
    engine_tx
        .send(EngineMessage::UiEvent(UiEvent::DeleteGroup("Gmail".into())))
        .await
        .unwrap();

    // Since tree is now empty, UI should close
    let close_cmd = ui_rx.recv().await.expect("Expected UiCommand::Close");
    assert!(matches!(close_cmd, UiCommand::Close));

    // Verify count is 0
    let (reply_tx, reply_rx) = oneshot::channel();
    engine_tx
        .send(EngineMessage::ShowNotificationCount { reply: reply_tx })
        .await
        .unwrap();
    let (count, _) = reply_rx.await.unwrap();
    assert_eq!(count, 0, "All notifications in group must be deleted");

    engine_tx.send(EngineMessage::Quit).await.unwrap();
    let _ = engine_handle.await;
}

