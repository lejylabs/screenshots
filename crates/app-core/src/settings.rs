use anyhow::{Context, Result, ensure};
use directories::{ProjectDirs, UserDirs};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureHotkeys(pub [String; 3]);

impl Default for CaptureHotkeys {
    fn default() -> Self {
        Self([
            "Ctrl+Shift+S".into(),
            "Ctrl+Shift+R".into(),
            "Ctrl+Shift+E".into(),
        ])
    }
}

impl CaptureHotkeys {
    pub const LABELS: [&'static str; 3] =
        ["Primary display", "Quick region", "Capture and annotate"];

    pub fn validated(&self) -> Result<Self> {
        let mut result = Self(std::array::from_fn(|_| String::new()));
        for (index, value) in self.0.iter().enumerate() {
            ensure!(value.len() <= 64, "Shortcut is too long");
            if value.trim().is_empty() {
                continue;
            }
            let tokens: Vec<_> = value.split('+').map(str::trim).collect();
            let mut modifiers = [false; 4];
            for token in &tokens[..tokens.len() - 1] {
                let position = match token.to_ascii_lowercase().as_str() {
                    "ctrl" => 0,
                    "alt" => 1,
                    "shift" => 2,
                    "super" => 3,
                    _ => anyhow::bail!("Use Ctrl, Alt, Shift or Super before the key"),
                };
                ensure!(!modifiers[position], "Repeated shortcut modifier");
                modifiers[position] = true;
            }
            let key = tokens.last().unwrap().to_ascii_uppercase();
            let function = key
                .strip_prefix('F')
                .and_then(|n| n.parse::<u8>().ok())
                .filter(|n| (1..=24).contains(n) && key == format!("F{n}"));
            let character = key.len() == 1 && key.as_bytes()[0].is_ascii_alphanumeric();
            ensure!(
                character || function.is_some() || key == "PRINTSCREEN",
                "Use a letter, digit, F1–F24 or PrintScreen"
            );
            ensure!(
                !character || modifiers[0] || modifiers[1] || modifiers[3],
                "Letters and digits need Ctrl, Alt or Super"
            );
            let mut parts: Vec<&str> = ["Ctrl", "Alt", "Shift", "Super"]
                .into_iter()
                .zip(modifiers)
                .filter_map(|(name, enabled)| enabled.then_some(name))
                .collect();
            parts.push(if key == "PRINTSCREEN" {
                "PrintScreen"
            } else {
                &key
            });
            result.0[index] = parts.join("+");
            ensure!(
                !result.0[..index].contains(&result.0[index]),
                "Two commands cannot use the same shortcut"
            );
        }
        Ok(result)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiTheme {
    Oldschool,
    #[default]
    #[serde(other)]
    Default,
}

#[derive(Serialize, Deserialize)]
pub struct CaptureSettings {
    version: u32,
    pub output_directory: PathBuf,
    #[serde(default)]
    pub theme: UiTheme,
    #[serde(default)]
    pub hotkeys: CaptureHotkeys,
    #[serde(default)]
    pub output_profile: CaptureOutputProfile,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureOutputProfile {
    CopyOnly,
    SaveOnly,
    #[default]
    SaveAndCopy,
    AnnotateFirst,
}

impl CaptureOutputProfile {
    pub const ALL: [Self; 4] = [
        Self::CopyOnly,
        Self::SaveOnly,
        Self::SaveAndCopy,
        Self::AnnotateFirst,
    ];
    pub const LABELS: [&'static str; 4] = [
        "Copy only",
        "Save only",
        "Save and copy (default)",
        "Annotate first",
    ];

    pub fn copies(self) -> bool {
        matches!(self, Self::CopyOnly | Self::SaveAndCopy)
    }
    pub fn saves(self) -> bool {
        matches!(self, Self::SaveOnly | Self::SaveAndCopy)
    }
    pub fn requires_single_image(self) -> bool {
        matches!(self, Self::CopyOnly | Self::AnnotateFirst)
    }
    pub fn label(self) -> &'static str {
        Self::LABELS[Self::ALL
            .iter()
            .position(|profile| *profile == self)
            .unwrap()]
    }
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
            theme: UiTheme::default(),
            hotkeys: CaptureHotkeys::default(),
            output_profile: CaptureOutputProfile::default(),
        })
    }

    pub fn with_theme(mut self, theme: UiTheme) -> Self {
        self.theme = theme;
        self
    }

    pub fn with_hotkeys(mut self, hotkeys: CaptureHotkeys) -> Result<Self> {
        self.hotkeys = hotkeys.validated()?;
        Ok(self)
    }

    pub fn with_output_profile(mut self, profile: CaptureOutputProfile) -> Self {
        self.output_profile = profile;
        self
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
        Ok(Self::for_directory(settings.output_directory)?
            .with_theme(settings.theme)
            .with_hotkeys(settings.hotkeys)?
            .with_output_profile(settings.output_profile))
    }

    pub fn save(&self) -> Result<()> {
        self.save_at(&Self::config_path()?)
    }

    fn save_at(&self, path: &Path) -> Result<()> {
        self.hotkeys.validated()?;
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
    fn output_profiles_round_trip_preserve_other_settings_and_reject_unknown_values() -> Result<()>
    {
        let dir = std::env::temp_dir().join(format!(
            "screenshots-output-profiles-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir)?;
        let path = dir.join("settings.json");
        let hotkeys = CaptureHotkeys(["Alt+Q".into(), "".into(), "Ctrl+F9".into()]);
        for profile in CaptureOutputProfile::ALL {
            CaptureSettings::for_directory(dir.clone())?
                .with_hotkeys(hotkeys.clone())?
                .with_theme(UiTheme::Oldschool)
                .with_output_profile(profile)
                .save_at(&path)?;
            let restored = CaptureSettings::load_at(&path, dir.clone())?;
            assert_eq!(restored.output_profile, profile);
            assert_eq!(restored.hotkeys, hotkeys);
            assert_eq!(restored.theme, UiTheme::Oldschool);
            let mut invalid_folder = restored;
            invalid_folder.output_directory = dir.join("missing");
            assert!(invalid_folder.save_at(&path).is_err());
            assert_eq!(
                CaptureSettings::load_at(&path, dir.clone())?.output_profile,
                profile
            );
        }
        fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({"version":1,"output_directory":dir}))?,
        )?;
        assert_eq!(
            CaptureSettings::load_at(&path, dir.clone())?.output_profile,
            CaptureOutputProfile::SaveAndCopy
        );
        fs::write(
            &path,
            serde_json::to_vec(
                &serde_json::json!({"version":1,"output_directory":dir,"output_profile":"upload"}),
            )?,
        )?;
        assert!(CaptureSettings::load_at(&path, dir.clone()).is_err());
        fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn shortcuts_validate_normalize_and_reject_duplicates() -> Result<()> {
        let keys = CaptureHotkeys([" shift + ctrl + q ".into(), "PrintScreen".into(), "".into()])
            .validated()?;
        assert_eq!(keys.0, ["Ctrl+Shift+Q", "PrintScreen", ""]);
        for invalid in [
            "Q",
            "Shift+Q",
            "Ctrl+Ctrl+Q",
            "Ctrl++Q",
            "Ctrl+Q+Alt",
            "Ctrl+F25",
            "Ctrl+F01",
            "Ctrl+é",
            "Ctrl+Enter",
        ] {
            assert!(
                CaptureHotkeys([invalid.into(), "".into(), "".into()])
                    .validated()
                    .is_err(),
                "{invalid}"
            );
        }
        assert!(
            CaptureHotkeys(["ctrl+shift+q".into(), "Shift+Ctrl+Q".into(), "".into()])
                .validated()
                .is_err()
        );
        assert!(
            CaptureHotkeys([" ".repeat(65), "".into(), "".into()])
                .validated()
                .is_err()
        );
        assert!(
            CaptureHotkeys(["".into(), "".into(), "".into()])
                .validated()
                .is_ok()
        );
        Ok(())
    }

    #[test]
    fn shortcut_settings_survive_other_preferences_and_failed_save() -> Result<()> {
        let dir =
            std::env::temp_dir().join(format!("screenshots-hotkeys-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir)?;
        let path = dir.join("settings.json");
        let hotkeys = CaptureHotkeys(["Ctrl+Alt+Q".into(), "F12".into(), "".into()]);
        CaptureSettings::for_directory(dir.clone())?
            .with_hotkeys(hotkeys.clone())?
            .save_at(&path)?;
        let mut restored = CaptureSettings::load_at(&path, dir.clone())?;
        restored.theme = UiTheme::Oldschool;
        restored.save_at(&path)?;
        let bytes = fs::read(&path)?;
        assert_eq!(
            CaptureSettings::load_at(&path, dir.clone())?.hotkeys,
            hotkeys
        );
        restored.hotkeys.0[0] = "F12".into();
        assert!(restored.save_at(&path).is_err());
        assert_eq!(fs::read(&path)?, bytes);
        restored.hotkeys = hotkeys.clone();
        restored.output_directory = dir.join("missing");
        assert!(restored.save_at(&path).is_err());
        assert_eq!(
            CaptureSettings::load_at(&path, dir.clone())?.hotkeys,
            hotkeys
        );
        fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({"version":1,"output_directory":dir}))?,
        )?;
        assert_eq!(
            CaptureSettings::load_at(&path, dir.clone())?.hotkeys,
            CaptureHotkeys::default()
        );
        fs::remove_dir_all(dir)?;
        Ok(())
    }

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

    #[test]
    fn themes_round_trip_and_older_settings_keep_working() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("screenshots-themes-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir)?;
        let path = dir.join("settings.json");
        assert_eq!(
            CaptureSettings::load_at(&path, dir.clone())?.theme,
            UiTheme::Default
        );
        for theme in [UiTheme::Default, UiTheme::Oldschool] {
            CaptureSettings::for_directory(dir.clone())?
                .with_theme(theme)
                .save_at(&path)?;
            let restored = CaptureSettings::load_at(&path, dir.clone())?;
            assert_eq!(restored.theme, theme);
            assert_eq!(restored.output_directory, dir);
        }
        for theme in [None, Some("future_theme"), Some("safelight")] {
            let mut value = serde_json::json!({"version": 1, "output_directory": dir});
            if let Some(theme) = theme {
                value["theme"] = theme.into();
            }
            fs::write(&path, serde_json::to_vec(&value)?)?;
            assert_eq!(
                CaptureSettings::load_at(&path, dir.clone())?.theme,
                UiTheme::Default
            );
        }
        fs::remove_dir_all(dir)?;
        Ok(())
    }
}
