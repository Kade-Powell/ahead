use std::{
    ffi::{OsStr, c_void},
    fs::File,
    io::{self, Write},
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle},
    },
    path::{Component, Path, Prefix},
    ptr,
};

use windows_sys::Win32::{
    Foundation::{
        HANDLE, INVALID_HANDLE_VALUE, NTSTATUS, RtlNtStatusToDosError,
        UNICODE_STRING,
    },
    Storage::FileSystem::{
        DELETE, FILE_ADD_FILE, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_DELETE_CHILD, FILE_DISPOSITION_INFO, FILE_GENERIC_READ,
        FILE_INFO_BY_HANDLE_CLASS, FILE_LIST_DIRECTORY, FILE_NON_DIRECTORY_FILE,
        FILE_READ_ATTRIBUTES, FILE_RENAME_INFO, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FILE_TRAVERSE, FILE_WRITE_DATA, FileDispositionInfo,
        FileRenameInfo, SetFileInformationByHandle,
    },
    System::{IO::IO_STATUS_BLOCK, IO::IO_STATUS_BLOCK_0, Kernel},
};

const FILE_OPEN: u32 = 1;
const FILE_CREATE: u32 = 2;
const FILE_OPEN_IF: u32 = 3;
const FILE_DIRECTORY_FILE: u32 = 0x0000_0001;
const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x0000_0020;
const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
const FILE_GENERIC_WRITE_ACCESS: u32 = 0x0012_0116;
const STATUS_REPARSE_POINT_ENCOUNTERED: NTSTATUS = 0xC000_050B_u32 as i32;
const BOOLEAN_TRUE: u8 = 1;

#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root_directory: HANDLE,
    object_name: *const UNICODE_STRING,
    attributes: u32,
    security_descriptor: *const c_void,
    security_quality_of_service: *const c_void,
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtCreateFile(
        file_handle: *mut HANDLE,
        desired_access: u32,
        object_attributes: *const ObjectAttributes,
        io_status_block: *mut IO_STATUS_BLOCK,
        allocation_size: *const i64,
        file_attributes: u32,
        share_access: u32,
        create_disposition: u32,
        create_options: u32,
        ea_buffer: *const c_void,
        ea_length: u32,
    ) -> NTSTATUS;
}

fn invalid_path(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn nt_path(path: &Path) -> io::Result<Vec<u16>> {
    if !path.is_absolute() {
        return Err(invalid_path("secure filesystem root must be absolute"));
    }
    let (prefix, source_offset) = match path.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(_) => ("\\??\\", 0),
            Prefix::VerbatimDisk(_) => ("\\??\\", 4),
            Prefix::UNC(_, _) => ("\\??\\UNC\\", 2),
            Prefix::VerbatimUNC(_, _) => ("\\??\\UNC\\", 8),
            _ => return Err(invalid_path("unsupported secure filesystem root")),
        },
        _ => {
            return Err(invalid_path(
                "secure filesystem root is not a Windows path",
            ));
        }
    };
    let mut result: Vec<u16> = OsStr::new(prefix).encode_wide().collect();
    result.extend(
        path.as_os_str()
            .encode_wide()
            .skip(source_offset)
            .map(|unit| {
                if unit == b'/' as u16 {
                    b'\\' as u16
                } else {
                    unit
                }
            }),
    );
    result.push(0);
    Ok(result)
}

fn component_name(name: &OsStr) -> io::Result<Vec<u16>> {
    let mut units: Vec<u16> = name.encode_wide().collect();
    if units.is_empty()
        || units
            .iter()
            .any(|unit| *unit == 0 || *unit == b'\\' as u16 || *unit == b'/' as u16)
    {
        return Err(invalid_path("secure filesystem name must be one component"));
    }
    units.push(0);
    Ok(units)
}

fn open_handle(
    root_directory: HANDLE,
    mut name: Vec<u16>,
    desired_access: u32,
    create_disposition: u32,
    create_options: u32,
) -> io::Result<File> {
    let length = u16::try_from((name.len() - 1) * size_of::<u16>())
        .map_err(|_| invalid_path("secure filesystem name is too long"))?;
    let maximum_length = u16::try_from(name.len() * size_of::<u16>())
        .map_err(|_| invalid_path("secure filesystem name is too long"))?;
    let object_name = UNICODE_STRING {
        Length: length,
        MaximumLength: maximum_length,
        Buffer: name.as_mut_ptr(),
    };
    let attributes = ObjectAttributes {
        length: size_of::<ObjectAttributes>() as u32,
        root_directory,
        object_name: &object_name,
        attributes: Kernel::OBJ_CASE_INSENSITIVE as u32
            | Kernel::OBJ_DONT_REPARSE as u32,
        security_descriptor: ptr::null(),
        security_quality_of_service: ptr::null(),
    };
    let mut status_block = IO_STATUS_BLOCK {
        Anonymous: IO_STATUS_BLOCK_0 { Status: 0 },
        Information: 0,
    };
    let mut handle = ptr::null_mut();
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            desired_access | SYNCHRONIZE_ACCESS,
            &attributes,
            &mut status_block,
            ptr::null(),
            FILE_ATTRIBUTE_NORMAL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            create_disposition,
            create_options | FILE_SYNCHRONOUS_IO_NONALERT,
            ptr::null(),
            0,
        )
    };
    if status < 0 {
        if status == STATUS_REPARSE_POINT_ENCOUNTERED {
            return Err(invalid_path(
                "secure filesystem path contains a reparse point",
            ));
        }
        let code = unsafe { RtlNtStatusToDosError(status) };
        return Err(io::Error::from_raw_os_error(code as i32));
    }
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::other("NtCreateFile returned an invalid handle"));
    }
    Ok(unsafe { File::from(OwnedHandle::from_raw_handle(handle as RawHandle)) })
}

fn validate_regular_file(file: &File) -> io::Result<()> {
    use std::os::windows::fs::MetadataExt;

    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || metadata.number_of_links() != Some(1)
    {
        return Err(invalid_path(
            "secure filesystem target must be a singly-linked regular file",
        ));
    }
    Ok(())
}

fn one_component_name(name: &OsStr) -> io::Result<&OsStr> {
    let mut components = Path::new(name).components();
    let Some(Component::Normal(component)) = components.next() else {
        return Err(invalid_path("expected a single relative name"));
    };
    if components.next().is_some() {
        return Err(invalid_path("expected a single relative name"));
    }
    Ok(component)
}

pub fn open_canonical_directory(path: &Path) -> io::Result<File> {
    let mut components = path.components();
    if !matches!(components.next(), Some(Component::Prefix(_)))
        || !matches!(components.next(), Some(Component::RootDir))
        || components.any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(invalid_path("expected a canonical absolute directory path"));
    }
    let directory = open_handle(
        0,
        nt_path(path)?,
        FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY | FILE_TRAVERSE,
        FILE_OPEN,
        FILE_DIRECTORY_FILE,
    )?;
    if !directory.metadata()?.is_dir() {
        return Err(invalid_path("secure filesystem root is not a directory"));
    }
    Ok(directory)
}

pub fn open_relative_directory(root: &File, name: &OsStr) -> io::Result<File> {
    let name = one_component_name(name)?;
    let directory = open_handle(
        root.as_raw_handle() as HANDLE,
        component_name(name)?,
        FILE_READ_ATTRIBUTES
            | FILE_LIST_DIRECTORY
            | FILE_TRAVERSE
            | FILE_ADD_FILE
            | FILE_DELETE_CHILD,
        FILE_OPEN,
        FILE_DIRECTORY_FILE,
    )?;
    if !directory.metadata()?.is_dir() {
        return Err(invalid_path("secure filesystem target is not a directory"));
    }
    Ok(directory)
}

pub fn open_relative_regular_file(root: &File, relative: &Path) -> io::Result<File> {
    let mut components = relative.components().peekable();
    if components.peek().is_none() {
        return Err(invalid_path("expected a relative file path"));
    }
    let mut directory: Option<File> = None;
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            return Err(invalid_path("expected a workspace-relative file path"));
        };
        let parent = directory.as_ref().unwrap_or(root);
        let access = if components.peek().is_some() {
            FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY | FILE_TRAVERSE
        } else {
            FILE_GENERIC_READ
        };
        let options = if components.peek().is_some() {
            FILE_DIRECTORY_FILE
        } else {
            FILE_NON_DIRECTORY_FILE
        };
        let file = open_handle(
            parent.as_raw_handle() as HANDLE,
            component_name(name)?,
            access,
            FILE_OPEN,
            options,
        )?;
        if components.peek().is_some() {
            directory = Some(file);
        } else {
            validate_regular_file(&file)?;
            return Ok(file);
        }
    }
    Err(invalid_path("expected a relative file path"))
}

pub fn open_or_create_relative_lock_file(
    root: &File,
    name: &OsStr,
) -> io::Result<File> {
    let name = one_component_name(name)?;
    let file = open_handle(
        root.as_raw_handle() as HANDLE,
        component_name(name)?,
        FILE_GENERIC_READ | FILE_GENERIC_WRITE_ACCESS | FILE_READ_ATTRIBUTES,
        FILE_OPEN_IF,
        FILE_NON_DIRECTORY_FILE,
    )?;
    validate_regular_file(&file)?;
    Ok(file)
}

pub fn atomic_replace_relative_file(
    root: &File,
    name: &OsStr,
    contents: &[u8],
) -> io::Result<()> {
    let name = one_component_name(name)?;
    match open_relative_regular_file(root, Path::new(name)) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let temporary_name = format!(".ahead-tmp-{}", uuid::Uuid::new_v4());
    let mut temporary = open_handle(
        root.as_raw_handle() as HANDLE,
        component_name(OsStr::new(&temporary_name))?,
        FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | DELETE,
        FILE_CREATE,
        FILE_NON_DIRECTORY_FILE,
    )?;
    let result = (|| {
        temporary.write_all(contents)?;
        temporary.sync_all()?;
        rename_file_at(&temporary, root, name)
    })();
    match result {
        Ok(()) => Ok(()),
        Err(replacement_error) => match delete_file_handle(&temporary) {
            Ok(()) => Err(replacement_error),
            Err(cleanup_error) => Err(io::Error::other(format!(
                "atomic file replacement failed: {replacement_error}; temporary cleanup failed: {cleanup_error}"
            ))),
        },
    }
}

fn rename_file_at(file: &File, directory: &File, name: &OsStr) -> io::Result<()> {
    let name: Vec<u16> = name.encode_wide().collect();
    let name_length =
        u32::try_from(name.len() * size_of::<u16>()).map_err(|_| {
            invalid_path("secure filesystem destination name is too long")
        })?;
    let info_size = size_of::<FILE_RENAME_INFO>()
        .checked_add(name.len().saturating_sub(1) * size_of::<u16>())
        .ok_or_else(|| {
            invalid_path("secure filesystem rename buffer is too large")
        })?;
    let mut storage = vec![0_usize; info_size.div_ceil(size_of::<usize>())];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        (*info).Anonymous.ReplaceIfExists = BOOLEAN_TRUE;
        (*info).RootDirectory = directory.as_raw_handle() as HANDLE;
        (*info).FileNameLength = name_length;
        ptr::copy_nonoverlapping(
            name.as_ptr(),
            ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
            name.len(),
        );
    }
    let info_size = u32::try_from(info_size)
        .map_err(|_| invalid_path("secure filesystem rename buffer is too large"))?;
    let success = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle() as HANDLE,
            FileRenameInfo as FILE_INFO_BY_HANDLE_CLASS,
            info.cast(),
            info_size,
        )
    };
    if success == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn delete_file_handle(file: &File) -> io::Result<()> {
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: 1 };
    let success = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle() as HANDLE,
            FileDispositionInfo as FILE_INFO_BY_HANDLE_CLASS,
            (&disposition as *const FILE_DISPOSITION_INFO)
                .cast_mut()
                .cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    };
    if success == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn atomic_replace_stays_anchored_after_directory_path_changes() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let root_path = temporary
            .path()
            .canonicalize()
            .expect("canonical temporary root");
        let ahead_path = root_path.join(".ahead");
        let moved_path = root_path.join(".ahead-held");
        fs::create_dir(&ahead_path).expect("create AHEAD directory");
        fs::write(ahead_path.join("settings.toml"), "old")
            .expect("write initial settings");

        let root = open_canonical_directory(&root_path).expect("open root handle");
        let directory = open_relative_directory(&root, OsStr::new(".ahead"))
            .expect("open AHEAD directory handle");
        fs::rename(&ahead_path, &moved_path).expect("move opened directory");
        fs::create_dir(&ahead_path).expect("replace AHEAD directory path");
        fs::write(ahead_path.join("settings.toml"), "outside")
            .expect("write replacement settings");

        atomic_replace_relative_file(
            &directory,
            OsStr::new("settings.toml"),
            b"updated",
        )
        .expect("replace through anchored handle");

        assert_eq!(
            fs::read_to_string(ahead_path.join("settings.toml"))
                .expect("read replacement path"),
            "outside"
        );
        assert_eq!(
            fs::read_to_string(moved_path.join("settings.toml"))
                .expect("read held directory"),
            "updated"
        );
    }
}
