use codex_utils_absolute_path::AbsolutePathBuf;
use dirs::home_dir;
use std::ffi::OsStr;
use std::path::PathBuf;

/// Returns AHEAD's credential/runtime home, using `AHEAD_HOME` when set and
/// `~/.ahead` otherwise. This never consults `CODEX_HOME`.
pub fn find_ahead_home() -> std::io::Result<AbsolutePathBuf> {
    let ahead_home_env = std::env::var_os("AHEAD_HOME");
    find_ahead_home_from_env(ahead_home_env.as_deref())
}

fn find_ahead_home_from_env(ahead_home_env: Option<&OsStr>) -> std::io::Result<AbsolutePathBuf> {
    match ahead_home_env {
        Some(val) => {
            let path = PathBuf::from(val);
            let metadata = std::fs::metadata(&path).map_err(|err| match err.kind() {
                std::io::ErrorKind::NotFound => std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("AHEAD_HOME points to {val:?}, but that path does not exist"),
                ),
                _ => std::io::Error::new(
                    err.kind(),
                    format!("failed to read AHEAD_HOME {val:?}: {err}"),
                ),
            })?;

            if !metadata.is_dir() {
                Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("AHEAD_HOME points to {val:?}, but that path is not a directory"),
                ))
            } else {
                let canonical = path.canonicalize().map_err(|err| {
                    std::io::Error::new(
                        err.kind(),
                        format!("failed to canonicalize AHEAD_HOME {val:?}: {err}"),
                    )
                })?;
                AbsolutePathBuf::from_absolute_path(canonical)
            }
        }
        None => {
            let mut p = home_dir().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "Could not find home directory",
                )
            })?;
            p.push(".ahead");
            AbsolutePathBuf::from_absolute_path(p)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::find_ahead_home_from_env;
    use codex_utils_absolute_path::AbsolutePathBuf;
    use dirs::home_dir;
    use pretty_assertions::assert_eq;
    use std::fs;
    use std::io::ErrorKind;
    use tempfile::TempDir;

    #[test]
    fn ahead_home_override_rejects_missing_path_and_file() {
        let temp_home = TempDir::new().expect("temp home");
        let missing = temp_home.path().join("missing-ahead-home");
        let err =
            find_ahead_home_from_env(Some(missing.as_os_str())).expect_err("missing AHEAD_HOME");
        assert_eq!(err.kind(), ErrorKind::NotFound);
        assert!(err.to_string().contains("AHEAD_HOME"));
        assert!(find_ahead_home_from_env(Some(std::ffi::OsStr::new(""))).is_err());

        let file_path = temp_home.path().join("ahead-home.txt");
        fs::write(&file_path, "not a directory").expect("write temp file");
        let err =
            find_ahead_home_from_env(Some(file_path.as_os_str())).expect_err("file AHEAD_HOME");
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        assert!(err.to_string().contains("not a directory"));
    }

    #[test]
    fn ahead_home_uses_its_own_default_and_override() {
        let temp_home = TempDir::new().expect("temp home");
        let resolved = find_ahead_home_from_env(Some(temp_home.path().as_os_str()))
            .expect("AHEAD_HOME override");
        let expected = AbsolutePathBuf::from_absolute_path(
            temp_home
                .path()
                .canonicalize()
                .expect("canonical temp path"),
        )
        .expect("absolute temp path");
        assert_eq!(resolved, expected);

        let default = find_ahead_home_from_env(None).expect("default AHEAD home");
        assert_eq!(
            default.as_path(),
            home_dir().expect("home dir").join(".ahead")
        );

        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let non_utf8 = temp_home
                .path()
                .join(std::ffi::OsString::from_vec(vec![0xff]));
            assert!(find_ahead_home_from_env(Some(non_utf8.as_os_str())).is_err());
        }
    }
}
