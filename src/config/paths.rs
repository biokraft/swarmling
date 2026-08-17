use std::path::PathBuf;

use directories::{BaseDirs, ProjectDirs};

/// Where swarmling keeps its own state. Falls back to a hidden directory in
/// the home dir, and finally to the working directory, so a machine with no
/// resolvable home never crashes the app.
pub fn data_dir() -> PathBuf {
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
    }
}
