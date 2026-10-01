use std::{
    io::{self, ErrorKind, Read},
    path::Path,
};

#[cfg(not(unix))]
use std::fs;

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

/// Reads one AHEAD provider-settings layer without following a project-owned
/// `.ahead` directory or config-file symlink.
pub fn read_ahead_config(root: &Path, filename: &str) -> io::Result<Option<String>> {
    if !matches!(
        filename,
        "settings.toml" | "config.toml" | "config.local.toml"
    ) {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "unsupported AHEAD config filename",
        ));
    }
    let root = root.canonicalize()?;
    let path = root.join(".ahead").join(filename);
    #[cfg(unix)]
    let mut file = match crate::secure_fs::open_canonical_regular_file(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    #[cfg(not(unix))]
    let mut file = {
        let ahead = root.join(".ahead");
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if !metadata.file_type().is_file()
            || !fs::symlink_metadata(&ahead)?.file_type().is_dir()
            || !ahead.canonicalize()?.starts_with(&root)
        {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "AHEAD config directory and file must not be symlinks",
            ));
        }
        fs::File::open(&path)?
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "AHEAD config must be a regular file of at most 1 MiB",
        ));
    }
    let mut content = String::new();
    (&mut file)
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_string(&mut content)?;
    if content.len() as u64 > MAX_CONFIG_BYTES {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "AHEAD config must be a regular file of at most 1 MiB",
        ));
    }
    Ok(Some(content))
}

#[cfg(test)]
mod tests {
    use super::read_ahead_config;

    #[test]
    fn reads_regular_config_and_rejects_links_and_large_files() {
        let root = tempfile::tempdir().expect("workspace");
        let ahead = root.path().join(".ahead");
        std::fs::create_dir(&ahead).expect("settings directory");
        std::fs::write(ahead.join("settings.toml"), "[ai]\nmodel = 'safe'\n")
            .expect("write settings");
        assert_eq!(
            read_ahead_config(root.path(), "settings.toml")
                .expect("read settings")
                .as_deref(),
            Some("[ai]\nmodel = 'safe'\n")
        );
        assert!(read_ahead_config(root.path(), "../outside.toml").is_err());
        std::fs::write(ahead.join("config.toml"), "x".repeat(1024 * 1024 + 1))
            .expect("write oversized config");
        assert!(read_ahead_config(root.path(), "config.toml").is_err());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let outside = root.path().join("outside.toml");
            std::fs::write(&outside, "[ai]\nmodel = 'outside'\n")
                .expect("write outside config");
            std::fs::remove_file(ahead.join("settings.toml"))
                .expect("remove settings");
            symlink(&outside, ahead.join("settings.toml")).expect("link settings");
            assert!(read_ahead_config(root.path(), "settings.toml").is_err());
            std::fs::remove_file(ahead.join("settings.toml"))
                .expect("remove settings link");
            std::fs::remove_file(ahead.join("config.toml"))
                .expect("remove oversized config");
            std::fs::remove_dir(&ahead).expect("remove settings directory");
            std::fs::write(root.path().join("config.toml"), "[ai]\n")
                .expect("write outside config");
            symlink(root.path(), &ahead).expect("link settings directory");
            assert!(read_ahead_config(root.path(), "config.toml").is_err());
        }
    }
}
