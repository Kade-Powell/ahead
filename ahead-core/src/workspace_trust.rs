use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{self, ErrorKind, Read, Write},
    path::{Path, PathBuf},
};

use fs4::fs_std::FileExt;

const MAX_TRUST_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrustDecision {
    Undecided,
    Allowed,
    Blocked,
}

#[derive(Default)]
struct TrustEntries {
    trusted: BTreeSet<String>,
    blocked: BTreeSet<String>,
}

#[cfg(unix)]
type TrustDirectory = File;
#[cfg(not(unix))]
type TrustDirectory = ();

fn trust_path() -> io::Result<PathBuf> {
    let directory =
        super::directory::Directory::config_directory().ok_or_else(|| {
            io::Error::other("AHEAD user config directory is unavailable")
        })?;
    canonical_trust_path(&directory)
}

fn canonical_trust_path(directory: &Path) -> io::Result<PathBuf> {
    if !fs::symlink_metadata(directory)?.is_dir() {
        return Err(io::Error::new(
            ErrorKind::PermissionDenied,
            "AHEAD trust directory must not be a symlink",
        ));
    }
    Ok(directory.canonicalize()?.join("trusted-workspaces.toml"))
}

pub fn is_trusted(workspace: &Path) -> io::Result<bool> {
    Ok(decision(workspace)? == TrustDecision::Allowed)
}

pub fn decision(workspace: &Path) -> io::Result<TrustDecision> {
    decision_at(&trust_path()?, workspace)
}

pub fn set_trusted(workspace: &Path, trusted: bool) -> io::Result<()> {
    set_trusted_at(&trust_path()?, workspace, trusted)
}

fn workspace_key(workspace: &Path) -> io::Result<String> {
    workspace
        .canonicalize()?
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| {
            io::Error::new(ErrorKind::InvalidInput, "workspace path is not UTF-8")
        })
}

fn validate_directory(path: &Path) -> io::Result<TrustDirectory> {
    let directory = path.parent().ok_or_else(|| {
        io::Error::new(ErrorKind::InvalidInput, "trust path has no parent")
    })?;
    if !fs::symlink_metadata(directory)?.is_dir() {
        return Err(io::Error::new(
            ErrorKind::PermissionDenied,
            "AHEAD trust directory must not be a symlink",
        ));
    }
    #[cfg(unix)]
    {
        let file = super::secure_fs::open_canonical_directory(directory)?;
        let metadata = rustix::fs::fstat(&file)?;
        if metadata.st_uid != rustix::process::geteuid().as_raw()
            || metadata.st_mode & 0o022 != 0
        {
            return Err(io::Error::new(
                ErrorKind::PermissionDenied,
                "AHEAD trust directory must be owned by you and not writable by others",
            ));
        }
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        Ok(())
    }
}

fn read_entries(path: &Path) -> io::Result<TrustEntries> {
    validate_directory(path)?;
    #[cfg(unix)]
    let mut file = match super::secure_fs::open_canonical_regular_file(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(TrustEntries::default());
        }
        Err(error) => return Err(error),
    };
    #[cfg(not(unix))]
    let mut file = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => File::open(path)?,
        Ok(_) => {
            return Err(io::Error::new(
                ErrorKind::PermissionDenied,
                "AHEAD trust file must not be a symlink",
            ));
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(TrustEntries::default());
        }
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_TRUST_FILE_BYTES {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "AHEAD trust file must be a regular file of at most 1 MiB",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o022 != 0
        {
            return Err(io::Error::new(
                ErrorKind::PermissionDenied,
                "AHEAD trust file must be owned by you and not writable by others",
            ));
        }
    }
    let mut content = String::new();
    (&mut file)
        .take(MAX_TRUST_FILE_BYTES + 1)
        .read_to_string(&mut content)?;
    if content.len() as u64 > MAX_TRUST_FILE_BYTES {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "AHEAD trust file exceeds 1 MiB",
        ));
    }
    let table = content.parse::<toml::Table>().map_err(|error| {
        io::Error::new(
            ErrorKind::InvalidData,
            format!("invalid AHEAD trust file: {error}"),
        )
    })?;
    let paths = |key: &str| -> io::Result<BTreeSet<String>> {
        table
            .get(key)
            .and_then(toml::Value::as_array)
            .ok_or_else(|| {
                io::Error::new(
                    ErrorKind::InvalidData,
                    format!("AHEAD trust file needs a {key} array"),
                )
            })?
            .iter()
            .map(|value| {
                let path = value.as_str().ok_or_else(|| {
                    io::Error::new(
                        ErrorKind::InvalidData,
                        "AHEAD trust entries must be paths",
                    )
                })?;
                if !Path::new(path).is_absolute() {
                    return Err(io::Error::new(
                        ErrorKind::InvalidData,
                        "AHEAD trust entries must be absolute paths",
                    ));
                }
                Ok(path.to_owned())
            })
            .collect()
    };
    let trusted = paths("trusted")?;
    let blocked = if table.contains_key("blocked") {
        paths("blocked")?
    } else {
        BTreeSet::new()
    };
    if !trusted.is_disjoint(&blocked) {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "AHEAD trust file has conflicting decisions",
        ));
    }
    Ok(TrustEntries { trusted, blocked })
}

fn lock_file(_path: &Path, _directory: &TrustDirectory) -> io::Result<File> {
    #[cfg(unix)]
    let lock: File = rustix::fs::openat(
        _directory,
        ".trusted-workspaces.lock",
        rustix::fs::OFlags::RDWR
            | rustix::fs::OFlags::CREATE
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )?
    .into();
    #[cfg(not(unix))]
    let lock = {
        let lock_path = _path.with_file_name(".trusted-workspaces.lock");
        match fs::symlink_metadata(&lock_path) {
            Ok(metadata) if !metadata.is_file() => {
                return Err(io::Error::new(
                    ErrorKind::PermissionDenied,
                    "invalid AHEAD trust lock",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?
    };
    if !lock.metadata()?.is_file() {
        return Err(io::Error::new(
            ErrorKind::PermissionDenied,
            "invalid AHEAD trust lock",
        ));
    }
    lock.lock_exclusive()?;
    Ok(lock)
}

fn decision_at(path: &Path, workspace: &Path) -> io::Result<TrustDecision> {
    let key = workspace_key(workspace)?;
    let entries = read_entries(path)?;
    Ok(if entries.trusted.contains(&key) {
        TrustDecision::Allowed
    } else if entries.blocked.contains(&key) {
        TrustDecision::Blocked
    } else {
        TrustDecision::Undecided
    })
}

fn set_trusted_at(path: &Path, workspace: &Path, trusted: bool) -> io::Result<()> {
    let key = workspace_key(workspace)?;
    let directory = validate_directory(path)?;
    let _lock = lock_file(path, &directory)?;
    let mut entries = read_entries(path)?;
    if trusted {
        entries.blocked.remove(&key);
        entries.trusted.insert(key);
    } else {
        entries.trusted.remove(&key);
        entries.blocked.insert(key);
    }
    let mut table = toml::Table::new();
    table.insert(
        "trusted".into(),
        toml::Value::Array(
            entries
                .trusted
                .into_iter()
                .map(toml::Value::String)
                .collect(),
        ),
    );
    table.insert(
        "blocked".into(),
        toml::Value::Array(
            entries
                .blocked
                .into_iter()
                .map(toml::Value::String)
                .collect(),
        ),
    );
    let content = toml::to_string(&table).map_err(io::Error::other)?;
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(ErrorKind::InvalidInput, "trust path has no parent")
    })?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(content.as_bytes())?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trust_is_user_scoped_and_revocable() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let root = root.path().canonicalize()?;
        let one = root.join("one");
        let two = root.join("two");
        fs::create_dir(&one)?;
        fs::create_dir(&two)?;
        let store = root.join("trusted-workspaces.toml");
        assert_eq!(decision_at(&store, &one)?, TrustDecision::Undecided);
        set_trusted_at(&store, &one, true)?;
        assert_eq!(decision_at(&store, &one)?, TrustDecision::Allowed);
        assert_eq!(decision_at(&store, &two)?, TrustDecision::Undecided);
        set_trusted_at(&store, &two, true)?;
        set_trusted_at(&store, &one, false)?;
        assert_eq!(decision_at(&store, &one)?, TrustDecision::Blocked);
        assert_eq!(decision_at(&store, &two)?, TrustDecision::Allowed);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn trust_file_symlink_is_rejected_without_touching_target() -> io::Result<()> {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir()?;
        let root = root.path().canonicalize()?;
        let workspace = root.join("workspace");
        fs::create_dir(&workspace)?;
        let target = root.join("target");
        fs::write(&target, "trusted = []\n")?;
        let store = root.join("trusted-workspaces.toml");
        symlink(&target, &store)?;
        assert!(decision_at(&store, &workspace).is_err());
        assert!(set_trusted_at(&store, &workspace, true).is_err());
        assert_eq!(fs::read_to_string(target)?, "trusted = []\n");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn trust_path_allows_parent_alias_but_rejects_symlinked_directory()
    -> io::Result<()> {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir()?;
        let root = root.path().canonicalize()?;
        let actual = root.join("actual");
        let config = actual.join("config");
        fs::create_dir_all(&config)?;
        let parent_alias = root.join("parent-alias");
        symlink(&actual, &parent_alias)?;
        assert_eq!(
            canonical_trust_path(&parent_alias.join("config"))?,
            config.join("trusted-workspaces.toml")
        );
        let leaf_alias = root.join("leaf-alias");
        symlink(&config, &leaf_alias)?;
        assert_eq!(
            canonical_trust_path(&leaf_alias).unwrap_err().kind(),
            ErrorKind::PermissionDenied
        );
        Ok(())
    }
}
