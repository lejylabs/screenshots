//! Native same-filesystem moves; journal ownership belongs to the caller.

use anyhow::Result;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
use anyhow::bail;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
#[cfg(not(target_os = "windows"))]
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct FileStamp {
    #[serde(default, skip_serializing_if = "crate::capture_format::is_png")]
    pub format: crate::ImageExportFormat,
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<[u8; 16]>,
}

#[cfg(test)]
thread_local! { static FAIL_DELETION_BARRIER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

#[cfg(test)]
pub(super) fn set_deletion_barrier_failure_for_test(enabled: bool) {
    FAIL_DELETION_BARRIER.with(|flag| flag.set(enabled));
}

#[cfg(test)]
fn deletion_barrier_checkpoint() -> Result<()> {
    anyhow::ensure!(
        !FAIL_DELETION_BARRIER.with(|flag| flag.get()),
        "Injected deletion parent barrier failure"
    );
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn image_contents(
    file: &mut std::fs::File,
) -> Result<(std::fs::Metadata, String, crate::SavedImageInfo)> {
    crate::saved_image::fingerprint_saved_image_file(file)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod posix {
    use super::*;
    use std::{
        ffi::{CString, OsStr},
        fs::{File, Metadata, OpenOptions},
        os::unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
            io::{AsRawFd, FromRawFd},
        },
        path::PathBuf,
    };

    fn check_path(path: &Path) -> Result<()> {
        let text = path.to_str().context("History move path must be UTF-8")?;
        ensure!(
            path.is_absolute() && text.len() <= 4096 && !text.chars().any(char::is_control),
            "History move path must be absolute, bounded and contain no controls"
        );
        Ok(())
    }

    fn name(value: &OsStr) -> Result<CString> {
        CString::new(value.as_bytes()).context("History move path contains NUL")
    }

    fn parent(path: &Path) -> Result<(File, PathBuf, CString)> {
        check_path(path)?;
        let parent = path
            .parent()
            .context("History move path has no parent")?
            .canonicalize()?;
        let mut options = OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW);
        let directory = options
            .open(&parent)
            .context("Could not open history move parent")?;
        Ok((
            directory,
            parent,
            name(
                path.file_name()
                    .context("History move path has no filename")?,
            )?,
        ))
    }

    fn open_at(directory: &File, name: &CString) -> Result<Option<File>> {
        let descriptor = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            )
        };
        if descriptor < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::NotFound {
                return Ok(None);
            }
            return Err(error).context("Could not open history move image");
        }
        Ok(Some(unsafe { File::from_raw_fd(descriptor) }))
    }

    fn same_metadata(a: &Metadata, b: &Metadata) -> bool {
        a.dev() == b.dev()
            && a.ino() == b.ino()
            && a.len() == b.len()
            && a.mtime() == b.mtime()
            && a.mtime_nsec() == b.mtime_nsec()
            && a.ctime() == b.ctime()
            && a.ctime_nsec() == b.ctime_nsec()
    }

    fn verify_parent(path: &Path, directory: &File) -> Result<()> {
        let current = std::fs::metadata(path)?;
        let pinned = directory.metadata()?;
        ensure!(
            current.dev() == pinned.dev() && current.ino() == pinned.ino(),
            "History move parent path changed"
        );
        Ok(())
    }

    fn stamp(file: &mut File) -> Result<FileStamp> {
        let (before, sha256, info) = super::image_contents(file)?;
        ensure!(
            same_metadata(&before, &file.metadata()?),
            "History move source changed during inspection"
        );
        Ok(FileStamp {
            format: info.format,
            device: before.dev(),
            inode: before.ino(),
            size: before.len(),
            sha256,
            width: info.width,
            height: info.height,
            file_id: None,
        })
    }

    pub(in super::super) struct LockedSource {
        file: File,
        source_parent: File,
        destination_parent: File,
        source_parent_path: PathBuf,
        destination_parent_path: PathBuf,
        source_name: CString,
        destination_name: CString,
        stamp: FileStamp,
    }

    impl LockedSource {
        pub(in super::super) fn open(source: &Path, destination: &Path) -> Result<Self> {
            let (source_parent, source_parent_path, source_name) = parent(source)?;
            let (destination_parent, destination_parent_path, destination_name) =
                parent(destination)?;
            ensure!(
                source_parent.metadata()?.dev() == destination_parent.metadata()?.dev(),
                "History move must stay on the same filesystem"
            );
            ensure!(
                source_parent_path != destination_parent_path || source_name != destination_name,
                "History move destination is the source"
            );
            ensure!(
                open_at(&destination_parent, &destination_name)?.is_none(),
                "History move destination already exists"
            );
            let mut file =
                open_at(&source_parent, &source_name)?.context("History move source is missing")?;
            let stamp = stamp(&mut file)?;
            ensure!(
                crate::ImageExportFormat::from_path(destination)? == stamp.format,
                "History destination extension does not match source contents"
            );
            Ok(Self {
                file,
                source_parent,
                destination_parent,
                source_parent_path,
                destination_parent_path,
                source_name,
                destination_name,
                stamp,
            })
        }
        pub(in super::super) fn stamp(&self) -> &FileStamp {
            &self.stamp
        }
        pub(in super::super) fn rename(&mut self) -> Result<()> {
            ensure!(
                stamp(&mut self.file)? == self.stamp,
                "History move source changed before rename"
            );
            let mut current = open_at(&self.source_parent, &self.source_name)?
                .context("History move source disappeared")?;
            ensure!(
                stamp(&mut current)? == self.stamp,
                "History move source was replaced before rename"
            );
            #[cfg(target_os = "linux")]
            let renamed = unsafe {
                libc::renameat2(
                    self.source_parent.as_raw_fd(),
                    self.source_name.as_ptr(),
                    self.destination_parent.as_raw_fd(),
                    self.destination_name.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            };
            #[cfg(target_os = "macos")]
            let renamed = unsafe {
                libc::renameatx_np(
                    self.source_parent.as_raw_fd(),
                    self.source_name.as_ptr(),
                    self.destination_parent.as_raw_fd(),
                    self.destination_name.as_ptr(),
                    libc::RENAME_EXCL,
                )
            };
            if renamed != 0 {
                return Err(std::io::Error::last_os_error())
                    .context("Could not rename history image without replacing a file");
            }
            self.verify_destination()
        }
        pub(in super::super) fn verify_destination(&self) -> Result<()> {
            let mut current = open_at(&self.destination_parent, &self.destination_name)?
                .context("History move destination disappeared")?;
            ensure!(
                stamp(&mut current)? == self.stamp,
                "History move destination does not match the opened source"
            );
            Ok(())
        }
        pub(in super::super) fn sync_parents(&self) -> Result<()> {
            self.file.sync_all()?;
            self.source_parent.sync_all()?;
            self.destination_parent.sync_all()?;
            verify_parent(&self.source_parent_path, &self.source_parent)?;
            verify_parent(&self.destination_parent_path, &self.destination_parent)?;
            Ok(())
        }
    }

    pub(in super::super) fn inspect(path: &Path) -> Result<Option<FileStamp>> {
        let (directory, _, name) = match parent(path) {
            Ok(value) => value,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let Some(mut file) = open_at(&directory, &name)? else {
            return Ok(None);
        };
        Ok(Some(stamp(&mut file)?))
    }
    pub(in super::super) fn sync_paths(
        source: &Path,
        destination: &Path,
        expected: &FileStamp,
    ) -> Result<()> {
        let (source, source_path, _) = parent(source)?;
        let (destination, destination_path, destination_name) = parent(destination)?;
        ensure!(
            source.metadata()?.dev() == destination.metadata()?.dev(),
            "History move must stay on the same filesystem"
        );
        let mut file = open_at(&destination, &destination_name)?
            .context("History move destination disappeared before sync")?;
        ensure!(
            stamp(&mut file)? == *expected,
            "History move destination changed before sync"
        );
        file.sync_all()?;
        source.sync_all()?;
        destination.sync_all()?;
        verify_parent(&source_path, &source)?;
        verify_parent(&destination_path, &destination)?;
        Ok(())
    }
    pub(in super::super) fn remove_expected(path: &Path, expected: &FileStamp) -> Result<()> {
        let (directory, directory_path, name) = parent(path)?;
        let mut file = open_at(&directory, &name)?.context("Quarantined trash file disappeared")?;
        ensure!(
            stamp(&mut file)? == *expected,
            "Quarantined trash file changed; preserved"
        );
        let mut named =
            open_at(&directory, &name)?.context("Quarantined trash file disappeared")?;
        ensure!(
            stamp(&mut named)? == *expected,
            "Quarantined trash file was replaced; preserved"
        );
        ensure!(
            unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } == 0,
            "Could not remove quarantined trash file: {}",
            std::io::Error::last_os_error()
        );
        #[cfg(test)]
        super::deletion_barrier_checkpoint()?;
        directory.sync_all()?;
        verify_parent(&directory_path, &directory)?;
        Ok(())
    }
    pub(in super::super) fn sync_absent(path: &Path) -> Result<()> {
        let (directory, directory_path, name) = parent(path)?;
        ensure!(
            open_at(&directory, &name)?.is_none(),
            "Deleted history path reappeared before its parent barrier"
        );
        #[cfg(test)]
        super::deletion_barrier_checkpoint()?;
        directory.sync_all()?;
        verify_parent(&directory_path, &directory)?;
        ensure!(
            open_at(&directory, &name)?.is_none(),
            "Deleted history path reappeared during its parent barrier"
        );
        Ok(())
    }
    pub(in super::super) fn copy_to_stage(
        source: &Path,
        stage: &Path,
        expected: &FileStamp,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<FileStamp> {
        use std::io::{Read, Seek, Write};
        let (source_directory, source_parent, source_name) = parent(source)?;
        let (directory, directory_path, name) = parent(stage)?;
        let mut input =
            open_at(&source_directory, &source_name)?.context("Transfer source disappeared")?;
        ensure!(
            stamp(&mut input)? == *expected,
            "Transfer source changed before copying"
        );
        let descriptor = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDWR
                    | libc::O_CREAT
                    | libc::O_EXCL
                    | libc::O_CLOEXEC
                    | libc::O_NOFOLLOW
                    | libc::O_NONBLOCK,
                0o600,
            )
        };
        ensure!(
            descriptor >= 0,
            "Could not reserve transfer staging file: {}",
            std::io::Error::last_os_error()
        );
        let mut output = unsafe { File::from_raw_fd(descriptor) };
        let owned = output.metadata()?;
        let result = (|| {
            let started = std::time::Instant::now();
            let mut buffer = [0u8; 64 * 1024];
            let mut remaining = expected.size;
            input.rewind()?;
            while remaining > 0 {
                ensure!(!cancelled(), "Transfer canceled while copying");
                ensure!(
                    started.elapsed() < std::time::Duration::from_secs(30),
                    "Transfer copy exceeded its cooperative deadline"
                );
                let count = (remaining as usize).min(buffer.len());
                input.read_exact(&mut buffer[..count])?;
                output.write_all(&buffer[..count])?;
                remaining -= count as u64;
            }
            output.sync_all()?;
            let copied = stamp(&mut output)?;
            ensure!(
                copied.size == expected.size
                    && copied.sha256 == expected.sha256
                    && copied.width == expected.width
                    && copied.height == expected.height,
                "Transfer copy does not match its source"
            );
            ensure!(
                stamp(&mut input)? == *expected,
                "Transfer source changed while copying"
            );
            let mut named = open_at(&source_directory, &source_name)?
                .context("Transfer source path disappeared")?;
            ensure!(
                stamp(&mut named)? == *expected,
                "Transfer source was replaced while copying"
            );
            directory.sync_all()?;
            verify_parent(&directory_path, &directory)?;
            verify_parent(&source_parent, &source_directory)?;
            Ok(copied)
        })();
        if result.is_err()
            && let Ok(Some(named)) = open_at(&directory, &name)
            && named.metadata().is_ok_and(|metadata| {
                metadata.dev() == owned.dev() && metadata.ino() == owned.ino()
            })
        {
            unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) };
            let _ = directory.sync_all();
        }
        result
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) use posix::{
    LockedSource, copy_to_stage, inspect, remove_expected, sync_absent, sync_paths,
};

#[cfg(target_os = "windows")]
#[path = "history_file_ops_windows.rs"]
mod windows;
#[cfg(target_os = "windows")]
pub(super) use windows::{
    LockedSource, copy_to_stage, inspect, remove_expected, sync_absent, sync_paths,
};

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub(super) struct LockedSource;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
impl LockedSource {
    pub(super) fn open(_: &Path, _: &Path) -> Result<Self> {
        bail!("History file moves are supported only on Linux")
    }
    pub(super) fn stamp(&self) -> &FileStamp {
        unreachable!()
    }
    pub(super) fn rename(&mut self) -> Result<()> {
        bail!("History file moves are supported only on Linux")
    }
    pub(super) fn verify_destination(&self) -> Result<()> {
        bail!("History file moves are supported only on Linux")
    }
    pub(super) fn sync_parents(&self) -> Result<()> {
        bail!("History file moves are supported only on Linux")
    }
}
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub(super) fn inspect(_: &Path) -> Result<Option<FileStamp>> {
    bail!("History file moves are supported only on Linux")
}
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub(super) fn sync_paths(_: &Path, _: &Path, _: &FileStamp) -> Result<()> {
    bail!("History file moves are supported only on Linux")
}
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub(super) fn remove_expected(_: &Path, _: &FileStamp) -> Result<()> {
    bail!("History file removal is unavailable on this operating system")
}
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub(super) fn sync_absent(_: &Path) -> Result<()> {
    bail!("History file deletion barriers are unavailable on this operating system")
}
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub(super) fn copy_to_stage(
    _: &Path,
    _: &Path,
    _: &FileStamp,
    _: &dyn Fn() -> bool,
) -> Result<FileStamp> {
    bail!("History file transfer is unavailable on this operating system")
}

#[cfg(all(
    test,
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
mod tests {
    use super::*;
    #[test]
    fn move_preserves_fingerprint_and_refuses_collision_or_symlink() {
        let root = std::env::temp_dir().join(format!(
            "screenshots-history-native-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        let source = root.join("source.png");
        let destination = root.join("moved.png");
        image::RgbaImage::from_pixel(2, 3, image::Rgba([1, 2, 3, 4]))
            .save(&source)
            .unwrap();
        let original = std::fs::read(&source).unwrap();
        let mut opened = LockedSource::open(&source, &destination).unwrap();
        let before = opened.stamp().clone();
        opened.rename().unwrap();
        opened.verify_destination().unwrap();
        opened.sync_parents().unwrap();
        drop(opened);
        assert!(!source.exists());
        assert_eq!(inspect(&destination).unwrap(), Some(before));
        assert_eq!(std::fs::read(&destination).unwrap(), original);
        assert!(inspect(&source).unwrap().is_none());
        std::fs::write(&source, b"preserve").unwrap();
        assert!(LockedSource::open(&destination, &source).is_err());
        assert_eq!(std::fs::read(&source).unwrap(), b"preserve");
        #[cfg(unix)]
        {
            let link = root.join("link.png");
            std::os::unix::fs::symlink(&destination, &link).unwrap();
            assert!(inspect(&link).is_err());
            assert!(LockedSource::open(&link, &root.join("other.png")).is_err());
        }
        let expected = inspect(&destination).unwrap().unwrap();
        sync_paths(&source, &destination, &expected).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn destination_race_and_source_replacement_do_not_rename_unchecked_files() {
        let root = std::env::temp_dir().join(format!(
            "screenshots-history-races-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        let source = root.join("source.png");
        let target = root.join("target.png");
        image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255]))
            .save(&source)
            .unwrap();
        let original = std::fs::read(&source).unwrap();
        let mut locked = LockedSource::open(&source, &target).unwrap();
        std::fs::write(&target, b"another writer").unwrap();
        assert!(locked.rename().is_err());
        assert_eq!(std::fs::read(&source).unwrap(), original);
        assert_eq!(std::fs::read(&target).unwrap(), b"another writer");
        drop(locked);
        std::fs::remove_file(&target).unwrap();
        let mut locked = LockedSource::open(&source, &target).unwrap();
        std::fs::rename(&source, root.join("original.png")).unwrap();
        image::RgbaImage::from_pixel(1, 1, image::Rgba([9, 8, 7, 255]))
            .save(&source)
            .unwrap();
        let replacement = std::fs::read(&source).unwrap();
        assert!(locked.rename().is_err());
        assert_eq!(std::fs::read(&source).unwrap(), replacement);
        assert!(!target.exists());
        assert!(inspect(&root.join("absent/never.png")).unwrap().is_none());
        drop(locked);
        std::fs::remove_dir_all(root).unwrap();
    }
}
