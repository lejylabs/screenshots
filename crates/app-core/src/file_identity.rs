//! Opened-object identity for explicit prepared input/command consumption.
use anyhow::{Result, ensure};
use std::{
    fs::{File, OpenOptions},
    path::Path,
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileIdentity(Vec<u8>);
impl FileIdentity {
    pub fn open(path: &Path) -> Result<(File, Self)> {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = options.open(path)?;
        let identity = Self::from_file(&file)?;
        Ok((file, identity))
    }
    pub fn from_file(file: &File) -> Result<Self> {
        let metadata = file.metadata()?;
        ensure!(
            metadata.is_file(),
            "Prepared input/program must remain a regular file"
        );
        let mut bytes = Vec::with_capacity(96);
        bytes.extend_from_slice(&metadata.len().to_le_bytes());
        let modified = match metadata.modified()?.duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_nanos() as i128,
            Err(e) => -(e.duration().as_nanos() as i128),
        };
        bytes.extend_from_slice(&modified.to_le_bytes());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            for n in [
                metadata.dev(),
                metadata.ino(),
                u64::from(metadata.mode()),
                metadata.ctime() as u64,
                metadata.ctime_nsec() as u64,
            ] {
                bytes.extend_from_slice(&n.to_le_bytes());
            }
        }
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_BASIC_INFO, FILE_ID_INFO, FileBasicInfo, FileIdInfo,
                GetFileInformationByHandleEx,
            };
            let mut id = FILE_ID_INFO::default();
            let mut basic = FILE_BASIC_INFO::default();
            ensure!(
                unsafe {
                    GetFileInformationByHandleEx(
                        file.as_raw_handle(),
                        FileIdInfo,
                        (&mut id as *mut FILE_ID_INFO).cast(),
                        std::mem::size_of_val(&id) as u32,
                    )
                } != 0,
                "Native 128-bit prepared file identity is unavailable"
            );
            ensure!(
                unsafe {
                    GetFileInformationByHandleEx(
                        file.as_raw_handle(),
                        FileBasicInfo,
                        (&mut basic as *mut FILE_BASIC_INFO).cast(),
                        std::mem::size_of_val(&basic) as u32,
                    )
                } != 0,
                "Native prepared file change stamp is unavailable"
            );
            bytes.extend_from_slice(&id.VolumeSerialNumber.to_le_bytes());
            bytes.extend_from_slice(&id.FileId.Identifier);
            bytes.extend_from_slice(&basic.CreationTime.to_le_bytes());
            bytes.extend_from_slice(&basic.ChangeTime.to_le_bytes());
            bytes.extend_from_slice(&basic.FileAttributes.to_le_bytes());
        }
        Ok(Self(bytes))
    }
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
    pub fn verify_file(&self, file: &File) -> Result<()> {
        ensure!(
            &Self::from_file(file)? == self,
            "Prepared input/program changed before consumption"
        );
        Ok(())
    }
    /// Checks the object opened at the launch boundary; not an atomic executable pin.
    pub fn verify_path(&self, path: &Path) -> Result<()> {
        let (file, _) = Self::open(path)?;
        self.verify_file(&file)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_opened_object_is_checked_and_retained_handle_survives_path_replacement() -> Result<()>
    {
        let root =
            std::env::temp_dir().join(format!("screenshots-file-id-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root)?;
        let path = root.join("input");
        std::fs::write(&path, b"original")?;
        let (file, id) = FileIdentity::open(&path)?;
        id.verify_file(&file)?;
        std::fs::rename(&path, root.join("prior"))?;
        std::fs::write(&path, b"replaced")?;
        assert!(id.verify_file(&File::open(&path)?).is_err());
        let mut original = String::new();
        std::io::Read::read_to_string(&mut &file, &mut original)?;
        assert_eq!(original, "original");
        drop(file);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}
