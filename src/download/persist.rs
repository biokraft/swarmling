use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::queue::QueueEntry;

const CURRENT_VERSION: u32 = 1;

/// The on-disk shape: a versioned envelope so a future field addition can be
/// distinguished from today's format instead of silently emptying every
/// existing user's queue.
#[derive(Serialize, Deserialize)]
struct Envelope {
    version: u32,
    entries: Vec<Value>,
}

/// Write via a temp file and rename, so an interrupted write can never
/// truncate the previous good state.
pub fn save_entries(path: &Path, entries: &[QueueEntry]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let envelope = Envelope {
        version: CURRENT_VERSION,
        entries: entries
            .iter()
            .map(|e| serde_json::to_value(e).expect("QueueEntry always serializes"))
            .collect(),
    };
    let json = serde_json::to_vec_pretty(&envelope)
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
/// app, so both yield an empty queue rather than an error. Accepts both the
/// current versioned envelope and a bare legacy `Vec<QueueEntry>`. Entries are
/// deserialized individually so one malformed entry is skipped rather than
/// discarding the whole file.
pub fn load_entries(path: &Path) -> Vec<QueueEntry> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };

    let raw_entries: Vec<Value> = if let Ok(envelope) = serde_json::from_slice::<Envelope>(&bytes) {
        envelope.entries
    } else if let Ok(legacy) = serde_json::from_slice::<Vec<Value>>(&bytes) {
        legacy
    } else {
        return Vec::new();
    };

    raw_entries
        .into_iter()
        .filter_map(|v| serde_json::from_value::<QueueEntry>(v).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::queue::QueueEntry;

    #[test]
    fn a_queue_file_written_before_source_id_existed_still_loads() {
        // The project rule: an older state file must keep working. There are
        // queue files in the wild with no `source_id` key at all.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("queue.json");
        std::fs::write(
            &path,
            br#"{"version":1,"entries":[{"infohash":"aa","magnet":"magnet:?xt=urn:btih:aa","title":"Old","added_unix":7,"paused":false}]}"#,
        )
        .expect("write");
        let entries = load_entries(&path);
        assert_eq!(entries.len(), 1, "the old file was dropped entirely");
        assert_eq!(entries[0].title, "Old");
        assert_eq!(entries[0].source_id, None);

        // Proof this is not vacuous: the loader really does drop an entry
        // that is missing a field it requires, so the assertion above is
        // load-bearing. (`Option` fields are optional to serde whether or
        // not `serde(default)` is present; the attribute states the intent
        // and survives the field's type changing.)
        let required = dir.path().join("required.json");
        std::fs::write(
            &required,
            br#"{"version":1,"entries":[{"infohash":"aa","magnet":"magnet:?xt=urn:btih:aa","added_unix":7,"paused":false}]}"#,
        )
        .expect("write");
        assert!(
            load_entries(&required).is_empty(),
            "an entry missing a required field should be skipped"
        );
    }

    fn entry(hash: &str) -> QueueEntry {
        QueueEntry {
            infohash: hash.into(),
            magnet: format!("magnet:?xt=urn:btih:{hash}"),
            title: "Example".into(),
            added_unix: 42,
            paused: false,
            source_id: None,
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
    fn envelope_round_trips_with_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.json");
        let entries = vec![entry("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")];
        save_entries(&path, &entries).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["version"], 1);
        assert_eq!(value["entries"].as_array().unwrap().len(), 1);

        assert_eq!(load_entries(&path), entries);
    }

    #[test]
    fn loads_a_legacy_bare_array() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.json");
        let entries = vec![entry("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")];
        std::fs::write(&path, serde_json::to_vec(&entries).unwrap()).unwrap();
        assert_eq!(load_entries(&path), entries);
    }

    #[test]
    fn one_bad_entry_among_good_ones_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.json");
        let good = entry("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let envelope = serde_json::json!({
            "version": 1,
            "entries": [
                serde_json::to_value(&good).unwrap(),
                { "not": "a valid entry" },
            ],
        });
        std::fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();
        assert_eq!(load_entries(&path), vec![good]);
    }

    #[test]
    fn save_creates_the_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("deeper").join("queue.json");
        save_entries(&path, &[entry("cccccccccccccccccccccccccccccccccccccccc")]).unwrap();
        assert_eq!(load_entries(&path).len(), 1);
    }
}
