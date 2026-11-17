use super::*;
use crate::{CaptureFormatPolicy, CaptureImageQuality, ImageExportFormat};

struct Fixture {
    root: PathBuf,
    database: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "screenshots-format-lifecycle-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        Self {
            database: root.join("history.sqlite3"),
            root,
        }
    }
    fn image(&self, policy: CaptureFormatPolicy) -> Result<(PathBuf, Vec<u8>, i64)> {
        let source = self.root.join("misleading.input");
        let alpha = !matches!(policy, CaptureFormatPolicy::Jpeg { .. });
        let pixels = image::RgbaImage::from_fn(7, 3, |x, y| {
            image::Rgba([
                (x * 31) as u8,
                (y * 67) as u8,
                43,
                if alpha { (x * 37) as u8 } else { 255 },
            ])
        });
        let mut bytes = Vec::new();
        policy.encode_to(&pixels, &mut bytes, &|| false)?;
        std::fs::write(&source, &bytes)?;
        let report = crate::import_capture_files(&self.database, std::slice::from_ref(&source))?;
        assert_eq!((report.added, report.failures.len()), (1, 0));
        let id = crate::capture_history_page(&self.database, None)?[0].id;
        Ok((source, bytes, id))
    }
    fn mime(&self, id: i64) -> Result<String> {
        super::super::open_existing(&self.database)?
            .query_row("SELECT mime_type FROM captures WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .map_err(Into::into)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn policies() -> [CaptureFormatPolicy; 4] {
    [
        CaptureFormatPolicy::Png,
        CaptureFormatPolicy::Jpeg {
            quality: CaptureImageQuality::new(90).unwrap(),
        },
        CaptureFormatPolicy::WebpLossless,
        CaptureFormatPolicy::WebpLossy {
            quality: CaptureImageQuality::new(75).unwrap(),
        },
    ]
}

#[test]
fn every_format_import_move_transfer_trash_restore_and_purge_preserves_exact_bytes() -> Result<()> {
    for policy in policies() {
        let f = Fixture::new();
        let (source, bytes, id) = f.image(policy)?;
        assert_eq!(f.mime(id)?, policy.mime_type());
        let incorrect = f.root.join(if policy.format() == ImageExportFormat::Png {
            "wrong.jpg"
        } else {
            "wrong.png"
        });
        assert!(move_history_file(&f.database, id, &incorrect).is_err());
        assert_eq!(std::fs::read(&source)?, bytes);
        let moved = f.root.join(format!("moved.{}", policy.extension()));
        let outcome = move_history_file(&f.database, id, &moved)?;
        assert_eq!(outcome.status, "moved");
        assert!(!source.exists());
        assert_eq!(std::fs::read(&moved)?, bytes);
        let transferred = f.root.join(format!("transferred.{}", policy.extension()));
        transfer_history_file(&f.database, id, &transferred)?;
        assert!(!moved.exists());
        assert_eq!(std::fs::read(&transferred)?, bytes);
        let trash = f.root.join("trash");
        std::fs::create_dir(&trash)?;
        let removed = trash_history_file(&f.database, id, &trash)?;
        let trash_id = removed.trash_id.context("Trash operation needs its ID")?;
        let record = trash_history_page(&f.database, None)?.entries.remove(0);
        assert_eq!(record.trash_path.extension().unwrap(), policy.extension());
        assert_eq!(std::fs::read(&record.trash_path)?, bytes);
        assert!(!transferred.exists());
        let restored = restore_history_trash(&f.database, &trash_id, None)?;
        assert_eq!(restored.destination_path, transferred);
        assert_eq!(f.mime(restored.capture_id)?, policy.mime_type());
        assert_eq!(std::fs::read(&transferred)?, bytes);
        let removed = trash_history_file(&f.database, restored.capture_id, &trash)?;
        let trash_id = removed.trash_id.unwrap();
        let trash_path = trash_history_page(&f.database, None)?
            .entries
            .remove(0)
            .trash_path;
        purge_history_trash(&f.database, &trash_id)?;
        assert!(!trash_path.exists());
        assert!(trash_history_page(&f.database, None)?.entries.is_empty());
        assert!(pending_history_move(&f.database)?.is_none());
    }
    Ok(())
}

#[test]
fn interrupted_native_moves_recover_actual_kind_and_refuse_forged_mime() -> Result<()> {
    for policy in policies() {
        let f = Fixture::new();
        let (source, bytes, id) = f.image(policy)?;
        let destination = f.root.join(format!("recovered.{}", policy.extension()));
        let row = capture_row(&super::super::open_existing(&f.database)?, id)?;
        let mut locked = native::LockedSource::open(&source, &destination)?;
        assert_eq!(locked.stamp().format, policy.format());
        let stamp_json = serde_json::to_value(locked.stamp())?;
        assert_eq!(
            stamp_json.get("format").is_none(),
            policy.format() == ImageExportFormat::Png
        );
        let (pending, payload) = prepare(
            &f.database,
            row,
            source,
            destination.clone(),
            locked.stamp().clone(),
        )?;
        locked.rename()?;
        locked.sync_parents()?;
        drop(locked);
        assert_eq!(
            recover_history_move(&f.database, &pending.operation_id, MoveRecovery::Finish)?.status,
            "moved"
        );
        assert_eq!(std::fs::read(&destination)?, bytes);
        assert_eq!(f.mime(id)?, policy.mime_type());
        assert_eq!(payload.stamp.format, policy.format());
    }
    let f = Fixture::new();
    let policy = policies()[1];
    let (source, bytes, id) = f.image(policy)?;
    let destination = f.root.join("not-published.jpg");
    let row = capture_row(&super::super::open_existing(&f.database)?, id)?;
    let locked = native::LockedSource::open(&source, &destination)?;
    let (pending, mut payload) = prepare(
        &f.database,
        row,
        source.clone(),
        destination.clone(),
        locked.stamp().clone(),
    )?;
    drop(locked);
    payload.stamp.format = ImageExportFormat::Png;
    super::super::open_existing(&f.database)?.execute(
        "UPDATE pending_file_move SET payload=?1",
        [serde_json::to_string(&payload)?],
    )?;
    assert!(
        recover_history_move(&f.database, &pending.operation_id, MoveRecovery::Finish).is_err()
    );
    assert_eq!(std::fs::read(source)?, bytes);
    assert!(!destination.exists());
    Ok(())
}

#[test]
fn relink_changes_mime_to_actual_contents_and_default_restore_corrects_misleading_suffix()
-> Result<()> {
    let f = Fixture::new();
    let (source, _, id) = f.image(policies()[1])?;
    let old = crate::lookup_history_entry(&f.database, id)?.unwrap();
    let replacement = f.root.join("replacement.wrong");
    let pixels = image::RgbaImage::from_pixel(9, 4, image::Rgba([11, 23, 37, 128]));
    let mut bytes = Vec::new();
    CaptureFormatPolicy::WebpLossless.encode_to(&pixels, &mut bytes, &|| false)?;
    std::fs::write(&replacement, &bytes)?;
    crate::relink_history_entry(&f.database, &old, &replacement)?;
    assert_eq!(f.mime(id)?, "image/webp");
    assert!(source.exists());
    let current = crate::lookup_history_entry(&f.database, id)?.unwrap();
    assert_eq!((current.width, current.height), (9, 4));
    assert!(crate::relink_history_entry(&f.database, &old, &source).is_err());
    let trash = f.root.join("trash");
    std::fs::create_dir(&trash)?;
    let removed = trash_history_file(&f.database, id, &trash)?;
    let restored = restore_history_trash(&f.database, &removed.trash_id.unwrap(), None)?;
    assert_eq!(
        restored.destination_path,
        replacement.with_extension("webp")
    );
    assert_eq!(std::fs::read(&restored.destination_path)?, bytes);
    assert_eq!(f.mime(restored.capture_id)?, "image/webp");
    Ok(())
}

#[test]
fn missing_trash_targets_reserve_move_transfer_restore_and_capture_save_for_every_policy()
-> Result<()> {
    for policy in policies() {
        let f = Fixture::new();
        let (_, _, reserved_capture) = f.image(policy)?;
        let trash = f.root.join("trash");
        std::fs::create_dir(&trash)?;
        let reserved_id = trash_history_file(&f.database, reserved_capture, &trash)?
            .trash_id
            .unwrap();
        let reserved = trash_history_page(&f.database, None)?
            .entries
            .remove(0)
            .trash_path;
        let connection = super::super::open_existing(&f.database)?;
        let original_metadata: (String, String) = connection.query_row(
            "SELECT trash_path,payload FROM history_trash WHERE trash_id=?1",
            [&reserved_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        for extension in ["png", "jpg", "jpeg", "webp", "PnG", "JpG", "JpEg", "WeBp"] {
            let candidate = trash.join(format!(".screenshots-trash-{reserved_id}.{extension}"));
            let key: Option<String> = connection.query_row(
                "SELECT screenshots_trash_id(?1)",
                [candidate.to_str().unwrap()],
                |row| row.get(0),
            )?;
            assert_eq!(key.as_deref(), Some(reserved_id.as_str()), "{extension}");
        }
        for extension in ["bin", "png.part", "webp.bak", "unknown"] {
            let candidate = trash.join(format!(".screenshots-trash-{reserved_id}.{extension}"));
            let key: Option<String> = connection.query_row(
                "SELECT screenshots_trash_id(?1)",
                [candidate.to_str().unwrap()],
                |row| row.get(0),
            )?;
            assert!(
                key.is_none(),
                "Unknown file types are not owned trash names"
            );
        }
        drop(connection);
        std::fs::remove_file(&reserved)?;
        let unknown = trash.join(format!(".screenshots-trash-{reserved_id}.bin"));
        std::fs::write(&unknown, b"untracked foreign contents")?;

        let pixels = std::sync::Arc::new(image::RgbaImage::from_pixel(
            7,
            3,
            image::Rgba([19, 41, 73, 255]),
        ));
        let mut active_bytes = Vec::new();
        policy.encode_to(&pixels, &mut active_bytes, &|| false)?;
        let active = f.root.join(format!("active.{}", policy.extension()));
        std::fs::write(&active, &active_bytes)?;
        assert_eq!(
            crate::import_capture_files(&f.database, std::slice::from_ref(&active))?.added,
            1
        );
        let active_id = crate::capture_history_page(&f.database, None)?[0].id;
        let failure = move_history_file(&f.database, active_id, &reserved).unwrap_err();
        assert!(format!("{failure:#}").contains("reserved"));
        assert!(pending_history_move(&f.database)?.is_none());
        assert_eq!(std::fs::read(&active)?, active_bytes);
        assert!(!reserved.exists());
        let failure = transfer_history_file(&f.database, active_id, &reserved).unwrap_err();
        assert!(format!("{failure:#}").contains("reserved"));
        assert!(pending_history_move(&f.database)?.is_none());
        assert_eq!(std::fs::read(&active)?, active_bytes);
        assert!(!reserved.exists());

        let captured = crate::CapturedImage {
            desktop_layout: None,
            display_id: "private-reservation-fixture".into(),
            window_id: None,
            width: 7,
            height: 3,
            pixels: pixels.clone(),
            png_path: f.root.join("unsaved.png"),
            elapsed_ms: 0,
            source_format: crate::PixelFormat::Rgba8UnormSrgb,
            display_geometry: None,
        };
        let failure = crate::save_capture_with_history_policy(
            &f.database,
            &captured,
            &reserved,
            policy,
            || false,
        )
        .unwrap_err();
        assert!(format!("{failure:#}").contains("reserved"));
        assert!(crate::pending_capture_save(&f.database)?.is_none());
        assert_eq!(captured.pixels.as_ref(), pixels.as_ref());
        assert!(!captured.png_path.exists() && !reserved.exists());

        let restoring = trash_history_file(&f.database, active_id, &trash)?
            .trash_id
            .unwrap();
        let restoring_path = trash_history_page(&f.database, None)?
            .entries
            .into_iter()
            .find(|entry| entry.trash_id == restoring)
            .unwrap()
            .trash_path;
        let failure = restore_history_trash(&f.database, &restoring, Some(&reserved)).unwrap_err();
        assert!(format!("{failure:#}").contains("reserved"));
        assert!(pending_history_move(&f.database)?.is_none());
        assert_eq!(std::fs::read(&restoring_path)?, active_bytes);
        assert!(!reserved.exists());
        let actual_metadata: (String, String) = super::super::open_existing(&f.database)?
            .query_row(
                "SELECT trash_path,payload FROM history_trash WHERE trash_id=?1",
                [&reserved_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
        assert_eq!(actual_metadata, original_metadata);
        assert_eq!(std::fs::read(&unknown)?, b"untracked foreign contents");
        assert!(!std::fs::read_dir(&trash)?.any(|entry| {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            name.starts_with(".screenshots-transfer-") || name.starts_with(".screenshots-save-")
        }));
    }
    Ok(())
}
