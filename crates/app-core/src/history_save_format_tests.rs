//! Companion to the held journal-v2 integration. Uses only private temporary files.
use super::*;
use crate::{CaptureFormatPolicy, CaptureImageQuality, ImageExportFormat};
use std::sync::Arc;

struct Fixture {
    root: PathBuf,
    database: PathBuf,
    image: crate::CapturedImage,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "screenshots-format-journal-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        Self {
            database: root.join("history.sqlite3"),
            image: crate::CapturedImage {
                desktop_layout: None,
                display_id: "private-format-fixture".into(),
                window_id: None,
                width: 7,
                height: 3,
                pixels: Arc::new(image::RgbaImage::from_fn(7, 3, |x, y| {
                    image::Rgba([(x * 31) as u8, (y * 61) as u8, 71, 255])
                })),
                png_path: root.join("source-default.png"),
                elapsed_ms: 0,
                source_format: crate::PixelFormat::Rgba8UnormSrgb,
                display_geometry: None,
            },
            root,
        }
    }
    fn target(&self, policy: CaptureFormatPolicy) -> PathBuf {
        self.root.join(format!("capture.{}", policy.extension()))
    }
    fn save(&self, policy: CaptureFormatPolicy) -> Result<CaptureSaveOutcome> {
        save_capture_with_history_policy(
            &self.database,
            &self.image,
            &self.target(policy),
            policy,
            || false,
        )
    }
    fn pending(&self) -> Intent {
        load(&super::super::open_existing(&self.database).unwrap())
            .unwrap()
            .unwrap()
    }
    fn rows(&self) -> i64 {
        super::super::open_existing(&self.database)
            .unwrap()
            .query_row("SELECT count(*) FROM captures", [], |r| r.get(0))
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        STOP_AT.with(|s| *s.borrow_mut() = None);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn policies() -> [CaptureFormatPolicy; 3] {
    [
        CaptureFormatPolicy::Jpeg {
            quality: CaptureImageQuality::new(91).unwrap(),
        },
        CaptureFormatPolicy::WebpLossless,
        CaptureFormatPolicy::WebpLossy {
            quality: CaptureImageQuality::new(67).unwrap(),
        },
    ]
}

#[test]
fn non_png_saves_recover_ready_and_published_with_true_mime_and_unchanged_pixels() -> Result<()> {
    for policy in policies() {
        for checkpoint in ["ready", "published", "before_metadata"] {
            let f = Fixture::new();
            let original = f.image.pixels.as_raw().clone();
            STOP_AT.with(|s| *s.borrow_mut() = Some(checkpoint));
            let saved = f.save(policy);
            STOP_AT.with(|s| *s.borrow_mut() = None);
            if checkpoint == "before_metadata" {
                assert_eq!(saved?.status, "saved_unindexed");
            } else {
                assert!(saved.is_err());
            }
            assert_eq!(f.rows(), 0);
            let intent = f.pending();
            assert_eq!(intent.version, 2);
            assert_eq!(intent.format, policy.format());
            assert_eq!(intent.target, f.target(policy));
            let held = if checkpoint == "ready" {
                &intent.stage
            } else {
                &intent.target
            };
            let bytes = std::fs::read(held)?;
            assert_eq!(
                image::guess_format(&bytes)?,
                match policy.format() {
                    ImageExportFormat::Png => image::ImageFormat::Png,
                    ImageExportFormat::Jpeg => image::ImageFormat::Jpeg,
                    ImageExportFormat::WebP => image::ImageFormat::WebP,
                }
            );
            assert!(bytes.len() as u64 <= crate::capture_format::MAX_SAVED_JPEG_WEBP_BYTES);
            let report = recover_capture_save(
                &f.database,
                &intent.operation_id,
                super::super::MoveRecovery::Finish,
            )?;
            assert_eq!(report.status, "indexed");
            assert!(!report.pending && !intent.stage.exists());
            assert_eq!(f.rows(), 1);
            assert_eq!(std::fs::read(&intent.target)?, bytes);
            let actual: (String, u32, u32) = super::super::open_existing(&f.database)?.query_row(
                "SELECT mime_type,width,height FROM captures",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            assert_eq!(actual, (policy.mime_type().into(), 7, 3));
            assert_eq!(f.image.pixels.as_raw(), &original);
            assert!(!f.image.png_path.exists());
        }
    }
    Ok(())
}

#[test]
fn recovery_rejects_forged_kind_even_when_hash_still_matches_encoded_bytes() -> Result<()> {
    let f = Fixture::new();
    let policy = policies()[0];
    STOP_AT.with(|s| *s.borrow_mut() = Some("ready"));
    assert!(f.save(policy).is_err());
    STOP_AT.with(|s| *s.borrow_mut() = None);
    let intent = f.pending();
    let bytes = std::fs::read(&intent.stage)?;
    let mut forged = serde_json::to_value(&intent)?;
    forged["format"] = "png".into();
    let target = f.root.join("forged.png");
    forged["target"] = target.to_str().unwrap().into();
    let connection = super::super::open_existing(&f.database)?;
    connection.execute(
        "UPDATE pending_capture_save SET target_path=?1,payload=?2",
        params![target.to_str().unwrap(), serde_json::to_string(&forged)?],
    )?;
    assert!(
        recover_capture_save(
            &f.database,
            &intent.operation_id,
            super::super::MoveRecovery::Finish
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&intent.stage)?, bytes);
    assert!(!target.exists() && !intent.target.exists());
    assert!(pending_capture_save(&f.database)?.is_some());
    assert_eq!(f.rows(), 0);
    Ok(())
}

#[test]
fn png_v1_recovery_and_generic_byte_copy_remain_compatible() -> Result<()> {
    let f = Fixture::new();
    STOP_AT.with(|s| *s.borrow_mut() = Some("ready"));
    assert!(f.save(CaptureFormatPolicy::Png).is_err());
    STOP_AT.with(|s| *s.borrow_mut() = None);
    let intent = f.pending();
    let mut value = serde_json::to_value(&intent)?;
    value["version"] = 1.into();
    value.as_object_mut().unwrap().remove("format");
    super::super::open_existing(&f.database)?.execute(
        "UPDATE pending_capture_save SET payload=?1",
        [serde_json::to_string(&value)?],
    )?;
    assert_eq!(
        recover_capture_save(
            &f.database,
            &intent.operation_id,
            super::super::MoveRecovery::Finish
        )?
        .status,
        "indexed"
    );
    let png_bytes = std::fs::read(f.target(CaptureFormatPolicy::Png))?;
    assert_eq!(
        image::load_from_memory(&png_bytes)?.into_rgba8(),
        *f.image.pixels
    );
    for (index, policy) in policies().into_iter().enumerate() {
        let source = f
            .root
            .join(format!("source-{index}.{}", policy.extension()));
        let mut output = std::fs::File::create(&source)?;
        policy.encode_to(&f.image.pixels, &mut output, &|| false)?;
        drop(output);
        let original = std::fs::read(&source)?;
        let destination = f.root.join(format!("copy-{index}.{}", policy.extension()));
        let outcome = copy_saved_image_with_history(&f.database, &source, &destination, || false)?;
        assert_eq!(outcome.status, "indexed");
        assert_eq!(std::fs::read(&destination)?, original);
        assert_eq!(std::fs::read(&source)?, original);
    }
    assert_eq!(f.rows(), 4);
    Ok(())
}

#[test]
fn recovery_compares_typed_intents_but_updates_exact_observed_json() -> Result<()> {
    let f = Fixture::new();
    STOP_AT.with(|s| *s.borrow_mut() = Some("ready"));
    assert!(f.save(CaptureFormatPolicy::Png).is_err());
    STOP_AT.with(|s| *s.borrow_mut() = None);
    let intent = f.pending();
    let connection = super::super::open_existing(&f.database)?;
    let mut value = serde_json::to_value(&intent)?;
    connection.execute(
        "UPDATE pending_capture_save SET payload=?1",
        [serde_json::to_string_pretty(&value)?],
    )?;
    let observed = matching_payload(&connection, &intent)?;
    assert!(observed.contains('\n'));
    value["ready"]["sha256"] = "0".repeat(64).into();
    connection.execute(
        "UPDATE pending_capture_save SET payload=?1",
        [serde_json::to_string(&value)?],
    )?;
    assert!(clear(&connection, &intent).is_err());
    assert!(load(&connection)?.is_some());
    assert!(intent.stage.exists());
    connection.execute("UPDATE pending_capture_save SET payload=?1", [observed])?;
    drop(connection);
    assert_eq!(
        recover_capture_save(
            &f.database,
            &intent.operation_id,
            super::super::MoveRecovery::Finish
        )?
        .status,
        "indexed"
    );
    assert_eq!(f.rows(), 1);
    assert_eq!(image::open(&intent.target)?.into_rgba8(), *f.image.pixels);
    Ok(())
}

#[test]
fn transparency_mismatched_suffix_and_cancel_do_not_reserve_or_publish() -> Result<()> {
    let mut f = Fixture::new();
    f.image.pixels = Arc::new(image::RgbaImage::from_pixel(
        7,
        3,
        image::Rgba([11, 23, 37, 128]),
    ));
    let original = f.image.pixels.as_raw().clone();
    assert!(f.save(policies()[0]).is_err());
    assert!(!f.target(policies()[0]).exists());
    assert!(!f.database.exists());
    assert!(
        save_capture_with_history_policy(
            &f.database,
            &f.image,
            &f.root.join("wrong.jpg"),
            CaptureFormatPolicy::WebpLossless,
            || false
        )
        .is_err()
    );
    assert!(
        save_capture_with_history_policy(
            &f.database,
            &f.image,
            &f.target(CaptureFormatPolicy::WebpLossless),
            CaptureFormatPolicy::WebpLossless,
            || true
        )
        .is_err()
    );
    assert!(!f.database.exists());
    assert_eq!(f.image.pixels.as_raw(), &original);
    assert_eq!(std::fs::read_dir(&f.root)?.count(), 0);
    Ok(())
}

#[test]
#[ignore = "short optimized synthetic 4K journal-format observation; no native capture"]
fn capture_format_journal_cost() -> Result<()> {
    let pixels = Arc::new(image::RgbaImage::from_fn(3840, 2160, |x, y| {
        image::Rgba([
            (x % 251) as u8,
            (y % 251) as u8,
            ((x / 31 + y / 17) % 251) as u8,
            255,
        ])
    }));
    for (label, policy) in [
        ("legacy PNG", CaptureFormatPolicy::Png),
        ("policy PNG", CaptureFormatPolicy::Png),
        ("policy JPEG quality 91", policies()[0]),
        ("policy lossless WebP", policies()[1]),
        ("policy lossy WebP quality 67", policies()[2]),
    ] {
        let mut samples = Vec::new();
        let mut encoded = 0;
        for _ in 0..3 {
            let mut f = Fixture::new();
            f.image.width = 3840;
            f.image.height = 2160;
            f.image.pixels = pixels.clone();
            let start = Instant::now();
            let report = if label == "legacy PNG" {
                save_capture_with_history(&f.database, &f.image, &f.target(policy), || false)?
            } else {
                f.save(policy)?
            };
            samples.push(start.elapsed());
            encoded = std::fs::metadata(report.path)?.len();
        }
        samples.sort_unstable();
        eprintln!(
            "4K {label}: median {:?}; {encoded} encoded bytes; {} shared source capacity; three synthetic encode+durable journal publication samples; codec scratch/process RSS, presentation and native input unmeasured",
            samples[1],
            pixels.as_raw().capacity()
        );
    }
    Ok(())
}
