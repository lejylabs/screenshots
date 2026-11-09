use anyhow::{Context, Result, ensure};
use directories::{ProjectDirs, UserDirs};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
pub struct CaptureSettings {
    version: u32,
    pub output_directory: PathBuf,
}

impl CaptureSettings {
    pub fn for_directory(output_directory: PathBuf) -> Result<Self> {
        ensure!(
            output_directory.is_absolute(),
            "The save folder must be an absolute path"
        );
        Ok(Self {
            version: 1,
            output_directory,
        })
    }

    pub(crate) fn config_path() -> Result<PathBuf> {
        Ok(ProjectDirs::from("", "lejylabs", "screenshots")
            .context("Application settings folder is unavailable")?
            .config_dir()
            .join("settings.json"))
    }

    pub fn load() -> Result<Self> {
        let config = Self::config_path()?;
        let default = UserDirs::new()
            .and_then(|dirs| dirs.picture_dir().map(Path::to_path_buf))
            .unwrap_or_else(|| config.parent().unwrap().join("captures"))
            .join("Screenshots");
        Self::load_at(&config, default)
    }

    fn load_at(path: &Path, default: PathBuf) -> Result<Self> {
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Self::for_directory(default);
            }
            Err(error) => return Err(error.into()),
        };
        let mut bytes = Vec::new();
        file.take(65537).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 65536, "Settings file is too large");
        let settings: Self = serde_json::from_slice(&bytes).context("Could not read settings")?;
        ensure!(settings.version == 1, "Unsupported settings version");
        Self::for_directory(settings.output_directory)
    }

    pub fn save(&self) -> Result<()> {
        self.save_at(&Self::config_path()?)
    }

    fn save_at(&self, path: &Path) -> Result<()> {
        ensure!(self.output_directory.is_absolute(), "Invalid save folder");
        ensure!(
            self.output_directory.is_dir(),
            "The selected save folder does not exist"
        );
        let parent = path.parent().context("Invalid settings path")?;
        fs::create_dir_all(parent)?;
        let temporary = parent.join(format!("settings-{}.part", uuid::Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(&serde_json::to_vec_pretty(self)?)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn folder_settings_survive_restart_and_replacement() -> Result<()> {
        let dir =
            std::env::temp_dir().join(format!("screenshots-settings-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(dir.join("first"))?;
        fs::create_dir_all(dir.join("second"))?;
        let path = dir.join("settings.json");
        let first = dir.join("first");
        assert_eq!(
            CaptureSettings::load_at(&path, first.clone())?.output_directory,
            first
        );
        CaptureSettings::for_directory(first)?.save_at(&path)?;
        let second = dir.join("second");
        CaptureSettings::for_directory(second.clone())?.save_at(&path)?;
        assert_eq!(
            CaptureSettings::load_at(&path, dir.clone())?.output_directory,
            second
        );
        fs::write(&path, br#"{"version":1,"output_directory":"relative"}"#)?;
        assert!(CaptureSettings::load_at(&path, dir.clone()).is_err());
        fs::remove_dir_all(dir)?;
        Ok(())
    }
}
