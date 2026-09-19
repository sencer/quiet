use quiet::config::Config;
use quiet::data::persistence::default_dump_path;
use quiet::dbus::NotificationsInterface;
use quiet::engine::{Engine, EngineMessage};
use quiet::ui::spawn_ui_worker;
use clap::Parser;
use rustix::fs::{flock, FlockOperation};
use std::fs::OpenOptions;
use std::io::Write;
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

#[derive(Parser, Debug)]
#[command(
    name = "quiet",
    author = "Sencer Selcuk <sencerselcuk@gmail.com>",
    version,
    about = "High-performance, pure-Rust, Wayland-native notification daemon and center"
)]
struct Cli {
    /// Run in foreground / nodaemon mode (default for systemd)
    #[arg(long, default_value_t = true)]
    nodaemon: bool,

    /// Terminate any running quiet notification daemon
    #[arg(short, long)]
    kill: bool,

    /// Toggle notifications UI on or off
    #[arg(short, long)]
    toggle: bool,

    /// Dump active notifications as JSON to stdout
    #[arg(short, long)]
    dump: bool,

    /// Output current notification count and urgency
    #[arg(short = 'n', long)]
    count: bool,

    /// Path to custom config file (-c or -C)
    #[arg(short = 'c', short_alias = 'C', long)]
    config: Option<PathBuf>,

    /// Path to notification dump file for state persistence
    #[arg(long)]
    dump_path: Option<PathBuf>,

    /// Populate daemon with sample test notifications (Gmail, WhatsApp, ungrouped)
    #[arg(long)]
    test: bool,
}

fn pid_file_path() -> PathBuf {
    if let Some(runtime_dir) = dirs::runtime_dir() {
        runtime_dir.join("quiet.pid")
    } else {
        PathBuf::from("/tmp/quiet.pid")
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Client action: --kill
    if cli.kill {
        if let Ok(conn) = zbus::connection::Connection::session().await {
            let proxy = zbus::Proxy::new(
                &conn,
                "org.freedesktop.Notifications",
                "/org/freedesktop/Notifications",
                "org.freedesktop.Notifications",
            )
            .await;
            if let Ok(proxy) = proxy {
                let call_res: zbus::Result<()> = proxy.call("Quit", &()).await;
                if call_res.is_ok() {
                    println!("Quiet notification daemon terminated.");
                    return Ok(());
                }
            }
        }

        // Fallback to PID file kill
        let pid_path = pid_file_path();
        if pid_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&pid_path) {
                if let Ok(pid) = content.trim().parse::<i32>() {
                    if pid > 0 {
                        if let Some(proc_pid) = rustix::process::Pid::from_raw(pid) {
                            let _ = rustix::process::kill_process(
                                proc_pid,
                                rustix::process::Signal::Term,
                            );
                            println!("Sent SIGTERM to quiet PID {}", pid);
                            let _ = std::fs::remove_file(&pid_path);
                            return Ok(());
                        }
                    }
                }
            }
        }
        println!("No running quiet instance found.");
        return Ok(());
    }

    // Client action: --dump
    if cli.dump {
        let conn = zbus::connection::Connection::session().await?;
        let proxy = zbus::Proxy::new(
            &conn,
            "org.freedesktop.Notifications",
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
        )
        .await?;
        let dump_json: String = proxy.call("DumpNotifications", &()).await?;
        println!("{}", dump_json);
        return Ok(());
    }

    // Client action: --toggle
    if cli.toggle {
        let conn = zbus::connection::Connection::session().await?;
        let proxy = zbus::Proxy::new(
            &conn,
            "org.freedesktop.Notifications",
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
        )
        .await?;
        let _: () = proxy.call("ShowNotifications", &()).await?;
        return Ok(());
    }

    // Client action: --count
    if cli.count {
        let conn = zbus::connection::Connection::session().await?;
        let proxy = zbus::Proxy::new(
            &conn,
            "org.freedesktop.Notifications",
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
        )
        .await?;
        let (count, urgency): (u32, u32) = proxy.call("ShowNotificationCount", &()).await?;
        println!("Count: {}, Urgency: {}", count, urgency);
        return Ok(());
    }

    // Client action: --test
    if cli.test {
        let conn = zbus::connection::Connection::session().await?;
        let proxy = zbus::Proxy::new(
            &conn,
            "org.freedesktop.Notifications",
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
        )
        .await?;

        println!("Populating test notifications...");
        let test_notifications: Vec<(&str, &str, &str, &str, u8)> = vec![
            // Gmail notifications (Chrome with mail.google.com prefix)
            ("Google Chrome", "gmail", "Alice Smith", "mail.google.com\nHey, let's review the Q4 architecture plan.", 1),
            ("Google Chrome", "gmail", "Bob Jones", "mail.google.com\nTeam sync tomorrow at 10:00 AM", 1),
            ("Google Chrome", "gmail", "GitHub Notifications", "mail.google.com\n[quiet] 3 new comments on PR #42", 1),

            // WhatsApp notifications (Chrome with web.whatsapp.com prefix)
            ("Google Chrome", "whatsapp", "Mom", "web.whatsapp.com\nAre you coming over for dinner on Sunday?", 1),
            ("Google Chrome", "whatsapp", "Mom", "web.whatsapp.com\nLet me know so I can cook your favorites!", 1),
            ("Google Chrome", "whatsapp", "Dev Team", "web.whatsapp.com\nDeployment to production is complete 🚀", 1),

            // Ungrouped notifications
            ("Spotify", "spotify", "Daft Punk", "Get Lucky (feat. Pharrell Williams)", 1),
            ("Alacritty", "utilities-terminal", "Cargo Build", "Finished release [optimized] target(s) in 51.94s", 1),
            ("Power Manager", "battery-caution", "Battery Low", "Battery level is at 18%, please plug in AC adapter.", 2),
        ];

        for (app_name, icon, summary, body, urgency) in test_notifications {
            let mut hints: std::collections::HashMap<String, zvariant::Value> = std::collections::HashMap::new();
            hints.insert("urgency".to_string(), zvariant::Value::U8(urgency));
            let actions: Vec<String> = vec![];
            let _: u32 = proxy
                .call(
                    "Notify",
                    &(app_name, 0u32, icon, summary, body, actions, hints, -1i32),
                )
                .await?;
        }

        println!("Successfully populated 9 test notifications:");
        println!("  - 3 Gmail (grouped under Gmail)");
        println!("  - 3 WhatsApp (grouped under WhatsApp, sub-grouped by contact)");
        println!("  - 3 Ungrouped (Spotify, Alacritty terminal, Power Manager critical)");
        println!();
        println!("Run 'quietctl -t' or 'quiet -t' to open the UI dropdown.");
        return Ok(());
    }

    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "quiet=info,warn".into()),
        )
        .init();

    info!(
        "Starting Quiet notification daemon v{}",
        env!("CARGO_PKG_VERSION")
    );

    // Acquire single instance lock
    let pid_path = pid_file_path();
    let mut pid_file = match OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&pid_path)
    {
        Ok(f) => f,
        Err(e) => {
            error!("Failed to open PID lock file {:?}: {}", pid_path, e);
            return Err(e.into());
        }
    };

    match flock(pid_file.as_fd(), FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => {
            let _ = pid_file.set_len(0);
            let _ = writeln!(pid_file, "{}", std::process::id());
        }
        Err(e) => {
            warn!(
                "Another instance of quiet is already running (locked {:?}): {}",
                pid_path, e
            );
            println!("Another instance of quiet is already running. Exiting.");
            return Ok(());
        }
    }

    // Load configuration
    let config = Config::load(cli.config.as_deref());
    let dump_path = cli.dump_path.unwrap_or_else(default_dump_path);

    // Create channels
    let (engine_tx, engine_rx) = mpsc::channel::<EngineMessage>(512);

    // Shared icon cache between Engine and UI thread
    let icon_cache = Arc::new(quiet::icon::IconCache::new(
        config.ui.icon_theme.clone(),
        config.ui.icon_size,
    ));

    // Spawn native Wayland layer-shell UI worker
    let ui_tx = spawn_ui_worker(config.clone(), engine_tx.clone(), icon_cache.clone());

    // Initialize Engine
    let mut engine = Engine::new(config, dump_path, ui_tx, engine_rx, icon_cache);

    // Setup D-Bus service
    let iface = NotificationsInterface::new(engine_tx.clone());
    let dbus_conn = match zbus::connection::Builder::session()?
        .name("org.freedesktop.Notifications")?
        .serve_at("/org/freedesktop/Notifications", iface)?
        .build()
        .await
    {
        Ok(c) => c,
        Err(e) => {
            error!(
                "Failed to acquire D-Bus name org.freedesktop.Notifications: {}",
                e
            );
            let _ = std::fs::remove_file(&pid_path);
            return Err(e.into());
        }
    };

    info!("Successfully acquired D-Bus name org.freedesktop.Notifications");
    engine.set_dbus_connection(dbus_conn);

    // Graceful signal listener
    let shutdown_tx = engine_tx.clone();
    tokio::spawn(async move {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate()).ok();
        let mut sigint = signal(SignalKind::interrupt()).ok();

        tokio::select! {
            _ = async {
                if let Some(ref mut s) = sigterm {
                    s.recv().await
                } else {
                    std::future::pending().await
                }
            } => {
                info!("Received SIGTERM; requesting clean shutdown");
            }
            _ = async {
                if let Some(ref mut s) = sigint {
                    s.recv().await
                } else {
                    std::future::pending().await
                }
            } => {
                info!("Received SIGINT; requesting clean shutdown");
            }
        }
        let _ = shutdown_tx.send(EngineMessage::Quit).await;
    });

    // Run main engine actor loop
    engine.run().await;

    // Cleanup PID file on exit
    let _ = std::fs::remove_file(&pid_path);
    info!("Quiet terminated cleanly");

    Ok(())
}
