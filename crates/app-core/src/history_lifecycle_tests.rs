use super::*;

struct Fixture {
    root: PathBuf,
    database: PathBuf,
    source: PathBuf,
    trash: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "screenshots-library-lifecycle-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let source = root.join("original.png");
        image::RgbaImage::from_pixel(3, 2, image::Rgba([1, 12, 123, 0]))
            .save(&source)
            .unwrap();
        let database = root.join("history.sqlite3");
        crate::history::record_saved_capture(&database, &source).unwrap();
        let trash = root.join("trash");
        std::fs::create_dir(&trash).unwrap();
        Self {
            root,
            database,
            source,
            trash,
        }
    }
    fn connection(&self) -> Connection {
        crate::history::open_existing(&self.database).unwrap()
    }
    fn trash(&self) -> String {
        trash_history_file(&self.database, 1, &self.trash)
            .unwrap()
            .trash_id
            .unwrap()
    }
    fn prepare_trash(&self) -> (native::LockedSource, PendingMove, Payload) {
        let row = capture_row(&self.connection(), 1).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let destination = self.trash.join(format!(".screenshots-trash-{id}.png"));
        let locked = native::LockedSource::open(&self.source, &destination).unwrap();
        let (pending, payload) = prepare_action(
            &self.database,
            row,
            self.source.clone(),
            destination,
            locked.stamp().clone(),
            FileAction::Trash {
                trash_id: id,
                trashed_at: "2020-01-01T00:00:00Z".into(),
            },
        )
        .unwrap();
        (locked, pending, payload)
    }
    fn prepare_purge(&self, id: &str) -> (native::LockedSource, PendingMove, Payload) {
        let record = read_record(&self.connection(), id).unwrap();
        let source = record.trash_path;
        let destination =
            source.with_file_name(format!(".screenshots-purge-{}.png", uuid::Uuid::new_v4()));
        let locked = native::LockedSource::open(&source, &destination).unwrap();
        let mut row = record.row;
        row.file_path = path_text(&source).unwrap().into();
        let (pending, payload) = prepare_action(
            &self.database,
            row,
            source,
            destination,
            record.stamp,
            FileAction::Purge {
                trash_id: id.into(),
                trashed_at: record.trashed_at,
            },
        )
        .unwrap();
        (locked, pending, payload)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn trash_restore_preserves_bytes_and_metadata_when_original_id_has_been_reused() {
    let f = Fixture::new();
    let original = capture_row(&f.connection(), 1).unwrap();
    let bytes = std::fs::read(&f.source).unwrap();
    let id = f.trash();
    assert!(!f.source.exists());
    assert!(
        crate::history::lookup_history_entry(&f.database, 1)
            .unwrap()
            .is_none()
    );
    let page = trash_history_page(&f.database, None).unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].trash_id, id);
    assert_eq!(page.entries[0].original_path, f.source);
    assert_eq!(std::fs::read(&page.entries[0].trash_path).unwrap(), bytes);
    let other = f.root.join("other.png");
    std::fs::write(&other, &bytes).unwrap();
    crate::history::record_saved_capture(&f.database, &other).unwrap();
    assert_eq!(
        crate::history::lookup_history_entry(&f.database, 1)
            .unwrap()
            .unwrap()
            .path,
        other
    );
    let outcome = restore_history_trash(&f.database, &id, None).unwrap();
    assert_eq!(outcome.kind, "restore");
    assert_eq!(outcome.status, "restored");
    assert_eq!(outcome.capture_id, 2);
    let restored = capture_row(&f.connection(), 2).unwrap();
    assert_eq!(restored.created_at, original.created_at);
    assert_eq!(restored.source, original.source);
    assert_eq!(restored.width, original.width);
    assert_eq!(restored.file_path, original.file_path);
    assert_eq!(std::fs::read(&f.source).unwrap(), bytes);
    assert_eq!(std::fs::read(&other).unwrap(), bytes);
    assert!(
        trash_history_page(&f.database, None)
            .unwrap()
            .entries
            .is_empty()
    );
    assert!(pending_history_move(&f.database).unwrap().is_none());
    assert_eq!(
        f.connection()
            .query_row(
                "SELECT count(*) FROM captures_search WHERE captures_search MATCH 'original'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
fn changed_trash_and_restore_collision_preserve_all_files_without_publishing_intent() {
    let f = Fixture::new();
    let id = f.trash();
    let record = read_record(&f.connection(), &id).unwrap();
    let original = std::fs::read(&record.trash_path).unwrap();
    let mut changed = original.clone();
    let end = changed.len() - 5;
    changed[end] ^= 1;
    std::fs::write(&record.trash_path, &changed).unwrap();
    assert!(purge_history_trash(&f.database, &id).is_err());
    assert!(restore_history_trash(&f.database, &id, None).is_err());
    assert_eq!(std::fs::read(&record.trash_path).unwrap(), changed);
    assert!(pending_history_move(&f.database).unwrap().is_none());
    std::fs::write(&record.trash_path, &original).unwrap();
    std::fs::write(&f.source, b"another file").unwrap();
    assert!(restore_history_trash(&f.database, &id, None).is_err());
    assert_eq!(std::fs::read(&f.source).unwrap(), b"another file");
    assert_eq!(std::fs::read(&record.trash_path).unwrap(), original);
    assert_eq!(
        purge_history_trash(&f.database, &id).unwrap().status,
        "purged"
    );
    assert!(!record.trash_path.exists());
    assert_eq!(std::fs::read(&f.source).unwrap(), b"another file");
    assert!(
        trash_history_page(&f.database, None)
            .unwrap()
            .entries
            .is_empty()
    );
}

#[test]
fn unapplied_trash_intent_reserves_capture_and_can_only_be_canceled_explicitly() {
    let f = Fixture::new();
    let (_locked, pending, _) = f.prepare_trash();
    assert_eq!(
        pending_history_move(&f.database).unwrap().unwrap().kind,
        "trash"
    );
    assert!(crate::history::remove_history_entry(&f.database, 1).is_err());
    assert!(
        recover_history_move(&f.database, &pending.operation_id, MoveRecovery::Cancel).is_err()
    );
    assert!(
        recover_history_file_operation(&f.database, &pending.operation_id, MoveRecovery::Finish)
            .is_err()
    );
    let canceled =
        recover_history_file_operation(&f.database, &pending.operation_id, MoveRecovery::Cancel)
            .unwrap();
    assert_eq!(canceled.status, "canceled");
    assert!(f.source.exists());
    assert_eq!(
        trash_history_page(&f.database, None).unwrap().entries.len(),
        0
    );
}

#[test]
fn renamed_trash_finishes_file_and_database_coordination_after_interruption() {
    let f = Fixture::new();
    let (mut locked, pending, payload) = f.prepare_trash();
    let bytes = std::fs::read(&f.source).unwrap();
    locked.rename().unwrap();
    locked.sync_parents().unwrap();
    drop(locked);
    assert!(!f.source.exists());
    assert!(
        crate::history::lookup_history_entry(&f.database, 1)
            .unwrap()
            .is_some()
    );
    assert!(
        recover_history_file_operation(&f.database, &pending.operation_id, MoveRecovery::Cancel)
            .is_err()
    );
    let result =
        recover_history_file_operation(&f.database, &pending.operation_id, MoveRecovery::Finish)
            .unwrap();
    assert_eq!(result.status, "trashed");
    assert_eq!(std::fs::read(&pending.destination_path).unwrap(), bytes);
    assert_eq!(
        read_record(&f.connection(), payload.action.trash_id().unwrap())
            .unwrap()
            .stamp,
        payload.stamp
    );
    assert!(
        crate::history::lookup_history_entry(&f.database, 1)
            .unwrap()
            .is_none()
    );
    assert!(pending_history_move(&f.database).unwrap().is_none());
}

#[test]
fn restore_interruption_reserves_new_id_and_finishes_exact_original_metadata() {
    let f = Fixture::new();
    let id = f.trash();
    let record = read_record(&f.connection(), &id).unwrap();
    let source = record.trash_path;
    let mut locked = native::LockedSource::open(&source, &f.source).unwrap();
    let mut row = record.row;
    row.id = 7;
    row.file_path = path_text(&source).unwrap().into();
    let (pending, _) = prepare_action(
        &f.database,
        row,
        source,
        f.source.clone(),
        record.stamp,
        FileAction::Restore { trash_id: id },
    )
    .unwrap();
    assert!(f.connection().execute("INSERT INTO captures(id,file_path,mime_type,width,height,file_size,created_at,source) VALUES(7,'other.png','image/png',3,2,99,'2020-01-01','capture')",[]).is_err());
    locked.rename().unwrap();
    locked.sync_parents().unwrap();
    drop(locked);
    assert!(
        crate::history::lookup_history_entry(&f.database, 7)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        recover_history_file_operation(&f.database, &pending.operation_id, MoveRecovery::Finish)
            .unwrap()
            .capture_id,
        7
    );
    assert_eq!(
        crate::history::lookup_history_entry(&f.database, 7)
            .unwrap()
            .unwrap()
            .path,
        f.source
    );
    assert!(
        trash_history_page(&f.database, None)
            .unwrap()
            .entries
            .is_empty()
    );
}

#[test]
fn interrupted_purge_finishes_only_matching_quarantine_or_already_missing_owned_file() {
    for removed in [false, true] {
        let f = Fixture::new();
        let id = f.trash();
        let (mut locked, pending, payload) = f.prepare_purge(&id);
        assert!(
            f.connection()
                .execute("DELETE FROM history_trash WHERE trash_id=?1", [&id])
                .is_err()
        );
        locked.rename().unwrap();
        locked.sync_parents().unwrap();
        drop(locked);
        if removed {
            native::remove_expected(&pending.destination_path, &payload.stamp).unwrap();
        }
        assert!(
            recover_history_move(&f.database, &pending.operation_id, MoveRecovery::Finish).is_err()
        );
        assert!(
            recover_history_file_operation(
                &f.database,
                &pending.operation_id,
                MoveRecovery::Cancel
            )
            .is_err()
        );
        assert_eq!(
            recover_history_file_operation(
                &f.database,
                &pending.operation_id,
                MoveRecovery::Finish
            )
            .unwrap()
            .status,
            "purged"
        );
        assert!(!pending.destination_path.exists());
        assert!(!pending.source_path.exists());
        assert!(
            trash_history_page(&f.database, None)
                .unwrap()
                .entries
                .is_empty()
        );
        assert!(pending_history_move(&f.database).unwrap().is_none());
    }
}

#[test]
fn canceled_purge_retains_quarantine_and_durable_intent_before_permanent_removal() {
    let f = Fixture::new();
    let id = f.trash();
    let record = read_record(&f.connection(), &id).unwrap();
    let bytes = std::fs::read(&record.trash_path).unwrap();
    assert!(purge_checked(&f.database, &id, &|| !record.trash_path.exists(), None).is_err());
    let pending = pending_history_move(&f.database).unwrap().unwrap();
    assert_eq!(pending.kind, "purge");
    assert_eq!(std::fs::read(&pending.destination_path).unwrap(), bytes);
    assert!(read_record(&f.connection(), &id).is_ok());
    recover_history_file_operation(&f.database, &pending.operation_id, MoveRecovery::Finish)
        .unwrap();
}

#[test]
fn purge_recovery_refuses_replaced_quarantine_without_deleting_its_bytes() {
    let f = Fixture::new();
    let id = f.trash();
    let (mut locked, pending, _) = f.prepare_purge(&id);
    locked.rename().unwrap();
    locked.sync_parents().unwrap();
    drop(locked);
    std::fs::write(&pending.destination_path, b"user replacement").unwrap();
    assert!(
        recover_history_file_operation(&f.database, &pending.operation_id, MoveRecovery::Finish)
            .is_err()
    );
    assert_eq!(
        std::fs::read(&pending.destination_path).unwrap(),
        b"user replacement"
    );
    assert!(pending_history_move(&f.database).unwrap().is_some());
    assert!(read_record(&f.connection(), &id).is_ok());
}

#[test]
fn retention_spans_pages_is_explicit_excludes_imports_and_rechecks_trash_fingerprints() {
    let f = Fixture::new();
    let pixels = std::fs::read(&f.source).unwrap();
    for number in 2..=26 {
        let path = f.root.join(format!("capture-{number}.png"));
        std::fs::write(&path, &pixels).unwrap();
        crate::history::record_saved_capture(&f.database, &path).unwrap();
    }
    f.connection()
        .execute(
            "UPDATE captures SET created_at='2020-01-01T00:00:00Z' WHERE id<=25",
            [],
        )
        .unwrap();
    let imported = f.root.join("imported.png");
    std::fs::write(&imported, &pixels).unwrap();
    crate::history::import_capture_files(&f.database, std::slice::from_ref(&imported)).unwrap();
    f.connection()
        .execute(
            "UPDATE captures SET created_at='2020-01-01T00:00:00Z' WHERE source='import'",
            [],
        )
        .unwrap();
    let policy = RetentionPolicy {
        version: 1,
        trash_directory: Some(f.trash.clone()),
        capture_days: Some(30),
        purge_days: None,
        include_imports: false,
        limit: 25,
    };
    let plan = plan_retention(&f.database, &policy).unwrap();
    assert_eq!(plan.capture_ids, (1..=25).collect::<Vec<_>>());
    assert!(!plan.has_more);
    assert!(f.source.exists());
    let canceled = apply_retention(&f.database, &policy, || true).unwrap();
    assert!(canceled.outcomes.is_empty());
    assert!(canceled.failure.is_some());
    assert!(f.source.exists());
    let report = apply_retention(&f.database, &policy, || false).unwrap();
    assert_eq!(report.outcomes.len(), 25);
    assert!(report.failure.is_none());
    assert!(imported.exists());
    assert!(f.root.join("capture-26.png").exists());
    let first = trash_history_page(&f.database, None).unwrap();
    assert_eq!(first.entries.len(), 20);
    assert!(first.has_more);
    let second = trash_history_page(&f.database, first.next_cursor).unwrap();
    assert_eq!(second.entries.len(), 5);
    assert!(!second.has_more);
    let connection = f.connection();
    let mut statement = connection
        .prepare("SELECT trash_id FROM history_trash")
        .unwrap();
    let ids = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    for id in ids {
        let mut record = read_record(&connection, &id).unwrap();
        record.trashed_at = "2020-01-01T00:00:00Z".into();
        connection
            .execute(
                "UPDATE history_trash SET trashed_at=?1,payload=?2 WHERE trash_id=?3",
                params![
                    record.trashed_at,
                    serde_json::to_string(&record).unwrap(),
                    id
                ],
            )
            .unwrap();
    }
    drop(statement);
    drop(connection);
    let policy = RetentionPolicy {
        capture_days: None,
        purge_days: Some(7),
        ..policy
    };
    let plan = plan_retention(&f.database, &policy).unwrap();
    assert_eq!(plan.trash_ids.len(), 25);
    let record = read_record(&f.connection(), &plan.trash_ids[0]).unwrap();
    let original = std::fs::read(&record.trash_path).unwrap();
    std::fs::write(&record.trash_path, b"preserve changed file").unwrap();
    let refused = apply_retention(&f.database, &policy, || false).unwrap();
    assert!(refused.failure.is_some());
    assert!(refused.outcomes.is_empty());
    assert_eq!(
        std::fs::read(&record.trash_path).unwrap(),
        b"preserve changed file"
    );
    std::fs::write(&record.trash_path, &original).unwrap();
    let report = apply_retention(&f.database, &policy, || false).unwrap();
    assert_eq!(report.outcomes.len(), 25);
    assert!(report.failure.is_none());
    assert!(
        trash_history_page(&f.database, None)
            .unwrap()
            .entries
            .is_empty()
    );
    assert!(imported.exists());
    assert!(f.root.join("capture-26.png").exists());
}

impl Fixture {
    fn prepare_transfer(&self, destination: &Path) -> (PendingMove, Payload, PathBuf) {
        let row = capture_row(&self.connection(), 1).unwrap();
        let stamp = native::inspect(&self.source).unwrap().unwrap();
        let stage = destination.with_file_name(format!(
            ".screenshots-transfer-{}.png",
            uuid::Uuid::new_v4()
        ));
        let (pending, payload) = prepare_action(
            &self.database,
            row,
            self.source.clone(),
            destination.to_owned(),
            stamp,
            FileAction::Transfer {
                stage_path: stage.clone(),
            },
        )
        .unwrap();
        (pending, payload, stage)
    }
}

#[test]
fn explicit_transfer_preserves_exact_bytes_id_and_metadata_with_a_new_native_identity() {
    let f = Fixture::new();
    let destination = f.root.join("transferred.png");
    let before = capture_row(&f.connection(), 1).unwrap();
    let stamp = native::inspect(&f.source).unwrap().unwrap();
    let bytes = std::fs::read(&f.source).unwrap();
    let outcome = transfer_history_file(&f.database, 1, &destination).unwrap();
    assert_eq!(outcome.kind, "transfer");
    assert_eq!(outcome.status, "transferred");
    assert_eq!(outcome.capture_id, 1);
    assert!(!f.source.exists());
    assert_eq!(std::fs::read(&destination).unwrap(), bytes);
    let after = native::inspect(&destination).unwrap().unwrap();
    assert!(same_contents(&after, &stamp));
    assert_ne!(
        (after.device, after.inode, after.file_id),
        (stamp.device, stamp.inode, stamp.file_id)
    );
    let row = capture_row(&f.connection(), 1).unwrap();
    assert_eq!(row.file_path, path_text(&destination).unwrap());
    assert_eq!(row.created_at, before.created_at);
    assert_eq!(row.source, before.source);
    assert!(row.thumbnail_path.is_none());
    assert!(pending_history_move(&f.database).unwrap().is_none());
}

#[test]
fn transfer_cancel_preserves_an_unverified_partial_and_removes_only_a_verified_copy() {
    for verified in [false, true] {
        let f = Fixture::new();
        let destination = f.root.join("transferred.png");
        let (pending, payload, stage) = f.prepare_transfer(&destination);
        if verified {
            copy_prepared(&f.database, &pending, &payload).unwrap();
        } else {
            std::fs::write(&stage, b"interrupted partial copy").unwrap();
        }
        assert!(
            recover_history_move(&f.database, &pending.operation_id, MoveRecovery::Cancel).is_err()
        );
        let outcome = recover_history_file_operation(
            &f.database,
            &pending.operation_id,
            MoveRecovery::Cancel,
        )
        .unwrap();
        assert_eq!(outcome.status, "canceled");
        assert_eq!(outcome.preserved_path, (!verified).then_some(stage.clone()));
        assert_eq!(stage.exists(), !verified);
        assert!(f.source.exists());
        assert!(!destination.exists());
        if !verified {
            assert_eq!(std::fs::read(&stage).unwrap(), b"interrupted partial copy");
        }
        assert!(pending_history_move(&f.database).unwrap().is_none());
    }
}

#[test]
fn transfer_recovers_verified_copy_publication_and_source_removal_boundaries() {
    for phase in 0..3 {
        let f = Fixture::new();
        let destination = f.root.join("transferred.png");
        let bytes = std::fs::read(&f.source).unwrap();
        let (pending, payload, stage) = f.prepare_transfer(&destination);
        let payload = copy_prepared(&f.database, &pending, &payload).unwrap();
        if phase >= 1 {
            let mut copy = native::LockedSource::open(&stage, &destination).unwrap();
            copy.rename().unwrap();
            copy.verify_destination().unwrap();
            copy.sync_parents().unwrap();
        }
        if phase >= 2 {
            native::remove_expected(&f.source, &payload.stamp).unwrap();
        }
        assert_eq!(
            pending_history_move(&f.database).unwrap().unwrap().kind,
            "transfer"
        );
        assert_eq!(
            recover_history_file_operation(
                &f.database,
                &pending.operation_id,
                MoveRecovery::Finish
            )
            .unwrap()
            .status,
            "transferred"
        );
        assert!(!f.source.exists());
        assert!(!stage.exists());
        assert_eq!(std::fs::read(&destination).unwrap(), bytes);
        assert_eq!(
            capture_row(&f.connection(), 1).unwrap().file_path,
            path_text(&destination).unwrap()
        );
        assert!(pending_history_move(&f.database).unwrap().is_none());
    }
}

#[test]
fn changed_transfer_source_prevents_deletion_and_conflicts_preserve_copy_and_intent() {
    let f = Fixture::new();
    let destination = f.root.join("transferred.png");
    let (pending, payload, stage) = f.prepare_transfer(&destination);
    copy_prepared(&f.database, &pending, &payload).unwrap();
    let copied = std::fs::read(&stage).unwrap();
    image::RgbaImage::from_pixel(3, 2, image::Rgba([9, 8, 7, 255]))
        .save(&f.source)
        .unwrap();
    let changed = std::fs::read(&f.source).unwrap();
    assert!(
        recover_history_file_operation(&f.database, &pending.operation_id, MoveRecovery::Finish)
            .is_err()
    );
    assert!(
        recover_history_file_operation(&f.database, &pending.operation_id, MoveRecovery::Cancel)
            .is_err()
    );
    assert_eq!(std::fs::read(&f.source).unwrap(), changed);
    assert_eq!(std::fs::read(&stage).unwrap(), copied);
    assert!(!destination.exists());
    assert!(pending_history_move(&f.database).unwrap().is_some());
}

#[test]
fn failed_copy_fingerprint_commit_leaves_original_and_recoverable_unverified_candidate() {
    let f = Fixture::new();
    let destination = f.root.join("transferred.png");
    let (pending, payload, stage) = f.prepare_transfer(&destination);
    f.connection().execute_batch("CREATE TRIGGER interrupt_copy_commit BEFORE UPDATE ON pending_file_move WHEN json_extract(new.payload,'$.destination_stamp') IS NOT NULL BEGIN SELECT RAISE(ABORT,'fixture copy commit'); END").unwrap();
    assert!(copy_prepared(&f.database, &pending, &payload).is_err());
    assert!(f.source.exists());
    assert_eq!(
        std::fs::read(&stage).unwrap(),
        std::fs::read(&f.source).unwrap()
    );
    assert!(
        recover_history_file_operation(&f.database, &pending.operation_id, MoveRecovery::Finish)
            .is_err()
    );
    let outcome =
        recover_history_file_operation(&f.database, &pending.operation_id, MoveRecovery::Cancel)
            .unwrap();
    assert_eq!(outcome.preserved_path, Some(stage.clone()));
    assert!(stage.exists());
    assert!(f.source.exists());
}

#[test]
fn canceled_native_copy_cleans_only_its_reserved_file_and_leaves_source_unchanged() {
    let f = Fixture::new();
    let stage = f.root.join(format!(
        ".screenshots-transfer-{}.png",
        uuid::Uuid::new_v4()
    ));
    let stamp = native::inspect(&f.source).unwrap().unwrap();
    let bytes = std::fs::read(&f.source).unwrap();
    assert!(native::copy_to_stage(&f.source, &stage, &stamp, &|| true).is_err());
    assert!(!stage.exists());
    assert_eq!(std::fs::read(&f.source).unwrap(), bytes);
    std::fs::write(&stage, b"foreign reservation").unwrap();
    assert!(native::copy_to_stage(&f.source, &stage, &stamp, &|| false).is_err());
    assert_eq!(std::fs::read(&stage).unwrap(), b"foreign reservation");
}

#[cfg(target_os = "linux")]
#[test]
fn transfer_crosses_real_linux_filesystems_with_exact_verified_output() {
    use std::os::unix::fs::MetadataExt;
    struct Secondary(PathBuf);
    impl Drop for Secondary {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let f = Fixture::new();
    let secondary = Secondary(Path::new("/dev/shm").join(format!(
        "screenshots-transfer-fixture-{}",
        uuid::Uuid::new_v4()
    )));
    std::fs::create_dir(&secondary.0).unwrap();
    let destination = secondary.0.join("transferred.png");
    let source_device = std::fs::metadata(&f.root).unwrap().dev();
    let target_device = std::fs::metadata(&secondary.0).unwrap().dev();
    assert_ne!(
        source_device, target_device,
        "This acceptance fixture needs distinct filesystem devices"
    );
    let original = std::fs::read(&f.source).unwrap();
    assert!(crate::history::move_history_file(&f.database, 1, &destination).is_err());
    assert!(pending_history_move(&f.database).unwrap().is_none());
    assert!(f.source.exists());
    assert_eq!(
        transfer_history_file(&f.database, 1, &destination)
            .unwrap()
            .status,
        "transferred"
    );
    assert!(!f.source.exists());
    assert_eq!(std::fs::read(&destination).unwrap(), original);
    assert_eq!(
        capture_row(&f.connection(), 1).unwrap().file_path,
        path_text(&destination).unwrap()
    );
    eprintln!("verified native cross-filesystem transfer: {source_device} -> {target_device}");
}

#[test]
fn explicit_missing_reference_reconciliation_spans_pages_and_refuses_ambiguous_selection() {
    let f = Fixture::new();
    let connection = f.connection();
    for id in 2..=26 {
        connection.execute("INSERT INTO captures(id,file_path,mime_type,width,height,file_size,created_at,source) VALUES(?1,?2,'image/png',3,2,99,'2020-01-01T00:00:00Z','capture')",
            params![id,path_text(&f.root.join(format!("missing-{id}.png"))).unwrap()]).unwrap();
    }
    let selected = (2..=26).collect::<Vec<i64>>();
    let mut ambiguous = selected.clone();
    ambiguous.push(1);
    assert!(crate::history::remove_missing_history_entries(&f.database, &ambiguous).is_err());
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM captures", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        26
    );
    assert_eq!(
        crate::history::remove_missing_history_entries(&f.database, &selected).unwrap(),
        25
    );
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM captures", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(f.source.exists());
    assert!(crate::history::remove_missing_history_entries(&f.database, &[1; 101]).is_err());
}

#[test]
fn retention_revalidates_age_after_selection_before_any_file_mutation() {
    use std::cell::Cell;
    let f = Fixture::new();
    f.connection()
        .execute("UPDATE captures SET created_at='2020-01-01T00:00:00Z'", [])
        .unwrap();
    let policy = RetentionPolicy {
        version: 1,
        trash_directory: Some(f.trash.clone()),
        capture_days: Some(30),
        purge_days: None,
        include_imports: false,
        limit: 20,
    };
    let changed = Cell::new(false);
    let report = apply_retention(&f.database, &policy, || {
        if !changed.replace(true) {
            f.connection()
                .execute(
                    "UPDATE captures SET created_at=?1",
                    [Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)],
                )
                .unwrap();
        }
        false
    })
    .unwrap();
    assert!(report.outcomes.is_empty());
    assert!(report.failure.is_some());
    assert!(f.source.exists());
    assert!(pending_history_move(&f.database).unwrap().is_none());
    let id = f.trash();
    let connection = f.connection();
    let mut record = read_record(&connection, &id).unwrap();
    record.trashed_at = "2020-01-01T00:00:00Z".into();
    connection
        .execute(
            "UPDATE history_trash SET trashed_at=?1,payload=?2 WHERE trash_id=?3",
            params![
                record.trashed_at,
                serde_json::to_string(&record).unwrap(),
                id
            ],
        )
        .unwrap();
    drop(connection);
    let changed = Cell::new(false);
    let policy = RetentionPolicy {
        capture_days: None,
        purge_days: Some(7),
        ..policy
    };
    let report = apply_retention(&f.database, &policy, || {
        if !changed.replace(true) {
            let connection = f.connection();
            let mut refreshed = read_record(&connection, &id).unwrap();
            refreshed.trashed_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            connection
                .execute(
                    "UPDATE history_trash SET trashed_at=?1,payload=?2 WHERE trash_id=?3",
                    params![
                        refreshed.trashed_at,
                        serde_json::to_string(&refreshed).unwrap(),
                        id
                    ],
                )
                .unwrap();
        }
        false
    })
    .unwrap();
    assert!(report.outcomes.is_empty());
    assert!(report.failure.is_some());
    assert!(record.trash_path.exists());
    assert!(pending_history_move(&f.database).unwrap().is_none());
}

#[test]
fn purge_admission_binds_timestamp_and_rejects_noncanonical_cutoff_ordering() {
    use std::cell::Cell;
    let f = Fixture::new();
    let id = f.trash();
    let original = read_record(&f.connection(), &id).unwrap();
    let changed = Cell::new(false);
    assert!(
        purge_checked(
            &f.database,
            &id,
            &|| {
                if !changed.replace(true) {
                    let connection = f.connection();
                    let mut record = read_record(&connection, &id).unwrap();
                    record.trashed_at = "2090-01-01T00:00:00Z".into();
                    connection
                        .execute(
                            "UPDATE history_trash SET trashed_at=?1,payload=?2 WHERE trash_id=?3",
                            params![
                                record.trashed_at,
                                serde_json::to_string(&record).unwrap(),
                                id
                            ],
                        )
                        .unwrap();
                }
                false
            },
            None
        )
        .is_err()
    );
    assert!(original.trash_path.exists());
    assert!(pending_history_move(&f.database).unwrap().is_none());
    let connection = f.connection();
    let mut record = read_record(&connection, &id).unwrap();
    record.trashed_at = "2090-01-01T00:00:00+00:00".into();
    connection
        .execute(
            "UPDATE history_trash SET trashed_at=?1,payload=?2 WHERE trash_id=?3",
            params![
                record.trashed_at,
                serde_json::to_string(&record).unwrap(),
                id
            ],
        )
        .unwrap();
    assert!(purge_history_trash(&f.database, &id).is_err());
    assert!(original.trash_path.exists());
    assert!(pending_history_move(&f.database).unwrap().is_none());
}

#[test]
fn restore_reservation_blocks_the_normal_automatic_capture_id_until_finish_or_cancel() {
    for action in [MoveRecovery::Finish, MoveRecovery::Cancel] {
        let f = Fixture::new();
        let id = f.trash();
        let record = read_record(&f.connection(), &id).unwrap();
        let source = record.trash_path;
        let mut locked = native::LockedSource::open(&source, &f.source).unwrap();
        let mut row = record.row;
        row.id = 1;
        row.file_path = path_text(&source).unwrap().into();
        let (pending, _) = prepare_action(
            &f.database,
            row,
            source,
            f.source.clone(),
            record.stamp,
            FileAction::Restore { trash_id: id },
        )
        .unwrap();
        let other = f.root.join("new-capture.png");
        image::RgbaImage::from_pixel(2, 3, image::Rgba([11, 22, 33, 255]))
            .save(&other)
            .unwrap();
        let bytes = std::fs::read(&other).unwrap();
        assert!(crate::history::record_saved_capture(&f.database, &other).is_err());
        assert_eq!(
            f.connection()
                .query_row("SELECT count(*) FROM captures", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            f.connection()
                .query_row("SELECT count(*) FROM captures_search", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(std::fs::read(&other).unwrap(), bytes);
        assert_eq!(
            pending_history_move(&f.database)
                .unwrap()
                .unwrap()
                .operation_id,
            pending.operation_id
        );
        if matches!(action, MoveRecovery::Finish) {
            locked.rename().unwrap();
            locked.sync_parents().unwrap();
        }
        drop(locked);
        recover_history_file_operation(&f.database, &pending.operation_id, action).unwrap();
        crate::history::record_saved_capture(&f.database, &other).unwrap();
        assert_eq!(std::fs::read(&other).unwrap(), bytes);
        assert!(pending_history_move(&f.database).unwrap().is_none());
    }
}

#[test]
fn schema_six_migrates_without_losing_rows_and_installs_the_assigned_id_guard() {
    let f = Fixture::new();
    let before = capture_row(&f.connection(), 1).unwrap();
    let database = f.root.join("legacy-six.sqlite3");
    let mut legacy = Connection::open(&database).unwrap();
    crate::history::migrations()
        .to_version(&mut legacy, 6)
        .unwrap();
    insert_legacy_capture(&legacy, &before, 1, &f.source);
    drop(legacy);
    let other = f.root.join("new.png");
    std::fs::copy(&f.source, &other).unwrap();
    crate::history::record_saved_capture(&database, &other).unwrap();
    let connection = crate::history::open_existing(&database).unwrap();
    assert_eq!(capture_row(&connection, 1).unwrap(), before);
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        10
    );
    assert!(connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='trigger' AND name='captures_move_assigned_insert')",[],|row|row.get::<_,bool>(0)).unwrap());
}

fn insert_legacy_capture(connection: &Connection, row: &CaptureRow, id: i64, path: &Path) {
    connection.execute("INSERT INTO captures(id,file_path,thumbnail_path,mime_type,width,height,file_size,created_at,source)
        VALUES(?1,?2,NULL,?3,?4,?5,?6,?7,?8)",params![id,path.to_str().unwrap(),row.mime_type,row.width,row.height,row.file_size,row.created_at,row.source]).unwrap();
}

#[test]
fn durable_transfer_stage_rejects_import_relink_and_update_through_the_entire_intent() {
    for action in [MoveRecovery::Finish, MoveRecovery::Cancel] {
        let f = Fixture::new();
        let destination = f.root.join("transferred.png");
        let other = f.root.join("other.png");
        std::fs::copy(&f.source, &other).unwrap();
        crate::history::record_saved_capture(&f.database, &other).unwrap();
        let other_entry = crate::history::lookup_history_entry(&f.database, 2)
            .unwrap()
            .unwrap();
        let (pending, payload, stage) = f.prepare_transfer(&destination);
        copy_prepared(&f.database, &pending, &payload).unwrap();
        let bytes = std::fs::read(&stage).unwrap();
        assert!(
            crate::history::import_capture_files(&f.database, std::slice::from_ref(&stage))
                .is_err()
        );
        assert!(crate::history::relink_history_entry(&f.database, &other_entry, &stage).is_err());
        assert!(
            f.connection()
                .execute(
                    "UPDATE captures SET file_path=?1 WHERE id=2",
                    [stage.to_str().unwrap()]
                )
                .is_err()
        );
        assert_eq!(
            f.connection()
                .query_row("SELECT count(*) FROM captures", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(std::fs::read(&stage).unwrap(), bytes);
        assert_eq!(
            pending_history_move(&f.database)
                .unwrap()
                .unwrap()
                .operation_id,
            pending.operation_id
        );
        recover_history_file_operation(&f.database, &pending.operation_id, action).unwrap();
        if matches!(action, MoveRecovery::Finish) {
            assert_eq!(std::fs::read(&destination).unwrap(), bytes);
        } else {
            assert_eq!(std::fs::read(&f.source).unwrap(), bytes);
        }
    }
}

#[test]
fn upgrading_a_legacy_adopted_stage_preserves_it_until_the_alias_is_resolved() {
    for action in [MoveRecovery::Finish, MoveRecovery::Cancel] {
        let f = Fixture::new();
        let database = f.root.join("legacy-seven.sqlite3");
        let destination = f.root.join("transferred.png");
        let stage = f.root.join(format!(
            ".screenshots-transfer-{}.png",
            uuid::Uuid::new_v4()
        ));
        std::fs::copy(&f.source, &stage).unwrap();
        let bytes = std::fs::read(&stage).unwrap();
        let row = capture_row(&f.connection(), 1).unwrap();
        let pending = PendingMove {
            operation_id: uuid::Uuid::new_v4().to_string(),
            capture_id: 1,
            source_path: f.source.clone(),
            destination_path: destination.clone(),
            kind: "transfer",
        };
        let payload = Payload {
            version: 1,
            row: row.clone(),
            source_path: f.source.clone(),
            destination_path: destination.clone(),
            stamp: native::inspect(&f.source).unwrap().unwrap(),
            action: FileAction::Transfer {
                stage_path: stage.clone(),
            },
            destination_stamp: native::inspect(&stage).unwrap(),
        };
        let mut legacy = Connection::open(&database).unwrap();
        crate::history::migrations()
            .to_version(&mut legacy, 7)
            .unwrap();
        insert_legacy_capture(&legacy, &row, 1, &f.source);
        legacy.execute("INSERT INTO pending_file_move(singleton,operation_id,capture_id,source_path,destination_path,payload)VALUES(1,?1,1,?2,?3,?4)",
            params![pending.operation_id,f.source.to_str().unwrap(),destination.to_str().unwrap(),serde_json::to_string(&payload).unwrap()]).unwrap();
        insert_legacy_capture(&legacy, &row, 2, &stage);
        drop(legacy);
        assert!(recover_history_file_operation(&database, &pending.operation_id, action).is_err());
        assert_eq!(std::fs::read(&stage).unwrap(), bytes);
        assert_eq!(std::fs::read(&f.source).unwrap(), bytes);
        assert!(!destination.exists());
        assert!(pending_history_move(&database).unwrap().is_some());
        crate::history::remove_history_entry(&database, 2).unwrap();
        recover_history_file_operation(&database, &pending.operation_id, action).unwrap();
        if matches!(action, MoveRecovery::Finish) {
            assert_eq!(std::fs::read(&destination).unwrap(), bytes);
        } else {
            assert_eq!(std::fs::read(&f.source).unwrap(), bytes);
        }
    }
}

#[test]
fn missing_trash_destinations_stay_reserved_before_move_transfer_or_restore() {
    let f = Fixture::new();
    let reserved_id = f.trash();
    let reserved = read_record(&f.connection(), &reserved_id).unwrap();
    std::fs::remove_file(&reserved.trash_path).unwrap();
    let source = f.root.join("active.png");
    image::RgbaImage::from_pixel(3, 2, image::Rgba([5, 6, 7, 255]))
        .save(&source)
        .unwrap();
    crate::history::record_saved_capture(&f.database, &source).unwrap();
    let bytes = std::fs::read(&source).unwrap();
    assert!(crate::history::move_history_file(&f.database, 1, &reserved.trash_path).is_err());
    assert!(pending_history_move(&f.database).unwrap().is_none());
    assert_eq!(std::fs::read(&source).unwrap(), bytes);
    assert!(!reserved.trash_path.exists());
    assert!(transfer_history_file(&f.database, 1, &reserved.trash_path).is_err());
    assert!(pending_history_move(&f.database).unwrap().is_none());
    assert_eq!(std::fs::read(&source).unwrap(), bytes);
    assert!(!reserved.trash_path.exists());
    let restoring = trash_history_file(&f.database, 1, &f.trash)
        .unwrap()
        .trash_id
        .unwrap();
    let record = read_record(&f.connection(), &restoring).unwrap();
    assert!(restore_history_trash(&f.database, &restoring, Some(&reserved.trash_path)).is_err());
    assert!(pending_history_move(&f.database).unwrap().is_none());
    assert_eq!(std::fs::read(&record.trash_path).unwrap(), bytes);
    assert!(!reserved.trash_path.exists());
    assert_eq!(
        read_record(&f.connection(), &reserved_id).unwrap(),
        reserved
    );
    assert_eq!(read_record(&f.connection(), &restoring).unwrap(), record);
    assert!(!std::fs::read_dir(&f.trash).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".screenshots-transfer-")
    }));
}

struct DeletionBarrierFailure;
impl DeletionBarrierFailure {
    fn new() -> Self {
        native::set_deletion_barrier_failure_for_test(true);
        Self
    }
}
impl Drop for DeletionBarrierFailure {
    fn drop(&mut self) {
        native::set_deletion_barrier_failure_for_test(false);
    }
}

#[test]
fn purge_unlink_before_parent_barrier_failure_keeps_intent_until_recovery_sync_succeeds() {
    let f = Fixture::new();
    let id = f.trash();
    let (mut locked, pending, payload) = f.prepare_purge(&id);
    locked.rename().unwrap();
    locked.sync_parents().unwrap();
    drop(locked);
    let failure = DeletionBarrierFailure::new();
    assert!(native::remove_expected(&pending.destination_path, &payload.stamp).is_err());
    assert!(!pending.source_path.exists() && !pending.destination_path.exists());
    assert!(
        recover_history_file_operation(&f.database, &pending.operation_id, MoveRecovery::Finish)
            .is_err()
    );
    assert!(pending_history_move(&f.database).unwrap().is_some());
    assert!(read_record(&f.connection(), &id).is_ok());
    drop(failure);
    assert_eq!(
        recover_history_file_operation(&f.database, &pending.operation_id, MoveRecovery::Finish)
            .unwrap()
            .status,
        "purged"
    );
    assert!(pending_history_move(&f.database).unwrap().is_none());
    assert!(read_record(&f.connection(), &id).is_err());
}

#[test]
fn transfer_source_unlink_barrier_must_succeed_again_before_recovery_forgets_intent() {
    let f = Fixture::new();
    let destination = f.root.join("transferred.png");
    let bytes = std::fs::read(&f.source).unwrap();
    let (pending, payload, stage) = f.prepare_transfer(&destination);
    let prepared = copy_prepared(&f.database, &pending, &payload).unwrap();
    let mut locked = native::LockedSource::open(&stage, &destination).unwrap();
    locked.rename().unwrap();
    locked.sync_parents().unwrap();
    drop(locked);
    let failure = DeletionBarrierFailure::new();
    assert!(native::remove_expected(&f.source, &prepared.stamp).is_err());
    assert!(!f.source.exists());
    assert!(
        recover_history_file_operation(&f.database, &pending.operation_id, MoveRecovery::Finish)
            .is_err()
    );
    assert_eq!(
        capture_row(&f.connection(), 1).unwrap().file_path,
        path_text(&f.source).unwrap()
    );
    assert!(pending_history_move(&f.database).unwrap().is_some());
    assert_eq!(std::fs::read(&destination).unwrap(), bytes);
    drop(failure);
    recover_history_file_operation(&f.database, &pending.operation_id, MoveRecovery::Finish)
        .unwrap();
    assert_eq!(
        capture_row(&f.connection(), 1).unwrap().file_path,
        path_text(&destination).unwrap()
    );
    assert!(pending_history_move(&f.database).unwrap().is_none());
}

#[test]
fn canceled_transfer_retries_the_verified_staging_deletion_barrier() {
    let f = Fixture::new();
    let destination = f.root.join("transferred.png");
    let (pending, payload, stage) = f.prepare_transfer(&destination);
    let prepared = copy_prepared(&f.database, &pending, &payload).unwrap();
    let failure = DeletionBarrierFailure::new();
    assert!(native::remove_expected(&stage, prepared.destination_stamp.as_ref().unwrap()).is_err());
    assert!(!stage.exists() && f.source.exists());
    assert!(
        recover_history_file_operation(&f.database, &pending.operation_id, MoveRecovery::Cancel)
            .is_err()
    );
    assert!(pending_history_move(&f.database).unwrap().is_some());
    drop(failure);
    recover_history_file_operation(&f.database, &pending.operation_id, MoveRecovery::Cancel)
        .unwrap();
    assert!(pending_history_move(&f.database).unwrap().is_none());
    assert!(f.source.exists());
}

#[test]
fn native_case_aliases_are_reserved_and_distinct_case_sensitive_files_remain_usable() {
    let f = Fixture::new();
    let id = f.trash();
    let record = read_record(&f.connection(), &id).unwrap();
    let bytes = std::fs::read(&record.trash_path).unwrap();
    let alias = record.trash_path.with_file_name(
        record
            .trash_path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_uppercase(),
    );
    let native_alias = super::super::reservations::identity(&alias)
        .unwrap()
        .is_some();
    if native_alias {
        assert!(
            crate::history::import_capture_files(&f.database, std::slice::from_ref(&alias))
                .is_err()
        );
        assert!(crate::history::record_saved_capture(&f.database, &alias).is_err());
        let active = f.root.join("active.png");
        std::fs::write(&active, &bytes).unwrap();
        crate::history::record_saved_capture(&f.database, &active).unwrap();
        let entry = crate::history::lookup_history_entry(&f.database, 1)
            .unwrap()
            .unwrap();
        assert!(crate::history::relink_history_entry(&f.database, &entry, &alias).is_err());
        let parked = f.root.join("parked.png");
        std::fs::rename(&record.trash_path, &parked).unwrap();
        assert!(crate::history::move_history_file(&f.database, 1, &alias).is_err());
        assert!(transfer_history_file(&f.database, 1, &alias).is_err());
        assert!(pending_history_move(&f.database).unwrap().is_none());
        assert_eq!(std::fs::read(&active).unwrap(), bytes);
        assert!(!alias.exists());
        std::fs::rename(&parked, &record.trash_path).unwrap();
        purge_history_trash(&f.database, &id).unwrap();
        assert_eq!(std::fs::read(&active).unwrap(), bytes);
    } else {
        std::fs::write(&alias, &bytes).unwrap();
        crate::history::record_saved_capture(&f.database, &alias).unwrap();
        purge_history_trash(&f.database, &id).unwrap();
        assert_eq!(std::fs::read(&alias).unwrap(), bytes);
        assert!(
            crate::history::lookup_history_entry(&f.database, 1)
                .unwrap()
                .is_some()
        );
    }
    eprintln!("native temporary volume case-insensitive alias resolved: {native_alias}");
}

#[test]
fn unicode_name_and_parent_aliases_use_opened_identity_instead_of_string_case_guesses() {
    let f = Fixture::new();
    let directory = f.root.join("Caf\u{e9}");
    std::fs::create_dir(&directory).unwrap();
    let id = trash_history_file(&f.database, 1, &directory)
        .unwrap()
        .trash_id
        .unwrap();
    let record = read_record(&f.connection(), &id).unwrap();
    let bytes = std::fs::read(&record.trash_path).unwrap();
    let alternate_parent = f.root.join("Cafe\u{301}");
    let alternate = alternate_parent.join(record.trash_path.file_name().unwrap());
    let parent_alias = super::super::reservations::identity(&alternate)
        .unwrap()
        .is_some();
    if parent_alias {
        assert!(
            crate::history::import_capture_files(&f.database, std::slice::from_ref(&alternate))
                .is_err()
        );
    } else {
        std::fs::create_dir(&alternate_parent).unwrap();
        std::fs::write(&alternate, &bytes).unwrap();
        crate::history::record_saved_capture(&f.database, &alternate).unwrap();
    }
    let unicode = record.trash_path.with_file_name(
        record
            .trash_path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .replacen('s', "\u{17f}", 1),
    );
    let unicode_alias = super::super::reservations::identity(&unicode)
        .unwrap()
        .is_some();
    if unicode_alias {
        assert!(
            crate::history::import_capture_files(&f.database, std::slice::from_ref(&unicode))
                .is_err()
        );
    } else {
        std::fs::write(&unicode, &bytes).unwrap();
        crate::history::record_saved_capture(&f.database, &unicode).unwrap();
    }
    purge_history_trash(&f.database, &id).unwrap();
    if !parent_alias {
        assert_eq!(std::fs::read(&alternate).unwrap(), bytes);
    }
    if !unicode_alias {
        assert_eq!(std::fs::read(&unicode).unwrap(), bytes);
    }
    eprintln!(
        "native normalization-parent alias: {parent_alias}; native long-s filename alias: {unicode_alias}"
    );
}

#[test]
fn hardlink_and_directory_aliases_cannot_adopt_owned_trash() {
    let f = Fixture::new();
    let id = f.trash();
    let record = read_record(&f.connection(), &id).unwrap();
    let bytes = std::fs::read(&record.trash_path).unwrap();
    let link = f.root.join("unrelated-name.png");
    std::fs::hard_link(&record.trash_path, &link).unwrap();
    assert!(
        crate::history::import_capture_files(&f.database, std::slice::from_ref(&link)).is_err()
    );
    #[cfg(unix)]
    {
        let directory = f.root.join("directory-alias");
        std::os::unix::fs::symlink(&f.trash, &directory).unwrap();
        let path = directory.join(record.trash_path.file_name().unwrap());
        assert!(
            crate::history::import_capture_files(&f.database, std::slice::from_ref(&path)).is_err()
        );
    }
    purge_history_trash(&f.database, &id).unwrap();
    assert_eq!(std::fs::read(&link).unwrap(), bytes);
}

#[test]
fn identity_priming_commits_bounded_batches_and_resumes_after_cancellation() {
    let f = Fixture::new();
    let original = std::fs::read(&f.source).unwrap();
    let database = f.root.join("legacy-many.sqlite3");
    let row = capture_row(&f.connection(), 1).unwrap();
    let mut legacy = Connection::open(&database).unwrap();
    crate::history::migrations()
        .to_version(&mut legacy, 7)
        .unwrap();
    insert_legacy_capture(&legacy, &row, 1, &f.source);
    for id in 2..=351 {
        insert_legacy_capture(&legacy, &row, id, &f.root.join(format!("missing-{id}.png")));
    }
    drop(legacy);
    let first = prime_history_file_identities(&database, None, || {
        Connection::open(&database)
            .unwrap()
            .query_row(
                "SELECT count(*) FROM captures WHERE file_identity IS NOT NULL",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(0)
            >= 100
    })
    .unwrap();
    assert!(first.canceled && !first.complete);
    assert_eq!(first.indexed, 100);
    assert_eq!(first.next_cursor, Some(100));
    assert_eq!(first.remaining, None);
    assert_eq!(std::fs::read(&f.source).unwrap(), original);
    let second = prime_history_file_identities(&database, first.next_cursor, || false).unwrap();
    assert!(second.complete);
    assert_eq!(second.indexed, 251);
    assert_eq!(second.remaining, Some(0));
    assert_eq!(std::fs::read(&f.source).unwrap(), original);
    assert_eq!(
        Connection::open(&database)
            .unwrap()
            .query_row(
                "SELECT count(*) FROM captures WHERE file_identity IS NOT NULL",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        351
    );
}

#[test]
fn identity_priming_reports_unavailable_references_across_continuations_without_touching_files() {
    let f = Fixture::new();
    let original = std::fs::read(&f.source).unwrap();
    let database = f.root.join("legacy-unavailable.sqlite3");
    let row = capture_row(&f.connection(), 1).unwrap();
    let mut legacy = Connection::open(&database).unwrap();
    crate::history::migrations()
        .to_version(&mut legacy, 7)
        .unwrap();
    insert_legacy_capture(&legacy, &row, 1, &f.source);
    insert_legacy_capture(&legacy, &row, 2, Path::new("unavailable-relative.png"));
    insert_legacy_capture(&legacy, &row, 3, &f.root.join("missing.png"));
    drop(legacy);
    let first = prime_history_file_identities(&database, None, || false).unwrap();
    assert!(!first.complete);
    assert_eq!(first.unavailable, 1);
    assert_eq!(first.unavailable_ids, vec![2]);
    assert_eq!(first.remaining, Some(1));
    assert_eq!(first.missing, 1);
    assert_eq!(std::fs::read(&f.source).unwrap(), original);
    let second = prime_history_file_identities(&database, Some(2), || false).unwrap();
    assert!(!second.complete);
    assert_eq!(second.unavailable, 0);
    assert_eq!(second.remaining, Some(1));
    assert_eq!(std::fs::read(&f.source).unwrap(), original);
}

#[cfg(unix)]
#[test]
fn legacy_arbitrary_symlink_stage_adoption_is_discovered_before_finish_or_cancel() {
    for action in [MoveRecovery::Finish, MoveRecovery::Cancel] {
        let f = Fixture::new();
        let database = f.root.join("legacy-link.sqlite3");
        let destination = f.root.join("transferred.png");
        let stage = f.root.join(format!(
            ".screenshots-transfer-{}.png",
            uuid::Uuid::new_v4()
        ));
        std::fs::copy(&f.source, &stage).unwrap();
        let bytes = std::fs::read(&stage).unwrap();
        let alias = f.root.join("unrelated-legacy-reference.png");
        std::os::unix::fs::symlink(&stage, &alias).unwrap();
        let row = capture_row(&f.connection(), 1).unwrap();
        let pending = PendingMove {
            operation_id: uuid::Uuid::new_v4().to_string(),
            capture_id: 1,
            source_path: f.source.clone(),
            destination_path: destination.clone(),
            kind: "transfer",
        };
        let payload = Payload {
            version: 1,
            row: row.clone(),
            source_path: f.source.clone(),
            destination_path: destination.clone(),
            stamp: native::inspect(&f.source).unwrap().unwrap(),
            action: FileAction::Transfer {
                stage_path: stage.clone(),
            },
            destination_stamp: native::inspect(&stage).unwrap(),
        };
        let mut legacy = Connection::open(&database).unwrap();
        crate::history::migrations()
            .to_version(&mut legacy, 7)
            .unwrap();
        insert_legacy_capture(&legacy, &row, 1, &f.source);
        legacy.execute("INSERT INTO pending_file_move(singleton,operation_id,capture_id,source_path,destination_path,payload)VALUES(1,?1,1,?2,?3,?4)",
            params![pending.operation_id,f.source.to_str().unwrap(),destination.to_str().unwrap(),serde_json::to_string(&payload).unwrap()]).unwrap();
        insert_legacy_capture(&legacy, &row, 2, &alias);
        drop(legacy);
        assert!(recover_history_file_operation(&database, &pending.operation_id, action).is_err());
        assert_eq!(std::fs::read(&alias).unwrap(), bytes);
        assert_eq!(std::fs::read(&stage).unwrap(), bytes);
        assert!(!destination.exists());
        assert!(pending_history_move(&database).unwrap().is_some());
        crate::history::remove_history_entry(&database, 2).unwrap();
        recover_history_file_operation(&database, &pending.operation_id, action).unwrap();
        assert_eq!(
            std::fs::read(if matches!(action, MoveRecovery::Finish) {
                &destination
            } else {
                &f.source
            })
            .unwrap(),
            bytes
        );
    }
}
