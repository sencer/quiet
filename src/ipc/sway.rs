use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tracing::{debug, info};

const IPC_MAGIC: &[u8; 6] = b"i3-ipc";
const MESSAGE_TYPE_RUN_COMMAND: u32 = 0;
const MESSAGE_TYPE_SUBSCRIBE: u32 = 2;
const EVENT_MASK: u32 = 1 << 31;

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

/// Executes a notification action race-freely with Sway/i3 window urgency focus.
/// Subscribes to workspace/window events and awaits acknowledgment BEFORE invoking the action,
/// ensuring that when the client application sets the window urgent, Quiet catches and focuses it.
pub fn spawn_race_free_action_focus<F, Fut>(
    timeout: Duration,
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

        // 4. Await urgency event within timeout
        let _ = tokio::time::timeout(timeout, async {
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
                        break;
                    }
                }
            }
        })
        .await;
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
}
