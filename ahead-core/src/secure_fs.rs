use std::{
    ffi::OsStr,
    fs::File,
    io::{self, Write},
    path::{Component, Path},
};

use rustix::fs::{Mode, OFlags, openat};

pub fn open_canonical_directory(path: &Path) -> io::Result<File> {
    let mut components = path.components();
    if !matches!(components.next(), Some(Component::RootDir)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected an absolute path",
        ));
    }
    let mut directory = File::open("/")?;
    for component in components {
        let Component::Normal(name) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected a canonical path",
            ));
        };
        directory = openat(
            &directory,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?
        .into();
    }
    Ok(directory)
}

pub fn open_canonical_regular_file(path: &Path) -> io::Result<File> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "expected a file path")
    })?;
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "expected a file path")
    })?;
    let directory = open_canonical_directory(parent)?;
    open_relative_regular_file(&directory, Path::new(name))
}

pub fn open_relative_regular_file(root: &File, relative: &Path) -> io::Result<File> {
    let mut components = relative.components().peekable();
    if components.peek().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a relative file path",
        ));
    }
    let mut directory: Option<File> = None;
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected a workspace-relative file path",
            ));
        };
        let parent = directory.as_ref().unwrap_or(root);
        if components.peek().is_some() {
            directory = Some(
                openat(
                    parent,
                    name,
                    OFlags::RDONLY
                        | OFlags::DIRECTORY
                        | OFlags::NOFOLLOW
                        | OFlags::CLOEXEC,
                    Mode::empty(),
                )?
                .into(),
            );
        } else {
            let file: File = openat(
                parent,
                name,
                OFlags::RDONLY
                    | OFlags::NOFOLLOW
                    | OFlags::NONBLOCK
                    | OFlags::CLOEXEC,
                Mode::empty(),
            )?
            .into();
            if !file.metadata()?.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "path is not a regular file",
                ));
            }
            return Ok(file);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "expected a relative file path",
    ))
}

pub fn open_relative_directory_path(
    root: &File,
    relative: &Path,
) -> io::Result<File> {
    let mut components = relative.components().peekable();
    if components.peek().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a relative directory path",
        ));
    }
    let mut directory = root.try_clone()?;
    for component in components {
        let Component::Normal(name) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected a workspace-relative directory path",
            ));
        };
        directory = openat(
            &directory,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?
        .into();
    }
    Ok(directory)
}

pub fn open_regular_file_in_directory(
    directory: &File,
    name: &OsStr,
) -> io::Result<File> {
    let mut components = Path::new(name).components();
    let Some(Component::Normal(name)) = components.next() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a single relative file name",
        ));
    };
    if components.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a single relative file name",
        ));
    }
    let file: File = openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?
    .into();
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "path is not a regular file",
        ));
    }
    Ok(file)
}

pub fn open_relative_directory(root: &File, name: &OsStr) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags, openat};

    let relative = Path::new(name);
    let mut components = relative.components();
    let Some(Component::Normal(component)) = components.next() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a single directory name",
        ));
    };
    if components.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a single directory name",
        ));
    }
    openat(
        root,
        component,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(Into::into)
    .map_err(io::Error::from)
}

pub fn open_or_create_relative_lock_file(
    root: &File,
    name: &OsStr,
) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags, openat};

    let relative = Path::new(name);
    let mut components = relative.components();
    let Some(Component::Normal(component)) = components.next() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a single lock-file name",
        ));
    };
    if components.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a single lock-file name",
        ));
    }
    let file: File = openat(
        root,
        component,
        OFlags::RDWR
            | OFlags::CREATE
            | OFlags::NOFOLLOW
            | OFlags::NONBLOCK
            | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )?
    .into();
    validate_regular_file(&file)?;
    Ok(file)
}

pub fn atomic_replace_relative_file(
    root: &File,
    name: &OsStr,
    contents: &[u8],
) -> io::Result<()> {
    use rustix::fs::{AtFlags, Mode, OFlags, openat, renameat, unlinkat};

    let relative = Path::new(name);
    let mut components = relative.components();
    let Some(Component::Normal(component)) = components.next() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a single destination file name",
        ));
    };
    if components.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a single destination file name",
        ));
    }
    match open_relative_regular_file(root, relative) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let temporary_name = format!(".ahead-tmp-{}", uuid::Uuid::new_v4());
    let result = (|| {
        let mut temporary: File = openat(
            root,
            temporary_name.as_str(),
            OFlags::WRONLY
                | OFlags::CREATE
                | OFlags::EXCL
                | OFlags::NOFOLLOW
                | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )?
        .into();
        temporary.write_all(contents)?;
        temporary.sync_all()?;
        renameat(root, temporary_name.as_str(), root, component)
            .map_err(io::Error::from)
    })();
    match result {
        Ok(()) => Ok(()),
        Err(replacement_error) => {
            match unlinkat(root, temporary_name.as_str(), AtFlags::empty()) {
                Ok(()) | Err(rustix::io::Errno::NOENT) => Err(replacement_error),
                Err(cleanup_error) => Err(io::Error::other(format!(
                    "atomic file replacement failed: {replacement_error}; temporary cleanup failed: {cleanup_error}"
                ))),
            }
        }
    }
}

fn validate_regular_file(file: &File) -> io::Result<()> {
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path is not a regular file",
        ));
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink};

    #[test]
    fn canonical_file_open_rejects_replaced_parent() {
        let temporary = tempfile::tempdir().expect("disposable project");
        let root = temporary
            .path()
            .canonicalize()
            .expect("canonical temp root");
        let workspace = root.join("workspace");
        let outside = root.join("outside");
        fs::create_dir_all(workspace.join("src")).expect("create project path");
        fs::create_dir_all(&outside).expect("create outside path");
        fs::write(outside.join("AGENTS.md"), "outside instructions")
            .expect("write outside file");
        fs::remove_dir(workspace.join("src")).expect("replace project path");
        symlink(&outside, workspace.join("src")).expect("link outside path");

        assert!(
            open_canonical_regular_file(&workspace.join("src/AGENTS.md")).is_err()
        );
    }
}
