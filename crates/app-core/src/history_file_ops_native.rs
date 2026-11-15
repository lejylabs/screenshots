//! Native same-filesystem moves; journal ownership belongs to the caller.

use anyhow::Result;
#[cfg(not(target_os = "linux"))]
use anyhow::bail;
#[cfg(target_os = "linux")]
use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct FileStamp {
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use sha2_10::{Digest, Sha256};
    use std::{
        ffi::{CString, OsStr},
        fs::{File, Metadata, OpenOptions},
        io::{Read, Seek},
        os::unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
            io::{AsRawFd, FromRawFd},
        },
        path::PathBuf,
        time::{Duration, Instant},
    };

    const MAX_BYTES: u64 = 272 * 1024 * 1024;

    fn check_path(path: &Path) -> Result<()> {
        let text = path.to_str().context("History move path must be UTF-8")?;
        ensure!(
            path.is_absolute() && text.len() <= 4096 && !text.chars().any(char::is_control),
            "History move path must be absolute, bounded and contain no controls"
        );
        ensure!(
            path.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("png")),
            "History move source and destination must use .png"
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
        let started = Instant::now();
        let before = file.metadata()?;
        ensure!(
            before.is_file() && before.len() >= 33 && before.len() <= MAX_BYTES,
            "History move source must be a regular PNG no larger than 272 MiB"
        );
        file.rewind()?;
        let mut header = [0u8; 33];
        file.read_exact(&mut header)?;
        ensure!(
            &header[..8] == b"\x89PNG\r\n\x1a\n"
                && header[8..12] == [0, 0, 0, 13]
                && &header[12..16] == b"IHDR",
            "History move source has no valid PNG header"
        );
        ensure!(
            crc32fast::hash(&header[12..29])
                == u32::from_be_bytes(header[29..33].try_into().unwrap()),
            "History move PNG header CRC failed"
        );
        let width = u32::from_be_bytes(header[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(header[20..24].try_into().unwrap());
        crate::validate_image_size(width, height)?;
        ensure!(
            matches!(
                (header[25], header[24]),
                (0, 1 | 2 | 4 | 8 | 16) | (2, 8 | 16) | (3, 1 | 2 | 4 | 8) | (4 | 6, 8 | 16)
            ) && header[26] == 0
                && header[27] == 0
                && header[28] <= 1,
            "History move PNG header is unsupported"
        );
        file.rewind()?;
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut remaining = before.len();
        while remaining > 0 {
            ensure!(
                started.elapsed() < Duration::from_secs(30),
                "History move inspection exceeded 30 seconds"
            );
            let length = buffer.len().min(remaining as usize);
            let count = match file.read(&mut buffer[..length]) {
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                result => result?,
            };
            ensure!(count > 0, "History move source changed while reading");
            hash.update(&buffer[..count]);
            remaining -= count as u64;
        }
        ensure!(
            same_metadata(&before, &file.metadata()?),
            "History move source changed during inspection"
        );
        file.rewind()?;
        Ok(FileStamp {
            device: before.dev(),
            inode: before.ino(),
            size: before.len(),
            sha256: hash.finalize().iter().map(|b| format!("{b:02x}")).collect(),
            width,
            height,
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
            let renamed = unsafe {
                libc::renameat2(
                    self.source_parent.as_raw_fd(),
                    self.source_name.as_ptr(),
                    self.destination_parent.as_raw_fd(),
                    self.destination_name.as_ptr(),
                    libc::RENAME_NOREPLACE,
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
}

#[cfg(target_os = "linux")]
pub(super) use linux::{LockedSource, inspect, sync_paths};

#[cfg(not(target_os = "linux"))]
pub(super) struct LockedSource;
#[cfg(not(target_os = "linux"))]
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
#[cfg(not(target_os = "linux"))]
pub(super) fn inspect(_: &Path) -> Result<Option<FileStamp>> {
    bail!("History file moves are supported only on Linux")
}
#[cfg(not(target_os = "linux"))]
pub(super) fn sync_paths(_: &Path, _: &Path, _: &FileStamp) -> Result<()> {
    bail!("History file moves are supported only on Linux")
}

#[cfg(all(test, target_os = "linux"))]
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
        assert!(!source.exists());
        assert_eq!(inspect(&destination).unwrap(), Some(before));
        assert_eq!(std::fs::read(&destination).unwrap(), original);
        assert!(inspect(&source).unwrap().is_none());
        std::fs::write(&source, b"preserve").unwrap();
        assert!(LockedSource::open(&destination, &source).is_err());
        assert_eq!(std::fs::read(&source).unwrap(), b"preserve");
        let link = root.join("link.png");
        std::os::unix::fs::symlink(&destination, &link).unwrap();
        assert!(inspect(&link).is_err());
        assert!(LockedSource::open(&link, &root.join("other.png")).is_err());
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
        std::fs::remove_dir_all(root).unwrap();
    }
}
