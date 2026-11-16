//! Explicit recoverable trash, restore and retention. No idle maintenance.

use super::*;
use chrono::{DateTime, Utc};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrashRecord {
    version: u8,
    row: CaptureRow,
    trash_path: PathBuf,
    stamp: native::FileStamp,
    trashed_at: String,
}

#[derive(Debug, Serialize)]
pub struct TrashEntry {
    pub sequence: i64,
    pub trash_id: String,
    pub original_capture_id: i64,
    pub original_path: PathBuf,
    pub trash_path: PathBuf,
    pub trashed_at: String,
    pub bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct TrashPage {
    pub entries: Vec<TrashEntry>,
    pub has_more: bool,
    pub next_cursor: Option<i64>,
}

fn timestamp(text: &str) -> Result<()> {
    ensure!(text.len() <= 64, "Invalid trash timestamp");
    let value = DateTime::parse_from_rfc3339(text).context("Invalid trash timestamp")?;
    ensure!(
        value.offset().local_minus_utc() == 0
            && value
                .with_timezone(&Utc)
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                == text,
        "Trash timestamps must use canonical UTC seconds"
    );
    Ok(())
}

fn owned_name(path: &Path, id: &str) -> Result<()> {
    operation_id(id)?;
    path_text(path)?;
    ensure!(
        path.file_name()
            .is_some_and(|name| name == format!(".screenshots-trash-{id}.png").as_str()),
        "Trash filename does not match its identity"
    );
    Ok(())
}

fn validate_record(id: &str, value: &TrashRecord) -> Result<()> {
    ensure!(
        value.version == 1,
        "Unsupported history trash record version"
    );
    validate_row(&value.row)?;
    owned_name(&value.trash_path, id)?;
    timestamp(&value.trashed_at)?;
    ensure!(
        value.stamp.size == value.row.file_size as u64
            && value.stamp.width == value.row.width
            && value.stamp.height == value.row.height
            && value.stamp.sha256.len() == 64
            && value
                .stamp
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "Invalid history trash fingerprint"
    );
    Ok(())
}

fn read_record(connection: &Connection, id: &str) -> Result<TrashRecord> {
    operation_id(id)?;
    let (path, at, json): (String, String, String) = connection.query_row(
        "SELECT CASE WHEN typeof(trash_path)='text' AND length(CAST(trash_path AS BLOB))<=4096 THEN trash_path END,
                CASE WHEN typeof(trashed_at)='text' AND length(trashed_at)<=64 THEN trashed_at END,
                CASE WHEN typeof(payload)='text' AND length(CAST(payload AS BLOB))<=16384 THEN payload END
         FROM history_trash WHERE trash_id=?1", [id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
    ).optional()?.context("History trash ID was not found")?;
    let value: TrashRecord = serde_json::from_str(&json)
        .map_err(|_| anyhow::anyhow!("Invalid bounded history trash record"))?;
    validate_record(id, &value)?;
    ensure!(
        path == path_text(&value.trash_path)? && at == value.trashed_at,
        "Trash columns differ from their owned record"
    );
    Ok(value)
}

pub(super) fn validate_action(payload: &Payload) -> Result<()> {
    if let FileAction::Purge { trashed_at, .. } = &payload.action {
        timestamp(trashed_at)?;
    }
    match &payload.action {
        FileAction::Move => (),
        FileAction::Transfer { stage_path } => {
            path_text(stage_path)?;
            let name = stage_path
                .file_name()
                .and_then(|name| name.to_str())
                .context("Invalid transfer staging filename")?;
            let id = name
                .strip_prefix(".screenshots-transfer-")
                .and_then(|name| name.strip_suffix(".png"))
                .context("Invalid transfer staging filename")?;
            operation_id(id)?;
            ensure!(
                stage_path.parent() == payload.destination_path.parent()
                    && stage_path != &payload.destination_path
                    && stage_path != &payload.source_path,
                "Transfer staging path must be distinct and beside its destination"
            );
            if let Some(stamp) = &payload.destination_stamp {
                ensure!(
                    same_contents(stamp, &payload.stamp),
                    "Transfer destination fingerprint does not match its source"
                );
            }
        }
        FileAction::Trash {
            trash_id,
            trashed_at,
        } => {
            owned_name(&payload.destination_path, trash_id)?;
            timestamp(trashed_at)?;
        }
        FileAction::Restore { trash_id } | FileAction::Purge { trash_id, .. } => {
            owned_name(&payload.source_path, trash_id)?;
            ensure!(
                payload.row.file_path == path_text(&payload.source_path)?,
                "Invalid trash operation source"
            );
            if matches!(payload.action, FileAction::Purge { .. }) {
                let name = payload
                    .destination_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .context("Invalid purge quarantine filename")?;
                let id = name
                    .strip_prefix(".screenshots-purge-")
                    .and_then(|name| name.strip_suffix(".png"))
                    .context("Invalid purge quarantine filename")?;
                operation_id(id)?;
                ensure!(
                    payload.source_path.parent() == payload.destination_path.parent(),
                    "Purge quarantine must stay in its trash directory"
                );
            }
        }
    }
    ensure!(
        matches!(payload.action, FileAction::Transfer { .. })
            || payload.destination_stamp.is_none(),
        "Only transfer operations can carry a destination fingerprint"
    );
    Ok(())
}

pub(super) fn verify_action(connection: &Connection, payload: &Payload) -> Result<()> {
    validate_action(payload)?;
    match &payload.action {
        FileAction::Move | FileAction::Transfer { .. } | FileAction::Trash { .. } => {
            ensure!(
                capture_row(connection, payload.row.id)? == payload.row,
                "History capture changed during file operation"
            );
            if let FileAction::Trash { trash_id, .. } = &payload.action {
                ensure!(
                    !connection.query_row(
                        "SELECT EXISTS(SELECT 1 FROM history_trash WHERE trash_id=?1)",
                        [trash_id],
                        |row| row.get::<_, bool>(0)
                    )?,
                    "Trash identity already exists"
                );
            }
        }
        FileAction::Restore { trash_id } | FileAction::Purge { trash_id, .. } => {
            let record = read_record(connection, trash_id)?;
            if let FileAction::Purge { trashed_at, .. } = &payload.action {
                ensure!(
                    &record.trashed_at == trashed_at,
                    "Trash age changed before purge; preserved"
                );
            }
            let mut row = record.row.clone();
            row.id = payload.row.id;
            row.file_path = path_text(&record.trash_path)?.to_owned();
            ensure!(
                row == payload.row
                    && record.trash_path == payload.source_path
                    && record.stamp == payload.stamp,
                "History trash record changed during operation"
            );
            if matches!(payload.action, FileAction::Restore { .. }) {
                ensure!(
                    !connection.query_row(
                        "SELECT EXISTS(SELECT 1 FROM captures WHERE id=?1)",
                        [payload.row.id],
                        |row| row.get::<_, bool>(0)
                    )?,
                    "Restore capture ID was claimed by another writer; preserved"
                );
            }
        }
    }
    let owner = if matches!(
        payload.action,
        FileAction::Move | FileAction::Transfer { .. } | FileAction::Trash { .. }
    ) {
        Some(payload.row.id)
    } else {
        None
    };
    let own_trash = if matches!(
        payload.action,
        FileAction::Restore { .. } | FileAction::Purge { .. }
    ) {
        payload.action.trash_id()
    } else {
        None
    };
    for path in [&payload.source_path, &payload.destination_path] {
        super::reservations::check_captures(connection, path, owner)?;
        super::reservations::check_trash(connection, path, own_trash)?;
    }
    if let FileAction::Transfer { stage_path } = &payload.action {
        super::reservations::check_captures(connection, stage_path, owner)?;
        super::reservations::check_trash(connection, stage_path, None)?;
    }
    Ok(())
}

pub(super) fn finalize_action(
    transaction: &rusqlite::Transaction<'_>,
    pending: &PendingMove,
    payload: &Payload,
) -> Result<()> {
    match &payload.action {
        FileAction::Move | FileAction::Transfer { .. } => unreachable!(),
        FileAction::Trash {
            trash_id,
            trashed_at,
        } => {
            let record = TrashRecord {
                version: 1,
                row: payload.row.clone(),
                trash_path: pending.destination_path.clone(),
                stamp: payload.stamp.clone(),
                trashed_at: trashed_at.clone(),
            };
            validate_record(trash_id, &record)?;
            let json = serde_json::to_string(&record)?;
            ensure!(
                json.len() <= 16384,
                "History trash record exceeds its byte limit"
            );
            ensure!(
                transaction.execute("DELETE FROM captures WHERE id=?1", [payload.row.id])? == 1,
                "History capture changed before trash commit"
            );
            transaction.execute("INSERT INTO history_trash(trash_id,trash_path,trashed_at,payload) VALUES(?1,?2,?3,?4)",
                                params![trash_id,path_text(&record.trash_path)?,trashed_at,json])?;
        }
        FileAction::Restore { trash_id } => {
            let record = read_record(transaction, trash_id)?;
            ensure!(
                transaction.execute("DELETE FROM history_trash WHERE trash_id=?1", [trash_id])?
                    == 1,
                "Trash record changed before restoration"
            );
            let row = record.row;
            transaction.execute("INSERT INTO captures(id,file_path,thumbnail_path,mime_type,width,height,file_size,created_at,source)
                                 VALUES(?1,?2,NULL,?3,?4,?5,?6,?7,?8)",
                                params![pending.capture_id,path_text(&pending.destination_path)?,row.mime_type,row.width,row.height,row.file_size,row.created_at,row.source])?;
        }
        FileAction::Purge { trash_id, .. } => {
            ensure!(
                transaction.execute("DELETE FROM history_trash WHERE trash_id=?1", [trash_id])?
                    == 1,
                "Trash record changed before purge commit"
            );
        }
    }
    Ok(())
}

fn existing_directory(path: &Path) -> Result<PathBuf> {
    path_text(path)?;
    let metadata = std::fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "Choose an existing regular trash directory"
    );
    let resolved = path.canonicalize()?;
    path_text(&resolved)?;
    Ok(resolved)
}

fn same_contents(a: &native::FileStamp, b: &native::FileStamp) -> bool {
    a.size == b.size && a.sha256 == b.sha256 && a.width == b.width && a.height == b.height
}

fn copy_prepared(database: &Path, pending: &PendingMove, payload: &Payload) -> Result<Payload> {
    let FileAction::Transfer { stage_path } = &payload.action else {
        anyhow::bail!("Not a transfer operation")
    };
    let mut connection = super::super::open_existing(database)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    verify_pending(&transaction, pending, payload)?;
    let copied =
        native::copy_to_stage(&pending.source_path, stage_path, &payload.stamp, &|| false)?;
    let mut prepared = payload.clone();
    prepared.destination_stamp = Some(copied);
    validate_action(&prepared)?;
    let json = serde_json::to_string(&prepared)?;
    ensure!(
        json.len() <= 16384,
        "Transfer journal exceeds its byte limit"
    );
    ensure!(
        transaction.execute(
            "UPDATE pending_file_move SET payload=?1 WHERE singleton=1 AND operation_id=?2",
            params![json, pending.operation_id]
        )? == 1,
        "Transfer journal changed before recording the copy"
    );
    transaction.commit()?;
    Ok(prepared)
}

/// Explicit copy, verify, publish and delete; supports a different destination volume.
pub fn transfer_history_file(database: &Path, id: i64, destination: &Path) -> Result<MoveOutcome> {
    supported()?;
    ensure!(id > 0, "History capture ID must be positive");
    let destination = normalized_path(destination)?;
    ensure!(
        destination
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("png")),
        "Transfer destination must use .png"
    );
    ensure!(
        std::fs::symlink_metadata(&destination)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
        "Transfer destination already exists or is unavailable"
    );
    let connection = crate::storage::open_read_only(database)?;
    let row = capture_row(&connection, id)?;
    drop(connection);
    let source = normalized_path(Path::new(&row.file_path))?;
    ensure!(
        source != destination,
        "Transfer source and destination are identical"
    );
    let stamp = native::inspect(&source)?.context("Transfer source is missing")?;
    let stage_path = destination.with_file_name(format!(
        ".screenshots-transfer-{}.png",
        uuid::Uuid::new_v4()
    ));
    let (pending, payload) = prepare_action(
        database,
        row,
        source,
        destination,
        stamp,
        FileAction::Transfer {
            stage_path: stage_path.clone(),
        },
    )?;
    let result = (|| {
        let prepared = copy_prepared(database, &pending, &payload)?;
        apply_prepared(database, &pending, &prepared, || {
            let mut locked = native::LockedSource::open(&stage_path, &pending.destination_path)?;
            ensure!(
                Some(locked.stamp()) == prepared.destination_stamp.as_ref(),
                "Transfer staging file changed; preserved"
            );
            locked.rename()?;
            locked.verify_destination()?;
            locked.sync_parents()?;
            drop(locked);
            native::remove_expected(&pending.source_path, &prepared.stamp)
        })
    })();
    result.with_context(|| {
        format!(
            "Transfer {} did not confirm completion; inspect pending-move before retrying",
            pending.operation_id
        )
    })
}

pub(super) fn recover_transfer(
    transaction: rusqlite::Transaction<'_>,
    pending: &PendingMove,
    payload: &Payload,
    action: MoveRecovery,
) -> Result<MoveOutcome> {
    let FileAction::Transfer { stage_path } = &payload.action else {
        unreachable!()
    };
    let source = native::inspect(&pending.source_path)?;
    let destination = native::inspect(&pending.destination_path)?;
    let stage_exists = match std::fs::symlink_metadata(stage_path) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error).context("Transfer staging path is unavailable"),
    };
    let mut preserved = None;
    match action {
        MoveRecovery::Cancel => {
            ensure!(
                source.as_ref() == Some(&payload.stamp) && destination.is_none(),
                "Cannot cancel transfer after publication or source change; preserved"
            );
            if stage_exists {
                if let Some(expected) = &payload.destination_stamp {
                    native::remove_expected(stage_path, expected)?;
                } else {
                    preserved = Some(stage_path.clone());
                }
            }
            if preserved.is_none() {
                native::sync_absent(stage_path)?;
            }
        }
        MoveRecovery::Finish => {
            let copied=payload.destination_stamp.as_ref().context("Transfer has no durable verified copy; cancel preserves any unverified staging file")?;
            ensure!(
                source.is_none() || source.as_ref() == Some(&payload.stamp),
                "Transfer source changed; both files preserved"
            );
            if destination.is_none() {
                ensure!(stage_exists, "Verified transfer copy is missing");
                let mut locked = native::LockedSource::open(stage_path, &pending.destination_path)?;
                ensure!(
                    locked.stamp() == copied,
                    "Transfer staging file changed; preserved"
                );
                locked.rename()?;
                locked.verify_destination()?;
                locked.sync_parents()?;
            } else {
                ensure!(
                    !stage_exists && destination.as_ref() == Some(copied),
                    "Transfer destination or staging state is ambiguous; preserved"
                );
                native::sync_paths(stage_path, &pending.destination_path, copied)?;
            }
            if source.is_some() {
                native::remove_expected(&pending.source_path, &payload.stamp)?;
            }
            native::sync_absent(&pending.source_path)?;
        }
    }
    let mut outcome = finalize_transaction(transaction, pending, payload, action)?;
    outcome.preserved_path = preserved;
    Ok(outcome)
}

/// Physically move one referenced PNG to the explicitly chosen existing directory.
pub fn trash_history_file(database: &Path, id: i64, directory: &Path) -> Result<MoveOutcome> {
    trash_checked(database, id, directory, &|| false, None)
}

fn trash_checked(
    database: &Path,
    id: i64,
    directory: &Path,
    cancelled: &dyn Fn() -> bool,
    expected: Option<&CaptureRow>,
) -> Result<MoveOutcome> {
    supported()?;
    ensure!(id > 0, "History capture ID must be positive");
    let directory = existing_directory(directory)?;
    let connection = crate::storage::open_read_only(database)?;
    let row = capture_row(&connection, id)?;
    ensure!(
        expected.is_none_or(|expected| expected == &row),
        "Retention capture changed after eligibility check; preserved"
    );
    drop(connection);
    let source = normalized_path(Path::new(&row.file_path))?;
    let trash_id = uuid::Uuid::new_v4().to_string();
    let destination = directory.join(format!(".screenshots-trash-{trash_id}.png"));
    let mut locked = native::LockedSource::open(&source, &destination)?;
    ensure!(!cancelled(), "Trash canceled before preparation");
    let action = FileAction::Trash {
        trash_id,
        trashed_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    };
    let (pending, payload) = prepare_action_checked(
        database,
        row,
        source,
        destination,
        locked.stamp().clone(),
        action,
        cancelled,
    )?;
    apply_prepared(database, &pending, &payload, || {
        ensure!(!cancelled(), "Trash canceled before moving the file");
        locked.rename()?;
        locked.verify_destination()?;
        locked.sync_parents()
    })
    .with_context(|| {
        format!(
            "Trash operation {} did not confirm completion; inspect pending-move before retrying",
            pending.operation_id
        )
    })
}

/// Restore original metadata under a new, reserved capture ID without overwriting a file.
pub fn restore_history_trash(
    database: &Path,
    id: &str,
    destination: Option<&Path>,
) -> Result<MoveOutcome> {
    supported()?;
    let connection = crate::storage::open_read_only(database)?;
    let record = read_record(&connection, id)?;
    let next: i64 =
        connection.query_row("SELECT coalesce(max(id),0) FROM captures", [], |row| {
            row.get(0)
        })?;
    let next = next
        .checked_add(1)
        .filter(|id| *id > 0)
        .context("No capture ID remains for restoration")?;
    drop(connection);
    let destination =
        normalized_path(destination.unwrap_or_else(|| Path::new(&record.row.file_path)))?;
    let source = normalized_path(&record.trash_path)?;
    ensure!(source == record.trash_path, "Trash parent path changed");
    let mut locked = native::LockedSource::open(&source, &destination)?;
    ensure!(
        locked.stamp() == &record.stamp,
        "Trashed file changed; preserved"
    );
    let mut row = record.row;
    row.id = next;
    row.file_path = path_text(&source)?.to_owned();
    let (pending, payload) = prepare_action(
        database,
        row,
        source,
        destination,
        record.stamp,
        FileAction::Restore {
            trash_id: id.to_owned(),
        },
    )?;
    apply_prepared(database, &pending, &payload, || {
        locked.rename()?;
        locked.verify_destination()?;
        locked.sync_parents()
    })
    .with_context(|| {
        format!(
            "Restore operation {} did not confirm completion; inspect pending-move before retrying",
            pending.operation_id
        )
    })
}

/// Permanently remove only the unchanged file of an explicitly selected trash record.
pub fn purge_history_trash(database: &Path, id: &str) -> Result<MoveOutcome> {
    purge_checked(database, id, &|| false, None)
}

fn purge_checked(
    database: &Path,
    id: &str,
    cancelled: &dyn Fn() -> bool,
    expected: Option<&TrashRecord>,
) -> Result<MoveOutcome> {
    supported()?;
    let connection = crate::storage::open_read_only(database)?;
    let record = read_record(&connection, id)?;
    ensure!(
        expected.is_none_or(|expected| expected == &record),
        "Retention trash changed after eligibility check; preserved"
    );
    drop(connection);
    let source = normalized_path(&record.trash_path)?;
    ensure!(source == record.trash_path, "Trash parent path changed");
    let destination =
        source.with_file_name(format!(".screenshots-purge-{}.png", uuid::Uuid::new_v4()));
    let mut locked = native::LockedSource::open(&source, &destination)?;
    ensure!(!cancelled(), "Purge canceled before preparation");
    ensure!(
        locked.stamp() == &record.stamp,
        "Trashed file changed; preserved"
    );
    let mut row = record.row;
    row.file_path = path_text(&source)?.to_owned();
    let (pending, payload) = prepare_action_checked(
        database,
        row,
        source,
        destination,
        record.stamp,
        FileAction::Purge {
            trash_id: id.to_owned(),
            trashed_at: record.trashed_at,
        },
        cancelled,
    )?;
    apply_prepared(database, &pending, &payload, || {
        ensure!(!cancelled(), "Purge canceled before quarantine");
        locked.rename()?;
        locked.verify_destination()?;
        locked.sync_parents()?;
        drop(locked);
        ensure!(
            !cancelled(),
            "Purge canceled before permanent removal; inspect pending-move"
        );
        native::remove_expected(&pending.destination_path, &payload.stamp)
    })
    .with_context(|| {
        format!(
            "Purge operation {} did not confirm completion; inspect pending-move before retrying",
            pending.operation_id
        )
    })
}

/// Explicit recovery may finish deleting an already quarantined purge file.
pub fn recover_history_file_operation(
    database: &Path,
    id: &str,
    action: MoveRecovery,
) -> Result<MoveOutcome> {
    recover_operation(database, id, action, true)
}

fn open_trash_query(database: &Path) -> Result<Connection> {
    let connection = crate::storage::open_read_only(database)?;
    let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    ensure!(
        (1..=10).contains(&version),
        "Unsupported history schema version"
    );
    ensure!(
        version >= 6,
        "Trash queries require history schema version 6"
    );
    connection.execute_batch("PRAGMA cache_size=-512; BEGIN DEFERRED;")?;
    Ok(connection)
}

/// Read one bounded metadata page; never inspect, migrate or open image files.
pub fn trash_history_page(database: &Path, cursor: Option<i64>) -> Result<TrashPage> {
    ensure!(cursor.is_none_or(|id| id > 0), "Invalid trash page cursor");
    let connection = open_trash_query(database)?;
    let mut statement=connection.prepare("SELECT sequence,CASE WHEN typeof(trash_id)='text' AND length(trash_id)=36 THEN trash_id END
        FROM history_trash WHERE sequence<?1 ORDER BY sequence DESC LIMIT 21")?;
    let selected = statement.query_map([cursor.unwrap_or(i64::MAX)], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut entries = Vec::with_capacity(21);
    for selected in selected {
        let (sequence, id) = selected?;
        ensure!(sequence > 0, "Invalid trash sequence");
        let record = read_record(&connection, &id)?;
        entries.push(TrashEntry {
            sequence,
            trash_id: id,
            original_capture_id: record.row.id,
            original_path: PathBuf::from(record.row.file_path),
            trash_path: record.trash_path,
            trashed_at: record.trashed_at,
            bytes: record.stamp.size,
        });
    }
    let has_more = entries.len() > 20;
    entries.truncate(20);
    let next_cursor = has_more.then(|| entries.last().unwrap().sequence);
    Ok(TrashPage {
        entries,
        has_more,
        next_cursor,
    })
}

fn default_limit() -> usize {
    20
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetentionPolicy {
    pub version: u8,
    pub trash_directory: Option<PathBuf>,
    pub capture_days: Option<u32>,
    pub purge_days: Option<u32>,
    #[serde(default)]
    pub include_imports: bool,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

#[derive(Debug, Serialize)]
pub struct RetentionPlan {
    pub capture_ids: Vec<i64>,
    pub trash_ids: Vec<String>,
    pub has_more: bool,
}

#[derive(Debug, Serialize)]
pub struct RetentionReport {
    pub outcomes: Vec<MoveOutcome>,
    pub has_more: bool,
    pub failure: Option<String>,
}

fn validate_policy(policy: &RetentionPolicy) -> Result<()> {
    ensure!(
        policy.version == 1 && (1..=100).contains(&policy.limit),
        "Retention requires version 1 and a limit of 1 to 100 per phase"
    );
    ensure!(
        policy.capture_days.is_some() || policy.purge_days.is_some(),
        "Retention policy has no explicit action"
    );
    ensure!(
        policy
            .capture_days
            .into_iter()
            .chain(policy.purge_days)
            .all(|days| (1..=36500).contains(&days)),
        "Retention days must be 1 to 36500"
    );
    ensure!(
        policy.capture_days.is_none() || policy.trash_directory.is_some(),
        "Capture retention needs an explicit trash directory"
    );
    if let Some(directory) = &policy.trash_directory {
        path_text(directory)?;
    }
    Ok(())
}

/// Preview bounded explicit retention; calls perform no filesystem mutations.
pub fn plan_retention(database: &Path, policy: &RetentionPolicy) -> Result<RetentionPlan> {
    validate_policy(policy)?;
    let now = Utc::now();
    let mut plan = RetentionPlan {
        capture_ids: Vec::new(),
        trash_ids: Vec::new(),
        has_more: false,
    };
    if let Some(days) = policy.capture_days {
        let date = now
            .date_naive()
            .checked_sub_days(chrono::Days::new(u64::from(days) + 1))
            .context("Retention date is outside the calendar")?;
        let mut query = crate::HistoryQuery {
            sort: crate::HistorySort::Oldest,
            filter: if policy.include_imports {
                super::super::HistoryFilter::All
            } else {
                super::super::HistoryFilter::Captures
            },
            through: Some(date.format("%Y-%m-%d").to_string()),
            ..Default::default()
        };
        loop {
            let page = crate::query_capture_history(
                database,
                &query,
                std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            )?;
            for entry in page.entries {
                if plan.capture_ids.len() == policy.limit {
                    plan.has_more = true;
                    break;
                }
                plan.capture_ids.push(entry.id);
            }
            if plan.capture_ids.len() == policy.limit || !page.has_more {
                plan.has_more |= page.has_more;
                break;
            }
            query.cursor = page.next_cursor;
        }
    }
    if let Some(days) = policy.purge_days {
        let cutoff = now
            .checked_sub_signed(chrono::Duration::days(i64::from(days)))
            .context("Retention timestamp is outside the calendar")?
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let connection = open_trash_query(database)?;
        let mut statement = connection.prepare(
            "SELECT CASE WHEN typeof(trash_id)='text' AND length(trash_id)=36 THEN trash_id END
             FROM history_trash WHERE trashed_at<?1 ORDER BY trashed_at,sequence LIMIT ?2",
        )?;
        let rows = statement.query_map(params![cutoff, (policy.limit + 1) as i64], |row| {
            row.get::<_, String>(0)
        })?;
        for row in rows {
            let id = row?;
            read_record(&connection, &id)?;
            if plan.trash_ids.len() == policy.limit {
                plan.has_more = true;
                break;
            }
            plan.trash_ids.push(id);
        }
    }
    Ok(plan)
}

fn eligible_capture(database: &Path, id: i64, policy: &RetentionPolicy) -> Result<CaptureRow> {
    let connection = crate::storage::open_read_only(database)?;
    let row = capture_row(&connection, id)?;
    let created = if row.created_at.len() == 10 {
        let parsed = chrono::NaiveDate::parse_from_str(&row.created_at, "%Y-%m-%d")?;
        ensure!(
            parsed.format("%Y-%m-%d").to_string() == row.created_at,
            "Invalid capture date"
        );
        parsed
    } else {
        let parsed = DateTime::parse_from_rfc3339(&row.created_at)?;
        ensure!(
            parsed.offset().local_minus_utc() == 0,
            "Capture retention requires UTC dates"
        );
        parsed.date_naive()
    };
    let days = policy
        .capture_days
        .context("Capture retention is not configured")?;
    let cutoff = Utc::now()
        .date_naive()
        .checked_sub_days(chrono::Days::new(u64::from(days)))
        .context("Retention date is outside the calendar")?;
    ensure!(
        created < cutoff && (policy.include_imports || row.source == "capture"),
        "Selected capture no longer matches retention age/source; preserved"
    );
    Ok(row)
}

fn eligible_trash(database: &Path, id: &str, policy: &RetentionPolicy) -> Result<TrashRecord> {
    let connection = crate::storage::open_read_only(database)?;
    let record = read_record(&connection, id)?;
    let days = policy
        .purge_days
        .context("Purge retention is not configured")?;
    let cutoff = Utc::now()
        .checked_sub_signed(chrono::Duration::days(i64::from(days)))
        .context("Retention timestamp is outside the calendar")?;
    ensure!(
        DateTime::parse_from_rfc3339(&record.trashed_at)? < cutoff,
        "Selected trash no longer matches retention age; preserved"
    );
    Ok(record)
}

/// Apply the explicit policy once, stopping at the first error or cancellation.
pub fn apply_retention(
    database: &Path,
    policy: &RetentionPolicy,
    cancelled: impl Fn() -> bool,
) -> Result<RetentionReport> {
    let started = Instant::now();
    let plan = plan_retention(database, policy)?;
    let mut report = RetentionReport {
        outcomes: Vec::new(),
        has_more: plan.has_more,
        failure: None,
    };
    for (is_trash, id) in plan
        .capture_ids
        .into_iter()
        .map(|id| (true, id.to_string()))
        .chain(plan.trash_ids.into_iter().map(|id| (false, id)))
    {
        let result = (|| {
            ensure!(
                !cancelled(),
                "Retention canceled before the next file operation"
            );
            ensure!(
                started.elapsed() < Duration::from_secs(300),
                "Retention operation exceeded its cooperative five-minute budget"
            );
            let stop = || cancelled() || started.elapsed() >= Duration::from_secs(300);
            if is_trash {
                let selected = eligible_capture(database, id.parse()?, policy)?;
                trash_checked(
                    database,
                    id.parse()?,
                    policy
                        .trash_directory
                        .as_deref()
                        .context("Retention trash directory is unavailable")?,
                    &stop,
                    Some(&selected),
                )
            } else {
                let selected = eligible_trash(database, &id, policy)?;
                purge_checked(database, &id, &stop, Some(&selected))
            }
        })();
        match result {
            Ok(outcome) => report.outcomes.push(outcome),
            Err(error) => {
                let mut text = format!(
                    "{} {id}: {error:#}",
                    if is_trash { "capture" } else { "trash" }
                );
                if text.len() > 2048 {
                    let mut end = 2045;
                    while !text.is_char_boundary(end) {
                        end -= 1;
                    }
                    text.truncate(end);
                    text.push_str("...");
                }
                report.failure = Some(text);
                report.has_more = true;
                break;
            }
        }
    }
    Ok(report)
}

#[cfg(all(
    test,
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
#[path = "history_lifecycle_tests.rs"]
mod tests;
