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
    nodes: Vec<SwayNode>,
    #[serde(default)]
    floating_nodes: Vec<SwayNode>,
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
        let score = score_sway_container(
            title,
            node.app_id.as_deref(),
            class,
            hints,
        );
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

    let target_app = hints.app_name.to_lowercase();
    let target_summary = hints.summary.to_lowercase();

    // 1. Summary match in window title (e.g. contact "Mom" or document title)
    if !target_summary.is_empty() && name_lower.contains(&target_summary) {
        score += 100;
    }

    // 2. Match on app_name in app_id / class
    if !target_app.is_empty() {
        if app_id_lower == target_app || class_lower == target_app {
            score += 60;
        } else if app_id_lower.contains(&target_app) || class_lower.contains(&target_app) {
            score += 50;
        } else if target_app.contains(&app_id_lower) && !app_id_lower.is_empty() {
            score += 40;
        }

        // Window title contains app_name (e.g. "WhatsApp", "Gmail", "Spotify")
        if name_lower.contains(&target_app) {
            score += 40;
        }

        // Browser web-app matching (e.g. Chrome window running WhatsApp or Gmail)
        let is_browser = app_id_lower.contains("chrome")
            || app_id_lower.contains("chromium")
            || class_lower.contains("chrome")
            || class_lower.contains("chromium")
            || app_id_lower.contains("brave")
            || app_id_lower.contains("firefox");
        if is_browser && name_lower.contains(&target_app) {
            score += 60;
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
    tokio::spawn(async move {
        let sock_path = match find_ipc_socket() {
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

        // 4. Await urgency event within a responsive window (~75ms or configured timeout)
        let urgency_window = timeout.min(Duration::from_millis(75));
        let mut urgent_focused = false;

        let _ = tokio::time::timeout(urgency_window, async {
            loop {
                let (msg_type, payload) = match read_ipc_message(&mut stream).await {
                    Ok(res) => res,
                    Err(_) => break,
                };

                // Check if this is an event message (bit 31 set)
                if (msg_type & EVENT_MASK) != 0 {
                    let text = String::from_utf8_lossy(&payload);
                    if text.contains(r#""urgent":true"#) || text.contains(r#""change":"urgent""#) {
                        info!("Urgent window event detected from action; focusing window");
                        let focus_cmd = "[urgent=latest] focus";
                        let _ = send_ipc_message(
                            &mut stream,
                            MESSAGE_TYPE_RUN_COMMAND,
                            focus_cmd.as_bytes(),
                        )
                        .await;
                        let _ = read_ipc_message(&mut stream).await;
                        urgent_focused = true;
                        break;
                    }
                }
            }
        })
        .await;

        if urgent_focused {
            return;
        }

        // 5. If no urgency event was received, query GET_TREE and switch to / focus best matching window
        if !hints.app_name.is_empty() || !hints.summary.is_empty() {
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
                            }
                        }
                    }
                }
            }
        }
    })
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

        // Irrelevant Chrome tab (e.g. GitHub)
        let score3 = score_sway_container(
            Some("quiet PR #42 - GitHub - Google Chrome"),
            Some("google-chrome"),
            Some("Google-chrome"),
            &hints,
        );
        assert_eq!(score3, 0);

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
    }
}
