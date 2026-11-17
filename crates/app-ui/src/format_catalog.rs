//! Worker-side preference context. Tests never resolve a user Known Folder.
use anyhow::Result;
use app_core::{CaptureFormatPresence, CaptureFormatSettings, CaptureSettings};
use std::path::PathBuf;

#[derive(Clone)]
pub(super) struct FormatCatalog {
    path: Option<PathBuf>,
    #[cfg(test)]
    _owned: Option<std::sync::Arc<PrivateDirectory>>,
}

impl Default for FormatCatalog {
    fn default() -> Self {
        #[cfg(test)]
        {
            Self::isolated()
        }
        #[cfg(not(test))]
        {
            Self { path: None }
        }
    }
}

impl FormatCatalog {
    /// Resolve only on the blocking worker, never while rendering or initializing UI.
    pub(super) fn resolve(&self) -> Result<PathBuf> {
        if let Some(path) = &self.path {
            return Ok(path.clone());
        }
        #[cfg(test)]
        {
            anyhow::bail!("UI tests require an explicit private format catalog")
        }
        #[cfg(not(test))]
        {
            CaptureFormatSettings::config_path()
        }
    }

    pub(super) fn load(&self) -> Result<CaptureFormatSettings> {
        CaptureFormatSettings::load_at(&self.resolve()?)
    }
    pub(super) fn observe(&self) -> Result<CaptureFormatPresence> {
        CaptureFormatSettings::observe_at(&self.resolve()?)
    }
    pub(super) fn persist_main(&self, settings: &CaptureSettings) -> Result<()> {
        #[cfg(test)]
        {
            settings.save_at(&self.resolve()?.with_file_name("settings.json"))
        }
        #[cfg(not(test))]
        {
            settings.save()
        }
    }

    pub(super) fn restore(
        &self,
        desired: &CaptureFormatSettings,
        prior: Option<&CaptureFormatPresence>,
    ) -> Option<String> {
        let result = self.resolve().and_then(|path| desired.save_at(&path));
        result
            .err()
            .map(|error| format!("{}: {error:#}", self.failed_restore_state(desired, prior)))
    }

    fn failed_restore_state(
        &self,
        desired: &CaptureFormatSettings,
        prior: Option<&CaptureFormatPresence>,
    ) -> &'static str {
        match self.observe() {
            Ok(CaptureFormatPresence::Absent) => {
                "Format catalog is absent; built-in PNG capture and Preserve Library defaults are active"
            }
            Ok(CaptureFormatPresence::Present(actual)) if &actual == desired => {
                "Imported format preferences are present; their storage durability was not confirmed"
            }
            Ok(CaptureFormatPresence::Present(actual))
                if prior.is_some_and(|prior| matches!(prior, CaptureFormatPresence::Present(previous) if previous == &actual)) =>
            {
                "Prior format preferences remain active"
            }
            Ok(CaptureFormatPresence::Present(_)) => {
                "Different format preferences are currently active; inspect formats show"
            }
            Err(_) => "Format preference state could not be inspected; inspect formats show",
        }
    }

    #[cfg(test)]
    fn isolated() -> Self {
        let name = app_core::capture_file_name("UiFormats")
            .expect("fixed test filename")
            .unwrap();
        let root = std::env::temp_dir().join(format!("screenshots-{name}"));
        std::fs::create_dir(&root).expect("own a new private UI configuration directory");
        Self {
            path: Some(root.join("capture-format.json")),
            _owned: Some(std::sync::Arc::new(PrivateDirectory(root))),
        }
    }
    #[cfg(test)]
    pub(super) fn at(path: PathBuf) -> Self {
        assert!(path.is_absolute());
        Self {
            path: Some(path),
            _owned: None,
        }
    }
}

#[cfg(test)]
struct PrivateDirectory(PathBuf);
#[cfg(test)]
impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        for name in ["capture-format.json", "settings.json"] {
            let _ = std::fs::remove_file(self.0.join(name));
        }
        let _ = std::fs::remove_dir(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_catalog_is_private_for_every_test_constructor_and_missing_is_absent() -> Result<()> {
        let catalog = FormatCatalog::default();
        let path = catalog.resolve()?;
        assert!(path.is_absolute() && path.starts_with(std::env::temp_dir()));
        assert!(!path.exists());
        assert_eq!(catalog.observe()?, CaptureFormatPresence::Absent);
        let defaults = CaptureFormatSettings::default();
        assert_eq!(catalog.load()?, defaults);
        let state = catalog.failed_restore_state(&defaults, Some(&CaptureFormatPresence::Absent));
        assert!(state.contains("absent") && !state.contains("are present"));
        assert!(!path.exists());
        let forbidden = FormatCatalog {
            path: None,
            _owned: None,
        };
        assert!(forbidden.resolve().is_err());
        assert!(forbidden.load().is_err());
        Ok(())
    }

    #[test]
    fn private_catalog_lives_through_worker_clones_and_never_persists_main_to_profile() -> Result<()>
    {
        let catalog = FormatCatalog::default();
        let path = catalog.resolve()?;
        let clone = catalog.clone();
        let desired =
            CaptureFormatSettings::for_policy(app_core::CaptureFormatPolicy::WebpLossless);
        assert!(
            catalog
                .restore(&desired, Some(&CaptureFormatPresence::Absent))
                .is_none()
        );
        assert_eq!(catalog.observe()?, CaptureFormatPresence::Present(desired));
        let settings = CaptureSettings::for_directory(path.parent().unwrap().to_owned())?;
        catalog.persist_main(&settings)?;
        assert!(path.with_file_name("settings.json").is_file());
        drop(catalog);
        assert!(path.is_file());
        drop(clone);
        assert!(!path.exists() && !path.parent().unwrap().exists());
        Ok(())
    }
}
