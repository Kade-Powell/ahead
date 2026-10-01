use std::{
    fs::File,
    io,
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
