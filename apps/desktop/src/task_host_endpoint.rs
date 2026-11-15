//! Private endpoint publication; bearer tokens never use argv or logs.
use super::{EndpointInfo, json_object};
use anyhow::{Context, Result, ensure};
use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
};
use zeroize::Zeroizing;

#[cfg(unix)]
mod unix {
    use super::*;
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{
                ffi::OsStrExt,
                fs::{MetadataExt, OpenOptionsExt},
            },
        },
    };

    fn parent(path: &Path) -> Result<(File, CString)> {
        let path = std::path::absolute(path)?;
        ensure!(
            path.as_os_str().as_encoded_bytes().len() <= 4096,
            "Task-host endpoint path is too long"
        );
        let name = CString::new(
            path.file_name()
                .context("Task-host endpoint needs a filename")?
                .as_bytes(),
        )?;
        let directory = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(
                path.parent()
                    .context("Task-host endpoint needs a parent directory")?,
            )?;
        let info = directory.metadata()?;
        ensure!(
            info.is_dir() && info.uid() == unsafe { libc::geteuid() } && info.mode() & 0o022 == 0,
            "Task-host endpoint parent must be current-user-owned and not writable by group or others"
        );
        Ok((directory, name))
    }
    fn validate(file: &File) -> Result<std::fs::Metadata> {
        let info = file.metadata()?;
        ensure!(
            info.is_file()
                && info.uid() == unsafe { libc::geteuid() }
                && info.mode() & 0o777 == 0o600
                && info.nlink() == 1
                && (1..=4096).contains(&info.len()),
            "Task-host endpoint must be a private, current-user-owned regular file within 4096 bytes"
        );
        Ok(info)
    }
    pub(crate) struct Lease {
        directory: File,
        name: CString,
        device: u64,
        inode: u64,
    }
    impl Lease {
        pub(crate) fn create(path: &Path, value: &EndpointInfo) -> Result<Self> {
            let (directory, name) = parent(path)?;
            let temporary = CString::new(format!(
                ".screenshots-task-endpoint-{}.part",
                uuid::Uuid::new_v4()
            ))?;
            let fd = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    temporary.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o600,
                )
            };
            ensure!(fd >= 0, "Could not stage private task-host endpoint");
            let mut file = unsafe { File::from_raw_fd(fd) };
            let result = (|| {
                ensure!(
                    unsafe { libc::fchmod(file.as_raw_fd(), 0o600) } == 0,
                    "Could not protect task-host endpoint"
                );
                let bytes = Zeroizing::new(serde_json::to_vec(value)?);
                ensure!(bytes.len() <= 4096, "Task-host endpoint is too large");
                file.write_all(&bytes)?;
                file.sync_all()?;
                let info = file.metadata()?;
                ensure!(
                    unsafe {
                        libc::linkat(
                            directory.as_raw_fd(),
                            temporary.as_ptr(),
                            directory.as_raw_fd(),
                            name.as_ptr(),
                            0,
                        )
                    } == 0,
                    "Task-host endpoint already exists or could not be published; stale files must be inspected explicitly"
                );
                Ok((info.dev(), info.ino()))
            })();
            let cleaned =
                unsafe { libc::unlinkat(directory.as_raw_fd(), temporary.as_ptr(), 0) } == 0;
            let (device, inode) = result?;
            let lease = Self {
                directory,
                name,
                device,
                inode,
            };
            ensure!(cleaned, "Could not clean private endpoint staging file");
            lease.directory.sync_all()?;
            Ok(lease)
        }
        pub(crate) fn cleanup(&mut self) -> Result<()> {
            let mut info: libc::stat = unsafe { std::mem::zeroed() };
            let found = unsafe {
                libc::fstatat(
                    self.directory.as_raw_fd(),
                    self.name.as_ptr(),
                    &mut info,
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            };
            if found != 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::NotFound {
                    return Ok(());
                }
                return Err(error)
                    .context("Could not inspect owned task-host endpoint during cleanup");
            }
            if info.st_dev as u64 == self.device && info.st_ino as u64 == self.inode {
                ensure!(
                    unsafe { libc::unlinkat(self.directory.as_raw_fd(), self.name.as_ptr(), 0) }
                        == 0,
                    "Could not remove owned task-host endpoint"
                );
                self.directory.sync_all()?;
            }
            Ok(())
        }
    }
    impl Drop for Lease {
        fn drop(&mut self) {
            let _ = self.cleanup();
        }
    }
    pub(crate) fn read(path: &Path) -> Result<EndpointInfo> {
        let (directory, name) = parent(path)?;
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        ensure!(fd >= 0, "Could not open private task-host endpoint");
        let file = unsafe { File::from_raw_fd(fd) };
        let info = validate(&file)?;
        let mut bytes = Zeroizing::new(Vec::with_capacity(info.len() as usize));
        file.take(info.len()).read_to_end(&mut bytes)?;
        json_object(&bytes)
    }
}

#[cfg(target_os = "windows")]
pub(super) use super::windows::{EndpointLease as Lease, read_endpoint as read};
#[cfg(unix)]
pub(super) use unix::{Lease, read};

#[cfg(not(any(unix, target_os = "windows")))]
pub(super) struct Lease;
#[cfg(not(any(unix, target_os = "windows")))]
impl Lease {
    pub(super) fn create(_: &Path, _: &EndpointInfo) -> Result<Self> {
        anyhow::bail!("Private endpoints are unavailable on this platform")
    }
    pub(super) fn cleanup(&mut self) -> Result<()> {
        Ok(())
    }
}
#[cfg(not(any(unix, target_os = "windows")))]
pub(super) fn read(_: &Path) -> Result<EndpointInfo> {
    anyhow::bail!("Private endpoints are unavailable on this platform")
}
