use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{self, ErrorKind, Read, Write},
    path::{Path, PathBuf},
};

use fs4::fs_std::FileExt;

const MAX_TRUST_FILE_BYTES: u64 = 1024 * 1024;

#[cfg(unix)]
type TrustDirectory = File;
#[cfg(not(unix))]
type TrustDirectory = ();

fn trust_path() -> io::Result<PathBuf> {
    super::directory::Directory::config_directory()
        .map(|directory| directory.join("trusted-workspaces.toml"))
        .ok_or_else(|| {
            io::Error::other("AHEAD user config directory is unavailable")
        })
}

pub fn is_trusted(workspace: &Path) -> io::Result<bool> {
    is_trusted_at(&trust_path()?, workspace)
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

fn read_entries(path: &Path) -> io::Result<BTreeSet<String>> {
    validate_directory(path)?;
    #[cfg(unix)]
    let mut file = match super::secure_fs::open_canonical_regular_file(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(BTreeSet::new());
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
            return Ok(BTreeSet::new());
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
    let paths = table
        .get("trusted")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| {
            io::Error::new(
                ErrorKind::InvalidData,
                "AHEAD trust file needs a trusted array",
            )
        })?;
    paths
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

fn is_trusted_at(path: &Path, workspace: &Path) -> io::Result<bool> {
    let key = workspace_key(workspace)?;
    Ok(read_entries(path)?.contains(&key))
}

fn set_trusted_at(path: &Path, workspace: &Path, trusted: bool) -> io::Result<()> {
    let key = workspace_key(workspace)?;
    let directory = validate_directory(path)?;
    let _lock = lock_file(path, &directory)?;
    let mut entries = read_entries(path)?;
    if trusted {
        entries.insert(key);
    } else {
        entries.remove(&key);
    }
    let mut table = toml::Table::new();
    table.insert(
        "trusted".into(),
        toml::Value::Array(entries.into_iter().map(toml::Value::String).collect()),
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
        assert!(!is_trusted_at(&store, &one)?);
        set_trusted_at(&store, &one, true)?;
        assert!(is_trusted_at(&store, &one)?);
        assert!(!is_trusted_at(&store, &two)?);
        set_trusted_at(&store, &two, true)?;
        set_trusted_at(&store, &one, false)?;
        assert!(!is_trusted_at(&store, &one)?);
        assert!(is_trusted_at(&store, &two)?);
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
        assert!(is_trusted_at(&store, &workspace).is_err());
        assert!(set_trusted_at(&store, &workspace, true).is_err());
        assert_eq!(fs::read_to_string(target)?, "trusted = []\n");
        Ok(())
    }
}
