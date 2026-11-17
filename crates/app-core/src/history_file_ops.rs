//! Journaled file operations with explicit, verified finish/cancel recovery.

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

#[cfg(test)]
#[path = "history_format_lifecycle_tests.rs"]
mod format_tests;
#[path = "history_lifecycle.rs"]
mod lifecycle;
#[path = "history_file_ops_native.rs"]
mod native;
#[path = "history_reservations.rs"]
mod reservations;
pub use lifecycle::{
    RetentionPlan, RetentionPolicy, RetentionReport, TrashEntry, TrashPage, apply_retention,
    plan_retention, purge_history_trash, recover_history_file_operation, restore_history_trash,
    transfer_history_file, trash_history_file, trash_history_page,
};
pub use reservations::IdentityIndexReport;
pub(super) use reservations::{
    MIGRATION as RESERVATION_MIGRATION, register as register_reservations,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PendingMove {
    pub operation_id: String,
    pub capture_id: i64,
    pub source_path: PathBuf,
    pub destination_path: PathBuf,
    pub kind: &'static str,
}

#[derive(Debug, Serialize)]
pub struct MoveOutcome {
    pub operation_id: String,
    pub capture_id: i64,
    pub source_path: PathBuf,
    pub destination_path: PathBuf,
    pub status: &'static str,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trash_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preserved_path: Option<PathBuf>,
}

#[derive(Clone, Copy)]
pub enum MoveRecovery {
    Finish,
    Cancel,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptureRow {
    id: i64,
    file_path: String,
    thumbnail_path: Option<String>,
    mime_type: String,
    width: u32,
    height: u32,
    file_size: i64,
    created_at: String,
    source: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    version: u8,
    row: CaptureRow,
    source_path: PathBuf,
    destination_path: PathBuf,
    stamp: native::FileStamp,
    #[serde(default, skip_serializing_if = "FileAction::is_move")]
    action: FileAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    destination_stamp: Option<native::FileStamp>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum FileAction {
    #[default]
    Move,
    Transfer {
        stage_path: PathBuf,
    },
    Trash {
        trash_id: String,
        trashed_at: String,
    },
    Restore {
        trash_id: String,
    },
    Purge {
        trash_id: String,
        trashed_at: String,
    },
}
impl FileAction {
    fn is_move(&self) -> bool {
        matches!(self, Self::Move)
    }
    fn name(&self) -> &'static str {
        match self {
            Self::Move => "move",
            Self::Transfer { .. } => "transfer",
            Self::Trash { .. } => "trash",
            Self::Restore { .. } => "restore",
            Self::Purge { .. } => "purge",
        }
    }
    fn trash_id(&self) -> Option<&str> {
        match self {
            Self::Move | Self::Transfer { .. } => None,
            Self::Trash { trash_id, .. }
            | Self::Restore { trash_id }
            | Self::Purge { trash_id, .. } => Some(trash_id),
        }
    }
}

fn bounded_text(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

fn path_text(path: &Path) -> Result<&str> {
    let text = path.to_str().context("History file path must be UTF-8")?;
    ensure!(
        path.is_absolute() && bounded_text(text, 4096),
        "Invalid bounded history file path"
    );
    Ok(text)
}

fn normalized_path(path: &Path) -> Result<PathBuf> {
    path_text(path)?;
    let path: PathBuf = path.components().collect();
    ensure!(
        matches!(path.components().next_back(), Some(Component::Normal(_))),
        "History file path needs a filename"
    );
    let parent = path
        .parent()
        .context("History file path needs a parent")?
        .canonicalize()?;
    let normalized = parent.join(
        path.file_name()
            .context("History file path needs a filename")?,
    );
    path_text(&normalized)?;
    Ok(normalized)
}

fn validate_row(row: &CaptureRow) -> Result<()> {
    ensure!(row.id > 0, "Invalid capture row ID");
    path_text(Path::new(&row.file_path))?;
    ensure!(
        row.thumbnail_path
            .as_ref()
            .is_none_or(|path| path.len() <= 4096 && !path.chars().any(char::is_control)),
        "Invalid thumbnail reference"
    );
    let format = crate::capture_format::format_from_mime(&row.mime_type)?;
    ensure!(
        bounded_text(&row.created_at, 64) && bounded_text(&row.source, 64),
        "Invalid capture row metadata"
    );
    app_dimensions(row.width, row.height)?;
    ensure!(
        (8..=crate::capture_format::encoded_limit(format) as i64).contains(&row.file_size),
        "Capture file size exceeds move admission"
    );
    Ok(())
}

fn app_dimensions(width: u32, height: u32) -> Result<()> {
    crate::validate_image_size(width, height)?;
    Ok(())
}

fn capture_row(connection: &Connection, id: i64) -> Result<CaptureRow> {
    let row = connection.query_row(
        "SELECT id,CASE WHEN typeof(file_path)='text' AND length(CAST(file_path AS BLOB))<=4096 THEN file_path END,
         CASE WHEN thumbnail_path IS NULL OR (typeof(thumbnail_path)='text' AND length(CAST(thumbnail_path AS BLOB))<=4096) THEN thumbnail_path ELSE 0 END,
         CASE WHEN typeof(mime_type)='text' AND length(CAST(mime_type AS BLOB))<=64 THEN mime_type END,width,height,file_size,
         CASE WHEN typeof(created_at)='text' AND length(CAST(created_at AS BLOB))<=64 THEN created_at END,
         CASE WHEN typeof(source)='text' AND length(CAST(source AS BLOB))<=64 THEN source END FROM captures WHERE id=?1",
        [id], |row| Ok(CaptureRow { id: row.get(0)?, file_path: row.get(1)?, thumbnail_path: row.get(2)?, mime_type: row.get(3)?, width: row.get(4)?, height: row.get(5)?, file_size: row.get(6)?, created_at: row.get(7)?, source: row.get(8)? }),
    ).optional()?.context("History capture ID was not found")?;
    validate_row(&row)?;
    Ok(row)
}

pub(super) fn selected_path(connection: &Connection, id: i64) -> Result<PathBuf> {
    Ok(PathBuf::from(capture_row(connection, id)?.file_path))
}

fn operation_id(value: &str) -> Result<()> {
    ensure!(
        value.len() == 36
            && uuid::Uuid::parse_str(value).is_ok_and(|id| id.hyphenated().to_string() == value),
        "Invalid history move operation ID"
    );
    Ok(())
}

fn read_pending(connection: &Connection) -> Result<Option<(PendingMove, Payload)>> {
    let record = connection.query_row(
        "SELECT CASE WHEN typeof(operation_id)='text' AND length(operation_id)=36 THEN operation_id END,capture_id,
         CASE WHEN typeof(source_path)='text' AND length(CAST(source_path AS BLOB))<=4096 THEN source_path END,
         CASE WHEN typeof(destination_path)='text' AND length(CAST(destination_path AS BLOB))<=4096 THEN destination_path END,
         CASE WHEN typeof(payload)='text' AND length(CAST(payload AS BLOB))<=16384 THEN payload END FROM pending_file_move WHERE singleton=1",
        [], |row| Ok((PendingMove { operation_id: row.get(0)?, capture_id: row.get(1)?, source_path: PathBuf::from(row.get::<_,String>(2)?), destination_path: PathBuf::from(row.get::<_,String>(3)?), kind: "move" }, row.get::<_,String>(4)?)),
    ).optional()?;
    let Some((mut pending, json)) = record else {
        return Ok(None);
    };
    operation_id(&pending.operation_id)?;
    path_text(&pending.source_path)?;
    path_text(&pending.destination_path)?;
    ensure!(
        pending.source_path != pending.destination_path,
        "Invalid history move paths"
    );
    let payload: Payload = serde_json::from_str(&json)
        .map_err(|_| anyhow::anyhow!("Invalid history move journal payload"))?;
    validate_row(&payload.row)?;
    lifecycle::validate_action(&payload)?;
    pending.kind = payload.action.name();
    ensure!(
        payload.version == 1
            && payload.row.id == pending.capture_id
            && payload.source_path == pending.source_path
            && payload.destination_path == pending.destination_path,
        "Invalid history move journal version or row"
    );
    let stamp = &payload.stamp;
    ensure!(
        stamp.format == crate::capture_format::format_from_mime(&payload.row.mime_type)?
            && stamp.size == payload.row.file_size as u64
            && stamp.width == payload.row.width
            && stamp.height == payload.row.height
            && stamp.sha256.len() == 64
            && stamp
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "Invalid history move file stamp"
    );
    Ok(Some((pending, payload)))
}

/// Inspect a pending move without creating or migrating a database or accessing images.
pub fn pending_history_move(database: &Path) -> Result<Option<PendingMove>> {
    let connection = crate::storage::open_read_only(database)?;
    let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    ensure!(
        (1..=10).contains(&version),
        "Unsupported history schema version"
    );
    if version < 5 {
        return Ok(None);
    }
    Ok(read_pending(&connection)?.map(|(pending, _)| pending))
}

/// Refresh metadata identities in bounded batches; never read image contents or move files.
pub fn prime_history_file_identities(
    database: &Path,
    cursor: Option<i64>,
    cancelled: impl Fn() -> bool,
) -> Result<IdentityIndexReport> {
    supported()?;
    ensure!(
        cursor.is_none_or(|id| id > 0),
        "Invalid identity index cursor"
    );
    ensure!(
        !cancelled(),
        "Reference identity preparation canceled before opening storage"
    );
    let mut connection = super::open_existing(database)?;
    reservations::prime(&mut connection, cursor, false, &cancelled)
}

fn prepare_identities(connection: &mut Connection, cancelled: &dyn Fn() -> bool) -> Result<()> {
    let progress = reservations::prime(connection, None, true, cancelled)?;
    ensure!(
        progress.complete,
        "Reference identity preparation incomplete: {} indexed, {} OS-missing, {} unavailable, {} unresolved, {} deferred; use index-identities to continue; image files are unchanged",
        progress.indexed,
        progress.missing,
        progress.unavailable,
        progress
            .remaining
            .map_or_else(|| "unknown".to_owned(), |value| value.to_string()),
        progress
            .protected
            .map_or_else(|| "unknown".to_owned(), |value| value.to_string())
    );
    Ok(())
}

fn prepare(
    database: &Path,
    row: CaptureRow,
    source: PathBuf,
    destination: PathBuf,
    stamp: native::FileStamp,
) -> Result<(PendingMove, Payload)> {
    prepare_action(database, row, source, destination, stamp, FileAction::Move)
}

fn prepare_action(
    database: &Path,
    row: CaptureRow,
    source: PathBuf,
    destination: PathBuf,
    stamp: native::FileStamp,
    action: FileAction,
) -> Result<(PendingMove, Payload)> {
    prepare_action_checked(database, row, source, destination, stamp, action, &|| false)
}

fn prepare_action_checked(
    database: &Path,
    row: CaptureRow,
    source: PathBuf,
    destination: PathBuf,
    stamp: native::FileStamp,
    action: FileAction,
    cancelled: &dyn Fn() -> bool,
) -> Result<(PendingMove, Payload)> {
    ensure!(
        stamp.format == crate::capture_format::format_from_mime(&row.mime_type)?
            && crate::ImageExportFormat::from_path(&destination)? == stamp.format
            && stamp.width == row.width
            && stamp.height == row.height
            && stamp.size == row.file_size as u64,
        "Source image no longer matches its capture row"
    );
    let pending = PendingMove {
        operation_id: uuid::Uuid::new_v4().to_string(),
        capture_id: row.id,
        source_path: source,
        destination_path: destination,
        kind: action.name(),
    };
    let payload = Payload {
        version: 1,
        row,
        source_path: pending.source_path.clone(),
        destination_path: pending.destination_path.clone(),
        stamp,
        action,
        destination_stamp: None,
    };
    let json = serde_json::to_string(&payload)?;
    let limit = if matches!(payload.action, FileAction::Transfer { .. }) {
        15360
    } else {
        16384
    };
    ensure!(
        json.len() <= limit,
        "History move journal exceeds its byte limit"
    );
    let mut connection = super::open_existing(database)?;
    ensure!(
        read_pending(&connection)?.is_none(),
        "A history file operation is pending; recover it first"
    );
    prepare_identities(&mut connection, cancelled)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    ensure!(
        read_pending(&transaction)?.is_none(),
        "A history file move is pending; recover it first"
    );
    lifecycle::verify_action(&transaction, &payload)?;
    let owner = if matches!(
        payload.action,
        FileAction::Move | FileAction::Transfer { .. } | FileAction::Trash { .. }
    ) {
        Some(payload.row.id)
    } else {
        None
    };
    reservations::check_captures(&transaction, &pending.destination_path, owner)?;
    reservations::check_trash(&transaction, &pending.destination_path, None)?;
    if let FileAction::Transfer { stage_path } = &payload.action {
        reservations::check_captures(&transaction, stage_path, owner)?;
        reservations::check_trash(&transaction, stage_path, None)?;
    }
    transaction.execute("INSERT INTO pending_file_move(singleton,operation_id,capture_id,source_path,destination_path,payload) VALUES(1,?1,?2,?3,?4,?5)",params![pending.operation_id,pending.capture_id,path_text(&pending.source_path)?,path_text(&pending.destination_path)?,json])?;
    transaction.commit().with_context(|| {
        format!(
            "Preparing history move {} failed; inspect this operation before retrying",
            pending.operation_id
        )
    })?;
    Ok((pending, payload))
}

fn finalize_transaction(
    transaction: rusqlite::Transaction<'_>,
    pending: &PendingMove,
    payload: &Payload,
    action: MoveRecovery,
) -> Result<MoveOutcome> {
    verify_pending(&transaction, pending, payload)?;
    transaction.execute(
        "DELETE FROM pending_file_move WHERE singleton=1 AND operation_id=?1",
        [&pending.operation_id],
    )?;
    if matches!(action, MoveRecovery::Finish)
        && matches!(
            payload.action,
            FileAction::Move | FileAction::Transfer { .. }
        )
    {
        let row = &payload.row;
        let size = i64::try_from(payload.stamp.size).context("Invalid move file size")?;
        let changed=transaction.execute("UPDATE captures SET file_path=?1,thumbnail_path=NULL,width=?2,height=?3,file_size=?4
            WHERE id=?5 AND file_path=?6 AND thumbnail_path IS ?7 AND mime_type=?8 AND width=?9 AND height=?10 AND file_size=?11 AND created_at=?12 AND source=?13",
            params![path_text(&pending.destination_path)?,payload.stamp.width,payload.stamp.height,size,row.id,row.file_path,row.thumbnail_path,row.mime_type,row.width,row.height,row.file_size,row.created_at,row.source])?;
        ensure!(
            changed == 1,
            "History capture changed before move finalization"
        );
    }
    if matches!(action, MoveRecovery::Finish)
        && !matches!(
            payload.action,
            FileAction::Move | FileAction::Transfer { .. }
        )
    {
        lifecycle::finalize_action(&transaction, pending, payload)?;
    }
    transaction.commit()?;
    Ok(MoveOutcome {
        operation_id: pending.operation_id.clone(),
        capture_id: pending.capture_id,
        source_path: pending.source_path.clone(),
        destination_path: pending.destination_path.clone(),
        status: if matches!(action, MoveRecovery::Finish) {
            match payload.action {
                FileAction::Move => "moved",
                FileAction::Transfer { .. } => "transferred",
                FileAction::Trash { .. } => "trashed",
                FileAction::Restore { .. } => "restored",
                FileAction::Purge { .. } => "purged",
            }
        } else {
            "canceled"
        },
        kind: payload.action.name(),
        trash_id: payload.action.trash_id().map(str::to_owned),
        preserved_path: None,
    })
}

fn verify_pending(connection: &Connection, pending: &PendingMove, payload: &Payload) -> Result<()> {
    ensure!(
        read_pending(connection)?
            .is_some_and(|(current, data)| current == *pending && data == *payload),
        "History move journal changed during recovery"
    );
    lifecycle::verify_action(connection, payload)?;
    Ok(())
}

fn apply_prepared(
    database: &Path,
    pending: &PendingMove,
    payload: &Payload,
    operation: impl FnOnce() -> Result<()>,
) -> Result<MoveOutcome> {
    let mut connection = super::open_existing(database)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    verify_pending(&transaction, pending, payload)?;
    operation()?;
    finalize_transaction(transaction, pending, payload, MoveRecovery::Finish)
}

fn supported() -> Result<()> {
    ensure!(
        cfg!(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "windows"
        )),
        "History file moves are unavailable on this operating system"
    );
    Ok(())
}

/// Move one saved image after persisting a recoverable intent. Call off the foreground executor.
pub fn move_history_file(database: &Path, id: i64, destination: &Path) -> Result<MoveOutcome> {
    supported()?;
    ensure!(id > 0, "History capture ID must be positive");
    let destination = normalized_path(destination)?;
    let connection = crate::storage::open_read_only(database)?;
    let row = capture_row(&connection, id)?;
    drop(connection);
    let source = normalized_path(Path::new(&row.file_path))?;
    ensure!(
        source != destination,
        "History move source and destination are identical"
    );
    let mut locked = native::LockedSource::open(&source, &destination)?;
    let (pending, payload) = prepare(database, row, source, destination, locked.stamp().clone())?;
    apply_prepared(database, &pending, &payload, || {
        locked.rename()?;
        locked.verify_destination()?;
        locked.sync_parents()?;
        Ok(())
    })
    .with_context(|| {
        format!(
            "History move {} did not return confirmed completion; inspect pending-move before retrying",
            pending.operation_id
        )
    })
}

/// Recover only an unambiguous filesystem state; this never moves or deletes files.
pub fn recover_history_move(
    database: &Path,
    id: &str,
    action: MoveRecovery,
) -> Result<MoveOutcome> {
    recover_operation(database, id, action, false)
}

fn recover_operation(
    database: &Path,
    id: &str,
    action: MoveRecovery,
    allow_lifecycle: bool,
) -> Result<MoveOutcome> {
    supported()?;
    operation_id(id)?;
    (|| {
        let mut connection = super::open_existing(database)?;
        let (current,_)=read_pending(&connection)?.context("No history file move is pending")?;
        ensure!(current.operation_id==id,"History move operation ID does not match");
        prepare_identities(&mut connection,&||false)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (pending, payload) =
            read_pending(&transaction)?.context("No history file move is pending")?;
        ensure!(
            pending.operation_id == id,
            "History move operation ID does not match"
        );
        ensure!(allow_lifecycle || payload.action.is_move(), "Use recover-file for a trash, restore or purge operation");
        lifecycle::verify_action(&transaction, &payload)?;
        ensure!(
            normalized_path(Path::new(&payload.row.file_path))? == pending.source_path,
            "Original history path changed its parent resolution"
        );
        ensure!(
            normalized_path(&pending.source_path)? == pending.source_path
                && normalized_path(&pending.destination_path)? == pending.destination_path,
            "History move parent paths changed"
        );
        if matches!(payload.action,FileAction::Transfer{..}) {
            return lifecycle::recover_transfer(transaction,&pending,&payload,action);
        }
        let source = native::inspect(&pending.source_path)?;
        let destination = native::inspect(&pending.destination_path)?;
        if matches!(payload.action, FileAction::Purge {..}) && matches!(action, MoveRecovery::Finish) {
            ensure!(source.is_none() && (destination.is_none() || destination.as_ref() == Some(&payload.stamp)),
                    "Cannot finish purge: file state is ambiguous or quarantine has not happened");
            if destination.is_some() {
                native::remove_expected(&pending.destination_path, &payload.stamp)?;
            }
            native::sync_absent(&pending.source_path)?;
            native::sync_absent(&pending.destination_path)?;
            return finalize_transaction(transaction, &pending, &payload, action);
        }
        match action {
            MoveRecovery::Finish => {
                ensure!(
                    source.is_none() && destination.as_ref() == Some(&payload.stamp),
                    "Cannot finish history move: file state is ambiguous"
                );
                native::sync_paths(
                    &pending.source_path,
                    &pending.destination_path,
                    &payload.stamp,
                )?;
            }
            MoveRecovery::Cancel => ensure!(
                source.as_ref() == Some(&payload.stamp) && destination.is_none(),
                "Cannot cancel history move: file state is ambiguous"
            ),
        }
        finalize_transaction(transaction, &pending, &payload, action)
    })()
    .with_context(|| {
        format!("History move {id} did not return confirmed recovery; inspect paths and pending-move before retrying")
    })
}

#[cfg(all(
    test,
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
mod tests {
    use super::*;

    struct Fixture {
        root: PathBuf,
        database: PathBuf,
        source: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir()
                .join(format!("screenshots-history-file-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&root).unwrap();
            let root = root.canonicalize().unwrap();
            let source = root.join("original.png");
            image::RgbaImage::from_pixel(3, 2, image::Rgba([7, 21, 83, 129]))
                .save(&source)
                .unwrap();
            let database = root.join("history.sqlite3");
            super::super::record_saved_capture(&database, &source).unwrap();
            Self {
                root,
                database,
                source,
            }
        }
        fn row(&self) -> CaptureRow {
            capture_row(&super::super::open_existing(&self.database).unwrap(), 1).unwrap()
        }
        fn destination(&self) -> PathBuf {
            self.root.join("moved.png")
        }
        fn prepared(&self) -> (PendingMove, Payload) {
            let locked = native::LockedSource::open(&self.source, &self.destination()).unwrap();
            prepare(
                &self.database,
                self.row(),
                self.source.clone(),
                self.destination(),
                locked.stamp().clone(),
            )
            .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn complete_move_preserves_bytes_identity_metadata_and_search() {
        let f = Fixture::new();
        let before = f.row();
        let bytes = std::fs::read(&f.source).unwrap();
        let outcome = move_history_file(&f.database, 1, &f.destination()).unwrap();
        assert_eq!(outcome.status, "moved");
        assert_eq!(outcome.capture_id, 1);
        assert!(!f.source.exists());
        assert_eq!(std::fs::read(f.destination()).unwrap(), bytes);
        let after = f.row();
        assert_eq!(after.id, before.id);
        assert_eq!(after.created_at, before.created_at);
        assert_eq!(after.source, before.source);
        assert_eq!(after.file_path, f.destination().to_str().unwrap());
        assert!(after.thumbnail_path.is_none());
        assert!(pending_history_move(&f.database).unwrap().is_none());
        let connection = super::super::open_existing(&f.database).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM captures_search WHERE captures_search MATCH 'moved'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn unapplied_intent_reserves_rows_and_paths_until_explicit_cancel() {
        let f = Fixture::new();
        let bytes = std::fs::read(&f.source).unwrap();
        let (pending, _) = f.prepared();
        assert_eq!(
            pending_history_move(&f.database).unwrap(),
            Some(pending.clone())
        );
        assert!(super::super::remove_history_entry(&f.database, 1).is_err());
        let connection = super::super::open_existing(&f.database).unwrap();
        assert!(
            connection
                .execute("UPDATE captures SET width=4 WHERE id=1", [])
                .is_err()
        );
        assert!(connection.execute("INSERT INTO captures(file_path,mime_type,width,height,file_size,created_at,source) VALUES(?1,'image/png',1,1,9,'date','import')",[f.destination().to_str().unwrap()]).is_err());
        assert!(move_history_file(&f.database, 1, &f.root.join("another.png")).is_err());
        assert!(
            recover_history_move(&f.database, &pending.operation_id, MoveRecovery::Finish).is_err()
        );
        let outcome =
            recover_history_move(&f.database, &pending.operation_id, MoveRecovery::Cancel).unwrap();
        assert_eq!(outcome.status, "canceled");
        assert_eq!(std::fs::read(&f.source).unwrap(), bytes);
        assert!(!f.destination().exists());
        assert!(pending_history_move(&f.database).unwrap().is_none());
        assert!(
            recover_history_move(&f.database, &pending.operation_id, MoveRecovery::Cancel).is_err()
        );
    }

    #[test]
    fn renamed_intent_finishes_without_renaming_again() {
        let f = Fixture::new();
        let before = f.row();
        let mut locked = native::LockedSource::open(&f.source, &f.destination()).unwrap();
        let (pending, _) = prepare(
            &f.database,
            before,
            f.source.clone(),
            f.destination(),
            locked.stamp().clone(),
        )
        .unwrap();
        locked.rename().unwrap();
        drop(locked);
        assert!(
            recover_history_move(&f.database, &pending.operation_id, MoveRecovery::Cancel).is_err()
        );
        assert!(pending_history_move(&f.database).unwrap().is_some());
        let outcome =
            recover_history_move(&f.database, &pending.operation_id, MoveRecovery::Finish).unwrap();
        assert_eq!(outcome.status, "moved");
        assert_eq!(f.row().file_path, f.destination().to_str().unwrap());
        assert!(pending_history_move(&f.database).unwrap().is_none());
    }

    #[test]
    fn conflicts_and_tampered_rows_keep_files_and_journal() {
        let f = Fixture::new();
        let (pending, payload) = f.prepared();
        std::fs::copy(&f.source, f.destination()).unwrap();
        for action in [MoveRecovery::Finish, MoveRecovery::Cancel] {
            assert!(recover_history_move(&f.database, &pending.operation_id, action).is_err());
        }
        assert!(f.source.exists() && f.destination().exists());
        std::fs::remove_file(f.destination()).unwrap();
        let connection = super::super::open_existing(&f.database).unwrap();
        connection
            .execute_batch("DROP TRIGGER captures_move_update")
            .unwrap();
        connection
            .execute("UPDATE captures SET source='changed' WHERE id=1", [])
            .unwrap();
        let mut finalize_connection = super::super::open_existing(&f.database).unwrap();
        let transaction = finalize_connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        assert!(
            finalize_transaction(transaction, &pending, &payload, MoveRecovery::Cancel).is_err()
        );
        assert!(pending_history_move(&f.database).unwrap().is_some());
        let unchanged = std::fs::read(&f.source).unwrap();
        assert!(
            recover_history_move(&f.database, &pending.operation_id, MoveRecovery::Cancel).is_err()
        );
        assert_eq!(std::fs::read(&f.source).unwrap(), unchanged);
    }

    #[test]
    fn malformed_journal_and_preexisting_destination_fail_closed() {
        let f = Fixture::new();
        std::fs::write(f.destination(), b"keep").unwrap();
        assert!(move_history_file(&f.database, 1, &f.destination()).is_err());
        assert_eq!(std::fs::read(f.destination()).unwrap(), b"keep");
        assert!(pending_history_move(&f.database).unwrap().is_none());
        std::fs::remove_file(f.destination()).unwrap();
        let (pending, mut payload) = f.prepared();
        payload.stamp.sha256 = "not-a-hash".into();
        let connection = super::super::open_existing(&f.database).unwrap();
        connection
            .execute(
                "UPDATE pending_file_move SET payload=?1",
                [serde_json::to_string(&payload).unwrap()],
            )
            .unwrap();
        assert!(pending_history_move(&f.database).is_err());
        assert!(
            recover_history_move(&f.database, &pending.operation_id, MoveRecovery::Cancel).is_err()
        );
        assert!(f.source.exists());
        assert!(!f.destination().exists());
    }

    #[test]
    fn pending_inspection_does_not_migrate_old_or_create_missing_database() {
        let root =
            std::env::temp_dir().join(format!("screenshots-history-old-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let db = root.join("old.sqlite3");
        Connection::open(&db)
            .unwrap()
            .execute_batch("PRAGMA user_version=4")
            .unwrap();
        assert!(pending_history_move(&db).unwrap().is_none());
        assert_eq!(
            Connection::open(&db)
                .unwrap()
                .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
                .unwrap(),
            4
        );
        let missing = root.join("missing.sqlite3");
        assert!(pending_history_move(&missing).is_err());
        assert!(!missing.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn canceled_prepare_cannot_later_rename_and_active_move_excludes_recovery() {
        use std::cell::Cell;
        let f = Fixture::new();
        let (pending, payload) = f.prepared();
        recover_history_move(&f.database, &pending.operation_id, MoveRecovery::Cancel).unwrap();
        let renamed = Cell::new(false);
        assert!(
            apply_prepared(&f.database, &pending, &payload, || {
                renamed.set(true);
                Ok(())
            })
            .is_err()
        );
        assert!(!renamed.get());
        assert!(f.source.exists());
        assert!(!f.destination().exists());
        let mut locked = native::LockedSource::open(&f.source, &f.destination()).unwrap();
        let (pending, payload) = prepare(
            &f.database,
            f.row(),
            f.source.clone(),
            f.destination(),
            locked.stamp().clone(),
        )
        .unwrap();
        let outcome = apply_prepared(&f.database, &pending, &payload, || {
            assert!(
                recover_history_move(&f.database, &pending.operation_id, MoveRecovery::Cancel)
                    .is_err()
            );
            locked.rename()?;
            locked.verify_destination()?;
            locked.sync_parents()?;
            Ok(())
        })
        .unwrap();
        assert_eq!(outcome.status, "moved");
    }
}
