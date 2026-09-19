# Quiet

> Ultra-fast, pure-Rust, Wayland-native notification daemon and notification center.

<p align="center">
  <img src="quiet_screencast_crop.gif" alt="Quiet Notification Center Screencast" width="760"/>
</p>

Quiet is an exact, pure-Rust drop-in replacement for `i3-notifier`, written by LLMs. It provides a modern notification daemon supporting the FreeDesktop `org.freedesktop.Notifications` standard, window manager keybindings (e.g. Sway and i3), and status bar modules like `py3notifier` (`py3status`) via zero-polling signal subscriptions. Instead of launching external menu tools like Rofi or dmenu, Quiet renders an in-process, flicker-free Wayland layer-shell overlay directly into shared memory.

---

## Highlights

- **Pure Rust, Zero C-Header Dependencies**: Built on `zbus 5` and `smithay-client-toolkit 0.19`. Requires no GTK, Qt, or Cairo C-development headers.
- **py3notifier Compatibility**: 100% compatible drop-in replacement for `i3-notifier` with zero-polling status bar updates.
- **Wayland Native Layer-Shell Overlay**:
  - Uses `zwlr_layer_shell_v1` on the `Overlay` layer.
  - Rendered via `tiny-skia` with font shaping by `cosmic-text` directly into `wl_shm`.
  - In-place reactive tree editing: dismissing items (`d`) or clearing all (`Shift+D`) updates the overlay without unmapping, re-opening, or flickering.
- **Ultra-Fast Companion Client (`quietctl`)**: Standalone, zero-dependency C tool that speaks raw D-Bus binary wire protocol over UNIX domain sockets in **~0.8–1.3 ms** for instantaneous window manager keybindings.
- **Sway & i3 Race-Free Urgency Focus**: Subscribes to window manager IPC events before invoking notification actions, ensuring that when an application marks its window urgent, Quiet immediately switches to that workspace and focuses the window.
- **Smart Grouping & Multi-Level Clustering**:
  - Automatic hierarchical clustering (e.g. *Browser → WhatsApp → Contact*).
  - Built-in recognition for Chrome web apps (Gmail, WhatsApp, Google Chat, Google Meet, Twitter/X, Instagram).
  - Auto-descent into single-child nodes for friction-free navigation.
- **Client Auto-Dismiss Protection (`ignore_close`)**: Prevents web apps (like Gmail or Chrome 5-second timers) from silently discarding unread notifications before you acknowledge them.
- **Vim & Ergonomic Navigation**: Full keyboard control alongside mouse support.
- **Atomic Persistence & Single-Instance Safety**:
  - State persisted periodically and at shutdown to `$XDG_STATE_HOME/quiet/dump` via atomic tempfile replacement.
  - Automatic migration from legacy `i3-notifier` dumps.
  - Single-instance enforcement via advisory file locking (`flock`) on `$XDG_RUNTIME_DIR/quiet.pid`.

---

## py3notifier Compatibility & Zero-Polling Architecture

Quiet was engineered as an exact drop-in replacement for `i3-notifier` and status bar modules such as **`py3notifier`** (part of [`py3status`](https://github.com/ultrabug/py3status)).

### Zero-Polling D-Bus Signals

Traditional notification applets poll for unread notification counts on a periodic timer, wasting CPU cycles and battery. Quiet and `py3notifier` eliminate polling completely by using an event-driven signal architecture:

- **Signal**: `org.freedesktop.Notifications.NotificationsUpdated(mode: u32, num: u32, urgency: u32, single_line: String)`
  - `mode`: `0` = Added, `1` = Deleted, `2` = Manual / Request
  - `num`: Current active notification count
  - `urgency`: Highest urgency level (`0` = Low, `1` = Normal, `2` = Critical)
  - `single_line`: Formatted string (`Summary : Body`) tailored for status bar display
- When idle, Quiet and your status bar consume **0% CPU** and perform **0 timer wakeups**.
- `py3notifier` sets `cached_until: CACHE_FOREVER` and updates instantly upon receiving the `NotificationsUpdated` signal.

### Implemented FreeDesktop & Extended D-Bus API

Quiet implements the full FreeDesktop notification specification plus the extended methods expected by `i3-notifier` and `py3notifier`:

| Method / Signal | Description |
|---|---|
| `Notify(...) -> id` | Standard notification creation / replacement |
| `CloseNotification(id)` | Explicit notification dismissal |
| `GetCapabilities() -> [...]` | Returns `actions`, `body`, `body-markup`, `icon-static`, `persistence` |
| `GetServerInformation() -> (...)` | Returns server identity (`quiet`, `quiet-project`, version, spec `1.2`) |
| `DumpNotifications() -> json` | Dumps non-transient notifications as JSON |
| `ShowNotificationCount() -> (count, urgency)` | Returns current unread count and highest urgency |
| `ShowNotifications()` | Toggles the layer-shell notification center overlay |
| `SignalNotificationCount()` | Requests an immediate `NotificationsUpdated` signal |
| `Quit()` | Triggers clean shutdown and state flush |
| `NotificationsUpdated` | Signal emitted whenever notification state changes |
| `NotificationClosed` | Standard FreeDesktop signal |
| `ActionInvoked` | Standard FreeDesktop signal |

### Automatic Migration from `i3-notifier`

If Quiet starts without an existing dump file, it automatically checks for an existing `i3-notifier` dump (`$XDG_STATE_HOME/i3-notifier/dump` or `$XDG_CACHE_HOME/i3-notifier/dump`). If found, Quiet loads the existing notifications and migrates them seamlessly.

### py3status Configuration Example

In your `~/.config/i3status.conf` or `~/.config/py3status/config`:

```ini
order += "py3notifier"

py3notifier {
    # py3notifier subscribes to NotificationsUpdated D-Bus signal;
    # no polling interval needed!
    format = "[\?color=urgency 🔔 {count}]"
    format_empty = "🔕 0"
    on_click 1 = "exec quietctl -t"
}
```

---

## Installation

### Pre-Built Binaries

Download pre-compiled Linux binaries (`x86_64` glibc and static musl, or `aarch64`) from the [Releases](https://github.com/sselcuk/quiet/releases) page.

### Building from Source

Ensure you have Rust (1.80+) installed:

```bash
git clone https://github.com/sselcuk/quiet.git
cd quiet

# Build quiet notification daemon
cargo build --release
install -Dm755 target/release/quiet ~/.local/bin/quiet

# Build and install quietctl companion client
make -C tools install PREFIX=~/.local
```

---

## Window Manager Configuration (Sway / i3)

### Fast Overlay Toggle with `quietctl`

Quiet includes `quietctl`, a high-performance C companion tool compiled without external dependencies that communicates directly with the D-Bus UNIX socket. It executes in **~0.8–1.3 ms** (~3–5× faster than `busctl` or Python scripts):

```sway
# Sway (~/.config/sway/config) or i3 (~/.config/i3/config):

# Option 1: Ultra-fast companion tool (recommended)
bindsym $mod+e exec quietctl

# Option 2: Built-in CLI client
bindsym $mod+e exec quiet --toggle

# Option 3: Standard dbus-send
bindsym $mod+e exec dbus-send --session --dest=org.freedesktop.Notifications /org/freedesktop/Notifications org.freedesktop.Notifications.ShowNotifications
```

> **Note**: Toggling only opens the overlay when notifications exist. If there are no active notifications, invoking `$mod+e` does not trigger the UI overlay.

---

## Systemd User Service

Run Quiet as a systemd user service (`~/.config/systemd/user/quiet.service`):

```ini
[Unit]
Description=Quiet notification daemon and center
After=graphical-session.target
PartOf=graphical-session.target

[Service]
Type=simple
ExecStart=%h/.local/bin/quiet --nodaemon
Restart=always
RestartSec=3

[Install]
WantedBy=sway-session.target
```

Enable and start the service:

```bash
systemctl --user daemon-reload
systemctl --user enable --now quiet.service
```

---

## Keyboard & Mouse Shortcuts

When the notification overlay is open:

| Keybinding | Action |
|---|---|
| `j` / `Down` | Select next item |
| `k` / `Up` | Select previous item |
| `Enter` / `Space` | Expand group, or invoke default notification action |
| `Shift+Enter` | Trigger default action on group/item directly without expanding |
| `d` / `Delete` | Dismiss selected notification or entire group |
| `Shift+Delete` / `Ctrl+Delete` | Dismiss notification within group |
| `Shift+D` | Clear all notifications (Caps-Lock safe; requires Shift modifier) |
| `Esc` / `h` / `Left` / `Backspace` | Return to parent group (or close overlay if at root) |
| `q` / `` ` `` (grave) / `Ctrl+Backspace` | Close notification overlay |
| **Mouse Left Click** | Select, expand group, or invoke notification action |
| **Mouse Right Click** | Dismiss clicked item |

---

## Configuration

Quiet looks for configuration files at the following locations in order:
1. `--config <PATH>` (command line override)
2. `~/.config/quiet.toml`
3. `~/.config/quiet/config.toml`
4. `~/.config/sway/quiet.toml`
5. `~/.config/i3/quiet.toml`

### Example `quiet.toml`

```toml
[behavior]
sort = "newest"               # Global default sort: "newest" or "oldest"
default_expires = false
urgent_timeout_ms = 2500

[ui]
font_family = "Fira Mono"
font_size = 21.0
width = 600
margin_top = 0
padding = 10.0                # Window padding around cards (px)
card_height = 98.0            # Height of each notification card (px)
card_spacing = 10.0           # Vertical spacing between cards (px)
corner_radius = 3.0           # Rounded corner radius (px)
max_visible_cards = 8         # Maximum cards visible before scrolling
show_icons = true
icon_size = 64

# Declarative grouping and matching rules:
[[rules]]
name = "WhatsApp"
app_name = "Google Chrome"
body_prefix = "web.whatsapp.com"
strip_body_prefix = "web.whatsapp.com"
set_app_name = "WhatsApp"
set_app_icon = "whatsapp"
group_by = ["app_name", "summary"]
sort = "oldest"               # Sort WhatsApp messages from a person oldest-to-newest!

[[rules]]
name = "Gmail"
app_name = "Google Chrome"
body_prefix = "mail.google.com"
strip_body_prefix = "mail.google.com"
set_app_name = "Gmail"
set_app_icon = "gmail"
group_by = ["app_name", "body"]

[[rules]]
name = "notify-send"
app_name = "notify-send"
set_expires = true
group_by = ["app_name", "body"]
```

---

## CLI Reference

### `quiet`

```text
quiet [OPTIONS]

Options:
      --nodaemon               Run in foreground (default for systemd)
  -k, --kill                   Terminate any running quiet notification daemon
  -t, --toggle                 Toggle notifications UI overlay on or off
  -d, --dump                   Dump active notifications as JSON to stdout
  -n, --count                  Output current notification count and urgency
  -c, -C, --config <CONFIG>    Path to custom config file
      --dump-path <DUMP_PATH>  Path to notification dump file
      --test                   Populate daemon with sample test notifications
  -h, --help                   Print help
  -V, --version                Print version
```

### `quietctl`

```text
quietctl [OPTIONS]

Options:
  -t, --toggle    Toggle notification center overlay (default, async)
  -w, --wait      Toggle notification center and wait for ACK (sync)
  -n, --count     Print current notification count to stdout
  -u, --urgency   Print current notification urgency (0=low, 1=normal, 2=critical)
  -k, --kill      Terminate running Quiet daemon via D-Bus Quit
  -h, --help      Show this help message
```

---

## License

GNU General Public License v3.0 or later ([GPL-3.0-or-later](LICENSE)).
