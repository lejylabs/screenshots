//! Held-parent operations for the capture-save journal; no directory traversal.
use anyhow::{Context, Result, ensure};
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
};

pub(super) fn identity(file: &File) -> Result<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = file.metadata()?;
        return Ok(format!("u:{:016x}:{:016x}", m.dev(), m.ino()));
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx,
        };
        let mut value = FILE_ID_INFO::default();
        ensure!(
            unsafe {
                GetFileInformationByHandleEx(
                    file.as_raw_handle(),
                    FileIdInfo,
                    (&mut value as *mut FILE_ID_INFO).cast(),
                    std::mem::size_of_val(&value) as u32,
                )
            } != 0,
            "Save object identity is unavailable"
        );
        let id: String = value
            .FileId
            .Identifier
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        return Ok(format!("w:{:016x}:{id}", value.VolumeSerialNumber));
    }
    #[cfg(not(any(unix, target_os = "windows")))]
    anyhow::bail!("Native save identities are unavailable")
}

fn options(directory: bool) -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(!directory);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .custom_flags(
                libc::O_CLOEXEC
                    | libc::O_NOFOLLOW
                    | libc::O_NONBLOCK
                    | if directory { libc::O_DIRECTORY } else { 0 },
            )
            .mode(0o600);
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
            FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options
            .custom_flags(
                FILE_FLAG_OPEN_REPARSE_POINT
                    | if directory {
                        FILE_FLAG_BACKUP_SEMANTICS
                    } else {
                        0
                    },
            )
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    }
    options
}

fn regular(file: File) -> Result<File> {
    ensure!(
        file.metadata()?.is_file(),
        "Save path must be a regular file"
    );
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            file.metadata()?.file_attributes()
                & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
                == 0,
            "Save refuses reparse points"
        );
    }
    Ok(file)
}

pub(in super::super) struct Directory {
    pub path: PathBuf,
    file: File,
    id: String,
}
impl Directory {
    pub fn open(path: &Path, expected: Option<&str>) -> Result<Self> {
        ensure!(
            std::fs::symlink_metadata(path)?.is_dir(),
            "Save parent must be a real directory"
        );
        let file = options(true).open(path)?;
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::fs::MetadataExt;
            ensure!(
                file.metadata()?.file_attributes()
                    & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
                    == 0,
                "Save parent refuses reparse points"
            );
        }
        let id = identity(&file)?;
        ensure!(
            expected.is_none_or(|expected| expected == id),
            "Save parent changed; all files preserved"
        );
        Ok(Self {
            path: path.into(),
            file,
            id,
        })
    }
    pub fn identity(&self) -> &str {
        &self.id
    }
    pub fn verify(&self) -> Result<()> {
        let named = Self::open(&self.path, Some(&self.id))?;
        ensure!(
            identity(&self.file)? == named.id,
            "Save parent changed; all files preserved"
        );
        Ok(())
    }
    fn name(name: &str) -> Result<()> {
        ensure!(
            !name.is_empty()
                && Path::new(name).file_name().and_then(|n| n.to_str()) == Some(name)
                && name != "."
                && name != ".."
                && !name.contains(['/', '\\', '\0']),
            "Invalid save filename"
        );
        #[cfg(target_os = "windows")]
        {
            let reserved = name.split('.').next().unwrap_or("").to_ascii_uppercase();
            ensure!(
                !name.contains(':')
                    && name == name.trim_end_matches([' ', '.'])
                    && !matches!(reserved.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                    && !(reserved.len() == 4
                        && (reserved.starts_with("COM") || reserved.starts_with("LPT"))
                        && matches!(reserved.as_bytes()[3], b'1'..=b'9')),
                "Save refuses device names and alternate streams"
            );
        }
        Ok(())
    }
    pub fn open_file(&self, name: &str, create: bool) -> Result<Option<File>> {
        self.open_access(name, create, create)
    }
    pub fn open_writable(&self, name: &str) -> Result<Option<File>> {
        self.open_access(name, false, true)
    }
    fn open_access(&self, name: &str, create: bool, writable: bool) -> Result<Option<File>> {
        Self::name(name)?;
        self.verify()?;
        #[cfg(unix)]
        {
            use std::os::fd::{AsRawFd, FromRawFd};
            let name = std::ffi::CString::new(name)?;
            let fd = unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_CLOEXEC
                        | libc::O_NOFOLLOW
                        | libc::O_NONBLOCK
                        | if writable {
                            libc::O_RDWR
                        } else {
                            libc::O_RDONLY
                        }
                        | if create {
                            libc::O_CREAT | libc::O_EXCL
                        } else {
                            0
                        },
                    0o600,
                )
            };
            if fd < 0 {
                let error = std::io::Error::last_os_error();
                if !create && error.kind() == std::io::ErrorKind::NotFound {
                    return Ok(None);
                }
                return Err(error).context("Could not open the journaled save file");
            }
            return Ok(Some(regular(unsafe { File::from_raw_fd(fd) })?));
        }
        #[cfg(not(unix))]
        {
            let mut options = options(false);
            options.write(writable).create_new(create);
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::fs::OpenOptionsExt;
                use windows_sys::Win32::{
                    Foundation::{GENERIC_READ, GENERIC_WRITE},
                    Storage::FileSystem::DELETE,
                };
                options
                    .access_mode(GENERIC_READ | DELETE | if writable { GENERIC_WRITE } else { 0 });
            }
            match options.open(self.path.join(name)) {
                Ok(file) => Ok(Some(regular(file)?)),
                Err(error) if !create && error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error).context("Could not open the journaled save file"),
            }
        }
    }
    pub fn publish(&self, stage: &str, target: &str, held: &File) -> Result<()> {
        Self::name(stage)?;
        Self::name(target)?;
        self.verify()?;
        let named = self
            .open_file(stage, false)?
            .context("Save staging file disappeared before publication")?;
        ensure!(
            identity(&named)? == identity(held)?,
            "Save staging file was replaced before publication; all files preserved"
        );
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            use std::os::fd::AsRawFd;
            let stage = std::ffi::CString::new(stage)?;
            let target = std::ffi::CString::new(target)?;
            #[cfg(target_os = "linux")]
            let result = unsafe {
                libc::renameat2(
                    self.file.as_raw_fd(),
                    stage.as_ptr(),
                    self.file.as_raw_fd(),
                    target.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            };
            #[cfg(target_os = "macos")]
            let result = unsafe {
                libc::renameatx_np(
                    self.file.as_raw_fd(),
                    stage.as_ptr(),
                    self.file.as_raw_fd(),
                    target.as_ptr(),
                    libc::RENAME_EXCL,
                )
            };
            if result != 0 {
                return Err(std::io::Error::last_os_error())
                    .context("Could not publish capture without replacing a file");
            }
            Ok(())
        }
        #[cfg(target_os = "windows")]
        {
            crate::capture::publish_output(&self.path.join(stage), &self.path.join(target))
                .map_err(Into::into)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        anyhow::bail!("Durable native save publication is unavailable")
    }
    pub fn remove_owned(&self, name: &str, held: File) -> Result<()> {
        let current = self
            .open_file(name, false)?
            .context("Save staging file disappeared")?;
        ensure!(
            identity(&current)? == identity(&held)?,
            "Save staging file was replaced; preserved"
        );
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let name = std::ffi::CString::new(name)?;
            ensure!(
                unsafe { libc::unlinkat(self.file.as_raw_fd(), name.as_ptr(), 0) } == 0,
                "Could not remove owned save staging file: {}",
                std::io::Error::last_os_error()
            );
        }
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
            };
            ensure!(
                held.metadata()?.len() == 0,
                "Windows removal requires a flushed empty staging object"
            );
            held.sync_all()?;
            let info = FILE_DISPOSITION_INFO { DeleteFile: true };
            ensure!(
                unsafe {
                    SetFileInformationByHandle(
                        held.as_raw_handle(),
                        FileDispositionInfo,
                        (&info as *const FILE_DISPOSITION_INFO).cast(),
                        std::mem::size_of_val(&info) as u32,
                    )
                } != 0,
                "Could not remove owned save staging file: {}",
                std::io::Error::last_os_error()
            );
        }
        // FileDispositionInfo is delete-on-close, not completed removal. Close
        // both our metadata reopen and the original locked handle before return.
        drop(current);
        drop(held);
        self.sync()?;
        ensure!(
            self.open_file(name, false)?.is_none(),
            "Staging deletion is not complete; recovery intent must remain"
        );
        Ok(())
    }
    pub fn create_child(&self, name: &str) -> Result<Self> {
        Self::name(name)?;
        self.verify()?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let native_name = std::ffi::CString::new(name)?;
            let result =
                unsafe { libc::mkdirat(self.file.as_raw_fd(), native_name.as_ptr(), 0o700) };
            if result != 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err(std::io::Error::last_os_error())
                    .context("Could not create storage directory");
            }
        }
        #[cfg(not(unix))]
        {
            if let Err(error) = std::fs::create_dir(self.path.join(name)) {
                if error.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err(error).context("Could not create storage directory");
                }
            }
        }
        self.verify()?;
        let child = Self::open(&self.path.join(name), None)?;
        child.sync()?;
        #[cfg(all(test, unix))]
        super::checkpoint("ancestor_created")?;
        self.sync()?;
        Ok(child)
    }
    pub fn sync(&self) -> Result<()> {
        #[cfg(all(test, unix))]
        super::checkpoint("parent_sync")?;
        #[cfg(unix)]
        self.file.sync_all()?;
        // Windows publication uses MOVEFILE_WRITE_THROUGH; FlushFileBuffers on a
        // directory is not supported for an ordinary user handle.
        self.verify()
    }
}

#[cfg(any(target_os = "windows", test))]
pub(super) fn require_single_link(file: &File) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            file.metadata()?.nlink() == 1,
            "Cancellation refuses a staging object with other hardlinks; all bytes preserved"
        );
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        ensure!(
            unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } != 0,
            "Cancellation could not inspect hardlink ownership"
        );
        ensure!(
            info.nNumberOfLinks == 1,
            "Cancellation refuses a staging object with other hardlinks; all bytes preserved"
        );
    }
    Ok(())
}
