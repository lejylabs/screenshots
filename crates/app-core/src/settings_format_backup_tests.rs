use super::*;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "screenshots-format-backup-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn combined_backup_validates_both_sections_before_returning_and_preserves_legacy_absence()
-> Result<()> {
    let f = Fixture::new();
    let settings = CaptureSettings::for_directory(f.0.clone())?.with_jpeg_quality(73)?;
    let mut formats = crate::CaptureFormatSettings::for_policy(crate::CaptureFormatPolicy::Jpeg {
        quality: crate::CaptureImageQuality::new(81)?,
    });
    formats.library_export = crate::LibraryExportPolicy::Convert {
        encoding: crate::CaptureFormatPolicy::WebpLossless,
    };
    let backup = f.0.join("backup.json");
    settings.export_at_with_formats(&backup, Some(formats.clone()))?;
    let bytes = fs::read(&backup)?;
    let (loaded, kept, loaded_formats) =
        CaptureSettings::import_with_formats(&backup, f.0.clone())?;
    assert!(!kept);
    assert_eq!(loaded.jpeg_quality, 73);
    assert_eq!(loaded_formats, Some(formats));
    assert_eq!(fs::read(&backup)?, bytes);
    assert_eq!(fs::read_dir(&f.0)?.count(), 1);
    let mut invalid: serde_json::Value = serde_json::from_slice(&bytes)?;
    invalid["capture_formats"]["capture"]["quality"] = 0.into();
    fs::write(&backup, serde_json::to_vec(&invalid)?)?;
    assert!(CaptureSettings::import_with_formats(&backup, f.0.clone()).is_err());
    assert_eq!(fs::read_dir(&f.0)?.count(), 1);
    invalid["capture_formats"]["capture"]["quality"] = 81.into();
    invalid["version"] = 2.into();
    fs::write(&backup, serde_json::to_vec(&invalid)?)?;
    assert!(CaptureSettings::import_with_formats(&backup, f.0.clone()).is_err());
    let legacy = f.0.join("legacy.json");
    settings.export_at(&legacy)?;
    assert!(
        CaptureSettings::import_with_formats(&legacy, f.0.clone())?
            .2
            .is_none()
    );
    Ok(())
}

#[test]
fn combined_backup_rejects_duplicate_format_fields_and_growing_or_oversized_sources() -> Result<()>
{
    let f = Fixture::new();
    let settings = CaptureSettings::for_directory(f.0.clone())?;
    let backup = f.0.join("backup.json");
    let mut bytes = serde_json::to_vec(&settings)?;
    bytes.pop();
    bytes.extend_from_slice(
        br#", "capture_formats":{"version":1},"capture_formats":{"version":1}}"#,
    );
    fs::write(&backup, &bytes)?;
    assert!(CaptureSettings::import_with_formats(&backup, f.0.clone()).is_err());
    let file = fs::OpenOptions::new().write(true).open(&backup)?;
    file.set_len(65537)?;
    assert!(CaptureSettings::import_with_formats(&backup, f.0.clone()).is_err());
    assert_eq!(fs::read_dir(&f.0)?.count(), 1);
    Ok(())
}
