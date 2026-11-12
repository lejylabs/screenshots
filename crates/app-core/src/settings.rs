use anyhow::{Context, Result, ensure};
use directories::{ProjectDirs, UserDirs};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub const DEFAULT_JPEG_QUALITY: u8 = 90;
fn default_jpeg_quality() -> u8 { DEFAULT_JPEG_QUALITY }
fn default_capture_delay() -> u8 { 3 }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "StoredCaptureHotkeys")]
pub struct CaptureHotkeys(pub [String; 6]);

#[derive(Deserialize)]
#[serde(untagged)]
enum StoredCaptureHotkeys {
    Current([String; 6]),
    FiveCommands([String; 5]),
    FourCommands([String; 4]),
    Legacy([String; 3]),
}

impl From<StoredCaptureHotkeys> for CaptureHotkeys {
    fn from(stored: StoredCaptureHotkeys) -> Self {
        match stored {
            StoredCaptureHotkeys::Current(keys) => Self(keys),
            StoredCaptureHotkeys::FiveCommands([primary, region, edit, active, repeat]) => {
                Self([primary, region, edit, active, repeat, String::new()])
            }
            StoredCaptureHotkeys::FourCommands([primary, region, edit, active]) => {
                Self([primary, region, edit, active, String::new(), String::new()])
            }
            StoredCaptureHotkeys::Legacy([primary, region, edit]) => {
                Self([primary, region, edit, String::new(), String::new(), String::new()])
            }
        }
    }
}

impl Default for CaptureHotkeys {
    fn default() -> Self {
        Self([
            "Ctrl+Shift+S".into(),
            "Ctrl+Shift+R".into(),
            "Ctrl+Shift+E".into(),
            String::new(),
            String::new(),
            String::new(),
        ])
    }
}

impl CaptureHotkeys {
    pub const LABELS: [&'static str; 6] = [
        "Primary display",
        "Quick region",
        "Capture and annotate",
        "Active window",
        "Repeat last region",
        "Active display",
    ];

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
    #[serde(default)]
    pub region_presets: Vec<RegionPreset>,
    #[serde(default = "default_jpeg_quality")]
    pub jpeg_quality: u8,
    #[serde(default)]
    pub export_format: crate::ImageExportFormat,
    #[serde(default)]
    pub filename_template: String,
    #[serde(default = "default_capture_delay")]
    pub capture_delay_seconds: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionPreset {
    pub name: String,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl RegionPreset {
    pub fn new(name: &str, area: crate::PixelRegion) -> Result<Self> {
        let preset = Self { name: name.trim().into(), x: area.x, y: area.y, width: area.width, height: area.height };
        preset.validate()?;
        Ok(preset)
    }
    pub fn area(&self) -> crate::PixelRegion {
        crate::PixelRegion { x: self.x, y: self.y, width: self.width, height: self.height }
    }
    fn validate(&self) -> Result<()> {
        ensure!(!self.name.is_empty() && self.name.len() <= 64 && self.name.trim() == self.name
            && !self.name.chars().any(char::is_control), "Use a preset name of 1–64 bytes without control characters");
        crate::validate_image_size(self.width, self.height)?;
        ensure!(self.x.checked_add(self.width).is_some() && self.y.checked_add(self.height).is_some(), "Preset coordinates overflow");
        Ok(())
    }
}

fn validate_region_presets(presets: &[RegionPreset]) -> Result<()> {
    ensure!(presets.len() <= 16, "At most 16 region presets can be saved");
    for (index, preset) in presets.iter().enumerate() {
        preset.validate()?;
        ensure!(!presets[..index].iter().any(|other| other.name == preset.name), "Preset names must be unique");
    }
    Ok(())
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
            region_presets: Vec::new(),
            jpeg_quality: DEFAULT_JPEG_QUALITY,
            export_format: crate::ImageExportFormat::default(),
            filename_template: String::new(),
            capture_delay_seconds: default_capture_delay(),
        })
    }

    pub fn with_theme(mut self, theme: UiTheme) -> Self {
        self.theme = theme;
        self
    }

    pub fn with_capture_delay(mut self, seconds: u8) -> Result<Self> {
        ensure!((1..=60).contains(&seconds), "Capture delay must be between 1 and 60 seconds");
        self.capture_delay_seconds = seconds;
        Ok(self)
    }

    pub fn with_hotkeys(mut self, hotkeys: CaptureHotkeys) -> Result<Self> {
        self.hotkeys = hotkeys.validated()?;
        Ok(self)
    }

    pub fn with_output_profile(mut self, profile: CaptureOutputProfile) -> Self {
        self.output_profile = profile;
        self
    }

    pub fn with_region_presets(mut self, presets: Vec<RegionPreset>) -> Result<Self> {
        validate_region_presets(&presets)?;
        self.region_presets = presets;
        Ok(self)
    }

    pub fn with_jpeg_quality(mut self, quality: u8) -> Result<Self> {
        ensure!((1..=100).contains(&quality), "JPEG quality must be between 1 and 100");
        self.jpeg_quality = quality;
        Ok(self)
    }

    pub fn with_export_format(mut self, format: crate::ImageExportFormat) -> Self {
        self.export_format = format;
        self
    }

    pub fn with_filename_template(mut self, template: String) -> Result<Self> {
        crate::validate_capture_name_template(&template)?;
        self.filename_template = template.trim().to_owned();
        Ok(self)
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
            .with_output_profile(settings.output_profile)
            .with_region_presets(settings.region_presets)?
            .with_jpeg_quality(settings.jpeg_quality)?
            .with_export_format(settings.export_format)
            .with_filename_template(settings.filename_template)?
            .with_capture_delay(settings.capture_delay_seconds)?)
    }

    pub fn save(&self) -> Result<()> {
        self.save_at(&Self::config_path()?)
    }

    fn save_at(&self, path: &Path) -> Result<()> {
        ensure!((1..=60).contains(&self.capture_delay_seconds), "Capture delay must be between 1 and 60 seconds");
        self.hotkeys.validated()?;
        validate_region_presets(&self.region_presets)?;
        crate::validate_capture_name_template(&self.filename_template)?;
        ensure!((1..=100).contains(&self.jpeg_quality), "JPEG quality must be between 1 and 100");
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
    fn capture_delay_persists_preserves_preferences_and_rejects_invalid_settings() -> Result<()> {
        let root = std::env::temp_dir().join(format!("screenshots-delay-settings-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root)?;
        let path = root.join("settings.json");
        let preset = RegionPreset::new("Small", crate::PixelRegion { x: 1, y: 2, width: 3, height: 4 })?;
        for seconds in [1, 5, 60] {
            CaptureSettings::for_directory(root.clone())?.with_theme(UiTheme::Oldschool)
                .with_output_profile(CaptureOutputProfile::CopyOnly).with_region_presets(vec![preset.clone()])?
                .with_jpeg_quality(73)?.with_export_format(crate::ImageExportFormat::WebP)
                .with_filename_template("Shot-{date}".into())?.with_capture_delay(seconds)?.save_at(&path)?;
            let loaded = CaptureSettings::load_at(&path, root.clone())?;
            assert_eq!(loaded.capture_delay_seconds, seconds);
            assert_eq!(loaded.theme, UiTheme::Oldschool);
            assert_eq!(loaded.output_profile, CaptureOutputProfile::CopyOnly);
            assert_eq!(loaded.region_presets, vec![preset.clone()]);
            assert_eq!(loaded.jpeg_quality, 73);
            assert_eq!(loaded.export_format, crate::ImageExportFormat::WebP);
            assert_eq!(loaded.filename_template, "Shot-{date}");
            loaded.with_theme(UiTheme::Default).save_at(&path)?;
            assert_eq!(CaptureSettings::load_at(&path, root.clone())?.capture_delay_seconds, seconds);
        }
        let before = fs::read(&path)?;
        for seconds in [0, 61, 255] {
            assert!(CaptureSettings::for_directory(root.clone())?.with_capture_delay(seconds).is_err());
            let mut invalid = CaptureSettings::for_directory(root.clone())?;
            invalid.capture_delay_seconds = seconds;
            assert!(invalid.save_at(&path).is_err());
            assert_eq!(fs::read(&path)?, before);
            let mut stored: serde_json::Value = serde_json::from_slice(&before)?;
            stored["capture_delay_seconds"] = seconds.into();
            fs::write(&path, serde_json::to_vec(&stored)?)?;
            assert!(CaptureSettings::load_at(&path, root.clone()).is_err());
            fs::write(&path, &before)?;
        }
        let mut legacy: serde_json::Value = serde_json::from_slice(&before)?;
        legacy.as_object_mut().unwrap().remove("capture_delay_seconds");
        fs::write(&path, serde_json::to_vec(&legacy)?)?;
        assert_eq!(CaptureSettings::load_at(&path, root.clone())?.capture_delay_seconds, 3);
        fs::remove_file(path)?; fs::remove_dir(root)?;
        Ok(())
    }

    #[test]
    fn filename_templates_persist_validate_and_default_for_legacy_settings() -> Result<()> {
        let root = std::env::temp_dir().join(format!("screenshots-name-settings-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root)?;
        let path = root.join("settings.json");
        CaptureSettings::for_directory(root.clone())?.with_jpeg_quality(73)?
            .with_export_format(crate::ImageExportFormat::WebP).with_filename_template(" Shot-{date}_{time} ".into())?.save_at(&path)?;
        let loaded = CaptureSettings::load_at(&path, root.clone())?;
        assert_eq!(loaded.filename_template, "Shot-{date}_{time}");
        assert_eq!(loaded.jpeg_quality, 73);
        assert_eq!(loaded.export_format, crate::ImageExportFormat::WebP);
        loaded.with_theme(UiTheme::Oldschool).save_at(&path)?;
        assert_eq!(CaptureSettings::load_at(&path, root.clone())?.filename_template, "Shot-{date}_{time}");
        let before = fs::read(&path)?;
        let mut invalid = CaptureSettings::load_at(&path, root.clone())?;
        invalid.filename_template = "../outside".into();
        assert!(invalid.save_at(&path).is_err());
        assert_eq!(fs::read(&path)?, before);
        let mut stored: serde_json::Value = serde_json::from_slice(&before)?;
        stored["filename_template"] = "{unknown}".into();
        fs::write(&path, serde_json::to_vec(&stored)?)?;
        assert!(CaptureSettings::load_at(&path, root.clone()).is_err());
        stored.as_object_mut().unwrap().remove("filename_template");
        fs::write(&path, serde_json::to_vec(&stored)?)?;
        assert!(CaptureSettings::load_at(&path, root.clone())?.filename_template.is_empty());
        fs::remove_file(path)?; fs::remove_dir(root)?;
        Ok(())
    }

    #[test]
    fn export_format_round_trips_defaults_and_rejects_unknown_values() -> Result<()> {
        let root = std::env::temp_dir().join(format!("screenshots-format-settings-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root)?;
        let path = root.join("settings.json");
        for format in crate::ImageExportFormat::ALL {
            CaptureSettings::for_directory(root.clone())?.with_jpeg_quality(73)?
                .with_output_profile(CaptureOutputProfile::CopyOnly).with_export_format(format).save_at(&path)?;
            let loaded = CaptureSettings::load_at(&path, root.clone())?;
            assert_eq!(loaded.export_format, format);
            assert_eq!(loaded.jpeg_quality, 73);
            assert_eq!(loaded.output_profile, CaptureOutputProfile::CopyOnly);
            loaded.with_theme(UiTheme::Oldschool).save_at(&path)?;
            assert_eq!(CaptureSettings::load_at(&path, root.clone())?.export_format, format);
            assert_eq!(crate::ImageExportFormat::from_path(Path::new(&format!("image.{}", format.extension()))).unwrap(), format);
        }
        let mut legacy: serde_json::Value = serde_json::from_slice(&fs::read(&path)?)?;
        legacy.as_object_mut().unwrap().remove("export_format");
        fs::write(&path, serde_json::to_vec(&legacy)?)?;
        assert_eq!(CaptureSettings::load_at(&path, root.clone())?.export_format, crate::ImageExportFormat::Png);
        legacy["export_format"] = "gif".into();
        fs::write(&path, serde_json::to_vec(&legacy)?)?;
        assert!(CaptureSettings::load_at(&path, root.clone()).is_err());
        fs::remove_file(path)?; fs::remove_dir(root)?;
        Ok(())
    }

    #[test]
    fn jpeg_quality_persists_rejects_invalid_files_and_defaults_for_older_settings() -> Result<()> {
        let root = std::env::temp_dir().join(format!("screenshots-jpeg-settings-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root)?;
        let config = root.join("settings.json");
        let preset = RegionPreset::new("Small", crate::PixelRegion { x: 0, y: 0, width: 3, height: 2 })?;
        for quality in [1, 73, 100] {
            CaptureSettings::for_directory(root.clone())?.with_theme(UiTheme::Oldschool)
                .with_output_profile(CaptureOutputProfile::CopyOnly).with_region_presets(vec![preset.clone()])?
                .with_jpeg_quality(quality)?.save_at(&config)?;
            let loaded = CaptureSettings::load_at(&config, root.clone())?;
            assert_eq!(loaded.jpeg_quality, quality);
            assert_eq!(loaded.region_presets, [preset.clone()]);
            assert_eq!(loaded.output_profile, CaptureOutputProfile::CopyOnly);
            loaded.with_theme(UiTheme::Default).save_at(&config)?;
            assert_eq!(CaptureSettings::load_at(&config, root.clone())?.jpeg_quality, quality);
        }
        let before = fs::read(&config)?;
        for quality in [0, 101, 255] {
            assert!(CaptureSettings::for_directory(root.clone())?.with_jpeg_quality(quality).is_err());
            let mut invalid = CaptureSettings::load_at(&config, root.clone())?;
            invalid.jpeg_quality = quality;
            assert!(invalid.save_at(&config).is_err());
            assert_eq!(fs::read(&config)?, before);
            let mut stored: serde_json::Value = serde_json::from_slice(&before)?;
            stored["jpeg_quality"] = quality.into();
            fs::write(&config, serde_json::to_vec(&stored)?)?;
            assert!(CaptureSettings::load_at(&config, root.clone()).is_err());
            fs::write(&config, &before)?;
        }
        let mut legacy: serde_json::Value = serde_json::from_slice(&before)?;
        legacy.as_object_mut().unwrap().remove("jpeg_quality");
        fs::write(&config, serde_json::to_vec(&legacy)?)?;
        assert_eq!(CaptureSettings::load_at(&config, root.clone())?.jpeg_quality, 90);
        fs::remove_file(config)?;
        fs::remove_dir(root)?;
        Ok(())
    }

    #[test]
    fn region_presets_round_trip_validate_and_keep_other_settings() -> Result<()> {
        let root = std::env::temp_dir().join(format!("screenshots-preset-settings-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root)?;
        let config = root.join("settings.json");
        let preset = RegionPreset::new(" Small ", crate::PixelRegion { x: 10, y: 20, width: 30, height: 40 })?;
        assert_eq!(preset.name, "Small");
        CaptureSettings::for_directory(root.clone())?.with_output_profile(CaptureOutputProfile::CopyOnly)
            .with_region_presets(vec![preset.clone()])?.save_at(&config)?;
        let loaded = CaptureSettings::load_at(&config, root.clone())?;
        assert_eq!(loaded.region_presets, [preset.clone()]);
        assert_eq!(loaded.output_profile, CaptureOutputProfile::CopyOnly);
        let presets = loaded.region_presets.clone();
        loaded.with_theme(UiTheme::Oldschool).with_output_profile(CaptureOutputProfile::SaveOnly)
            .save_at(&config)?;
        let loaded = CaptureSettings::load_at(&config, root.clone())?;
        assert_eq!(loaded.region_presets, presets);
        assert_eq!(loaded.theme, UiTheme::Oldschool);
        let before = fs::read(&config)?;
        let mut invalid = CaptureSettings::load_at(&config, root.clone())?;
        invalid.region_presets = vec![preset.clone(); 2];
        assert!(invalid.save_at(&config).is_err());
        assert_eq!(fs::read(&config)?, before);
        for name in ["", "\n", "bad\nname", &"x".repeat(65)] {
            assert!(RegionPreset::new(name, preset.area()).is_err());
        }
        for area in [crate::PixelRegion { width: 0, ..preset.area() },
            crate::PixelRegion { x: u32::MAX, ..preset.area() },
            crate::PixelRegion { width: u32::MAX, height: u32::MAX, ..preset.area() }] {
            assert!(RegionPreset::new("Invalid", area).is_err());
        }
        let presets: Vec<_> = (0..16).map(|index| RegionPreset::new(&format!("Region {index}"), preset.area()).unwrap()).collect();
        let mut settings = CaptureSettings::for_directory(root.clone())?.with_region_presets(presets)?;
        settings.save_at(&config)?;
        settings.region_presets.push(preset);
        assert!(settings.save_at(&config).is_err());
        let mut legacy: serde_json::Value = serde_json::from_slice(&before)?;
        legacy["region_presets"][0]["width"] = 0.into();
        fs::write(&config, serde_json::to_vec(&legacy)?)?;
        assert!(CaptureSettings::load_at(&config, root.clone()).is_err());
        legacy.as_object_mut().unwrap().remove("region_presets");
        fs::write(&config, serde_json::to_vec(&legacy)?)?;
        assert!(CaptureSettings::load_at(&config, root.clone())?.region_presets.is_empty());
        fs::remove_file(config)?;
        fs::remove_dir(root)?;
        Ok(())
    }
    #[test]
    fn legacy_hotkeys_keep_bindings_and_active_window_starts_disabled() -> Result<()> {
        let legacy: CaptureHotkeys = serde_json::from_str(r#"["Alt+Q","","Ctrl+F9"]"#)?;
        assert_eq!(legacy.0, ["Alt+Q", "", "Ctrl+F9", "", "", ""]);
        let four: CaptureHotkeys = serde_json::from_str(r#"["Alt+Q","","Ctrl+F9","Alt+W"]"#)?;
        assert_eq!(four.0, ["Alt+Q", "", "Ctrl+F9", "Alt+W", "", ""]);
        let five: CaptureHotkeys = serde_json::from_str(r#"["Alt+Q","","Ctrl+F9","Alt+W","Alt+R"]"#)?;
        assert_eq!(five.0, ["Alt+Q", "", "Ctrl+F9", "Alt+W", "Alt+R", ""]);
        assert!(CaptureHotkeys::default().0[4].is_empty());
        assert!(CaptureHotkeys::default().0[3].is_empty());
        assert!(CaptureHotkeys::default().0[5].is_empty());
        let mut current = legacy.clone();
        current.0[3] = "Ctrl+Alt+W".into();
        current.0[4] = "Alt+R".into();
        current.0[5] = "Alt+D".into();
        assert_eq!(
            serde_json::from_slice::<CaptureHotkeys>(&serde_json::to_vec(&current)?)?,
            current
        );
        current.0[5] = current.0[0].clone();
        assert!(current.validated().is_err());
        current.0[5] = "Alt+D".into();
        current.0[3] = "alt+q".into();
        assert!(current.validated().is_err());
        assert!(serde_json::from_str::<CaptureHotkeys>(r#"["Q","R"]"#).is_err());
        Ok(())
    }
    #[test]
    fn output_profiles_round_trip_preserve_other_settings_and_reject_unknown_values() -> Result<()>
    {
        let dir = std::env::temp_dir().join(format!(
            "screenshots-output-profiles-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir)?;
        let path = dir.join("settings.json");
        let hotkeys = CaptureHotkeys([
            "Alt+Q".into(),
            "".into(),
            "Ctrl+F9".into(),
            "".into(),
            "".into(),
            String::new(),
        ]);
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
        let keys = CaptureHotkeys([
            " shift + ctrl + q ".into(),
            "PrintScreen".into(),
            "".into(),
            "".into(),
            "".into(),
            String::new(),
        ])
        .validated()?;
        assert_eq!(keys.0, ["Ctrl+Shift+Q", "PrintScreen", "", "", "", ""]);
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
                CaptureHotkeys([invalid.into(), "".into(), "".into(), "".into(), "".into(), String::new()])
                    .validated()
                    .is_err(),
                "{invalid}"
            );
        }
        assert!(
            CaptureHotkeys([
                "ctrl+shift+q".into(),
                "Shift+Ctrl+Q".into(),
                "".into(),
                "".into(),
                "".into(),
                String::new(),
            ])
            .validated()
            .is_err()
        );
        assert!(
            CaptureHotkeys([" ".repeat(65), "".into(), "".into(), "".into(), "".into(), String::new()])
                .validated()
                .is_err()
        );
        assert!(
            CaptureHotkeys(["".into(), "".into(), "".into(), "".into(), "".into(), String::new()])
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
        let hotkeys = CaptureHotkeys([
            "Ctrl+Alt+Q".into(),
            "F12".into(),
            "".into(),
            "".into(),
            "".into(),
            "Alt+D".into(),
        ]);
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
