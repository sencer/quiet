use serde::Deserialize;
use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tracing::{debug, info};

const IPC_MAGIC: &[u8; 6] = b"i3-ipc";
const MESSAGE_TYPE_RUN_COMMAND: u32 = 0;
const MESSAGE_TYPE_SUBSCRIBE: u32 = 2;
const MESSAGE_TYPE_GET_TREE: u32 = 4;
const EVENT_MASK: u32 = 1 << 31;

#[derive(Debug, Clone, Default)]
pub struct FocusTargetHints {
    pub app_name: String,
    pub original_app_name: Option<String>,
    pub desktop_entry: Option<String>,
    pub summary: String,
    pub body: String,
}

#[derive(Debug, Deserialize)]
struct SwayNode {
    id: i64,
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "type")]
    node_type: Option<String>,
    #[serde(default)]
    app_id: Option<String>,
    #[serde(default)]
    window_properties: Option<SwayWindowProperties>,
    #[serde(default)]
    urgent: bool,
    #[serde(default)]
    nodes: Vec<SwayNode>,
    #[serde(default)]
    floating_nodes: Vec<SwayNode>,
}

#[derive(Debug, Deserialize)]
struct SwayWindowEvent {
    #[serde(default)]
    change: String,
    #[serde(default)]
    container: Option<SwayNode>,
}

#[derive(Debug, Deserialize)]
struct SwayWindowProperties {
    #[serde(default)]
    class: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

fn find_best_matching_container(node: &SwayNode, hints: &FocusTargetHints) -> (i64, i32) {
    let mut best_id = -1;
    let mut best_score = 0;

    let is_window = node.app_id.is_some()
        || node.window_properties.is_some()
        || (node.node_type.as_deref() == Some("con") && node.nodes.is_empty() && node.name.is_some());

    if is_window {
        let class = node
            .window_properties
            .as_ref()
            .and_then(|wp| wp.class.as_deref());
        let title = node
            .name
            .as_deref()
            .or_else(|| node.window_properties.as_ref().and_then(|wp| wp.title.as_deref()));
        let mut score = score_sway_container(
            title,
            node.app_id.as_deref(),
            class,
            hints,
        );
        if score > 0 && node.urgent {
            score += 150;
        }
        if score > best_score {
            best_score = score;
            best_id = node.id;
        }
    }

    for child in node.nodes.iter().chain(node.floating_nodes.iter()) {
        let (cid, cscore) = find_best_matching_container(child, hints);
        if cscore > best_score {
            best_score = cscore;
            best_id = cid;
        }
    }

    (best_id, best_score)
}

fn normalize_app_token(s: &str) -> String {
    s.trim().to_lowercase().replace(['-', '_'], " ")
}

fn is_browser_name(s: &str) -> bool {
    let lower = s.to_lowercase();
    lower.contains("chrome")
        || lower.contains("chromium")
        || lower.contains("brave")
        || lower.contains("firefox")
}

fn is_known_web_app(app: &str) -> bool {
    matches!(
        app,
        "gmail"
            | "chat"
            | "google chat"
            | "meet"
            | "google meet"
            | "whatsapp"
            | "twitter"
            | "instagram"
    )
}

fn extract_domain_keywords(app: &str) -> Vec<&str> {
    if !app.contains('.') || app.contains(' ') {
        return Vec::new();
    }
    let parts: Vec<&str> = app.split('.').filter(|p| !p.is_empty()).collect();
    let filtered: Vec<&str> = parts
        .iter()
        .copied()
        .filter(|&p| {
            !matches!(
                p,
                "www"
                    | "web"
                    | "app"
                    | "m"
                    | "com"
                    | "org"
                    | "net"
                    | "io"
                    | "co"
                    | "edu"
                    | "gov"
                    | "corp"
            )
        })
        .collect();

    let without_google: Vec<&str> = filtered
        .iter()
        .copied()
        .filter(|&p| p != "google")
        .collect();
    if !without_google.is_empty() {
        without_google
    } else if !filtered.is_empty() {
        filtered
    } else {
        parts.into_iter().take(1).collect()
    }
}

pub fn score_sway_container(
    name: Option<&str>,
    app_id: Option<&str>,
    class: Option<&str>,
    hints: &FocusTargetHints,
) -> i32 {
    let mut score = 0;
    let name_lower = name.map(|s| s.to_lowercase()).unwrap_or_default();
    let app_id_lower = app_id.map(|s| s.to_lowercase()).unwrap_or_default();
    let class_lower = class.map(|s| s.to_lowercase()).unwrap_or_default();
    let app_id_norm = normalize_app_token(&app_id_lower);
    let class_norm = normalize_app_token(&class_lower);

    let target_app = hints.app_name.trim().to_lowercase();
    let target_app_norm = normalize_app_token(&target_app);
    let orig_app = hints
        .original_app_name
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let orig_app_norm = normalize_app_token(&orig_app);
    let desktop_entry = hints
        .desktop_entry
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let desktop_entry_norm = normalize_app_token(&desktop_entry);
    let target_summary = hints.summary.trim().to_lowercase();

    let domain_keywords = extract_domain_keywords(&target_app);

    let is_browser = is_browser_name(&app_id_lower)
        || is_browser_name(&class_lower)
        || name_lower.ends_with("- google chrome")
        || name_lower.ends_with("- chromium")
        || name_lower.ends_with("- brave")
        || name_lower.ends_with("— mozilla firefox");

    // 1. Summary match in window title (e.g. contact "Mom" or document title)
    if !target_summary.is_empty() && name_lower.contains(&target_summary) {
        score += 100;
    }

    // 2. Match on app_name in app_id / class
    if !target_app.is_empty() {
        if (!app_id_norm.is_empty() && app_id_norm == target_app_norm)
            || (!class_norm.is_empty() && class_norm == target_app_norm)
        {
            score += 60;
        } else if (!app_id_norm.is_empty() && app_id_norm.contains(&target_app_norm))
            || (!class_norm.is_empty() && class_norm.contains(&target_app_norm))
        {
            score += 50;
        } else if (!app_id_norm.is_empty() && target_app_norm.contains(&app_id_norm))
            || (!class_norm.is_empty() && target_app_norm.contains(&class_norm))
        {
            score += 40;
        }

        // Window title contains app_name (e.g. "WhatsApp", "Gmail", "Spotify")
        if name_lower.contains(&target_app)
            || (is_browser && target_app == "gmail" && name_lower.contains("mail"))
        {
            score += 40;
        }

        // Browser web-app matching (e.g. Chrome window running WhatsApp, Gmail, or domain)
        if is_browser {
            let matches_domain_kw = domain_keywords
                .iter()
                .any(|kw| kw.len() >= 2 && name_lower.contains(kw));
            let matches_web_title = name_lower.contains(&target_app)
                || (target_app == "gmail" && name_lower.contains("mail"))
                || matches_domain_kw;
            if matches_web_title {
                score += 60;
            }
        }
    }

    // 3. Fallback match on original_app_name / desktop_entry or browser container when
    // Chrome's active tab is currently a different tab than the one that sent the notification.
    let notif_is_from_browser = is_browser_name(&orig_app)
        || is_browser_name(&desktop_entry)
        || is_browser_name(&target_app)
        || !domain_keywords.is_empty()
        || is_known_web_app(&target_app);

    if is_browser && notif_is_from_browser {
        score += 25;
        let matches_orig = (!orig_app_norm.is_empty()
            && ((app_id_norm.contains(&orig_app_norm) || orig_app_norm.contains(&app_id_norm) && !app_id_norm.is_empty())
                || (class_norm.contains(&orig_app_norm) || orig_app_norm.contains(&class_norm) && !class_norm.is_empty())))
            || (!desktop_entry_norm.is_empty()
                && ((!app_id_norm.is_empty() && app_id_norm.contains(&desktop_entry_norm))
                    || (!class_norm.is_empty() && class_norm.contains(&desktop_entry_norm))));
        if matches_orig {
            score += 10;
        }
    } else if !orig_app_norm.is_empty() && orig_app_norm != target_app_norm {
        if (!app_id_norm.is_empty() && app_id_norm == orig_app_norm)
            || (!class_norm.is_empty() && class_norm == orig_app_norm)
        {
            score += 35;
        }
    }

    score
}

/// Locates the active Sway or i3 IPC Unix domain socket.
pub fn find_ipc_socket() -> Option<PathBuf> {
    // 1. Check environment variables
    if let Ok(sock) = std::env::var("SWAYSOCK") {
        let path = PathBuf::from(sock);
        if path.exists() {
            return Some(path);
        }
    }
    if let Ok(sock) = std::env::var("I3SOCK") {
        let path = PathBuf::from(sock);
        if path.exists() {
            return Some(path);
        }
    }

    // 2. Search /run/user/<uid>/
    if let Some(runtime_dir) = dirs::runtime_dir() {
        if let Ok(entries) = std::fs::read_dir(&runtime_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if (name_str.starts_with("sway-ipc") || name_str.starts_with("sway-"))
                    && name_str.ends_with(".sock")
                {
                    return Some(entry.path());
                }
            }
        }

        // Also check i3 directory
        let i3_dir = runtime_dir.join("i3");
        if i3_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&i3_dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if name_str.starts_with("ipc-socket") {
                        return Some(entry.path());
                    }
                }
            }
        }
    }

    None
}

/// Executes a notification action race-freely with Sway/i3 window urgency focus,
/// falling back to smart container search across outputs & workspaces.
pub fn spawn_race_free_action_focus<F, Fut>(
    timeout: Duration,
    hints: FocusTargetHints,
    invoke_action: F,
) -> tokio::task::JoinHandle<()>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    spawn_race_free_action_focus_with_socket(find_ipc_socket(), timeout, hints, invoke_action)
}

pub fn spawn_race_free_action_focus_with_socket<F, Fut>(
    sock_opt: Option<PathBuf>,
    timeout: Duration,
    hints: FocusTargetHints,
    invoke_action: F,
) -> tokio::task::JoinHandle<()>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        let sock_path = match sock_opt {
            Some(p) => p,
            None => {
                debug!("No Sway or i3 IPC socket found; invoking action directly");
                invoke_action().await;
                return;
            }
        };

        let mut stream = match UnixStream::connect(&sock_path).await {
            Ok(s) => s,
            Err(e) => {
                debug!(
                    "Failed to connect to Sway/i3 IPC socket {:?}: {}; invoking action directly",
                    sock_path, e
                );
                invoke_action().await;
                return;
            }
        };

        // 1. Pre-subscribe to workspace and window events
        let sub_payload = r#"["workspace", "window"]"#;
        if let Err(e) =
            send_ipc_message(&mut stream, MESSAGE_TYPE_SUBSCRIBE, sub_payload.as_bytes()).await
        {
            debug!(
                "Failed to send subscribe message: {}; invoking action directly",
                e
            );
            invoke_action().await;
            return;
        }

        // 2. Read subscription acknowledgment
        if let Err(e) = read_ipc_message(&mut stream).await {
            debug!(
                "Failed to read subscribe acknowledgment: {}; invoking action directly",
                e
            );
            invoke_action().await;
            return;
        }

        // 3. Subscription active! NOW invoke the D-Bus action
        invoke_action().await;

        // 4. Phase 1: Await an immediate urgency / new-window / tab-title event within a fast
        // initial window (~150ms or configured timeout) so active D-Bus handlers can raise
        // or switch tabs first.
        let fast_window = timeout.min(Duration::from_millis(150));
        let mut event_focused = false;

        let _ = tokio::time::timeout(fast_window, async {
            loop {
                let (msg_type, payload) = match read_ipc_message(&mut stream).await {
                    Ok(res) => res,
                    Err(_) => break,
                };

                if (msg_type & EVENT_MASK) != 0 {
                    if let Some(focus_cmd) =
                        focus_command_from_window_event(&payload, &hints, 0)
                    {
                        info!("Window event detected from action ({}); focusing window", focus_cmd);
                        let _ = send_ipc_message(
                            &mut stream,
                            MESSAGE_TYPE_RUN_COMMAND,
                            focus_cmd.as_bytes(),
                        )
                        .await;
                        let _ = read_ipc_message(&mut stream).await;
                        event_focused = true;
                        break;
                    }
                }
            }
        })
        .await;

        if event_focused {
            return;
        }

        // 5. Phase 2: Query GET_TREE and switch to / focus best matching window immediately
        // so notifications whose D-Bus ID was already closed by Chrome (e.g. ignore_close = true
        // or restored from dump) still focus the browser without waiting for the full timeout.
        let mut tree_focused_score = 0;
        let has_target_hints = !hints.app_name.is_empty()
            || !hints.summary.is_empty()
            || hints.original_app_name.is_some()
            || hints.desktop_entry.is_some();

        if has_target_hints {
            if let Ok(mut tree_stream) = UnixStream::connect(&sock_path).await {
                if send_ipc_message(&mut tree_stream, MESSAGE_TYPE_GET_TREE, &[]).await.is_ok() {
                    if let Ok((_msg_type, payload)) = read_ipc_message(&mut tree_stream).await {
                        if let Ok(root_node) = serde_json::from_slice::<SwayNode>(&payload) {
                            let (best_id, best_score) =
                                find_best_matching_container(&root_node, &hints);
                            if best_id > 0 && best_score > 0 {
                                info!(
                                    "Found matching Sway container ID {} (score {}); switching workspace & focusing",
                                    best_id, best_score
                                );
                                let focus_cmd = format!("[con_id={}] focus", best_id);
                                let _ = send_ipc_message(
                                    &mut tree_stream,
                                    MESSAGE_TYPE_RUN_COMMAND,
                                    focus_cmd.as_bytes(),
                                )
                                .await;
                                let _ = read_ipc_message(&mut tree_stream).await;
                                tree_focused_score = best_score;
                            }
                        }
                    }
                }
            }
        }

        // 6. Phase 3: Continue listening for the remainder of `timeout` (e.g. urgent_timeout_ms = 2500ms)
        // in case a slow Chrome Service Worker finishes waking up after 150ms and marks a window urgent
        // or switches tabs / opens a new window.
        let remaining = timeout.saturating_sub(fast_window);
        if !remaining.is_zero() {
            let _ = tokio::time::timeout(remaining, async {
                loop {
                    let (msg_type, payload) = match read_ipc_message(&mut stream).await {
                        Ok(res) => res,
                        Err(_) => break,
                    };

                    if (msg_type & EVENT_MASK) != 0 {
                        if let Some(focus_cmd) = focus_command_from_window_event(
                            &payload,
                            &hints,
                            tree_focused_score,
                        ) {
                            info!(
                                "Delayed window event detected from action ({}); focusing window",
                                focus_cmd
                            );
                            let _ = send_ipc_message(
                                &mut stream,
                                MESSAGE_TYPE_RUN_COMMAND,
                                focus_cmd.as_bytes(),
                            )
                            .await;
                            let _ = read_ipc_message(&mut stream).await;
                            break;
                        }
                    }
                }
            })
            .await;
        }
    })
}

fn focus_command_from_window_event(
    payload: &[u8],
    hints: &FocusTargetHints,
    min_score_exclusive: i32,
) -> Option<String> {
    let text = String::from_utf8_lossy(payload);
    if text.contains(r#""urgent":true"#) || text.contains(r#""change":"urgent""#) {
        return Some("[urgent=latest] focus".to_string());
    }

    if let Ok(ev) = serde_json::from_slice::<SwayWindowEvent>(payload) {
        if ev.change == "new" || ev.change == "title" {
            if let Some(ref container) = ev.container {
                let (cid, cscore) = find_best_matching_container(container, hints);
                if cid > 0 && cscore > min_score_exclusive {
                    return Some(format!("[con_id={}] focus", cid));
                }
            }
        }
    }

    None
}

async fn send_ipc_message(
    stream: &mut UnixStream,
    msg_type: u32,
    payload: &[u8],
) -> Result<(), std::io::Error> {
    let length = payload.len() as u32;
    let mut header = [0u8; 14];
    header[0..6].copy_from_slice(IPC_MAGIC);
    header[6..10].copy_from_slice(&length.to_le_bytes());
    header[10..14].copy_from_slice(&msg_type.to_le_bytes());

    stream.write_all(&header).await?;
    if !payload.is_empty() {
        stream.write_all(payload).await?;
    }
    stream.flush().await?;
    Ok(())
}

async fn read_ipc_message(
    stream: &mut UnixStream,
) -> Result<(u32, Vec<u8>), Box<dyn std::error::Error + Send + Sync>> {
    let mut header = [0u8; 14];
    stream.read_exact(&mut header).await?;

    if &header[0..6] != IPC_MAGIC {
        return Err("Invalid i3-ipc magic bytes in stream header".into());
    }

    let length = u32::from_le_bytes([header[6], header[7], header[8], header[9]]) as usize;
    if length > 16 * 1024 * 1024 {
        return Err("i3-ipc payload size exceeds safety limit of 16MB".into());
    }
    let msg_type = u32::from_le_bytes([header[10], header[11], header[12], header[13]]);

    let mut payload = vec![0u8; length];
    if length > 0 {
        stream.read_exact(&mut payload).await?;
    }

    Ok((msg_type, payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipc_magic() {
        assert_eq!(IPC_MAGIC, b"i3-ipc");
    }

    #[test]
    fn test_score_sway_container_whatsapp_chrome() {
        let hints = FocusTargetHints {
            app_name: "WhatsApp".into(),
            original_app_name: Some("Google Chrome".into()),
            desktop_entry: None,
            summary: "Mom".into(),
            body: "Dinner on Sunday?".into(),
        };

        // Exact match with Chrome WhatsApp tab and contact in title
        let score1 = score_sway_container(
            Some("(2) WhatsApp - Mom - Google Chrome"),
            Some("google-chrome"),
            Some("Google-chrome"),
            &hints,
        );
        assert!(score1 >= 200);

        // General WhatsApp Chrome tab without contact name
        let score2 = score_sway_container(
            Some("WhatsApp - Google Chrome"),
            Some("google-chrome"),
            Some("Google-chrome"),
            &hints,
        );
        assert!(score2 >= 100 && score2 < score1);

        // Chrome window on another tab (e.g. GitHub) while WhatsApp is a background tab:
        // must still score > 0 so Chrome gets focused if no foreground WhatsApp tab exists,
        // but strictly less than a foreground WhatsApp tab (score2).
        let score3 = score_sway_container(
            Some("quiet PR #42 - GitHub - Google Chrome"),
            Some("google-chrome"),
            Some("Google-chrome"),
            &hints,
        );
        assert!(score3 > 0 && score3 < score2);

        // Another app (e.g. Alacritty)
        let score4 = score_sway_container(
            Some("zsh"),
            Some("Alacritty"),
            Some("Alacritty"),
            &hints,
        );
        assert_eq!(score4, 0);
    }

    #[test]
    fn test_score_sway_container_native_app() {
        let hints = FocusTargetHints {
            app_name: "Spotify".into(),
            original_app_name: Some("Spotify".into()),
            desktop_entry: None,
            summary: "Daft Punk".into(),
            body: "Get Lucky".into(),
        };

        let score = score_sway_container(
            Some("Spotify Free"),
            Some("spotify"),
            Some("Spotify"),
            &hints,
        );
        assert!(score >= 100);

        // Unrelated Chrome window must score 0 for a native Spotify notification
        let chrome_score = score_sway_container(
            Some("quiet PR #42 - GitHub - Google Chrome"),
            None,
            Some("Google-chrome"),
            &hints,
        );
        assert_eq!(chrome_score, 0);
    }

    #[test]
    fn test_score_sway_container_web_push_hostname_matches_browser_tab() {
        let hints = FocusTargetHints {
            app_name: "cleverpush.com".into(),
            original_app_name: Some("Google Chrome".into()),
            desktop_entry: None,
            summary: "Germany is world champion!".into(),
            body: "Click to read more".into(),
        };

        let score = score_sway_container(
            Some("CleverPush - Push notifications for web browsers - Google Chrome"),
            None, // XWayland Chrome has no app_id
            Some("Google-chrome"),
            &hints,
        );

        assert!(score >= 80);

        // Subdomains like www.instagram.com and calendar.google.com
        let insta_hints = FocusTargetHints {
            app_name: "www.instagram.com".into(),
            original_app_name: Some("Google Chrome".into()),
            desktop_entry: None,
            summary: "New message".into(),
            body: "".into(),
        };
        let insta_tab_score = score_sway_container(
            Some("Instagram - Google Chrome"),
            None,
            Some("Google-chrome"),
            &insta_hints,
        );
        let other_tab_score = score_sway_container(
            Some("New Tab - Google Chrome"),
            None,
            Some("Google-chrome"),
            &insta_hints,
        );
        assert!(insta_tab_score > other_tab_score);
        assert!(other_tab_score > 0);
    }

    #[test]
    fn test_score_sway_container_xwayland_chrome_gmail_and_background_tab() {
        let hints = FocusTargetHints {
            app_name: "Gmail".into(),
            original_app_name: Some("Google Chrome".into()),
            desktop_entry: None,
            summary: "Alice Smith".into(),
            body: "Q4 architecture plan".into(),
        };

        // Corp Gmail tab ("Google.com Mail") on XWayland (app_id = None, class = "Google-chrome")
        let corp_gmail_score = score_sway_container(
            Some("Inbox (3) - user@google.com - Google.com Mail - Google Chrome"),
            None,
            Some("Google-chrome"),
            &hints,
        );
        // Chrome window on an unrelated tab ("New Tab - Google Chrome") on XWayland
        let bg_chrome_score = score_sway_container(
            Some("New Tab - Google Chrome"),
            None,
            Some("Google-chrome"),
            &hints,
        );
        assert!(corp_gmail_score >= 100);
        assert!(bg_chrome_score > 0 && bg_chrome_score < corp_gmail_score);
    }

    #[test]
    fn test_focus_command_from_window_event_urgent_and_title_change() {
        let hints = FocusTargetHints {
            app_name: "Gmail".into(),
            original_app_name: Some("Google Chrome".into()),
            desktop_entry: None,
            summary: "Alice".into(),
            body: "Hello".into(),
        };

        let urgent_event = br#"{"change":"urgent","container":{"id":42,"urgent":true}}"#;
        assert_eq!(
            focus_command_from_window_event(urgent_event, &hints, 0).as_deref(),
            Some("[urgent=latest] focus")
        );

        let title_event = br#"{"change":"title","container":{"id":99,"name":"Alice - Gmail - Google Chrome","window_properties":{"class":"Google-chrome"}}}"#;
        assert_eq!(
            focus_command_from_window_event(title_event, &hints, 35).as_deref(),
            Some("[con_id=99] focus")
        );
    }

    #[tokio::test]
    async fn test_race_free_action_focus_get_tree_and_delayed_urgency() {
        use std::sync::{Arc, Mutex};
        use tokio::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("mock-sway.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();

        let received_commands: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let cmds_clone = received_commands.clone();

        // Spawn mock Sway IPC server handling:
        // Connection 1: SUBSCRIBE -> sends delayed urgency event at 220ms -> receives [urgent=latest] focus
        // Connection 2: GET_TREE -> returns XWayland Chrome window on "New Tab" -> receives [con_id=77] focus
        let server_handle = tokio::spawn(async move {
            // Accept Connection 1 (event subscription stream)
            let (mut sub_stream, _) = listener.accept().await.unwrap();
            let (msg_type, _payload) = read_ipc_message(&mut sub_stream).await.unwrap();
            assert_eq!(msg_type, MESSAGE_TYPE_SUBSCRIBE);
            send_ipc_message(&mut sub_stream, MESSAGE_TYPE_SUBSCRIBE, br#"{"success":true}"#)
                .await
                .unwrap();

            // Accept Connection 2 (GET_TREE stream at ~150ms)
            let (mut tree_stream, _) = listener.accept().await.unwrap();
            let (t_type, _) = read_ipc_message(&mut tree_stream).await.unwrap();
            assert_eq!(t_type, MESSAGE_TYPE_GET_TREE);
            let tree_json = br#"{
                "id": 1,
                "type": "root",
                "nodes": [
                    {
                        "id": 77,
                        "type": "con",
                        "name": "New Tab - Google Chrome",
                        "window_properties": { "class": "Google-chrome" }
                    }
                ]
            }"#;
            send_ipc_message(&mut tree_stream, MESSAGE_TYPE_GET_TREE, tree_json)
                .await
                .unwrap();

            // Read [con_id=77] focus command on tree_stream
            let (cmd_type, cmd_payload) = read_ipc_message(&mut tree_stream).await.unwrap();
            assert_eq!(cmd_type, MESSAGE_TYPE_RUN_COMMAND);
            cmds_clone
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(&cmd_payload).to_string());
            send_ipc_message(&mut tree_stream, MESSAGE_TYPE_RUN_COMMAND, br#"[{"success":true}]"#)
                .await
                .unwrap();

            // Now simulate a slow Chrome Service Worker emitting an urgent window event 50ms later
            tokio::time::sleep(Duration::from_millis(50)).await;
            let urgent_payload = br#"{"change":"urgent","container":{"id":88,"urgent":true}}"#;
            send_ipc_message(&mut sub_stream, EVENT_MASK | 3, urgent_payload)
                .await
                .unwrap();

            // Read [urgent=latest] focus command on sub_stream
            let (u_type, u_payload) = read_ipc_message(&mut sub_stream).await.unwrap();
            assert_eq!(u_type, MESSAGE_TYPE_RUN_COMMAND);
            cmds_clone
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(&u_payload).to_string());
            send_ipc_message(&mut sub_stream, MESSAGE_TYPE_RUN_COMMAND, br#"[{"success":true}]"#)
                .await
                .unwrap();
        });

        let hints = FocusTargetHints {
            app_name: "Gmail".into(),
            original_app_name: Some("Google Chrome".into()),
            desktop_entry: None,
            summary: "Alice".into(),
            body: "Meeting".into(),
        };

        let focus_handle = spawn_race_free_action_focus_with_socket(
            Some(sock_path),
            Duration::from_millis(1000),
            hints,
            || async {},
        );

        focus_handle.await.unwrap();
        server_handle.await.unwrap();

        let cmds = received_commands.lock().unwrap().clone();
        assert_eq!(
            cmds,
            vec![
                "[con_id=77] focus".to_string(),
                "[urgent=latest] focus".to_string()
            ]
        );
    }
}
