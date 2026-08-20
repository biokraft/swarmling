use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub vpn_required: bool,
    pub vpn_adapter: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            vpn_required: false,
            vpn_adapter: "generic".into(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    #[serde(default = "one")]
    version: u32,
    #[serde(default)]
    settings: Settings,
}

fn one() -> u32 {
    1
}

pub fn save(path: &Path, settings: &Settings) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let env = Envelope {
        version: 1,
        settings: settings.clone(),
    };
    let json = serde_json::to_vec_pretty(&env)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, json)?;
    match std::fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            Err(e)
        }
    }
}

/// Settings never fail to load: an absent file is a first run and a corrupt
/// one must not stop the program, so both yield defaults.
pub fn load(path: &Path) -> Settings {
    let Ok(bytes) = std::fs::read(path) else {
        return Settings::default();
    };
    serde_json::from_slice::<Envelope>(&bytes)
        .map(|e| e.settings)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_off_and_generic() {
        let s = Settings::default();
        assert!(!s.vpn_required);
        assert_eq!(s.vpn_adapter, "generic");
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let s = Settings {
            vpn_required: true,
            vpn_adapter: "nordvpn".into(),
        };
        save(&path, &s).unwrap();
        assert_eq!(load(&path), s);
    }

    #[test]
    fn a_missing_or_corrupt_file_yields_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(&dir.path().join("nope.json")), Settings::default());
        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, b"{ not json").unwrap();
        assert_eq!(load(&bad), Settings::default());
        let wrong_shape = dir.path().join("wrong-shape.json");
        std::fs::write(&wrong_shape, br#"{"settings":[]}"#).unwrap();
        assert_eq!(load(&wrong_shape), Settings::default());
    }

    #[test]
    fn an_older_file_missing_new_fields_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.json");
        std::fs::write(&path, br#"{"version":1}"#).unwrap();
        assert_eq!(load(&path), Settings::default());
    }
}
