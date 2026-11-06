use std::path::PathBuf;

use directories::{BaseDirs, ProjectDirs};

use crate::meta::NAME;

pub struct Directory {}

impl Directory {
    pub fn home_dir() -> Option<PathBuf> {
        if let Some(path) = std::env::var_os("AHEAD_USER_HOME") {
            return Self::home_dir_from_override(PathBuf::from(path));
        }
        BaseDirs::new().map(|d| PathBuf::from(d.home_dir()))
    }

    fn home_dir_from_override(path: PathBuf) -> Option<PathBuf> {
        if !path.is_absolute() {
            tracing::error!(path = %path.display(), "AHEAD_USER_HOME must be absolute");
            return None;
        }
        Some(path)
    }

    fn project_dirs() -> Option<ProjectDirs> {
        if let Some(path) = std::env::var_os("AHEAD_DATA_HOME") {
            return Self::project_dirs_from_data_home(PathBuf::from(path));
        }
        ProjectDirs::from("io", "ahead", NAME)
    }

    fn project_dirs_from_data_home(path: PathBuf) -> Option<ProjectDirs> {
        if !path.is_absolute() {
            tracing::error!(path = %path.display(), "AHEAD_DATA_HOME must be absolute");
            return None;
        }
        ProjectDirs::from_path(path)
    }

    // Get path of local data directory
    // Local data directory differs from data directory
    // on some platforms and is not transferred across
    // machines
    pub fn data_local_directory() -> Option<PathBuf> {
        match Self::project_dirs() {
            Some(dir) => {
                let dir = dir.data_local_dir();
                if !dir.exists()
                    && let Err(err) = std::fs::create_dir_all(dir)
                {
                    tracing::error!("{:?}", err);
                }
                Some(dir.to_path_buf())
            }
            None => None,
        }
    }

    /// Get the path to logs directory
    /// Each log file is for individual application startup
    pub fn logs_directory() -> Option<PathBuf> {
        if let Some(dir) = Self::data_local_directory() {
            let dir = dir.join("logs");
            if !dir.exists()
                && let Err(err) = std::fs::create_dir(&dir)
            {
                tracing::error!("{:?}", err);
            }
            Some(dir)
        } else {
            None
        }
    }

    /// Get the path to cache directory
    pub fn cache_directory() -> Option<PathBuf> {
        if let Some(dir) = Self::data_local_directory() {
            let dir = dir.join("cache");
            if !dir.exists()
                && let Err(err) = std::fs::create_dir(&dir)
            {
                tracing::error!("{:?}", err);
            }
            Some(dir)
        } else {
            None
        }
    }

    /// Directory to store proxy executables used on local
    /// host as well, as ones uploaded to remote host when
    /// connecting
    pub fn proxy_directory() -> Option<PathBuf> {
        if let Some(dir) = Self::data_local_directory() {
            let dir = dir.join("proxy");
            if !dir.exists()
                && let Err(err) = std::fs::create_dir(&dir)
            {
                tracing::error!("{:?}", err);
            }
            Some(dir)
        } else {
            None
        }
    }
    /// Get the path to the themes folder
    /// Themes are stored within as individual toml files
    pub fn themes_directory() -> Option<PathBuf> {
        if let Some(dir) = Self::data_local_directory() {
            let dir = dir.join("themes");
            if !dir.exists()
                && let Err(err) = std::fs::create_dir(&dir)
            {
                tracing::error!("{:?}", err);
            }
            Some(dir)
        } else {
            None
        }
    }
    // Get the path to plugins directory
    // Each plugin has own directory that contains
    // metadata file and plugin wasm
    pub fn plugins_directory() -> Option<PathBuf> {
        if let Some(dir) = Self::data_local_directory() {
            let dir = dir.join("plugins");
            if !dir.exists()
                && let Err(err) = std::fs::create_dir(&dir)
            {
                tracing::error!("{:?}", err);
            }
            Some(dir)
        } else {
            None
        }
    }

    // Config directory contain only configuration files
    pub fn config_directory() -> Option<PathBuf> {
        match Self::project_dirs() {
            Some(dir) => {
                let dir = dir.config_dir();
                if !dir.exists()
                    && let Err(err) = std::fs::create_dir_all(dir)
                {
                    tracing::error!("{:?}", err);
                }
                Some(dir.to_path_buf())
            }
            None => None,
        }
    }

    pub fn local_socket() -> Option<PathBuf> {
        Self::data_local_directory().map(|dir| dir.join("local.sock"))
    }

    pub fn updates_directory() -> Option<PathBuf> {
        if let Some(dir) = Self::data_local_directory() {
            let dir = dir.join("updates");
            if !dir.exists()
                && let Err(err) = std::fs::create_dir(&dir)
            {
                tracing::error!("{:?}", err);
            }
            Some(dir)
        } else {
            None
        }
    }

    pub fn queries_directory() -> Option<PathBuf> {
        if let Some(dir) = Self::config_directory() {
            let dir = dir.join("queries");
            if !dir.exists()
                && let Err(err) = std::fs::create_dir(&dir)
            {
                tracing::error!("{:?}", err);
            }

            Some(dir)
        } else {
            None
        }
    }

    pub fn grammars_directory() -> Option<PathBuf> {
        if let Some(dir) = Self::data_local_directory() {
            let dir = dir.join("grammars");
            if !dir.exists()
                && let Err(err) = std::fs::create_dir(&dir)
            {
                tracing::error!("{:?}", err);
            }

            Some(dir)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Directory;
    use std::path::PathBuf;

    #[test]
    fn explicit_data_home_keeps_config_and_plugins_together() {
        let root = std::env::temp_dir().join("ahead-disposable-data-home");
        let dirs = Directory::project_dirs_from_data_home(root.clone())
            .expect("absolute path");
        assert_eq!(dirs.data_local_dir(), root);
        assert_eq!(dirs.config_dir(), root);
        assert!(
            Directory::project_dirs_from_data_home(PathBuf::from("relative"))
                .is_none()
        );
    }

    #[test]
    fn explicit_user_home_must_be_absolute() {
        let root = std::env::temp_dir().join("ahead-disposable-user-home");
        assert_eq!(Directory::home_dir_from_override(root.clone()), Some(root));
        assert!(
            Directory::home_dir_from_override(PathBuf::from("relative")).is_none()
        );
    }
}
