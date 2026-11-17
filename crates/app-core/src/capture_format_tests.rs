use super::*;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("screenshots-format-{}", uuid::Uuid::new_v4()));
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
fn policy_is_explicit_and_rejects_ignored_quality_or_unknown_fields() -> Result<()> {
    for value in [
        r#"{"format":"png","quality":90}"#,
        r#"{"format":"png","quality":null}"#,
        r#"{"format":"png","unknown":true}"#,
        r#"{"format":"webp_lossless","quality":90}"#,
        r#"{"format":"webp_lossless","unknown":true}"#,
        r#"{"format":"jpeg"}"#,
        r#"{"format":"jpeg","quality":0}"#,
        r#"{"format":"jpeg","quality":101}"#,
        r#"{"format":"webp_lossy","quality":1.5}"#,
        r#"{"format":"jpeg","quality":10,"quality":20}"#,
        r#"["png"]"#,
        r#"["jpeg",90]"#,
        r#"["webp_lossless"]"#,
        r#"["webp_lossy",90]"#,
    ] {
        assert!(
            serde_json::from_str::<CaptureFormatPolicy>(value).is_err(),
            "{value}"
        );
    }
    for value in [
        r#"{"mode":"preserve","encoding":{"format":"png"}}"#,
        r#"{"mode":"preserve","unknown":true}"#,
        r#"{"mode":"convert","encoding":{"format":"png","quality":90}}"#,
        r#"["preserve"]"#,
        r#"["convert",["jpeg",90]]"#,
        r#"{"mode":"convert","encoding":["jpeg",90]}"#,
    ] {
        assert!(
            serde_json::from_str::<LibraryExportPolicy>(value).is_err(),
            "{value}"
        );
    }
    for value in [
        r#"[1]"#,
        r#"[1,["jpeg",90],["preserve"]]"#,
        r#"{"version":1,"capture":["png"]}"#,
        r#"{"version":1,"library_export":["preserve"]}"#,
        r#"{"version":1,"version":1}"#,
        r#"{"version":1,"capture":{"format":"png"},"capture":{"format":"png"}}"#,
        r#"{"version":1,"unknown":true}"#,
    ] {
        assert!(
            serde_json::from_str::<CaptureFormatSettings>(value).is_err(),
            "{value}"
        );
    }
    assert_eq!(
        serde_json::from_str::<CaptureFormatSettings>(r#"{"version":1}"#)?,
        CaptureFormatSettings::default()
    );
    for quality in [1, 90, 100] {
        let policy = CaptureFormatPolicy::Jpeg {
            quality: CaptureImageQuality::new(quality)?,
        };
        assert_eq!(
            serde_json::from_slice::<CaptureFormatPolicy>(&serde_json::to_vec(&policy)?)?,
            policy
        );
        assert!(policy.is_lossy());
        assert_eq!(policy.mime_type(), "image/jpeg");
        assert_eq!(
            policy.destination(Path::new("/tmp/original.png")),
            Path::new("/tmp/original.jpg")
        );
        assert!(
            policy
                .validate_destination(Path::new("/tmp/UPPER.JPEG"))
                .is_ok()
        );
        assert!(
            policy
                .validate_destination(Path::new("/tmp/incorrect.png"))
                .is_err()
        );
    }
    assert!(!CaptureFormatPolicy::WebpLossless.is_lossy());
    assert!(
        CaptureFormatPolicy::WebpLossy {
            quality: CaptureImageQuality::new(100)?
        }
        .is_lossy()
    );
    assert!(
        CaptureFormatPolicy::Jpeg {
            quality: CaptureImageQuality::new(90)?
        }
        .validate_dimensions(65536, 1)
        .is_err()
    );
    assert!(
        CaptureFormatPolicy::WebpLossless
            .validate_dimensions(16385, 1)
            .is_err()
    );
    assert!(
        CaptureFormatPolicy::WebpLossy {
            quality: CaptureImageQuality::new(90)?
        }
        .validate_dimensions(16384, 1)
        .is_err()
    );
    Ok(())
}

#[test]
fn growth_between_catalog_admission_steps_cannot_hide_trailing_bytes() -> Result<()> {
    let fixture = Fixture::new();
    let path = fixture.0.join("growing-catalog.json");
    let original = serde_json::to_vec(&CaptureFormatSettings::default())?;
    for required_backup in [false, true] {
        for oversized in [false, true] {
            fs::write(&path, &original)?;
            assert_eq!(
                CaptureFormatSettings::load_at(&path)?,
                CaptureFormatSettings::default()
            );
            let changed = path.clone();
            let suffix = if oversized {
                vec![b'!'; MAX_SETTINGS_BYTES as usize + 1]
            } else {
                b"not-json".to_vec()
            };
            let expected_len = (original.len() + suffix.len()) as u64;
            AFTER_CATALOG_METADATA.with(|hook| {
                hook.set(Some(Box::new(move || {
                    OpenOptions::new()
                        .append(true)
                        .open(changed)
                        .unwrap()
                        .write_all(&suffix)
                        .unwrap();
                })))
            });
            let result = if required_backup {
                CaptureFormatSettings::import(&path)
            } else {
                CaptureFormatSettings::load_at(&path)
            };
            assert!(
                result.is_err(),
                "changed source was admitted as its earlier valid prefix"
            );
            assert_eq!(fs::metadata(&path)?.len(), expected_len);
            assert!(AFTER_CATALOG_METADATA.with(|hook| hook.take().is_none()));
        }
    }
    Ok(())
}

#[test]
fn settings_default_persist_and_backup_without_affecting_ui_settings() -> Result<()> {
    let f = Fixture::new();
    let path = f.0.join("capture-format.json");
    assert_eq!(
        CaptureFormatSettings::load_at(&path)?,
        CaptureFormatSettings::default()
    );
    assert!(!path.exists());
    fs::write(f.0.join("settings.json"), b"legacy UI settings sentinel")?;
    let mut settings = CaptureFormatSettings::for_policy(CaptureFormatPolicy::WebpLossless);
    settings.library_export = LibraryExportPolicy::Convert {
        encoding: CaptureFormatPolicy::Jpeg {
            quality: CaptureImageQuality::new(73)?,
        },
    };
    settings.save_at(&path)?;
    assert_eq!(CaptureFormatSettings::load_at(&path)?, settings);
    assert_eq!(
        fs::read(f.0.join("settings.json"))?,
        b"legacy UI settings sentinel"
    );
    let backup = f.0.join("backup.json");
    settings.export(&backup)?;
    assert_eq!(CaptureFormatSettings::import(&backup)?, settings);
    assert!(settings.export(&backup).is_err());
    assert!(CaptureFormatSettings::import(&f.0.join("missing.json")).is_err());
    fs::write(&path, br#"{"version":1,"capture":{"format":"png"}}"#)?;
    assert_eq!(
        CaptureFormatSettings::load_at(&path)?.library_export,
        LibraryExportPolicy::Preserve
    );
    for value in [
        r#"{"version":2}"#,
        r#"{"version":1,"capture":{"format":"jpeg","quality":101}}"#,
        r#"{"version":1,"capture":{"format":"png"},"unknown":1}"#,
        r#"{"version":1,"version":1}"#,
    ] {
        fs::write(&path, value)?;
        assert!(CaptureFormatSettings::load_at(&path).is_err());
    }
    fs::write(&path, vec![b' '; MAX_SETTINGS_BYTES as usize + 1])?;
    assert!(CaptureFormatSettings::load_at(&path).is_err());
    assert!(CaptureFormatSettings::load_at(&f.0).is_err());
    assert_eq!(
        fs::read_dir(&f.0)?
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|e| e == "part"))
            .count(),
        0
    );
    Ok(())
}

#[test]
fn encoders_publish_actual_kind_and_lossless_channels() -> Result<()> {
    let alpha = RgbaImage::from_fn(7, 3, |x, y| {
        image::Rgba([(x * 23) as u8, (y * 47) as u8, 71, (x * 37) as u8])
    });
    for policy in [CaptureFormatPolicy::Png, CaptureFormatPolicy::WebpLossless] {
        let mut bytes = Vec::new();
        policy.encode_to(&alpha, &mut bytes, &|| false)?;
        assert_eq!(
            image::guess_format(&bytes)?,
            if policy == CaptureFormatPolicy::Png {
                image::ImageFormat::Png
            } else {
                image::ImageFormat::WebP
            }
        );
        assert_eq!(image::load_from_memory(&bytes)?.into_rgba8(), alpha);
    }
    let opaque = RgbaImage::from_pixel(16, 16, image::Rgba([51, 101, 151, 255]));
    for policy in [
        CaptureFormatPolicy::Jpeg {
            quality: CaptureImageQuality::new(90)?,
        },
        CaptureFormatPolicy::WebpLossy {
            quality: CaptureImageQuality::new(60)?,
        },
    ] {
        let mut bytes = Vec::new();
        policy.encode_to(&opaque, &mut bytes, &|| false)?;
        assert_eq!(
            image::guess_format(&bytes)?,
            if matches!(policy, CaptureFormatPolicy::Jpeg { .. }) {
                image::ImageFormat::Jpeg
            } else {
                image::ImageFormat::WebP
            }
        );
        let decoded = image::load_from_memory(&bytes)?.into_rgba8();
        assert_eq!(decoded.dimensions(), opaque.dimensions());
        assert!(decoded.pixels().all(|p| p[3] == 255
            && p[0].abs_diff(51) <= 8
            && p[1].abs_diff(101) <= 8
            && p[2].abs_diff(151) <= 8));
    }
    let mut bytes = Vec::new();
    CaptureFormatPolicy::WebpLossy {
        quality: CaptureImageQuality::new(20)?,
    }
    .encode_to(&alpha, &mut bytes, &|| false)?;
    let decoded = image::load_from_memory(&bytes)?.into_rgba8();
    assert_eq!(
        decoded.pixels().map(|p| p[3]).collect::<Vec<_>>(),
        alpha.pixels().map(|p| p[3]).collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn transparency_cancellation_and_encoded_limit_fail_without_writing_pixels() -> Result<()> {
    let alpha = RgbaImage::from_pixel(1, 1, image::Rgba([7, 11, 13, 254]));
    let mut bytes = Vec::new();
    assert!(
        CaptureFormatPolicy::Jpeg {
            quality: CaptureImageQuality::new(90)?
        }
        .encode_to(&alpha, &mut bytes, &|| false)
        .is_err()
    );
    assert!(bytes.is_empty());
    for policy in [
        CaptureFormatPolicy::Png,
        CaptureFormatPolicy::WebpLossless,
        CaptureFormatPolicy::WebpLossy {
            quality: CaptureImageQuality::new(75)?,
        },
    ] {
        assert!(policy.encode_to(&alpha, &mut bytes, &|| true).is_err());
        assert!(bytes.is_empty());
    }
    let mut destination = Vec::new();
    let mut limited = LimitedWriter {
        inner: &mut destination,
        remaining: 4,
    };
    limited.write_all(&[1, 2, 3])?;
    assert!(limited.write_all(&[4, 5]).is_err());
    assert_eq!(destination, [1, 2, 3]);
    Ok(())
}

#[cfg(unix)]
#[test]
fn settings_refuse_fifo_without_waiting_for_a_writer() -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let f = Fixture::new();
    let path = f.0.join("settings.pipe");
    let name = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(CaptureFormatSettings::load_at(&path).is_err());
    Ok(())
}

#[test]
fn required_backup_disappearing_before_its_single_open_never_imports_defaults() -> Result<()> {
    fn disappear(path: &Path) {
        fs::rename(path, path.with_extension("moved")).unwrap();
    }
    let f = Fixture::new();
    let backup = f.0.join("required-backup.json");
    let expected = CaptureFormatSettings::for_policy(CaptureFormatPolicy::WebpLossless);
    expected.export(&backup)?;
    let bytes = fs::read(&backup)?;
    assert!(backup.try_exists()?);
    BEFORE_CATALOG_OPEN.with(|hook| hook.set(Some(disappear)));
    assert!(CaptureFormatSettings::import(&backup).is_err());
    assert!(!backup.exists());
    assert_eq!(fs::read(backup.with_extension("moved"))?, bytes);
    assert_eq!(
        CaptureFormatSettings::observe_at(&backup)?,
        CaptureFormatPresence::Absent
    );
    assert_eq!(
        CaptureFormatSettings::load_at(&backup)?,
        CaptureFormatSettings::default()
    );
    assert!(CaptureFormatSettings::import(&backup).is_err());
    expected.save_at(&backup)?;
    assert_eq!(
        CaptureFormatSettings::observe_at(&backup)?,
        CaptureFormatPresence::Present(expected)
    );
    Ok(())
}
