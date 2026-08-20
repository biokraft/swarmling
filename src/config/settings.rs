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

/// Bump when the on-disk shape changes incompatibly.
pub const CURRENT_VERSION: u32 = 1;

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
        version: CURRENT_VERSION,
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

/// Settings never fail to load — a corrupt file must not stop the program —
/// but "never fail" is not the same as "silently assume the defaults".
///
/// An absent file is a genuine first run and correctly yields defaults. A
/// file that exists but cannot be parsed, or that was written by a newer
/// version, means the user's real preferences are unknown; for a safety flag
/// the unknown value has to be the SAFE one, not the convenient one.
/// `vpn_required` therefore comes back `true` in that case, with a warning,
/// rather than quietly flipping off the protection the user asked for.
pub fn load(path: &Path) -> Settings {
    let Ok(bytes) = std::fs::read(path) else {
        return Settings::default();
    };
    match serde_json::from_slice::<Envelope>(&bytes) {
        Ok(env) if env.version > CURRENT_VERSION => {
            eprintln!(
                "warn: {} was written by a newer swarmling (version {}, this build understands {});                  keeping safe settings instead of half-reading it",
                path.display(),
                env.version,
                CURRENT_VERSION
            );
            safe_fallback()
        }
        Ok(env) => env.settings,
        Err(e) => {
            eprintln!(
                "warn: {} could not be read ({e}); assuming a VPN is required",
                path.display()
            );
            safe_fallback()
        }
    }
}

/// What to use when the file cannot be trusted: defaults everywhere except
/// the safety flag, which points at protection.
fn safe_fallback() -> Settings {
    Settings {
        vpn_required: true,
        ..Settings::default()
    }
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
    fn a_missing_file_is_a_first_run_and_yields_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(&dir.path().join("nope.json")), Settings::default());
    }

    #[test]
    fn a_corrupt_file_fails_toward_requiring_a_vpn() {
        let dir = tempfile::tempdir().unwrap();
        for (name, body) in [
            ("bad.json", &b"{ not json"[..]),
            (
                "wrong-type.json",
                &br#"{"settings":{"vpn_required":"yes"}}"#[..],
            ),
            (
                "truncated.json",
                &br#"{"version":1,"settings":{"vpn_re"#[..],
            ),
        ] {
            let path = dir.path().join(name);
            std::fs::write(&path, body).unwrap();
            assert!(
                load(&path).vpn_required,
                "{name}: an unreadable settings file must not silently disable the VPN requirement"
            );
        }
    }

    #[test]
    fn a_file_from_a_newer_version_is_not_half_parsed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("future.json");
        std::fs::write(
            &path,
            br#"{"version":99,"settings":{"vpn_required":false,"vpn_adapter":"generic"}}"#,
        )
        .unwrap();
        assert!(
            load(&path).vpn_required,
            "a newer file's unknown fields could carry the real policy; keep the safe value"
        );
    }

    #[test]
    fn an_older_file_missing_new_fields_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.json");
        std::fs::write(&path, br#"{"version":1}"#).unwrap();
        assert_eq!(load(&path), Settings::default());
    }
}
