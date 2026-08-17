use std::io;
use std::path::Path;

use super::queue::QueueEntry;

/// Write via a temp file and rename, so an interrupted write can never
/// truncate the previous good state.
pub fn save_entries(path: &Path, entries: &[QueueEntry]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(entries)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, json)?;
    match std::fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Never leave a stray temp file behind on a failed rename.
            let _ = std::fs::remove_file(&temp);
            Err(e)
        }
    }
}

/// A missing state file is a first run and a corrupt one must not brick the
/// app, so both yield an empty queue rather than an error.
pub fn load_entries(path: &Path) -> Vec<QueueEntry> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::queue::QueueEntry;

    fn entry(hash: &str) -> QueueEntry {
        QueueEntry {
            infohash: hash.into(),
            magnet: format!("magnet:?xt=urn:btih:{hash}"),
            title: "Example".into(),
            added_unix: 42,
            paused: false,
        }
    }

    #[test]
    fn round_trips_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.json");
        let entries = vec![entry("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")];
        save_entries(&path, &entries).unwrap();
        assert_eq!(load_entries(&path), entries);
    }

    #[test]
    fn missing_file_loads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_entries(&dir.path().join("nope.json")).is_empty());
    }

    #[test]
    fn corrupt_file_loads_as_empty_rather_than_failing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.json");
        std::fs::write(&path, b"{ this is not json").unwrap();
        assert!(load_entries(&path).is_empty());
    }

    #[test]
    fn save_replaces_previous_state_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.json");
        save_entries(&path, &[entry("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")]).unwrap();
        save_entries(&path, &[entry("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")]).unwrap();

        let loaded = load_entries(&path);
        assert_eq!(loaded.len(), 1);
        assert_eq!(
            loaded[0].infohash,
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        );

        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "queue.json")
            .collect();
        assert!(
            leftovers.is_empty(),
            "atomic write left temp files behind: {leftovers:?}"
        );
    }

    #[test]
    fn save_creates_the_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("deeper").join("queue.json");
        save_entries(&path, &[entry("cccccccccccccccccccccccccccccccccccccccc")]).unwrap();
        assert_eq!(load_entries(&path).len(), 1);
    }
}
