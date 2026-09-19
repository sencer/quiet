use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};
use tracing::{debug, info};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopEntry {
    pub name: String,
    pub icon: String,
    pub startup_wm_class: Option<String>,
}

static DESKTOP_CACHE: OnceLock<RwLock<HashMap<String, Option<DesktopEntry>>>> = OnceLock::new();

fn get_cache() -> &'static RwLock<HashMap<String, Option<DesktopEntry>>> {
    DESKTOP_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Returns standard XDG data application directories to search for `.desktop` files,
/// strictly prioritizing user directories over system directories.
pub fn get_desktop_directories() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // 1. $XDG_DATA_HOME/applications (default: ~/.local/share/applications)
    if let Some(data_dir) = dirs::data_dir() {
        dirs.push(data_dir.join("applications"));
    }

    // 2. User Flatpak exports
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".local/share/flatpak/exports/share/applications"));
    }

    // 3. $XDG_DATA_DIRS/applications (default: /usr/local/share, /usr/share)
    if let Ok(data_dirs) = std::env::var("XDG_DATA_DIRS") {
        for p in data_dirs.split(':') {
            if !p.is_empty() {
                dirs.push(PathBuf::from(p).join("applications"));
            }
        }
    } else {
        dirs.push(PathBuf::from("/usr/local/share/applications"));
        dirs.push(PathBuf::from("/usr/share/applications"));
    }

    // 4. System Flatpak exports
    dirs.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));

    // 5. Snap applications
    dirs.push(PathBuf::from("/var/lib/snapd/desktop/applications"));

    dirs
}

/// Looks up desktop entry metadata (Name and Icon) by application name or desktop file ID.
pub fn lookup_desktop_entry(id: &str) -> Option<DesktopEntry> {
    if id.is_empty() {
        return None;
    }

    let clean_id = id.trim();

    // Check memoized cache
    let cache = get_cache();
    if let Ok(c) = cache.read() {
        if let Some(cached) = c.get(clean_id) {
            return cached.clone();
        }
    }

    let result = resolve_desktop_entry(clean_id);

    const MAX_DESKTOP_CACHE: usize = 256;

    // Update memoized cache (including negative misses to prevent re-walking trees)
    if let Ok(mut c) = cache.write() {
        if c.len() >= MAX_DESKTOP_CACHE {
            c.clear();
        }
        c.insert(clean_id.to_string(), result.clone());
    }

    result
}

fn resolve_desktop_entry(id: &str) -> Option<DesktopEntry> {
    let filename = if id.ends_with(".desktop") {
        id.to_string()
    } else {
        format!("{}.desktop", id)
    };

    let dirs = get_desktop_directories();

    // 1. Exact filename match in any directory
    for dir in &dirs {
        let path = dir.join(&filename);
        if path.is_file() {
            if let Some(entry) = parse_desktop_file(&path) {
                debug!("Found exact desktop entry for {:?} at {:?}", id, path);
                return Some(entry);
            }
        }
    }

    // 2. Normalized candidates: lowercase, space-to-hyphen, and reverse-DNS suffix
    let lower_name = filename.to_lowercase();
    let hyphenated = format!("{}.desktop", id.to_lowercase().replace(' ', "-"));
    let base_id = id.strip_suffix(".desktop").unwrap_or(id);
    let stripped_suffix = base_id.split('.').next_back().unwrap_or(base_id).to_lowercase();
    let stripped_filename = format!("{}.desktop", stripped_suffix);

    for dir in &dirs {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let fname = entry.file_name();
                let fname_str = fname.to_string_lossy();
                let fname_lower = fname_str.to_lowercase();

                if fname_lower == lower_name
                    || fname_lower == hyphenated
                    || fname_lower == stripped_filename
                {
                    if let Some(entry) = parse_desktop_file(&entry.path()) {
                        info!(
                            "Found desktop entry via fallback {:?} at {:?}",
                            id, entry.name
                        );
                        return Some(entry);
                    }
                }
            }
        }
    }

    None
}

/// Parses a `.desktop` file conforming to the XDG Desktop Entry Specification.
pub fn parse_desktop_file(path: &Path) -> Option<DesktopEntry> {
    let content = fs::read_to_string(path).ok()?;
    let mut in_desktop_entry_section = false;
    let mut name = None;
    let mut icon = None;
    let mut startup_wm_class = None;
    let mut is_hidden = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }

        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_desktop_entry_section = trimmed == "[Desktop Entry]";
            continue;
        }

        if in_desktop_entry_section {
            if let Some((k, v)) = trimmed.split_once('=') {
                let key = k.trim();
                let val = v.trim();
                match key {
                    "Name" if name.is_none() => {
                        name = Some(unescape_desktop_val(val));
                    }
                    "Icon" if icon.is_none() => {
                        icon = Some(unescape_desktop_val(val));
                    }
                    "StartupWMClass" if startup_wm_class.is_none() => {
                        startup_wm_class = Some(unescape_desktop_val(val));
                    }
                    "Hidden" if val.eq_ignore_ascii_case("true") => {
                        is_hidden = true;
                    }
                    _ => {}
                }
            }
        }
    }

    if is_hidden {
        return None;
    }

    if let (Some(name), Some(icon)) = (name, icon) {
        Some(DesktopEntry {
            name,
            icon,
            startup_wm_class,
        })
    } else {
        None
    }
}

/// Unescapes common XDG desktop entry escape sequences.
fn unescape_desktop_val(val: &str) -> String {
    let trimmed = val.trim().trim_matches('"');
    trimmed
        .replace(r"\s", " ")
        .replace(r"\n", "\n")
        .replace(r"\t", "\t")
        .replace(r"\r", "\r")
        .replace(r"\\", "\\")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_parse_desktop_file_with_escapes() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("slack.desktop");
        let content = r#"
[Desktop Entry]
Name=Slack\sDesktop
Comment=Slack Desktop App
Exec=/usr/bin/slack %U
Icon=slack
Type=Application
StartupWMClass=Slack
"#;
        fs::write(&file_path, content).unwrap();

        let entry = parse_desktop_file(&file_path).expect("Failed to parse desktop file");
        assert_eq!(entry.name, "Slack Desktop");
        assert_eq!(entry.icon, "slack");
        assert_eq!(entry.startup_wm_class.as_deref(), Some("Slack"));
    }

    #[test]
    fn test_hidden_desktop_file_ignored() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("hidden.desktop");
        let content = r#"
[Desktop Entry]
Name=OldApp
Icon=old-icon
Hidden=true
"#;
        fs::write(&file_path, content).unwrap();

        assert!(parse_desktop_file(&file_path).is_none());
    }

    #[test]
    fn test_reverse_dns_desktop_entry_resolution() {
        let base_id = "org.gnome.Calculator.desktop".strip_suffix(".desktop").unwrap();
        let stripped_suffix = base_id.split('.').next_back().unwrap_or(base_id).to_lowercase();
        assert_eq!(stripped_suffix, "calculator");
    }
}
