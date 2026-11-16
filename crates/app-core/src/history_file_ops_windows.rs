//! Windows no-replace moves with held native file and parent identities.

use super::FileStamp;
use anyhow::{Context, Result, ensure};
use std::{
    fs::{File, OpenOptions},
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle},
    path::{Path, PathBuf, Prefix},
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FileIdInfo, GetFileInformationByHandle,
    GetFileInformationByHandleEx, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};
use windows_sys::Win32::{
    Foundation::{GENERIC_READ, GENERIC_WRITE},
    Storage::FileSystem::{
        DELETE, FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
    },
};

#[derive(PartialEq, Eq)]
struct Identity {
    volume: u64,
    file: [u8; 16],
    attributes: u32,
    length: u64,
    written: u64,
    created: u64,
}

fn information(file: &File) -> Result<Identity> {
    let mut id = FILE_ID_INFO::default();
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    ensure!(
        unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileIdInfo,
                (&mut id as *mut FILE_ID_INFO).cast(),
                std::mem::size_of::<FILE_ID_INFO>() as u32,
            )
        } != 0,
        "Could not inspect native history file identity: {}",
        std::io::Error::last_os_error()
    );
    ensure!(
        unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } != 0,
        "Could not inspect native history file metadata: {}",
        std::io::Error::last_os_error()
    );
    ensure!(
        info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT == 0,
        "History move refuses reparse points"
    );
    Ok(Identity {
        volume: id.VolumeSerialNumber,
        file: id.FileId.Identifier,
        attributes: info.dwFileAttributes,
        length: (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow),
        written: (u64::from(info.ftLastWriteTime.dwHighDateTime) << 32)
            | u64::from(info.ftLastWriteTime.dwLowDateTime),
        created: (u64::from(info.ftCreationTime.dwHighDateTime) << 32)
            | u64::from(info.ftCreationTime.dwLowDateTime),
    })
}

fn check_path(path: &Path) -> Result<()> {
    let text = path.to_str().context("History move path must be UTF-8")?;
    ensure!(
        path.is_absolute() && text.len() <= 4096 && !text.chars().any(char::is_control),
        "History move path must be absolute, bounded and contain no controls"
    );
    let prefix = match path.components().next() {
        Some(std::path::Component::Prefix(value)) => value.kind(),
        _ => anyhow::bail!("History move needs a native absolute path"),
    };
    ensure!(
        matches!(
            prefix,
            Prefix::Disk(_)
                | Prefix::VerbatimDisk(_)
                | Prefix::UNC(_, _)
                | Prefix::VerbatimUNC(_, _)
        ),
        "History move refuses device paths"
    );
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("History move needs a filename")?;
    let reserved = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    ensure!(
        !name.contains(':')
            && name == name.trim_end_matches([' ', '.'])
            && !matches!(reserved.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            && !(reserved.len() == 4
                && (reserved.starts_with("COM") || reserved.starts_with("LPT"))
                && matches!(reserved.as_bytes()[3], b'1'..=b'9')),
        "History move refuses device names or alternate streams"
    );
    ensure!(
        path.extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("png")),
        "History move source and destination must use .png"
    );
    Ok(())
}

fn opened(path: &Path, directory: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(!directory)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(
            FILE_FLAG_OPEN_REPARSE_POINT
                | if directory {
                    FILE_FLAG_BACKUP_SEMANTICS
                } else {
                    0
                },
        );
    if !directory {
        options.access_mode(GENERIC_READ | GENERIC_WRITE | DELETE);
    }
    let file = options.open(path)?;
    let info = information(&file)?;
    ensure!(
        (info.attributes & FILE_ATTRIBUTE_DIRECTORY != 0) == directory,
        "History move path has an unexpected native type"
    );
    Ok(file)
}

fn parent(path: &Path) -> Result<(File, PathBuf)> {
    check_path(path)?;
    let parent = path
        .parent()
        .context("History move path has no parent")?
        .canonicalize()?;
    Ok((opened(&parent, true)?, parent))
}

fn stamp(file: &mut File) -> Result<FileStamp> {
    let before = information(file)?;
    let (metadata, sha256, width, height) = super::png_contents(file)?;
    ensure!(
        information(file)? == before && before.length == metadata.len(),
        "History move source changed during inspection"
    );
    Ok(FileStamp {
        device: before.volume,
        inode: u64::from_le_bytes(before.file[..8].try_into().unwrap()),
        file_id: Some(before.file),
        size: before.length,
        sha256,
        width,
        height,
    })
}

fn verify_parent(path: &Path, directory: &File) -> Result<()> {
    let named = information(&opened(path, true)?)?;
    let pinned = information(directory)?;
    ensure!(
        named.volume == pinned.volume && named.file == pinned.file,
        "History move parent path changed"
    );
    Ok(())
}

pub(in super::super) struct LockedSource {
    file: File,
    source: PathBuf,
    destination: PathBuf,
    source_parent: File,
    destination_parent: File,
    source_parent_path: PathBuf,
    destination_parent_path: PathBuf,
    stamp: FileStamp,
}

impl LockedSource {
    pub(in super::super) fn open(source: &Path, destination: &Path) -> Result<Self> {
        let (source_parent, source_parent_path) = parent(source)?;
        let (destination_parent, destination_parent_path) = parent(destination)?;
        ensure!(
            information(&source_parent)?.volume == information(&destination_parent)?.volume,
            "History move must stay on the same filesystem"
        );
        ensure!(
            source != destination,
            "History move destination is the source"
        );
        ensure!(
            std::fs::symlink_metadata(destination)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
            "History move destination already exists or is unavailable"
        );
        let mut file = opened(source, false)?;
        let stamp = stamp(&mut file)?;
        Ok(Self {
            file,
            source: source.to_owned(),
            destination: destination.to_owned(),
            source_parent,
            destination_parent,
            source_parent_path,
            destination_parent_path,
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
        ensure!(
            inspect(&self.source)?.as_ref() == Some(&self.stamp),
            "History move source was replaced before rename"
        );
        verify_parent(&self.source_parent_path, &self.source_parent)?;
        verify_parent(&self.destination_parent_path, &self.destination_parent)?;
        self.file.sync_all()?;
        let source: Vec<u16> = self
            .source
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let destination: Vec<u16> = self
            .destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        ensure!(
            unsafe {
                MoveFileExW(
                    source.as_ptr(),
                    destination.as_ptr(),
                    MOVEFILE_WRITE_THROUGH,
                )
            } != 0,
            "Could not rename history image without replacing a file: {}",
            std::io::Error::last_os_error()
        );
        self.verify_destination()
    }
    pub(in super::super) fn verify_destination(&self) -> Result<()> {
        ensure!(
            inspect(&self.destination)?.as_ref() == Some(&self.stamp),
            "History move destination does not match the opened source"
        );
        Ok(())
    }
    pub(in super::super) fn sync_parents(&self) -> Result<()> {
        self.file.sync_all()?;
        verify_parent(&self.source_parent_path, &self.source_parent)?;
        verify_parent(&self.destination_parent_path, &self.destination_parent)?;
        Ok(())
    }
}

pub(in super::super) fn inspect(path: &Path) -> Result<Option<FileStamp>> {
    check_path(path)?;
    match parent(path) {
        Ok(_) => (),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    }
    let mut file = match opened(path, false) {
        Ok(file) => file,
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    Ok(Some(stamp(&mut file)?))
}

pub(in super::super) fn sync_paths(
    source: &Path,
    destination: &Path,
    expected: &FileStamp,
) -> Result<()> {
    let (source_parent, source_path) = parent(source)?;
    let (destination_parent, destination_path) = parent(destination)?;
    ensure!(
        information(&source_parent)?.volume == information(&destination_parent)?.volume,
        "History move must stay on the same filesystem"
    );
    let mut file = opened(destination, false)?;
    ensure!(
        stamp(&mut file)? == *expected,
        "History move destination changed before sync"
    );
    file.sync_all()?;
    verify_parent(&source_path, &source_parent)?;
    verify_parent(&destination_path, &destination_parent)?;
    Ok(())
}

pub(in super::super) fn remove_expected(path: &Path, expected: &FileStamp) -> Result<()> {
    let (directory, parent_path) = parent(path)?;
    let mut file = opened(path, false)?;
    ensure!(
        stamp(&mut file)? == *expected,
        "Quarantined trash file changed; preserved"
    );
    verify_parent(&parent_path, &directory)?;
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    ensure!(
        unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle(),
                FileDispositionInfo,
                (&disposition as *const FILE_DISPOSITION_INFO).cast(),
                std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        } != 0,
        "Could not remove quarantined trash file: {}",
        std::io::Error::last_os_error()
    );
    drop(file);
    #[cfg(test)]
    super::deletion_barrier_checkpoint()?;
    verify_parent(&parent_path, &directory)?;
    Ok(())
}

pub(in super::super) fn sync_absent(path: &Path) -> Result<()> {
    let (directory, parent_path) = parent(path)?;
    let absent = || -> Result<()> {
        ensure!(
            std::fs::symlink_metadata(path)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
            "Deleted history path reappeared or is unavailable"
        );
        Ok(())
    };
    absent()?;
    #[cfg(test)]
    super::deletion_barrier_checkpoint()?;
    // Windows exposes no unprivileged POSIX directory-fsync equivalent here.
    verify_parent(&parent_path, &directory)?;
    absent()
}

pub(in super::super) fn copy_to_stage(
    source: &Path,
    stage: &Path,
    expected: &FileStamp,
    cancelled: &dyn Fn() -> bool,
) -> Result<FileStamp> {
    use std::io::{Read, Seek, Write};
    let (source_directory, source_parent) = parent(source)?;
    let (directory, directory_path) = parent(stage)?;
    let mut input = opened(source, false)?;
    ensure!(
        stamp(&mut input)? == *expected,
        "Transfer source changed before copying"
    );
    let mut output = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .access_mode(GENERIC_READ | GENERIC_WRITE | DELETE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(stage)?;
    let result = (|| {
        let started = std::time::Instant::now();
        let mut remaining = expected.size;
        let mut buffer = [0u8; 64 * 1024];
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
            stamp(&mut input)? == *expected && inspect(source)?.as_ref() == Some(expected),
            "Transfer source changed while copying"
        );
        verify_parent(&source_parent, &source_directory)?;
        verify_parent(&directory_path, &directory)?;
        Ok(copied)
    })();
    if result.is_err() {
        let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
        unsafe {
            SetFileInformationByHandle(
                output.as_raw_handle(),
                FileDispositionInfo,
                (&disposition as *const FILE_DISPOSITION_INFO).cast(),
                std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        };
    }
    result
}
