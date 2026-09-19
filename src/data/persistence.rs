use super::notification::Notification;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

pub fn default_dump_path() -> PathBuf {
    if let Some(state_dir) = dirs::state_dir() {
        state_dir.join("quiet").join("dump")
    } else if let Some(cache_dir) = dirs::cache_dir() {
        cache_dir.join("quiet").join("dump")
    } else {
        PathBuf::from("/tmp/quiet-dump")
    }
}

/// Loads dumped notifications from disk, filtering out expired items.
pub fn load_dump(path: &Path) -> Vec<Notification> {
    let effective_path = if path.exists() {
        path.to_path_buf()
    } else if path == default_dump_path() {
        let i3_dump = dirs::state_dir()
            .map(|s| s.join("i3-notifier").join("dump"))
            .or_else(|| dirs::cache_dir().map(|c| c.join("i3-notifier").join("dump")));
        if let Some(ref p) = i3_dump {
            if p.exists() {
                info!("Found existing i3-notifier dump at {:?}; importing active notifications", p);
                p.clone()
            } else {
                info!("No dump file found at {:?}, starting clean", path);
                return Vec::new();
            }
        } else {
            info!("No dump file found at {:?}, starting clean", path);
            return Vec::new();
        }
    } else {
        info!("No dump file found at {:?}, starting clean", path);
        return Vec::new();
    };

    let file = match File::open(&effective_path) {
        Ok(f) => f,
        Err(e) => {
            warn!("Failed to open dump file {:?}: {}", effective_path, e);
            return Vec::new();
        }
    };

    let reader = BufReader::new(file);
    let raw_list: Result<Vec<Notification>, _> = serde_json::from_reader(reader);

    let notifications = match raw_list {
        Ok(list) => list,
        Err(e) => {
            warn!(
                "Failed to parse dump file {:?} as JSON (corrupted/truncated?): {}",
                path, e
            );
            return Vec::new();
        }
    };

    let now_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;

    let valid: Vec<Notification> = notifications
        .into_iter()
        .filter(|n| {
            if n.expires {
                if let Some(exp_ns) = n.expires_at {
                    return exp_ns > now_ns;
                }
            }
            true
        })
        .collect();

    info!(
        "Successfully restored {} active notifications from {:?}",
        valid.len(),
        path
    );
    valid
}

/// Atomically persists notifications to disk via temporary file rename.
pub fn save_dump(path: &Path, notifications: &[Notification]) -> std::io::Result<()> {
    let parent = match path.parent() {
        Some(p) => p,
        None => Path::new("/tmp"),
    };

    fs::create_dir_all(parent)?;

    let mut temp = tempfile::Builder::new()
        .prefix(".dump_")
        .suffix(".tmp")
        .tempfile_in(parent)?;

    {
        let mut writer = BufWriter::new(&mut temp);
        serde_json::to_writer_pretty(&mut writer, notifications).map_err(std::io::Error::other)?;
        writer.flush()?;
    }

    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_save_and_load_dump_roundtrip() {
        let dir = tempdir().unwrap();
        let dump_path = dir.path().join("dump");

        let n1 = Notification::new(
            1,
            "slack".into(),
            "".into(),
            "sum".into(),
            "body".into(),
            vec![],
            1,
            -1, // Does not expire
        );
        let n2 = Notification::new(
            2,
            "mail".into(),
            "".into(),
            "sum2".into(),
            "body2".into(),
            vec![],
            2,
            60000, // Expire in 60s
        );

        let list = vec![n1.clone(), n2.clone()];
        save_dump(&dump_path, &list).unwrap();
        assert!(dump_path.exists());

        let restored = load_dump(&dump_path);
        assert_eq!(restored.len(), 2);
        assert_eq!(restored[0].id, 1);
        assert_eq!(restored[1].id, 2);
    }

    #[test]
    fn test_corrupt_dump_recovery() {
        let dir = tempdir().unwrap();
        let dump_path = dir.path().join("dump_corrupt");
        fs::write(&dump_path, b"[{not valid json").unwrap();

        let restored = load_dump(&dump_path);
        assert_eq!(restored.len(), 0);
    }

    #[test]
    fn test_nonexistent_custom_path_starts_clean() {
        let dir = tempdir().unwrap();
        let dump_path = dir.path().join("does_not_exist");
        let restored = load_dump(&dump_path);
        assert_eq!(restored.len(), 0);
    }
}
