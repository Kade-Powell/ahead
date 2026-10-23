use std::{
    ffi::{OsStr, c_void},
    fs::File,
    io::{self, Write},
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle},
    },
    path::{Component, Path, PathBuf, Prefix},
    ptr,
};

use windows_sys::Win32::{
    Foundation::{
        ERROR_INSUFFICIENT_BUFFER, GetLastError, HANDLE, HLOCAL,
        INVALID_HANDLE_VALUE, LocalFree, NTSTATUS, PSID, RtlNtStatusToDosError,
        UNICODE_STRING,
    },
    Security::{
        Authorization::{
            EXPLICIT_ACCESS_W, GetSecurityInfo, SE_FILE_OBJECT, SET_ACCESS,
            SetEntriesInAclW, SetSecurityInfo, TRUSTEE_IS_SID, TRUSTEE_IS_USER,
            TRUSTEE_W,
        },
        CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, EqualSid,
        GetTokenInformation, OBJECT_INHERIT_ACE, OWNER_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, TOKEN_QUERY, TOKEN_USER, TokenUser,
    },
    Storage::FileSystem::{
        DELETE, FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY, FILE_ALL_ACCESS,
        FILE_APPEND_DATA, FILE_ATTRIBUTE_NORMAL, FILE_DELETE_CHILD,
        FILE_DISPOSITION_INFO, FILE_GENERIC_READ, FILE_INFO_BY_HANDLE_CLASS,
        FILE_LIST_DIRECTORY, FILE_NON_DIRECTORY_FILE, FILE_READ_ATTRIBUTES,
        FILE_READ_DATA, FILE_RENAME_INFO, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FILE_TRAVERSE, FILE_WRITE_DATA, FileDispositionInfo,
        FileRenameInfo, READ_CONTROL, SetFileInformationByHandle, WRITE_DAC,
    },
    System::{
        IO::IO_STATUS_BLOCK,
        IO::IO_STATUS_BLOCK_0,
        Kernel,
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
};

const FILE_DIRECTORY_FILE: u32 = 0x0000_0001;
const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x0000_0020;
const FILE_OPEN: u32 = 1;
const FILE_CREATE: u32 = 2;
const FILE_OPEN_IF: u32 = 3;
const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
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
        return Err(invalid_path("memory filesystem root must be absolute"));
    }
    let (prefix, source_offset) = match path.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(_) => ("\\??\\", 0),
            Prefix::VerbatimDisk(_) => ("\\??\\", 4),
            Prefix::UNC(_, _) => ("\\??\\UNC\\", 2),
            Prefix::VerbatimUNC(_, _) => ("\\??\\UNC\\", 8),
            _ => return Err(invalid_path("unsupported memory filesystem root")),
        },
        _ => {
            return Err(invalid_path(
                "memory filesystem root is not a Windows path",
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
        return Err(invalid_path(
            "memory filesystem name must be one path component",
        ));
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
        .map_err(|_| invalid_path("memory filesystem name is too long"))?;
    let maximum_length = u16::try_from(name.len() * size_of::<u16>())
        .map_err(|_| invalid_path("memory filesystem name is too long"))?;
    let object_name = UNICODE_STRING {
        Length: length,
        MaximumLength: maximum_length,
        Buffer: name.as_mut_ptr(),
    };
    // Unlike a final-component open flag, this rejects reparse points during
    // the entire path traversal.
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
            return Err(invalid_path("memory path contains a reparse point"));
        }
        let code = unsafe { RtlNtStatusToDosError(status) };
        return Err(io::Error::from_raw_os_error(code as i32));
    }
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::other("NtCreateFile returned an invalid handle"));
    }
    Ok(unsafe { File::from(OwnedHandle::from_raw_handle(handle as RawHandle)) })
}

struct CurrentUserSid(Vec<usize>);

impl CurrentUserSid {
    fn load() -> io::Result<Self> {
        let mut token_handle = 0;
        if unsafe {
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token_handle)
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let token =
            unsafe { OwnedHandle::from_raw_handle(token_handle as RawHandle) };

        let mut required = 0;
        let result = unsafe {
            GetTokenInformation(
                token.as_raw_handle() as HANDLE,
                TokenUser,
                ptr::null_mut(),
                0,
                &mut required,
            )
        };
        if result == 0 {
            let error = unsafe { GetLastError() };
            if error != ERROR_INSUFFICIENT_BUFFER {
                return Err(io::Error::from_raw_os_error(error as i32));
            }
        }
        if required < size_of::<TOKEN_USER>() as u32 {
            return Err(io::Error::other(
                "Windows returned an invalid current-user token size",
            ));
        }

        let word_count = (required as usize).div_ceil(size_of::<usize>());
        let mut storage = vec![0; word_count];
        let storage_size = u32::try_from(storage.len() * size_of::<usize>())
            .map_err(|_| io::Error::other("current-user SID is too large"))?;
        let result = unsafe {
            GetTokenInformation(
                token.as_raw_handle() as HANDLE,
                TokenUser,
                storage.as_mut_ptr().cast(),
                storage_size,
                &mut required,
            )
        };
        if result == 0 {
            return Err(io::Error::last_os_error());
        }
        let sid = unsafe { (*storage.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        if sid.is_null() {
            return Err(io::Error::other(
                "Windows returned an invalid current-user SID",
            ));
        }
        Ok(Self(storage))
    }

    fn as_ptr(&self) -> PSID {
        unsafe { (*self.0.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }
}

fn set_owner_only_acl(file: &File, directory: bool) -> io::Result<()> {
    let current_user = CurrentUserSid::load()?;
    let handle = file.as_raw_handle() as HANDLE;
    let mut owner: PSID = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        if !descriptor.is_null() {
            unsafe { LocalFree(descriptor as HLOCAL) };
        }
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let is_owned =
        !owner.is_null() && unsafe { EqualSid(owner, current_user.as_ptr()) != 0 };
    if !descriptor.is_null() {
        unsafe { LocalFree(descriptor as HLOCAL) };
    }
    if !is_owned {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "AHEAD memory must be owned by the current user",
        ));
    }

    let access = EXPLICIT_ACCESS_W {
        grfAccessPermissions: FILE_ALL_ACCESS,
        grfAccessMode: SET_ACCESS,
        grfInheritance: if directory {
            OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
        } else {
            0
        },
        Trustee: TRUSTEE_W {
            pMultipleTrustee: ptr::null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_USER,
            ptstrName: current_user.as_ptr().cast(),
        },
    };
    let mut dacl = ptr::null_mut();
    let status = unsafe { SetEntriesInAclW(1, &access, ptr::null(), &mut dacl) };
    if status != 0 {
        if !dacl.is_null() {
            unsafe { LocalFree(dacl.cast()) };
        }
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let status = unsafe {
        SetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            dacl,
            ptr::null_mut(),
        )
    };
    unsafe { LocalFree(dacl.cast()) };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    Ok(())
}

fn root_directory_access(create: bool) -> u32 {
    let read_access = FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY | FILE_TRAVERSE;
    if create {
        read_access | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY | FILE_DELETE_CHILD
    } else {
        read_access
    }
}

fn open_root(path: &Path, create: bool) -> io::Result<File> {
    open_handle(
        0,
        nt_path(path)?,
        root_directory_access(create),
        FILE_OPEN,
        FILE_DIRECTORY_FILE,
    )
}

fn open_absolute_directory(path: &Path, create: bool) -> io::Result<Option<File>> {
    let mut components = path.components();
    let prefix = match components.next() {
        Some(Component::Prefix(prefix)) => prefix.as_os_str(),
        _ => return Err(invalid_path("memory filesystem root is not absolute")),
    };
    let root = match components.next() {
        Some(Component::RootDir) => OsStr::new("\\"),
        _ => return Err(invalid_path("memory filesystem root is not absolute")),
    };
    let mut current = Path::new(prefix).to_path_buf();
    current.push(root);
    let mut directory = match open_root(&current, false) {
        Ok(directory) => directory,
        Err(error) if !create && error.kind() == io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };

    for component in components {
        let Component::Normal(name) = component else {
            return Err(invalid_path("memory filesystem root is not canonical"));
        };
        current.push(name);
        directory = match open_root(&current, false) {
            Ok(directory) => directory,
            Err(error) if !create && error.kind() == io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) if create && error.kind() == io::ErrorKind::NotFound => {
                match open_handle(
                    0,
                    nt_path(&current)?,
                    root_directory_access(false),
                    FILE_CREATE,
                    FILE_DIRECTORY_FILE,
                ) {
                    Ok(directory) => directory,
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                        open_root(&current, false)?
                    }
                    Err(error) => return Err(error),
                }
            }
            Err(error) => return Err(error),
        };
    }
    if create {
        open_root(&current, true).map(Some)
    } else {
        Ok(Some(directory))
    }
}

fn open_directory_at(
    parent: &File,
    name: &OsStr,
    create: bool,
    secure_acl: bool,
) -> io::Result<Option<File>> {
    let root = parent.as_raw_handle() as HANDLE;
    let name = component_name(name)?;
    let acl_access = if secure_acl {
        READ_CONTROL | WRITE_DAC
    } else {
        0
    };
    let open = || {
        open_handle(
            root,
            name.clone(),
            root_directory_access(create) | acl_access,
            FILE_OPEN,
            FILE_DIRECTORY_FILE,
        )
    };
    match open() {
        Ok(directory) => Ok(Some(directory)),
        Err(error) if !create && error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) if create && error.kind() == io::ErrorKind::NotFound => {
            match open_handle(
                root,
                name.clone(),
                root_directory_access(true) | acl_access,
                FILE_CREATE,
                FILE_DIRECTORY_FILE,
            ) {
                Ok(directory) => Ok(Some(directory)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    open().map(Some)
                }
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

pub(super) fn open_memory_directory(
    memory_path: &Path,
    create: bool,
) -> io::Result<Option<File>> {
    let root = memory_path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or_else(|| {
            invalid_path("AHEAD memory path has no workspace or user root")
        })?;
    let root = match std::fs::canonicalize(root) {
        Ok(root) => root,
        Err(error) if !create && error.kind() == io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) if create && error.kind() == io::ErrorKind::NotFound => {
            root.to_path_buf()
        }
        Err(error) => return Err(error),
    };
    let Some(mut directory) = (match open_absolute_directory(&root, create) {
        Ok(directory) => directory,
        Err(error) if !create && error.kind() == io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    }) else {
        return Ok(None);
    };
    for (name, secure_acl) in [(".ahead", false), ("memories", true)] {
        let Some(next) =
            open_directory_at(&directory, OsStr::new(name), create, secure_acl)?
        else {
            return Ok(None);
        };
        directory = next;
        if secure_acl {
            set_owner_only_acl(&directory, true)?;
        }
    }
    Ok(Some(directory))
}

pub(super) fn secure_session_database_files(path: &Path) -> io::Result<PathBuf> {
    use std::os::windows::fs::MetadataExt;

    let absolute = std::path::absolute(path)?;
    let parent = absolute
        .parent()
        .ok_or_else(|| invalid_path("session database needs a parent directory"))?;
    let metadata = std::fs::symlink_metadata(parent)?;
    if !metadata.is_dir()
        || metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
    {
        return Err(invalid_path(
            "session database directory must be a non-reparse directory",
        ));
    }
    let parent = parent.canonicalize()?;
    let directory = open_handle(
        0,
        nt_path(&parent)?,
        READ_CONTROL | WRITE_DAC,
        FILE_OPEN,
        FILE_DIRECTORY_FILE,
    )?;
    set_owner_only_acl(&directory, true)?;
    let directory = open_handle(
        0,
        nt_path(&parent)?,
        root_directory_access(true),
        FILE_OPEN,
        FILE_DIRECTORY_FILE,
    )?;

    let name = absolute
        .file_name()
        .ok_or_else(|| invalid_path("session database needs a filename"))?;
    let mut names = vec![name.to_os_string()];
    for suffix in ["-journal", "-wal", "-shm"] {
        let mut sidecar = name.to_os_string();
        sidecar.push(suffix);
        names.push(sidecar);
    }

    let mut main_exists = false;
    let mut sidecar_exists = false;
    for (index, name) in names.iter().enumerate() {
        match open_handle(
            directory.as_raw_handle() as HANDLE,
            component_name(name)?,
            FILE_READ_ATTRIBUTES | READ_CONTROL | WRITE_DAC,
            FILE_OPEN,
            FILE_NON_DIRECTORY_FILE,
        ) {
            Ok(file) => {
                validate_regular_file(&file, "session database and sidecar")?;
                set_owner_only_acl(&file, false)?;
                if index == 0 {
                    main_exists = true;
                } else {
                    sidecar_exists = true;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }

    if !main_exists && sidecar_exists {
        return Err(io::Error::other(
            "session database is missing but recovery sidecars remain; keep these files for recovery",
        ));
    }
    if !main_exists {
        let name = component_name(name)?;
        let file = match open_handle(
            directory.as_raw_handle() as HANDLE,
            name.clone(),
            FILE_READ_ATTRIBUTES | READ_CONTROL | WRITE_DAC,
            FILE_CREATE,
            FILE_NON_DIRECTORY_FILE,
        ) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                open_handle(
                    directory.as_raw_handle() as HANDLE,
                    name,
                    FILE_READ_ATTRIBUTES | READ_CONTROL | WRITE_DAC,
                    FILE_OPEN,
                    FILE_NON_DIRECTORY_FILE,
                )?
            }
            Err(error) => return Err(error),
        };
        validate_regular_file(&file, "session database")?;
        set_owner_only_acl(&file, false)?;
    }

    Ok(parent.join(name))
}

fn open_memory_file_at(
    directory: &File,
    name: &OsStr,
    access: u32,
    disposition: u32,
    secure_acl: bool,
) -> io::Result<File> {
    open_handle(
        directory.as_raw_handle() as HANDLE,
        component_name(name)?,
        access
            | if secure_acl {
                READ_CONTROL | WRITE_DAC
            } else {
                0
            },
        disposition,
        FILE_NON_DIRECTORY_FILE,
    )
}

pub(super) fn open_existing_memory_file(
    directory: &File,
) -> io::Result<Option<File>> {
    match open_memory_file_at(
        directory,
        OsStr::new("MEMORY.md"),
        FILE_GENERIC_READ,
        FILE_OPEN,
        true,
    ) {
        Ok(file) => {
            validate_regular_file(&file, "AHEAD memory source")?;
            set_owner_only_acl(&file, false)?;
            Ok(Some(file))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn open_memory_append(directory: &File) -> io::Result<File> {
    let file = open_memory_file_at(
        directory,
        OsStr::new("MEMORY.md"),
        FILE_READ_DATA | FILE_APPEND_DATA | FILE_READ_ATTRIBUTES,
        FILE_OPEN_IF,
        true,
    )?;
    validate_regular_file(&file, "AHEAD memory source")?;
    set_owner_only_acl(&file, false)?;
    Ok(file)
}

fn validate_regular_file(file: &File, description: &str) -> io::Result<()> {
    use std::os::windows::fs::MetadataExt;

    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
        || metadata.number_of_links() != Some(1)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{description} must be a singly-linked regular file"),
        ));
    }
    Ok(())
}

pub(super) fn replace_memory_file(
    directory: &File,
    content: &[u8],
) -> io::Result<()> {
    let temporary_name = format!(".MEMORY.md.{}.tmp", uuid::Uuid::new_v4());
    let mut temporary = open_memory_file_at(
        directory,
        OsStr::new(&temporary_name),
        FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | DELETE,
        FILE_CREATE,
        true,
    )?;
    set_owner_only_acl(&temporary, false)?;
    let result = (|| {
        temporary.write_all(content)?;
        temporary.sync_all()?;
        rename_file_at(&temporary, directory, OsStr::new("MEMORY.md"))
    })();
    if let Err(error) = result {
        return match delete_file_handle(&temporary) {
            Ok(()) => Err(error),
            Err(cleanup_error) => Err(io::Error::other(format!(
                "memory replacement failed: {error}; temporary-file cleanup failed: {cleanup_error}"
            ))),
        };
    }
    Ok(())
}

fn rename_file_at(file: &File, directory: &File, name: &OsStr) -> io::Result<()> {
    let name: Vec<u16> = name.encode_wide().collect();
    let name_length = u32::try_from(name.len() * size_of::<u16>())
        .map_err(|_| invalid_path("memory destination name is too long"))?;
    let info_size = size_of::<FILE_RENAME_INFO>()
        .checked_add(name.len().saturating_sub(1) * size_of::<u16>())
        .ok_or_else(|| invalid_path("memory rename buffer is too large"))?;
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
        .map_err(|_| invalid_path("memory rename buffer is too large"))?;
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
    let disposition = FILE_DISPOSITION_INFO {
        DeleteFile: BOOLEAN_TRUE,
    };
    let success = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle() as HANDLE,
            FileDispositionInfo,
            (&raw const disposition).cast(),
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
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, GetAce, GetSecurityDescriptorControl, IsValidAcl,
        SE_DACL_PROTECTED,
    };

    #[test]
    fn memory_fs_converts_supported_absolute_paths() -> io::Result<()> {
        for (path, expected) in [
            (r"C:\work\repo", r"\??\C:\work\repo"),
            (r"\\?\C:\work\repo", r"\??\C:\work\repo"),
            (r"\\server\share\repo", r"\??\UNC\server\share\repo"),
            (r"\\?\UNC\server\share\repo", r"\??\UNC\server\share\repo"),
        ] {
            let actual = nt_path(Path::new(path))?;
            let expected: Vec<u16> =
                expected.encode_utf16().chain(std::iter::once(0)).collect();
            assert_eq!(actual, expected, "unexpected NT path for {path}");
        }
        Ok(())
    }

    fn assert_owner_only_acl(file: &File, directory: bool) -> io::Result<()> {
        const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;

        let current_user = CurrentUserSid::load()?;
        let mut owner: PSID = ptr::null_mut();
        let mut dacl = ptr::null_mut();
        let mut descriptor = ptr::null_mut();
        let status = unsafe {
            GetSecurityInfo(
                file.as_raw_handle() as HANDLE,
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                ptr::null_mut(),
                &mut dacl,
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        if status != 0 {
            if !descriptor.is_null() {
                unsafe { LocalFree(descriptor as HLOCAL) };
            }
            return Err(io::Error::from_raw_os_error(status as i32));
        }

        let is_private = unsafe {
            let owner_matches =
                !owner.is_null() && EqualSid(owner, current_user.as_ptr()) != 0;
            let mut control = 0;
            let mut revision = 0;
            let protected = GetSecurityDescriptorControl(
                descriptor,
                &mut control,
                &mut revision,
            ) != 0
                && control & SE_DACL_PROTECTED != 0;
            let one_valid_ace =
                !dacl.is_null() && IsValidAcl(dacl) != 0 && (*dacl).AceCount == 1;
            let mut ace = ptr::null_mut();
            let valid_ace = one_valid_ace
                && GetAce(dacl, 0, &mut ace) != 0
                && !ace.is_null()
                && (*ace.cast::<ACCESS_ALLOWED_ACE>()).Header.AceType
                    == ACCESS_ALLOWED_ACE_TYPE
                && (*ace.cast::<ACCESS_ALLOWED_ACE>()).Header.AceFlags
                    == if directory {
                        (OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE) as u8
                    } else {
                        0
                    }
                && (*ace.cast::<ACCESS_ALLOWED_ACE>()).Mask == FILE_ALL_ACCESS
                && EqualSid(
                    current_user.as_ptr(),
                    ptr::addr_of!((*ace.cast::<ACCESS_ALLOWED_ACE>()).SidStart)
                        .cast_mut()
                        .cast(),
                ) != 0;
            owner_matches && protected && valid_ace
        };
        if !descriptor.is_null() {
            unsafe { LocalFree(descriptor as HLOCAL) };
        }
        if !is_private {
            return Err(io::Error::other(
                "AHEAD memory ACL is not protected to the current user",
            ));
        }
        Ok(())
    }

    fn assert_inherited_owner_only_acl(file: &File) -> io::Result<()> {
        const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;

        let current_user = CurrentUserSid::load()?;
        let mut owner: PSID = ptr::null_mut();
        let mut dacl = ptr::null_mut();
        let mut descriptor = ptr::null_mut();
        let status = unsafe {
            GetSecurityInfo(
                file.as_raw_handle() as HANDLE,
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                ptr::null_mut(),
                &mut dacl,
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        if status != 0 {
            if !descriptor.is_null() {
                unsafe { LocalFree(descriptor as HLOCAL) };
            }
            return Err(io::Error::from_raw_os_error(status as i32));
        }

        let is_private = unsafe {
            let owner_matches =
                !owner.is_null() && EqualSid(owner, current_user.as_ptr()) != 0;
            let one_valid_ace =
                !dacl.is_null() && IsValidAcl(dacl) != 0 && (*dacl).AceCount == 1;
            let mut ace = ptr::null_mut();
            let valid_ace = one_valid_ace
                && GetAce(dacl, 0, &mut ace) != 0
                && !ace.is_null()
                && (*ace.cast::<ACCESS_ALLOWED_ACE>()).Header.AceType
                    == ACCESS_ALLOWED_ACE_TYPE
                && (*ace.cast::<ACCESS_ALLOWED_ACE>()).Header.AceFlags
                    & windows_sys::Win32::Security::INHERITED_ACE as u8
                    != 0
                && (*ace.cast::<ACCESS_ALLOWED_ACE>()).Mask == FILE_ALL_ACCESS
                && EqualSid(
                    current_user.as_ptr(),
                    ptr::addr_of!((*ace.cast::<ACCESS_ALLOWED_ACE>()).SidStart)
                        .cast_mut()
                        .cast(),
                ) != 0;
            owner_matches && valid_ace
        };
        if !descriptor.is_null() {
            unsafe { LocalFree(descriptor as HLOCAL) };
        }
        if !is_private {
            return Err(io::Error::other(
                "AHEAD sidecar did not inherit an owner-only ACL",
            ));
        }
        Ok(())
    }

    #[test]
    fn memory_directory_and_file_acl_are_current_user_only() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let memory_path = root.path().join(".ahead/memories/MEMORY.md");
        let directory = open_memory_directory(&memory_path, true)?
            .ok_or_else(|| io::Error::other("memory directory was not created"))?;
        replace_memory_file(&directory, b"private memory")?;
        let file = open_existing_memory_file(&directory)?
            .ok_or_else(|| io::Error::other("memory file was not created"))?;

        assert_owner_only_acl(&directory, true)?;
        assert_owner_only_acl(&file, false)?;
        Ok(())
    }

    #[test]
    fn memory_session_database_and_sidecar_acl_are_current_user_only()
    -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let directory_path = root.path().join(".ahead");
        std::fs::create_dir(&directory_path)?;
        let database_path = directory_path.join("session.db");
        let mut wal_path = database_path.as_os_str().to_owned();
        wal_path.push("-wal");
        let wal_path = PathBuf::from(wal_path);
        std::fs::write(&database_path, b"database bytes")?;
        std::fs::write(&wal_path, b"wal bytes")?;

        let database_path = secure_session_database_files(&database_path)?;
        let directory_path = database_path.parent().ok_or_else(|| {
            invalid_path("session database needs a parent directory")
        })?;
        let directory = open_handle(
            0,
            nt_path(directory_path)?,
            READ_CONTROL,
            FILE_OPEN,
            FILE_DIRECTORY_FILE,
        )?;
        let database = open_handle(
            directory.as_raw_handle() as HANDLE,
            component_name(OsStr::new("session.db"))?,
            FILE_READ_ATTRIBUTES | READ_CONTROL,
            FILE_OPEN,
            FILE_NON_DIRECTORY_FILE,
        )?;
        let wal = open_handle(
            directory.as_raw_handle() as HANDLE,
            component_name(OsStr::new("session.db-wal"))?,
            FILE_READ_ATTRIBUTES | READ_CONTROL,
            FILE_OPEN,
            FILE_NON_DIRECTORY_FILE,
        )?;

        assert_owner_only_acl(&directory, true)?;
        assert_owner_only_acl(&database, false)?;
        assert_owner_only_acl(&wal, false)?;
        assert_eq!(std::fs::read(&database_path)?.as_slice(), b"database bytes");
        assert_eq!(std::fs::read(wal_path)?.as_slice(), b"wal bytes");

        let mut shm_path = database_path.as_os_str().to_owned();
        shm_path.push("-shm");
        let shm_path = PathBuf::from(shm_path);
        std::fs::write(&shm_path, b"new sidecar")?;
        let shm = open_handle(
            directory.as_raw_handle() as HANDLE,
            component_name(OsStr::new("session.db-shm"))?,
            FILE_READ_ATTRIBUTES | READ_CONTROL,
            FILE_OPEN,
            FILE_NON_DIRECTORY_FILE,
        )?;
        assert_inherited_owner_only_acl(&shm)?;

        let fresh_directory = root.path().join("fresh");
        std::fs::create_dir(&fresh_directory)?;
        let fresh_database =
            secure_session_database_files(&fresh_directory.join("session.db"))?;
        let fresh_parent = open_handle(
            0,
            nt_path(fresh_database.parent().ok_or_else(|| {
                invalid_path("session database needs a parent directory")
            })?)?,
            READ_CONTROL,
            FILE_OPEN,
            FILE_DIRECTORY_FILE,
        )?;
        let fresh_file = open_handle(
            fresh_parent.as_raw_handle() as HANDLE,
            component_name(OsStr::new("session.db"))?,
            FILE_READ_ATTRIBUTES | READ_CONTROL,
            FILE_OPEN,
            FILE_NON_DIRECTORY_FILE,
        )?;
        assert_owner_only_acl(&fresh_parent, true)?;
        assert_owner_only_acl(&fresh_file, false)?;
        Ok(())
    }

    #[test]
    fn memory_session_database_keeps_orphan_sidecars_without_creating_a_database()
    -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let directory = root.path().join(".ahead");
        std::fs::create_dir(&directory)?;
        let database = directory.join("session.db");
        let mut wal = database.as_os_str().to_owned();
        wal.push("-wal");
        let wal = PathBuf::from(wal);
        std::fs::write(&wal, b"recovery data")?;

        assert!(secure_session_database_files(&database).is_err());
        assert!(!database.exists());
        assert_eq!(std::fs::read(wal)?.as_slice(), b"recovery data");
        Ok(())
    }

    #[test]
    fn memory_session_database_rejects_hardlinks_without_rewriting() -> io::Result<()>
    {
        let root = tempfile::tempdir()?;
        let directory = root.path().join(".ahead");
        std::fs::create_dir(&directory)?;
        let database = directory.join("session.db");
        let alias = directory.join("session-copy.db");
        std::fs::write(&database, b"preserve me")?;
        std::fs::hard_link(&database, &alias)?;

        assert!(secure_session_database_files(&database).is_err());
        assert_eq!(std::fs::read(&database)?.as_slice(), b"preserve me");
        assert_eq!(std::fs::read(alias)?.as_slice(), b"preserve me");
        Ok(())
    }
}
