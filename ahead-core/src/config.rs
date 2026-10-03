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
        "settings.toml" | "config.toml" | "config.local.toml" | "team.toml"
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
    if filename == "config.toml" {
        let table = content.parse::<toml::Table>().map_err(|_| {
            io::Error::new(
                ErrorKind::InvalidData,
                "invalid TOML in .ahead/config.toml",
            )
        })?;
        validate_tracked_config(&toml::Value::Table(table))?;
    } else if filename == "team.toml" {
        let table = content.parse::<toml::Table>().map_err(|_| {
            io::Error::new(
                ErrorKind::InvalidData,
                "invalid TOML in .ahead/team.toml",
            )
        })?;
        if has_secret_field(&toml::Value::Table(table)) {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "tracked .ahead/team.toml contains a credential field",
            ));
        }
    }
    Ok(Some(content))
}

pub fn validate_tracked_config(value: &toml::Value) -> io::Result<()> {
    if has_secret_field(value) {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "tracked .ahead/config.toml contains a credential field; use ignored settings.toml or config.local.toml",
        ));
    }
    Ok(())
}

fn has_secret_field(value: &toml::Value) -> bool {
    match value {
        toml::Value::Table(table) => table.iter().any(|(key, value)| {
            credential_key(key, value.is_str()) || has_secret_field(value)
        }),
        toml::Value::Array(values) => values.iter().any(has_secret_field),
        toml::Value::String(text) => url::Url::parse(text).ok().is_some_and(|url| {
            !url.username().is_empty()
                || url.password().is_some()
                || url.query_pairs().any(|(key, _)| credential_key(&key, true))
        }),
        _ => false,
    }
}

fn credential_key(key: &str, string_value: bool) -> bool {
    let key = key.replace('-', "_").to_ascii_lowercase();
    matches!(
        key.as_str(),
        "api_key"
            | "apikey"
            | "token"
            | "password"
            | "private_key"
            | "secret_key"
            | "client_secret"
            | "authorization"
            | "bearer_token"
            | "auth_token"
            | "access_token"
            | "refresh_token"
            | "authtoken"
            | "accesstoken"
            | "bearertoken"
            | "clientsecret"
    ) || (string_value
        && (key.ends_with("_token")
            || key.ends_with("_secret")
            || key.ends_with("_password")))
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

    #[test]
    fn tracked_config_rejects_nested_credentials_but_private_settings_allow_them() {
        let root = tempfile::tempdir().expect("workspace");
        let ahead = root.path().join(".ahead");
        std::fs::create_dir(&ahead).expect("settings directory");
        std::fs::write(
            ahead.join("config.toml"),
            "[ai]\nmax_tokens = 2048\n[[ai.connections]]\nname = 'Shared'\nauth_token = 'private'\n",
        )
        .expect("write tracked config");
        assert!(read_ahead_config(root.path(), "config.toml").is_err());

        for address in [
            "https://name:password@example.invalid/v1",
            "https://example.invalid/v1?api_key=private",
        ] {
            std::fs::write(
                ahead.join("config.toml"),
                format!("[ai]\nbase_url = '{address}'\n"),
            )
            .expect("write credential URL");
            assert!(read_ahead_config(root.path(), "config.toml").is_err());
        }

        std::fs::write(
            ahead.join("config.toml"),
            "[ai]\nmax_tokens = 2048\nsemantic_token = true\nbase_url = 'https://example.invalid/v1?max_tokens=2048'\n[[ai.connections]]\nname = 'Shared'\n",
        )
        .expect("write shareable config");
        assert!(
            read_ahead_config(root.path(), "config.toml")
                .expect("read shareable config")
                .is_some()
        );
        std::fs::write(ahead.join("settings.toml"), "[ai]\napi_key = 'private'\n")
            .expect("write private settings");
        assert!(
            read_ahead_config(root.path(), "settings.toml")
                .expect("read private settings")
                .is_some()
        );
    }

    #[test]
    fn team_manifest_is_readable_without_exposing_credentials() {
        let root = tempfile::tempdir().expect("workspace");
        let ahead = root.path().join(".ahead");
        std::fs::create_dir(&ahead).expect("team directory");
        std::fs::write(ahead.join("team.toml"), "members = []\n")
            .expect("write team manifest");
        assert_eq!(
            read_ahead_config(root.path(), "team.toml")
                .expect("read team manifest")
                .as_deref(),
            Some("members = []\n")
        );
        std::fs::write(ahead.join("team.toml"), "token = 'private'\n")
            .expect("write credential");
        assert!(read_ahead_config(root.path(), "team.toml").is_err());
    }
}
