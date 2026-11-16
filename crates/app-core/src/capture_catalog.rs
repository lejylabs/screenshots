//! Explicit capture catalog. UI preference saves never reconstruct this file.
use crate::{
    CaptureBackend, CaptureOptions, CapturedImage, DisplayInfo, PixelRegion, RememberedRegion,
    XcapBackend,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CaptureMonitorBinding {
    Physical { key: String },
    Connector { key: String },
}
impl CaptureMonitorBinding {
    pub(crate) fn validate(&self) -> Result<()> {
        let key = match self {
            Self::Physical { key } | Self::Connector { key } => key,
        };
        ensure!(
            !key.is_empty() && key.len() <= 1024 && !key.chars().any(char::is_control),
            "Monitor binding key must contain 1–1024 bytes without controls"
        );
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureMonitorIdentity {
    pub display_id: String,
    pub binding: CaptureMonitorBinding,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureTemplateGeometry {
    pub width: u32,
    pub height: u32,
    pub scale_factor: f32,
    pub rotation: f32,
}
impl CaptureTemplateGeometry {
    pub fn from_display(display: &DisplayInfo) -> Self {
        Self {
            width: display.width,
            height: display.height,
            scale_factor: display.scale_factor,
            rotation: display.rotation,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureRegionTemplate {
    pub name: String,
    pub monitor: CaptureMonitorBinding,
    pub geometry: CaptureTemplateGeometry,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub profile: Option<PathBuf>,
    #[serde(default)]
    pub hotkey: Option<String>,
}
impl CaptureRegionTemplate {
    pub fn area(&self) -> PixelRegion {
        PixelRegion {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
        }
    }
    pub(crate) fn validate(&mut self) -> Result<()> {
        ensure!(
            !self.name.is_empty()
                && self.name.len() <= 64
                && self.name.trim() == self.name
                && !self.name.chars().any(char::is_control),
            "Template name must contain 1–64 bytes without surrounding whitespace or controls"
        );
        self.monitor.validate()?;
        crate::validate_image_size(self.geometry.width, self.geometry.height)?;
        ensure!(
            self.geometry.scale_factor.is_finite()
                && self.geometry.scale_factor > 0.0
                && self.geometry.rotation.is_finite(),
            "Invalid template display geometry"
        );
        crate::validate_image_size(self.width, self.height)?;
        ensure!(
            self.x
                .checked_add(self.width)
                .is_some_and(|end| end <= self.geometry.width)
                && self
                    .y
                    .checked_add(self.height)
                    .is_some_and(|end| end <= self.geometry.height),
            "Template rectangle exceeds its display geometry"
        );
        if let Some(profile) = &self.profile {
            validate_path(profile)?;
        }
        if let Some(hotkey) = &mut self.hotkey {
            ensure!(
                self.profile.is_some(),
                "A template hotkey requires a task profile"
            );
            let parsed = crate::CaptureHotkeys([
                hotkey.clone(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            ])
            .validated()?;
            ensure!(
                !parsed.0[0].is_empty(),
                "Template hotkey must be nonempty or omitted"
            );
            *hotkey = parsed.0[0].clone();
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureCatalog {
    pub version: u32,
    #[serde(default)]
    pub defaults: CaptureOptions,
    #[serde(default)]
    pub templates: Vec<CaptureRegionTemplate>,
}
fn validate_path(path: &Path) -> Result<()> {
    ensure!(
        !path.as_os_str().is_empty() && path.as_os_str().as_encoded_bytes().len() <= 4096,
        "Capture catalog paths must contain 1–4096 native bytes"
    );
    Ok(())
}
impl CaptureCatalog {
    pub fn read(path: &Path) -> Result<Self> {
        validate_path(path)?;
        let canonical = std::fs::canonicalize(path).context("Capture catalog is unavailable")?;
        validate_path(&canonical)?;
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
        }
        let file = options
            .open(&canonical)
            .context("Cannot open capture catalog")?;
        let metadata = file.metadata()?;
        ensure!(
            metadata.is_file() && metadata.len() <= 65536,
            "Capture catalog must be a regular file within 64 KiB"
        );
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(65537).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 65536, "Capture catalog exceeds 64 KiB");
        let shape: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Invalid capture catalog JSON"))?;
        ensure!(
            shape.is_object()
                && shape
                    .get("defaults")
                    .is_none_or(serde_json::Value::is_object)
                && shape
                    .get("templates")
                    .is_none_or(
                        |items| items
                            .as_array()
                            .is_some_and(|items| items.iter().all(|item| item.is_object()
                                && item
                                    .get("monitor")
                                    .is_some_and(serde_json::Value::is_object)
                                && item
                                    .get("geometry")
                                    .is_some_and(serde_json::Value::is_object)))
                    ),
            "Capture catalog and template records must be JSON objects"
        );
        let mut catalog: Self = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Invalid capture catalog schema"))?;
        catalog.validate()?;
        let base = canonical
            .parent()
            .context("Capture catalog has no parent")?
            .to_path_buf();
        for template in &mut catalog.templates {
            if let Some(profile) = &mut template.profile {
                let resolved = std::fs::canonicalize(if profile.is_absolute() {
                    profile.clone()
                } else {
                    base.join(&*profile)
                })
                .context("Template task profile is unavailable")?;
                validate_path(&resolved)?;
                ensure!(
                    resolved.metadata()?.is_file(),
                    "Template task profile must be a regular file"
                );
                *profile = resolved;
            }
        }
        Ok(catalog)
    }
    pub fn validate(&mut self) -> Result<()> {
        ensure!(self.version == 1, "Capture catalog version must be 1");
        self.defaults.validate()?;
        ensure!(
            self.templates.len() <= 16,
            "Capture catalog admits at most 16 templates"
        );
        let mut names = std::collections::HashSet::new();
        let mut hotkeys = std::collections::HashSet::new();
        for template in &mut self.templates {
            template.validate()?;
            ensure!(
                names.insert(template.name.clone()),
                "Duplicate capture template name"
            );
            if let Some(hotkey) = &template.hotkey {
                ensure!(hotkeys.insert(hotkey.clone()), "Duplicate template hotkey");
            }
        }
        Ok(())
    }
    /// Catalog edits write a new destination, retaining the original catalog on every error.
    pub fn write_new(&self, path: &Path) -> Result<()> {
        validate_path(path)?;
        let mut catalog = self.clone();
        catalog.validate()?;
        let bytes = serde_json::to_vec_pretty(&catalog)?;
        ensure!(bytes.len() <= 65536, "Capture catalog exceeds 64 KiB");
        crate::capture::write_output(path, |file| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            }
            file.write_all(&bytes)?;
            Ok(())
        })?;
        Ok(())
    }
}

pub fn capture_monitor_identities(displays: &[DisplayInfo]) -> Result<Vec<CaptureMonitorIdentity>> {
    ensure!(
        (1..=32).contains(&displays.len()),
        "Monitor identity lookup requires 1–32 displays"
    );
    crate::capture_native::display_identities(displays)
}

#[derive(Clone, Debug)]
pub struct PreparedRegionTemplate {
    template: CaptureRegionTemplate,
    options: CaptureOptions,
}
impl PreparedRegionTemplate {
    pub(crate) fn from_native_parts(
        template: CaptureRegionTemplate,
        options: CaptureOptions,
    ) -> Self {
        Self { template, options }
    }
    pub fn read(catalog: &Path, name: &str) -> Result<Self> {
        ensure!(
            !name.is_empty() && name.len() <= 64,
            "Template name must contain 1–64 bytes"
        );
        let catalog = CaptureCatalog::read(catalog)?;
        let template = catalog
            .templates
            .into_iter()
            .find(|template| template.name == name)
            .context("Capture template was not found")?;
        Ok(Self {
            template,
            options: catalog.defaults,
        })
    }
    pub fn profile(&self) -> Option<&Path> {
        self.template.profile.as_deref()
    }
    pub fn options(&self) -> CaptureOptions {
        self.options
    }
    fn resolve(
        &self,
        displays: &[DisplayInfo],
        identities: &[CaptureMonitorIdentity],
    ) -> Result<(DisplayInfo, RememberedRegion)> {
        let mut matches = identities
            .iter()
            .filter(|identity| identity.binding == self.template.monitor);
        let identity = matches
            .next()
            .context("Bound monitor is unavailable; explicit rebinding is required")?;
        ensure!(
            matches.next().is_none(),
            "Bound monitor identity is ambiguous"
        );
        let mut matches = displays
            .iter()
            .filter(|display| display.id == identity.display_id);
        let display = matches.next().context("Bound display is unavailable")?;
        ensure!(matches.next().is_none(), "Bound display ID is ambiguous");
        ensure!(
            CaptureTemplateGeometry::from_display(display) == self.template.geometry,
            "Bound monitor geometry changed; explicit rebinding is required"
        );
        ensure!(
            display.scale_factor == 1.0 && display.rotation == 0.0,
            "Template capture currently requires unrotated 100% physical-pixel coordinates"
        );
        Ok((
            display.clone(),
            RememberedRegion::from_display(display, self.template.area())?,
        ))
    }
    pub fn capture_with_cancel(
        &self,
        output_dir: impl Into<PathBuf>,
        check: &impl Fn() -> Result<()>,
    ) -> Result<CapturedImage> {
        self.options.validate()?;
        crate::capture_native::validate_options(&self.options, false)?;
        check()?;
        crate::capture_native::validate_template_session()?;
        let deadline = Instant::now() + Duration::from_millis(self.options.delay_ms);
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            check()?;
            std::thread::sleep(remaining.min(Duration::from_millis(20)));
        }
        check()?;
        let mut options = self.options;
        options.delay_ms = 0;
        let output_dir = output_dir.into();
        if crate::native_capture_job::executor().is_some() {
            let mut template = self.template.clone();
            template.profile = None;
            template.hotkey = None;
            let backend = XcapBackend::new(output_dir).with_options(options)?;
            return backend
                .native_frames(
                    crate::NativeCaptureCall::Template { template, options },
                    &|| check().map_err(|_| crate::CaptureError::NativeCanceled),
                )?
                .pop()
                .context("Template capture returned no image");
        }
        self.capture_inline(output_dir, std::process::id(), check)
    }
    pub(crate) fn capture_inline(
        &self,
        output_dir: impl Into<PathBuf>,
        originating_pid: u32,
        check: &impl Fn() -> Result<()>,
    ) -> Result<CapturedImage> {
        self.options.validate()?;
        crate::capture_native::validate_options(&self.options, false)?;
        crate::capture_native::validate_template_session()?;
        check()?;
        let mut options = self.options;
        options.delay_ms = 0;
        let backend = XcapBackend::new(output_dir)
            .with_originating_process(originating_pid)
            .with_options(options)?;
        let displays = backend.list_displays()?;
        let identities = capture_monitor_identities(&displays)?;
        let (display, remembered) = self.resolve(&displays, &identities)?;
        check()?;
        let frame = backend
            .capture_batch(vec![crate::CaptureRequest::ExactRegion(remembered)])?
            .pop()
            .context("Template capture returned no image")?;
        check()?;
        let after = backend.list_displays()?;
        let after_ids = capture_monitor_identities(&after)?;
        let (current, _) = self.resolve(&after, &after_ids)?;
        ensure!(
            current.id == display.id && current.x == display.x && current.y == display.y,
            "Bound monitor moved or changed during capture"
        );
        ensure!(
            frame.display_id == display.id
                && frame.window_id.is_none()
                && frame.pixels.dimensions() == (self.template.width, self.template.height),
            "Template capture returned unexpected pixels"
        );
        check()?;
        Ok(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn display(id: &str, x: i32) -> DisplayInfo {
        DisplayInfo {
            id: id.into(),
            name: "not an identity".into(),
            x,
            y: 0,
            width: 100,
            height: 80,
            scale_factor: 1.0,
            rotation: 0.0,
            is_primary: false,
        }
    }
    fn template() -> CaptureRegionTemplate {
        CaptureRegionTemplate {
            name: "crop".into(),
            monitor: CaptureMonitorBinding::Physical {
                key: "fixture:hardware-id".into(),
            },
            geometry: CaptureTemplateGeometry::from_display(&display("old", 0)),
            x: 1,
            y: 2,
            width: 30,
            height: 20,
            profile: None,
            hotkey: None,
        }
    }
    #[test]
    fn template_rebinds_session_ids_and_positions_only_for_the_same_unambiguous_identity() {
        let prepared = PreparedRegionTemplate {
            template: template(),
            options: CaptureOptions::default(),
        };
        let mut displays = vec![display("new", -200)];
        let identities = vec![CaptureMonitorIdentity {
            display_id: "new".into(),
            binding: prepared.template.monitor.clone(),
        }];
        assert_eq!(prepared.resolve(&displays, &identities).unwrap().0.x, -200);
        assert!(prepared.resolve(&displays, &[]).is_err());
        assert!(
            prepared
                .resolve(&displays, &[identities[0].clone(), identities[0].clone()])
                .is_err()
        );
        displays[0].width = 101;
        assert!(prepared.resolve(&displays, &identities).is_err());
        let replacement = [CaptureMonitorIdentity {
            display_id: "new".into(),
            binding: CaptureMonitorBinding::Physical {
                key: "other hardware".into(),
            },
        }];
        assert!(
            prepared
                .resolve(&[display("new", 0)], &replacement)
                .is_err()
        );
    }
    #[test]
    fn catalog_checks_names_geometry_hotkeys_and_strict_schema_without_native_access() {
        let root =
            std::env::temp_dir().join(format!("screenshots-catalog-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("catalog.json");
        let catalog = CaptureCatalog {
            version: 1,
            defaults: CaptureOptions {
                delay_ms: 3600000,
                ..Default::default()
            },
            templates: vec![template()],
        };
        catalog.write_new(&path).unwrap();
        assert_eq!(
            PreparedRegionTemplate::read(&path, "crop")
                .unwrap()
                .options()
                .delay_ms,
            3600000
        );
        assert!(catalog.write_new(&path).is_err());
        for text in [
            "[]",
            r#"{"version":1,"version":1}"#,
            r#"{"version":1,"defaults":[]}"#,
            r#"{"version":1,"templates":[[]]}"#,
            r#"{"version":1,"extra":true}"#,
        ] {
            std::fs::write(&path, text).unwrap();
            assert!(CaptureCatalog::read(&path).is_err(), "{text}");
        }
        let mut invalid = catalog;
        invalid.templates.push(template());
        assert!(invalid.validate().is_err());
        invalid.templates.pop();
        invalid.templates[0].width = 100;
        assert!(invalid.validate().is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn catalog_snapshots_canonical_profiles_and_cancels_before_delay_or_native_access() {
        let root = std::env::temp_dir().join(format!(
            "screenshots-catalog-profile-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("task.json"), b"{}").unwrap();
        let mut template = template();
        template.profile = Some("task.json".into());
        template.hotkey = Some("shift+ctrl+f9".into());
        let catalog = CaptureCatalog {
            version: 1,
            defaults: CaptureOptions {
                delay_ms: 3600000,
                ..Default::default()
            },
            templates: vec![template],
        };
        let path = root.join("catalog.json");
        catalog.write_new(&path).unwrap();
        let loaded = CaptureCatalog::read(&path).unwrap();
        assert_eq!(loaded.templates[0].hotkey.as_deref(), Some("Ctrl+Shift+F9"));
        let prepared = PreparedRegionTemplate::read(&path, "crop").unwrap();
        assert_eq!(
            prepared.profile(),
            Some(root.join("task.json").canonicalize().unwrap().as_path())
        );
        let started = Instant::now();
        assert!(
            prepared
                .capture_with_cancel(PathBuf::new(), &|| anyhow::bail!("fixture cancel"))
                .is_err()
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn catalog_enforces_template_and_readable_byte_boundaries() {
        let mut catalog = CaptureCatalog {
            version: 1,
            ..Default::default()
        };
        for index in 0..16 {
            let mut value = template();
            value.name = format!("region-{index}");
            catalog.templates.push(value);
        }
        catalog.validate().unwrap();
        let mut extra = template();
        extra.name = "seventeenth".into();
        catalog.templates.push(extra);
        assert!(catalog.validate().is_err());
        let root = std::env::temp_dir().join(format!(
            "screenshots-catalog-bound-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("catalog.json");
        let mut bytes = br#"{"version":1}"#.to_vec();
        bytes.resize(65536, b' ');
        std::fs::write(&path, &bytes).unwrap();
        CaptureCatalog::read(&path).unwrap();
        bytes.push(b' ');
        std::fs::write(&path, &bytes).unwrap();
        assert!(CaptureCatalog::read(&path).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let fifo = root.join("fifo");
            let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            let started = Instant::now();
            assert!(CaptureCatalog::read(&fifo).is_err());
            assert!(started.elapsed() < Duration::from_secs(1));
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
