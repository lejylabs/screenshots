//! Optional FTPS CA bytes are admitted once and privately staged only during transfer.
use anyhow::{Result, ensure};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};
use zeroize::Zeroizing;
#[cfg(target_os = "windows")]
#[path = "capture_schedule_private_windows.rs"]
mod private;
#[derive(Clone)]
pub(super) struct Ca(Arc<Zeroizing<Vec<u8>>>);
impl Ca {
    pub fn read(path: &Path) -> Result<Self> {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = options.open(path)?;
        let metadata = file.metadata()?;
        ensure!(
            metadata.is_file() && metadata.len() <= 1024 * 1024,
            "Provider CA must be a regular file within 1 MiB"
        );
        let mut bytes = Zeroizing::new(Vec::with_capacity(metadata.len() as usize));
        file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 1024 * 1024, "Provider CA exceeds 1 MiB");
        Ok(Self(Arc::new(bytes)))
    }
    pub fn digest(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        Sha256::digest(self.0.as_slice()).into()
    }
    pub fn stage(&self) -> Result<Staged> {
        let directory =
            std::env::temp_dir().join(format!("screenshots-provider-ca-{}", uuid::Uuid::new_v4()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().mode(0o700).create(&directory)?;
        }
        #[cfg(target_os = "windows")]
        private::directory(&directory, true)?;
        let path = directory.join("ca.pem");
        #[cfg(target_os = "windows")]
        let created = private::open(&path, true);
        #[cfg(not(target_os = "windows"))]
        let created = {
            let mut options = OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            options.open(&path).map_err(Into::into)
        };
        let mut file = match created {
            Ok(file) => file,
            Err(error) => {
                let _ = std::fs::remove_dir(&directory);
                return Err(error);
            }
        };
        let staged = Staged {
            path,
            directory,
            file: Some(file.try_clone()?),
        };
        file.write_all(self.0.as_slice())?;
        file.sync_all()?;
        Ok(staged)
    }
}
pub(super) struct Staged {
    pub path: PathBuf,
    directory: PathBuf,
    file: Option<File>,
}
impl Drop for Staged {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let (Ok(owned), Ok(named)) = (
                self.file.as_ref().unwrap().metadata(),
                self.path.symlink_metadata(),
            ) && owned.dev() == named.dev()
                && owned.ino() == named.ino()
            {
                let _ = std::fs::remove_file(&self.path);
            }
        }
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
            };
            // The opened CA handle identifies the owned file even after a pathname replacement.
            let mut disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
            unsafe {
                SetFileInformationByHandle(
                    self.file.as_ref().unwrap().as_raw_handle(),
                    FileDispositionInfo,
                    (&mut disposition as *mut FILE_DISPOSITION_INFO).cast(),
                    std::mem::size_of_val(&disposition) as u32,
                );
            }
        }
        self.file.take();
        let _ = std::fs::remove_dir(&self.directory);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replacement_after_admission_cannot_change_staged_ca() -> Result<()> {
        let path =
            std::env::temp_dir().join(format!("screenshots-ca-input-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, b"original CA")?;
        let ca = Ca::read(&path)?;
        std::fs::write(&path, b"replacement CA")?;
        let staged = ca.stage()?;
        assert_eq!(std::fs::read(&staged.path)?, b"original CA");
        let output = staged.path.clone();
        drop(staged);
        assert!(!output.exists());
        std::fs::remove_file(path)?;
        Ok(())
    }
}
