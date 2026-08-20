use std::path::PathBuf;

use directories::{BaseDirs, ProjectDirs};

/// Where swarmling keeps its own state. Falls back to a hidden directory in
/// the home dir, and finally to the working directory, so a machine with no
/// resolvable home never crashes the app.
pub fn data_dir() -> PathBuf {
    // An explicit override keeps tests (and anyone running several profiles)
    // off the real state directory. Empty means "not set".
    if let Some(dir) = std::env::var_os("SWARMLING_DATA_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Some(dirs) = ProjectDirs::from("", "", "swarmling") {
        return dirs.data_dir().to_path_buf();
    }
    if let Some(base) = BaseDirs::new() {
        return base.home_dir().join(".swarmling");
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".swarmling")
}

/// The user's Downloads folder when there is one, else the home directory.
pub fn default_download_dir() -> PathBuf {
    if let Some(dirs) = directories::UserDirs::new() {
        if let Some(downloads) = dirs.download_dir() {
            return downloads.to_path_buf();
        }
        return dirs.home_dir().to_path_buf();
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

pub fn queue_state_path() -> PathBuf {
    data_dir().join("queue.json")
}

pub fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_absolute_and_nested_under_the_data_dir() {
        assert!(data_dir().is_absolute());
        assert!(default_download_dir().is_absolute());
        let state = queue_state_path();
        assert!(state.starts_with(data_dir()));
        assert_eq!(state.file_name().unwrap(), "queue.json");

        // The override is exercised here rather than in its own test: it
        // mutates the process environment, which would race a sibling test
        // running in parallel.
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("SWARMLING_DATA_DIR", dir.path());
        assert_eq!(data_dir(), dir.path());
        assert_eq!(settings_path(), dir.path().join("settings.json"));
        std::env::remove_var("SWARMLING_DATA_DIR");
    }
}
