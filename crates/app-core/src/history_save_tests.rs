use super::*;
use std::sync::Arc;

struct Fixture {
    root: PathBuf,
    database: PathBuf,
    target: PathBuf,
    image: crate::CapturedImage,
}
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("screenshots-save-journal-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        Self {
            database: root.join("history.sqlite3"),
            target: root.join("capture.png"),
            image: crate::CapturedImage {
                desktop_layout: None,
                display_id: "fixture".into(),
                window_id: None,
                width: 3,
                height: 2,
                pixels: Arc::new(image::RgbaImage::from_fn(3, 2, |x, y| {
                    image::Rgba([x as u8, y as u8, 37, if x == 0 { 0 } else { 255 }])
                })),
                png_path: root.join("unused.png"),
                elapsed_ms: 0,
                source_format: crate::PixelFormat::Rgba8UnormSrgb,
                display_geometry: None,
            },
            root,
        }
    }
    fn save(&self) -> Result<CaptureSaveOutcome> {
        save_capture_with_history(&self.database, &self.image, &self.target, || false)
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
    fn interrupt(&self, at: &'static str) {
        STOP_AT.with(|s| *s.borrow_mut() = Some(at));
        let result = self.save();
        STOP_AT.with(|s| *s.borrow_mut() = None);
        if at == "before_metadata" {
            assert_eq!(result.unwrap().status, "saved_unindexed");
        } else {
            assert!(result.is_err());
        }
    }
    fn recover(&self, action: super::super::MoveRecovery) -> Result<CaptureSaveOutcome> {
        recover_capture_save(&self.database, &self.pending().operation_id, action)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        STOP_AT.with(|s| *s.borrow_mut() = None);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn pixel_save_indexes_once_and_releases_stage_without_touching_input() -> Result<()> {
    let f = Fixture::new();
    let original = f.image.pixels.as_raw().clone();
    let report = f.save()?;
    assert_eq!(report.status, "indexed");
    assert_eq!(report.path, std::path::absolute(&f.target)?);
    assert!(!report.pending);
    assert!(report.capture_id.is_some());
    assert_eq!(f.rows(), 1);
    assert!(pending_capture_save(&f.database)?.is_none());
    assert_eq!(image::open(&f.target)?.into_rgba8(), *f.image.pixels);
    assert_eq!(f.image.pixels.as_raw(), &original);
    assert!(!std::fs::read_dir(&f.root)?.any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".screenshots-save-")
    }));
    Ok(())
}
#[test]
fn saved_png_copy_preserves_exact_bytes_without_reencoding() -> Result<()> {
    let f = Fixture::new();
    let source = f.root.join("original.png");
    f.image.save_png_as(&source)?;
    let original = std::fs::read(&source)?;
    let report = copy_saved_png_with_history(&f.database, &source, &f.target, || false)?;
    assert_eq!(report.status, "indexed");
    assert_eq!(std::fs::read(source)?, original);
    assert_eq!(std::fs::read(&f.target)?, original);
    Ok(())
}
#[test]
fn ready_crash_can_finish_or_cancel_only_its_verified_staging() -> Result<()> {
    for action in [
        super::super::MoveRecovery::Finish,
        super::super::MoveRecovery::Cancel,
    ] {
        let f = Fixture::new();
        f.interrupt("ready");
        let i = f.pending();
        assert!(i.stage.exists());
        assert!(!f.target.exists());
        assert_eq!(f.rows(), 0);
        let report = f.recover(action)?;
        assert!(!report.pending);
        assert!(pending_capture_save(&f.database)?.is_none());
        assert!(!i.stage.exists());
        if matches!(action, super::super::MoveRecovery::Finish) {
            assert_eq!(report.status, "indexed");
            assert_eq!(f.rows(), 1);
            assert!(f.target.exists());
        } else {
            assert_eq!(report.status, "canceled");
            assert_eq!(f.rows(), 0);
            assert!(!f.target.exists());
        }
    }
    Ok(())
}
#[test]
fn published_crash_is_visible_and_cancel_preserves_recoverable_output() -> Result<()> {
    let f = Fixture::new();
    f.interrupt("published");
    let i = f.pending();
    let original = std::fs::read(&f.target)?;
    assert!(!i.stage.exists());
    assert_eq!(f.rows(), 0);
    let cancel = f.recover(super::super::MoveRecovery::Cancel)?;
    assert_eq!(cancel.status, "saved_unindexed");
    assert!(cancel.pending);
    assert_eq!(std::fs::read(&f.target)?, original);
    let report = f.recover(super::super::MoveRecovery::Finish)?;
    assert_eq!(report.status, "indexed");
    assert_eq!(f.rows(), 1);
    assert!(
        recover_capture_save(
            &f.database,
            &i.operation_id,
            super::super::MoveRecovery::Finish
        )
        .is_err()
    );
    assert_eq!(f.rows(), 1);
    Ok(())
}
#[test]
fn metadata_failure_keeps_intent_and_published_file_then_recovers() -> Result<()> {
    let f = Fixture::new();
    let connection = super::super::open(&f.database)?;
    connection.execute_batch("CREATE TRIGGER fail_save BEFORE INSERT ON captures BEGIN SELECT RAISE(ABORT,'private injected insert failure'); END;")?;
    drop(connection);
    let report = f.save()?;
    assert_eq!(report.status, "saved_unindexed");
    assert!(report.pending);
    assert_eq!(f.rows(), 0);
    let pixels = std::fs::read(&f.target)?;
    assert!(
        save_capture_with_history(&f.database, &f.image, &f.root.join("other.png"), || false)
            .is_err()
    );
    super::super::open_existing(&f.database)?.execute_batch("DROP TRIGGER fail_save")?;
    assert_eq!(
        f.recover(super::super::MoveRecovery::Finish)?.status,
        "indexed"
    );
    assert_eq!(std::fs::read(&f.target)?, pixels);
    Ok(())
}
#[test]
fn normal_cancel_and_encoding_error_clean_the_exact_owned_stage() -> Result<()> {
    for cancel in [false, true] {
        let f = Fixture::new();
        let canceled = std::cell::Cell::new(false);
        let png = f.image.png_bytes()?;
        let result = save(
            &f.database,
            &f.target,
            || canceled.get(),
            |w| {
                w.write_all(&png)?;
                if cancel {
                    canceled.set(true);
                    Ok((3, 2))
                } else {
                    anyhow::bail!("private encoding failure")
                }
            },
        );
        assert!(result.is_err());
        assert!(!f.target.exists());
        assert!(pending_capture_save(&f.database)?.is_none());
        assert_eq!(f.rows(), 0);
        assert!(!std::fs::read_dir(&f.root)?.any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".screenshots-save-")
        }));
    }
    Ok(())
}

#[test]
fn encoding_cannot_publish_a_replaced_staging_name_as_saved_output() -> Result<()> {
    let f = Fixture::new();
    let png = f.image.png_bytes()?;
    let held_original = f.root.join("held-original.png");
    let result = save(
        &f.database,
        &f.target,
        || false,
        |writer| {
            writer.write_all(&png)?;
            let intent = f.pending();
            // The buffered encoder still writes through the original held file.
            // Replacing its name must not redirect publication to these bytes.
            std::fs::rename(&intent.stage, &held_original)?;
            std::fs::write(&intent.stage, b"foreign staging replacement")?;
            Ok((3, 2))
        },
    );
    assert!(
        result.is_err(),
        "A foreign staging replacement was reported saved"
    );
    assert!(!f.target.exists());
    let intent = f.pending();
    assert_eq!(
        std::fs::read(&intent.stage)?,
        b"foreign staging replacement"
    );
    assert_eq!(std::fs::read(held_original)?, png);
    assert_eq!(f.rows(), 0);
    for action in [
        super::super::MoveRecovery::Finish,
        super::super::MoveRecovery::Cancel,
    ] {
        assert!(recover_capture_save(&f.database, &intent.operation_id, action).is_err());
        assert_eq!(
            std::fs::read(&intent.stage)?,
            b"foreign staging replacement"
        );
    }
    Ok(())
}

#[test]
fn publication_rechecks_a_recovery_stage_after_held_content_validation() -> Result<()> {
    let f = Fixture::new();
    f.interrupt("ready");
    let intent = f.pending();
    let directory = native::Directory::open(
        intent.target.parent().unwrap(),
        Some(&intent.parent_identity),
    )?;
    let (stage, target) = names(&intent)?;
    let held = directory.open_file(stage, false)?.unwrap();
    held.try_lock()?;
    verify_ready(&held, &intent)?;
    std::fs::rename(&intent.stage, f.root.join("held-recovery-original.png"))?;
    std::fs::write(&intent.stage, b"foreign recovery replacement")?;
    assert!(directory.publish(stage, target, &held).is_err());
    assert!(!f.target.exists());
    assert_eq!(
        std::fs::read(&intent.stage)?,
        b"foreign recovery replacement"
    );
    assert_eq!(f.rows(), 0);
    Ok(())
}

#[test]
fn save_preserves_original_parent_spelling_only_for_the_same_native_directory() -> Result<()> {
    let f = Fixture::new();
    let child = f.root.join("child");
    std::fs::create_dir(&child)?;
    let requested = child.join("..").join("capture.png");
    let absolute = std::path::absolute(&requested)?;
    let report = save_capture_with_history(&f.database, &f.image, &requested, || false)?;
    assert_eq!(report.path, absolute);
    let history = super::super::capture_history_page(&f.database, None)?;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].path, absolute);
    assert_eq!(image::open(&f.target)?.into_rgba8(), *f.image.pixels);
    Ok(())
}

#[cfg(target_os = "windows")]
#[test]
fn windows_locked_writer_allows_metadata_reopen_and_hashes_its_own_handle() -> Result<()> {
    let f = Fixture::new();
    let (mut intent, directory, mut held) = prepare(&f.database, &f.target)?;
    let png = f.image.png_bytes()?;
    held.write_all(&png)?;
    held.sync_all()?;
    intent.ready = Some(Ready {
        sha256: format!("{:x}", Sha256::digest(&png)),
        bytes: png.len() as u64,
        width: 3,
        height: 2,
    });
    super::super::open_existing(&f.database)?.execute(
        "UPDATE pending_capture_save SET payload=?1 WHERE singleton=1",
        [serde_json::to_string(&intent)?],
    )?;
    let (stage, target) = names(&intent)?;
    let metadata_handle = directory.open_file(stage, false)?.unwrap();
    assert_eq!(
        native::identity(&metadata_handle)?,
        native::identity(&held)?
    );
    drop(metadata_handle);
    // prepare holds the real native exclusive byte-range lock. Reads through
    // this same handle work; the finish path must not hash a different reopen.
    verify_ready(&held, &intent)?;
    directory.publish(stage, target, &held)?;
    assert_eq!(
        finish(&f.database, &intent, &directory, &held, true)?.status,
        "indexed"
    );
    assert!(pending_capture_save(&f.database)?.is_none());
    assert_eq!(f.rows(), 1);
    drop(held);
    assert_eq!(image::open(&f.target)?.into_rgba8(), *f.image.pixels);
    Ok(())
}
#[test]
fn destination_conflict_never_replaces_or_indexes_existing_file() -> Result<()> {
    let f = Fixture::new();
    std::fs::write(&f.target, b"foreign file")?;
    assert!(f.save().is_err());
    assert_eq!(std::fs::read(&f.target)?, b"foreign file");
    if f.database.exists() {
        assert!(pending_capture_save(&f.database)?.is_none());
        assert_eq!(f.rows(), 0);
    }
    Ok(())
}
#[test]
fn recovery_preserves_replaced_or_changed_ready_objects() -> Result<()> {
    for published in [false, true] {
        for replace in [false, true] {
            let f = Fixture::new();
            f.interrupt(if published { "published" } else { "ready" });
            let i = f.pending();
            let path = if published { &f.target } else { &i.stage };
            let original = std::fs::read(path)?;
            if replace {
                std::fs::rename(path, f.root.join("held-original"))?;
                std::fs::write(path, &original)?;
            } else {
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(path)?
                    .write_all(b"changed")?;
            }
            let foreign = std::fs::read(path)?;
            for action in [
                super::super::MoveRecovery::Finish,
                super::super::MoveRecovery::Cancel,
            ] {
                assert!(recover_capture_save(&f.database, &i.operation_id, action).is_err());
                assert_eq!(std::fs::read(path)?, foreign);
                assert_eq!(f.rows(), 0);
            }
        }
    }
    Ok(())
}
#[test]
fn active_writer_lock_refuses_recovery_and_unverified_partial_is_preserved() -> Result<()> {
    let f = Fixture::new();
    let (i, _directory, mut file) = prepare(&f.database, &f.target)?;
    assert!(
        recover_capture_save(
            &f.database,
            &i.operation_id,
            super::super::MoveRecovery::Cancel
        )
        .is_err()
    );
    file.write_all(b"partial private fixture")?;
    file.sync_all()?;
    drop(file);
    assert!(f.recover(super::super::MoveRecovery::Finish).is_err());
    let report = f.recover(super::super::MoveRecovery::Cancel)?;
    assert_eq!(report.status, "incomplete_staging_preserved");
    assert!(report.pending);
    assert_eq!(std::fs::read(&i.stage)?, b"partial private fixture");
    std::fs::rename(&i.stage, f.root.join("explicitly-preserved.part"))?;
    let report = f.recover(super::super::MoveRecovery::Cancel)?;
    if cfg!(target_os = "windows") {
        assert_eq!(report.status, "cancellation_absence_unconfirmed");
        assert!(report.pending && pending_capture_save(&f.database)?.is_some());
    } else {
        assert_eq!(report.status, "canceled");
        assert!(pending_capture_save(&f.database)?.is_none());
    }
    Ok(())
}
#[test]
fn empty_crashed_stage_can_be_canceled_without_opening_any_other_path() -> Result<()> {
    let f = Fixture::new();
    let (i, _directory, file) = prepare(&f.database, &f.target)?;
    drop(file);
    assert_eq!(
        f.recover(super::super::MoveRecovery::Cancel)?.status,
        "canceled"
    );
    assert!(!i.stage.exists());
    assert!(pending_capture_save(&f.database)?.is_none());
    Ok(())
}
#[test]
fn journal_reserves_imports_and_other_physical_operations() -> Result<()> {
    let f = Fixture::new();
    f.interrupt("published");
    assert!(super::super::record_saved_capture(&f.database, &f.target).is_err());
    let connection = super::super::open_existing(&f.database)?;
    assert!(connection.execute("INSERT INTO pending_file_move(singleton,operation_id,capture_id,source_path,destination_path,payload) VALUES(1,?1,1,?2,?3,'{}')",params![uuid::Uuid::new_v4().to_string(),f.root.join("one.png").to_str(),f.root.join("two.png").to_str()]).is_err());
    assert_eq!(
        f.recover(super::super::MoveRecovery::Finish)?.status,
        "indexed"
    );
    Ok(())
}
#[cfg(unix)]
#[test]
fn recovery_refuses_symlinks_and_changed_parent_identity() -> Result<()> {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    f.interrupt("ready");
    let i = f.pending();
    let foreign = f.root.join("foreign");
    std::fs::write(&foreign, b"foreign")?;
    std::fs::rename(&i.stage, f.root.join("original-stage"))?;
    symlink(&foreign, &i.stage)?;
    assert!(f.recover(super::super::MoveRecovery::Cancel).is_err());
    assert_eq!(std::fs::read(&foreign)?, b"foreign");
    let mut i = load(&super::super::open_existing(&f.database)?)?.unwrap();
    i.parent_identity = "u:changed-parent".into();
    super::super::open_existing(&f.database)?.execute(
        "UPDATE pending_capture_save SET payload=?1",
        [serde_json::to_string(&i)?],
    )?;
    assert!(f.recover(super::super::MoveRecovery::Finish).is_err());
    Ok(())
}
#[test]
fn all_released_schema_versions_upgrade_additively_and_unknown_future_is_unchanged() -> Result<()> {
    for version in 1..=9 {
        let f = Fixture::new();
        let mut legacy = rusqlite::Connection::open(&f.database)?;
        super::super::file_ops::register_reservations(&legacy)?;
        super::super::migrations().to_version(&mut legacy, version)?;
        legacy.execute("INSERT INTO captures(file_path,mime_type,width,height,file_size,created_at,source) VALUES(?1,'image/png',3,2,67,'2026-11-16T00:00:00Z','capture')",[f.root.join("legacy.png").to_str()])?;
        drop(legacy);
        let upgraded = super::super::open_existing(&f.database)?;
        assert_eq!(
            upgraded.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))?,
            10
        );
        assert_eq!(f.rows(), 1);
        assert!(crate::check_history_database(&f.database).is_ok());
    }
    let f = Fixture::new();
    let connection = rusqlite::Connection::open(&f.database)?;
    connection.execute_batch("CREATE TABLE foreign_data(value TEXT);INSERT INTO foreign_data VALUES('preserved');PRAGMA user_version=11;")?;
    drop(connection);
    let before = std::fs::read(&f.database)?;
    assert!(super::super::open_existing(&f.database).is_err());
    assert!(super::super::open(&f.database).is_err());
    assert_eq!(std::fs::read(&f.database)?, before);
    assert!(!f.root.join("history.sqlite3-wal").exists());
    Ok(())
}
#[test]
fn malformed_intent_and_wrong_operation_never_modify_files() -> Result<()> {
    let f = Fixture::new();
    f.interrupt("ready");
    let i = f.pending();
    let original = std::fs::read(&i.stage)?;
    assert!(
        recover_capture_save(
            &f.database,
            &uuid::Uuid::new_v4().to_string(),
            super::super::MoveRecovery::Cancel
        )
        .is_err()
    );
    super::super::open_existing(&f.database)?
        .execute("UPDATE pending_capture_save SET payload='{}'", [])?;
    assert!(pending_capture_save(&f.database).is_err());
    assert!(
        recover_capture_save(
            &f.database,
            &i.operation_id,
            super::super::MoveRecovery::Cancel
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&i.stage)?, original);
    Ok(())
}
#[test]
fn encoded_output_limit_rejects_before_extending_file() -> Result<()> {
    let f = Fixture::new();
    let mut file = std::fs::File::create(f.root.join("bounded"))?;
    let mut writer = HashWriter {
        file: &mut file,
        hash: Sha256::new(),
        bytes: MAX_ENCODED,
    };
    assert!(writer.write(b"x").is_err());
    assert_eq!(file.metadata()?.len(), 0);
    Ok(())
}

#[test]
fn pending_inspection_never_migrates_an_older_database() -> Result<()> {
    let f = Fixture::new();
    let mut connection = rusqlite::Connection::open(&f.database)?;
    super::super::migrations().to_version(&mut connection, 1)?;
    drop(connection);
    let original = std::fs::read(&f.database)?;
    assert!(pending_capture_save(&f.database)?.is_none());
    assert_eq!(std::fs::read(&f.database)?, original);
    let connection = rusqlite::Connection::open(&f.database)?;
    assert_eq!(
        connection.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))?,
        1
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn cancel_retries_parent_barrier_after_successful_unlink_and_preserves_intent_on_eio() -> Result<()>
{
    let f = Fixture::new();
    f.interrupt("ready");
    let intent = f.pending();
    STOP_AT.with(|s| *s.borrow_mut() = Some("parent_sync"));
    let first = f.recover(super::super::MoveRecovery::Cancel).unwrap_err();
    assert_eq!(
        first
            .downcast_ref::<std::io::Error>()
            .and_then(|e| e.raw_os_error()),
        Some(libc::EIO)
    );
    assert!(!intent.stage.exists() && !f.target.exists());
    for _ in 0..2 {
        assert!(pending_capture_save(&f.database)?.is_some());
        let retry = f.recover(super::super::MoveRecovery::Cancel).unwrap_err();
        assert_eq!(
            retry
                .downcast_ref::<std::io::Error>()
                .and_then(|e| e.raw_os_error()),
            Some(libc::EIO)
        );
        assert!(pending_capture_save(&f.database)?.is_some());
    }
    STOP_AT.with(|s| *s.borrow_mut() = None);
    assert_eq!(
        f.recover(super::super::MoveRecovery::Cancel)?.status,
        "canceled"
    );
    assert!(pending_capture_save(&f.database)?.is_none());
    assert_eq!(f.rows(), 0);
    Ok(())
}

#[test]
fn cancellation_stamp_survives_each_scrub_close_and_metadata_boundary() -> Result<()> {
    for phase in [
        "cancel_intent",
        "cancel_truncated",
        "cancel_flushed",
        "cancel_zero_recorded",
        "cancel_deleted",
    ] {
        let f = Fixture::new();
        f.interrupt("ready");
        let intent = f.pending();
        let directory = native::Directory::open(
            intent.target.parent().unwrap(),
            Some(&intent.parent_identity),
        )?;
        let file = directory.open_writable(names(&intent)?.0)?.unwrap();
        file.try_lock()?;
        STOP_AT.with(|s| *s.borrow_mut() = Some(phase));
        assert!(cancel_with_content_flush(&f.database, intent.clone(), &directory, file).is_err());
        STOP_AT.with(|s| *s.borrow_mut() = None);
        let pending = f.pending();
        assert!(pending.canceling.is_some());
        assert_eq!(
            pending.canceling.as_ref().unwrap().zero_flushed,
            matches!(phase, "cancel_zero_recorded" | "cancel_deleted")
        );
        if phase == "cancel_deleted" {
            assert!(!intent.stage.exists());
        } else if phase != "cancel_intent" {
            assert_eq!(std::fs::metadata(&intent.stage)?.len(), 0);
        }
        assert_eq!(
            f.recover(super::super::MoveRecovery::Cancel)?.status,
            "canceled"
        );
        assert!(pending_capture_save(&f.database)?.is_none());
        assert!(!intent.stage.exists() && !f.target.exists());
        assert_eq!(f.rows(), 0);
    }
    Ok(())
}

#[test]
fn cancellation_stamp_refuses_changed_content_and_additional_hardlinks() -> Result<()> {
    let f = Fixture::new();
    f.interrupt("ready");
    let intent = f.pending();
    let directory = native::Directory::open(
        intent.target.parent().unwrap(),
        Some(&intent.parent_identity),
    )?;
    let file = directory.open_writable(names(&intent)?.0)?.unwrap();
    file.try_lock()?;
    STOP_AT.with(|s| *s.borrow_mut() = Some("cancel_intent"));
    assert!(cancel_with_content_flush(&f.database, intent.clone(), &directory, file).is_err());
    STOP_AT.with(|s| *s.borrow_mut() = None);
    std::fs::OpenOptions::new()
        .append(true)
        .open(&intent.stage)?
        .write_all(b"changed after cancel stamp")?;
    let changed = std::fs::read(&intent.stage)?;
    assert!(f.recover(super::super::MoveRecovery::Cancel).is_err());
    assert_eq!(std::fs::read(&intent.stage)?, changed);
    assert!(f.pending().canceling.is_some());
    let other = Fixture::new();
    other.interrupt("ready");
    let intent = other.pending();
    let alias = other.root.join("other-user-owned-name.png");
    std::fs::hard_link(&intent.stage, &alias)?;
    let original = std::fs::read(&alias)?;
    let directory = native::Directory::open(
        intent.target.parent().unwrap(),
        Some(&intent.parent_identity),
    )?;
    let file = directory.open_writable(names(&intent)?.0)?.unwrap();
    file.try_lock()?;
    assert!(cancel_with_content_flush(&other.database, intent.clone(), &directory, file).is_err());
    assert_eq!(std::fs::read(&alias)?, original);
    assert_eq!(std::fs::read(&intent.stage)?, original);
    assert!(other.pending().canceling.is_none());
    Ok(())
}

#[cfg(target_os = "windows")]
#[test]
fn windows_cancel_keeps_zero_proof_until_delete_pending_handles_close() -> Result<()> {
    let f = Fixture::new();
    f.interrupt("ready");
    let intent = f.pending();
    let directory = native::Directory::open(
        intent.target.parent().unwrap(),
        Some(&intent.parent_identity),
    )?;
    let external = directory.open_file(names(&intent)?.0, false)?.unwrap();
    assert!(f.recover(super::super::MoveRecovery::Cancel).is_err());
    assert_eq!(external.metadata()?.len(), 0);
    assert!(f.pending().canceling.as_ref().unwrap().zero_flushed);
    drop(external);
    let report = f.recover(super::super::MoveRecovery::Cancel)?;
    assert_eq!(report.status, "canceled");
    assert!(report.warning.as_deref().unwrap().contains("power-loss"));
    assert!(pending_capture_save(&f.database)?.is_none());
    assert!(!intent.stage.exists());
    Ok(())
}

#[cfg(unix)]
#[test]
fn failed_ancestor_barrier_stops_before_pixels_and_retry_repairs_only_known_chain() -> Result<()> {
    let f = Fixture::new();
    let target = f.root.join("new-output/one/two/capture.png");
    let database = f.root.join("new-db/one/two/history.sqlite3");
    STOP_AT.with(|s| *s.borrow_mut() = Some("ancestor_created"));
    assert!(save_capture_with_history(&database, &f.image, &target, || false).is_err());
    assert!(f.root.join("new-output").is_dir());
    assert!(!f.root.join("new-output/one").exists());
    assert!(!database.exists() && !target.exists());
    STOP_AT.with(|s| *s.borrow_mut() = Some("ancestor_parent_sync"));
    assert!(save_capture_with_history(&database, &f.image, &target, || false).is_err());
    assert!(!f.root.join("new-output/one").exists());
    STOP_AT.with(|s| *s.borrow_mut() = None);
    let report = save_capture_with_history(&database, &f.image, &target, || false)?;
    assert_eq!(report.status, "indexed");
    assert!(target.is_file());
    assert_eq!(
        super::super::capture_history_page(&database, None)?.len(),
        1
    );
    Ok(())
}

#[test]
fn directory_preparation_caps_missing_depth_before_creating_any_component() -> Result<()> {
    let f = Fixture::new();
    let deep = (0..65).fold(f.root.clone(), |path, _| path.join("new"));
    assert!(super::super::ensure_storage_directory(&deep).is_err());
    assert!(!f.root.join("new").exists());
    let too_long = f.root.join("x".repeat(4097));
    assert!(super::super::ensure_storage_directory(&too_long).is_err());
    Ok(())
}
