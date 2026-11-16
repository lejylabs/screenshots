//! Indexed metadata candidates plus native file/parent identity checks.

use super::{native::FileStamp, *};
use rusqlite::{
    functions::{Context as SqlContext, FunctionFlags},
    types::ValueRef,
};
use sha2_10::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use unicode_normalization::UnicodeNormalization;

const MAX_CANDIDATES: usize = 100;

pub(in super::super) const MIGRATION: &str = "
ALTER TABLE captures ADD COLUMN reservation_name TEXT;
ALTER TABLE captures ADD COLUMN file_identity TEXT;
ALTER TABLE history_trash ADD COLUMN file_identity TEXT;
DROP TRIGGER captures_search_update;
DROP TRIGGER captures_move_update;
DROP TRIGGER trash_pending_update;
CREATE TRIGGER captures_search_update AFTER UPDATE OF id,file_path,created_at,source ON captures BEGIN
 INSERT INTO captures_search(captures_search,rowid,file_path,created_at,source) VALUES('delete',old.id,old.file_path,old.created_at,old.source);
 INSERT INTO captures_search(rowid,file_path,created_at,source) VALUES(new.id,new.file_path,new.created_at,new.source); END;
UPDATE captures SET reservation_name=CASE WHEN typeof(file_path)='text' AND length(CAST(file_path AS BLOB))<=4096 THEN screenshots_path_name(file_path) END;
UPDATE history_trash SET file_identity=CASE WHEN typeof(payload)='text' AND length(CAST(payload AS BLOB))<=16384 THEN screenshots_stamp_identity(payload) END;
CREATE TRIGGER captures_move_update BEFORE UPDATE ON captures
 WHEN EXISTS(SELECT 1 FROM pending_file_move WHERE old.id=capture_id OR new.id=capture_id OR old.file_path IN (source_path,destination_path) OR new.file_path IN (source_path,destination_path))
 BEGIN SELECT RAISE(ABORT,'History file move is pending; recover it first'); END;
CREATE TRIGGER trash_pending_update BEFORE UPDATE ON history_trash
 WHEN EXISTS(SELECT 1 FROM pending_file_move WHERE json_extract(payload,'$.action.trash_id')=old.trash_id)
 BEGIN SELECT RAISE(ABORT,'History file operation is pending; recover it first'); END;
CREATE INDEX captures_reservation_name ON captures(reservation_name);
CREATE INDEX captures_file_identity ON captures(file_identity);
CREATE INDEX trash_file_identity ON history_trash(file_identity);
CREATE TRIGGER captures_reservation_keys_insert AFTER INSERT ON captures BEGIN
 UPDATE captures SET reservation_name=screenshots_path_name(new.file_path),file_identity=coalesce(screenshots_file_identity(new.file_path),'absent') WHERE id=new.id; END;
CREATE TRIGGER captures_reservation_keys_update AFTER UPDATE OF file_path ON captures BEGIN
 UPDATE captures SET reservation_name=screenshots_path_name(new.file_path),file_identity=coalesce(screenshots_file_identity(new.file_path),'absent') WHERE id=new.id; END;
CREATE TRIGGER trash_reservation_keys_insert AFTER INSERT ON history_trash BEGIN
 UPDATE history_trash SET file_identity=screenshots_stamp_identity(new.payload) WHERE trash_id=new.trash_id; END;
CREATE TRIGGER trash_reservation_keys_update AFTER UPDATE OF payload ON history_trash BEGIN
 UPDATE history_trash SET file_identity=screenshots_stamp_identity(new.payload) WHERE trash_id=new.trash_id; END;
CREATE TRIGGER captures_native_reserve_insert BEFORE INSERT ON captures
WHEN EXISTS(SELECT 1 FROM pending_file_move WHERE screenshots_paths_alias(new.file_path,source_path)
 OR screenshots_paths_alias(new.file_path,destination_path) OR screenshots_paths_alias(new.file_path,json_extract(payload,'$.action.stage_path')))
 OR CASE WHEN EXISTS(SELECT 1 FROM history_trash) THEN
 EXISTS(SELECT 1 FROM history_trash WHERE file_identity IS NULL)
 OR (SELECT count(*)>100 OR coalesce(max(screenshots_paths_alias(new.file_path,trash_path)),0)
     FROM (SELECT trash_path FROM history_trash WHERE file_identity=screenshots_file_identity(new.file_path) LIMIT 101))
 OR EXISTS(SELECT 1 FROM history_trash WHERE trash_id=screenshots_trash_id(new.file_path) AND screenshots_paths_alias(new.file_path,trash_path)) ELSE 0 END
BEGIN SELECT RAISE(ABORT,'Filesystem path is reserved by a pending operation or trash; preserve it and recover first'); END;
CREATE TRIGGER captures_native_reserve_update BEFORE UPDATE OF file_path,id ON captures
WHEN EXISTS(SELECT 1 FROM pending_file_move WHERE screenshots_paths_alias(new.file_path,source_path)
 OR screenshots_paths_alias(new.file_path,destination_path) OR screenshots_paths_alias(new.file_path,json_extract(payload,'$.action.stage_path')))
 OR CASE WHEN EXISTS(SELECT 1 FROM history_trash) THEN
 EXISTS(SELECT 1 FROM history_trash WHERE file_identity IS NULL)
 OR (SELECT count(*)>100 OR coalesce(max(screenshots_paths_alias(new.file_path,trash_path)),0)
     FROM (SELECT trash_path FROM history_trash WHERE file_identity=screenshots_file_identity(new.file_path) LIMIT 101))
 OR EXISTS(SELECT 1 FROM history_trash WHERE trash_id=screenshots_trash_id(new.file_path) AND screenshots_paths_alias(new.file_path,trash_path)) ELSE 0 END
BEGIN SELECT RAISE(ABORT,'Filesystem path is reserved by a pending operation or trash; preserve it and recover first'); END;
";

fn sql_text<'a>(context: &'a SqlContext<'_>, index: usize, limit: usize) -> Option<&'a str> {
    let ValueRef::Text(bytes) = context.get_raw(index) else {
        return None;
    };
    (bytes.len() <= limit)
        .then(|| std::str::from_utf8(bytes).ok())
        .flatten()
}

fn sql_error(_: anyhow::Error) -> rusqlite::Error {
    rusqlite::Error::UserFunctionError(Box::new(std::io::Error::other(
        "History filesystem reservation check failed",
    )))
}

fn valid_path(path: &Path) -> Result<()> {
    let text = path.to_str().context("Reservation path must be UTF-8")?;
    ensure!(
        path.is_absolute() && !text.is_empty() && text.len() <= 4096 && !text.contains('\0'),
        "Invalid bounded reservation path"
    );
    ensure!(
        path.file_name().is_some(),
        "Reservation path has no filename"
    );
    Ok(())
}

/// Deliberately broad candidate key, never proof that two filesystem paths alias.
/// Its algorithm/Unicode mapping is persisted: changes require a key migration.
fn projected_name(path: &str) -> Option<String> {
    if path.len() > 4096 || path.contains('\0') {
        return None;
    }
    let name = Path::new(path)
        .file_name()?
        .to_str()?
        .trim_end_matches([' ', '.']);
    let mut projected = String::new();
    for character in name
        .nfkd()
        .flat_map(char::to_uppercase)
        .nfkd()
        .filter(char::is_ascii)
    {
        if projected.len() == 16384 {
            return None;
        }
        projected.push(character);
    }
    Some(projected)
}

pub(super) fn name_key(path: &str) -> Option<String> {
    let projected = projected_name(path)?;
    let mut hash = Sha256::new();
    hash.update(b"screenshots-reservation-name-v1\0");
    hash.update(projected.as_bytes());
    Some(format!("v1:{:x}", hash.finalize()))
}

fn trash_id(path: &str) -> Option<String> {
    let name = projected_name(path)?;
    let (stem, extension) = name.rsplit_once('.')?;
    if !matches!(extension, "PNG" | "JPG" | "JPEG" | "WEBP") {
        return None;
    }
    let id = stem.strip_prefix(".SCREENSHOTS-TRASH-")?;
    let parsed = uuid::Uuid::parse_str(id).ok()?;
    (id.len() == 36).then(|| parsed.hyphenated().to_string())
}

pub(super) fn stamp_identity(stamp: &FileStamp) -> String {
    if let Some(id) = stamp.file_id {
        let encoded: String = id.iter().map(|byte| format!("{byte:02x}")).collect();
        format!("w:{:016x}:{encoded}", stamp.device)
    } else {
        format!("u:{:016x}:{:016x}", stamp.device, stamp.inode)
    }
}

fn open(path: &Path, directory: bool) -> Result<Option<File>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | if directory { libc::O_DIRECTORY } else { 0 });
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(if directory {
                FILE_FLAG_BACKUP_SEMANTICS
            } else {
                0
            });
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("Cannot inspect a reserved filesystem path"),
    };
    let metadata = file.metadata()?;
    ensure!(
        if directory {
            metadata.is_dir()
        } else {
            metadata.is_file()
        },
        "Reserved path has an unexpected filesystem type"
    );
    Ok(Some(file))
}

#[cfg(unix)]
fn opened_identity(file: &File) -> Result<String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok(format!("u:{:016x}:{:016x}", metadata.dev(), metadata.ino()))
}

#[cfg(target_os = "windows")]
fn opened_identity(file: &File) -> Result<String> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx,
    };
    let mut info = FILE_ID_INFO::default();
    ensure!(
        unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileIdInfo,
                (&mut info as *mut FILE_ID_INFO).cast(),
                std::mem::size_of::<FILE_ID_INFO>() as u32,
            )
        } != 0,
        "Cannot inspect native reservation identity: {}",
        std::io::Error::last_os_error()
    );
    let encoded: String = info
        .FileId
        .Identifier
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(format!("w:{:016x}:{encoded}", info.VolumeSerialNumber))
}

#[cfg(not(any(unix, target_os = "windows")))]
fn opened_identity(_: &File) -> Result<String> {
    anyhow::bail!("Native reservation identities are unavailable")
}

pub(super) fn identity(path: &Path) -> Result<Option<String>> {
    valid_path(path)?;
    open(path, false)?.as_ref().map(opened_identity).transpose()
}

fn sensitive_names_equal(directory: &File, a: &str, b: &str) -> Option<bool> {
    // Case-sensitivity alone does not specify a filesystem's Unicode normalization.
    // Use it only for plain ASCII case variants; all other missing-name ambiguity
    // retains the conservative candidate check below.
    if !a.is_ascii() || !b.is_ascii() || !a.eq_ignore_ascii_case(b) {
        return None;
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_CASE_SENSITIVE_INFO, FileCaseSensitiveInfo, GetFileInformationByHandleEx,
        };
        let mut info = FILE_CASE_SENSITIVE_INFO::default();
        if unsafe {
            GetFileInformationByHandleEx(
                directory.as_raw_handle(),
                FileCaseSensitiveInfo,
                (&mut info as *mut FILE_CASE_SENSITIVE_INFO).cast(),
                std::mem::size_of::<FILE_CASE_SENSITIVE_INFO>() as u32,
            )
        } != 0
            && info.Flags & 1 != 0
        {
            return Some(a == b);
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        if unsafe { libc::fpathconf(directory.as_raw_fd(), libc::_PC_CASE_SENSITIVE) } == 1 {
            return Some(a == b);
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = (directory, a, b);
    None
}

pub(super) fn equivalent(a: &Path, b: &Path) -> Result<bool> {
    valid_path(a)?;
    valid_path(b)?;
    if a == b {
        return Ok(true);
    }
    let first = open(a, false)?;
    let second = open(b, false)?;
    match (&first, &second) {
        (Some(a), Some(b)) => return Ok(opened_identity(a)? == opened_identity(b)?),
        (Some(_), None) | (None, Some(_)) => return Ok(false),
        (None, None) => (),
    }
    let first_parent = open(a.parent().context("Reservation path has no parent")?, true)?
        .context("Reservation parent is unavailable")?;
    let second_parent = open(b.parent().context("Reservation path has no parent")?, true)?
        .context("Reservation parent is unavailable")?;
    if opened_identity(&first_parent)? != opened_identity(&second_parent)? {
        return Ok(false);
    }
    let first_name = a
        .file_name()
        .and_then(|name| name.to_str())
        .context("Invalid reservation filename")?;
    let second_name = b
        .file_name()
        .and_then(|name| name.to_str())
        .context("Invalid reservation filename")?;
    if let Some(equal) = sensitive_names_equal(&first_parent, first_name, second_name) {
        return Ok(equal);
    }
    // Missing names cannot be resolved by the filesystem. Over-approximate aliases.
    Ok(name_key(a.to_str().unwrap()) == name_key(b.to_str().unwrap()))
}

pub(in super::super) fn register(connection: &Connection) -> Result<()> {
    let pure = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    connection.create_scalar_function("screenshots_path_name", 1, pure, |context| {
        Ok(sql_text(context, 0, 4096).and_then(name_key))
    })?;
    connection.create_scalar_function("screenshots_trash_id", 1, pure, |context| {
        Ok(sql_text(context, 0, 4096).and_then(trash_id))
    })?;
    connection.create_scalar_function("screenshots_stamp_identity", 1, pure, |context| {
        #[derive(Deserialize)]
        struct Stamped {
            stamp: FileStamp,
        }
        Ok(sql_text(context, 0, 16384)
            .and_then(|text| serde_json::from_str::<Stamped>(text).ok())
            .map(|value| stamp_identity(&value.stamp)))
    })?;
    connection.create_scalar_function(
        "screenshots_file_identity",
        1,
        FunctionFlags::SQLITE_UTF8,
        |context| {
            let Some(text) = sql_text(context, 0, 4096) else {
                return Ok(None);
            };
            if !Path::new(text).is_absolute() || text.contains('\0') {
                return Ok(None);
            }
            identity(Path::new(text)).map_err(sql_error)
        },
    )?;
    connection.create_scalar_function(
        "screenshots_paths_alias",
        2,
        FunctionFlags::SQLITE_UTF8,
        |context| {
            if matches!(context.get_raw(1), ValueRef::Null) {
                return Ok(false);
            }
            let a = sql_text(context, 0, 4096)
                .ok_or_else(|| sql_error(anyhow::anyhow!("Invalid reservation path")))?;
            let b = sql_text(context, 1, 4096)
                .ok_or_else(|| sql_error(anyhow::anyhow!("Invalid reservation path")))?;
            equivalent(Path::new(a), Path::new(b)).map_err(sql_error)
        },
    )?;
    Ok(())
}

pub(super) fn check_captures(
    connection: &Connection,
    path: &Path,
    except: Option<i64>,
) -> Result<()> {
    valid_path(path)?;
    let except = except.unwrap_or(-1);
    ensure!(
        !connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM captures WHERE reservation_name IS NULL AND id<>?1)",
            [except],
            |row| row.get::<_, bool>(0)
        )?,
        "A malformed capture path prevents safe alias validation"
    );
    let key = name_key(path.to_str().unwrap()).context("Invalid reservation key")?;
    let id = identity(path)?;
    let mut statement=connection.prepare("SELECT id,CASE WHEN typeof(file_path)='text' AND length(CAST(file_path AS BLOB))<=4096 THEN file_path END
        FROM captures WHERE id<>?3 AND (reservation_name=?1 OR file_identity=?2 OR file_identity IS NULL) LIMIT 101")?;
    let rows = statement.query_map(params![key, id, except], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    for (count, row) in rows.enumerate() {
        ensure!(
            count < MAX_CANDIDATES,
            "Too many possible filesystem aliases; no file operation started"
        );
        let (_, candidate) = row?;
        ensure!(
            !equivalent(Path::new(&candidate), path)?,
            "Filesystem path is referenced by another capture; preserve it and resolve the alias"
        );
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct IdentityIndexReport {
    pub checked: usize,
    pub indexed: usize,
    pub missing: usize,
    pub unavailable: usize,
    pub unavailable_ids: Vec<i64>,
    pub remaining: Option<u64>,
    pub protected: Option<u64>,
    pub next_cursor: Option<i64>,
    pub complete: bool,
    pub canceled: bool,
    pub timed_out: bool,
}

const UNRESOLVED_SELECT: &str = "SELECT id,CASE WHEN typeof(file_path)='text' AND length(CAST(file_path AS BLOB))<=4096 THEN file_path END
 FROM captures c INDEXED BY captures_unresolved_id WHERE file_identity IS NULL AND id>?1
 AND NOT EXISTS(SELECT 1 FROM pending_file_move p WHERE p.capture_id=c.id OR c.file_path IN(p.source_path,p.destination_path)) ORDER BY id LIMIT 100";
const REFRESH_SELECT: &str = "SELECT id,CASE WHEN typeof(file_path)='text' AND length(CAST(file_path AS BLOB))<=4096 THEN file_path END
 FROM captures c WHERE id>?1 AND NOT EXISTS(SELECT 1 FROM pending_file_move p WHERE p.capture_id=c.id OR c.file_path IN(p.source_path,p.destination_path)) ORDER BY id LIMIT 100";
const UNRESOLVED_AFTER: &str = "SELECT count(*) FROM captures c INDEXED BY captures_unresolved_id WHERE file_identity IS NULL AND id>?1
 AND NOT EXISTS(SELECT 1 FROM pending_file_move p WHERE p.capture_id=c.id OR c.file_path IN(p.source_path,p.destination_path))";
const REFRESH_AFTER: &str = "SELECT count(*) FROM captures c WHERE id>?1
 AND NOT EXISTS(SELECT 1 FROM pending_file_move p WHERE p.capture_id=c.id OR c.file_path IN(p.source_path,p.destination_path))";

#[cfg(test)]
thread_local! {static PRIME_VM_CALLS:std::cell::Cell<usize>=const{std::cell::Cell::new(0)};}

struct PrimeBudget<'a> {
    cancelled: &'a dyn Fn() -> bool,
    deadline: std::time::Instant,
    canceled: std::cell::Cell<bool>,
    timed_out: std::cell::Cell<bool>,
    panicked: std::cell::Cell<bool>,
}
impl PrimeBudget<'_> {
    fn stopped(&self) -> bool {
        if self.canceled.get() || self.timed_out.get() || self.panicked.get() {
            return true;
        }
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (self.cancelled)())) {
            Ok(true) => self.canceled.set(true),
            Ok(false) => (),
            Err(_) => self.panicked.set(true),
        }
        if std::time::Instant::now() >= self.deadline {
            self.timed_out.set(true);
        }
        self.canceled.get() || self.timed_out.get() || self.panicked.get()
    }
}

struct PrimeProgress<'a, 'c> {
    connection: &'c mut Connection,
    state: Box<PrimeBudget<'a>>,
}
impl<'a, 'c> PrimeProgress<'a, 'c> {
    fn new(
        connection: &'c mut Connection,
        cancelled: &'a dyn Fn() -> bool,
        deadline: std::time::Instant,
    ) -> Self {
        unsafe extern "C" fn progress(pointer: *mut std::ffi::c_void) -> std::ffi::c_int {
            // SQLite invokes this synchronously on this exclusively borrowed connection.
            // The box is stable until Drop unregisters the hook; no borrowed callback escapes.
            let state = unsafe { &*pointer.cast::<PrimeBudget<'_>>() };
            #[cfg(test)]
            PRIME_VM_CALLS.with(|calls| calls.set(calls.get() + 1));
            std::ffi::c_int::from(state.stopped())
        }
        let value = Self {
            connection,
            state: Box::new(PrimeBudget {
                cancelled,
                deadline,
                canceled: std::cell::Cell::new(false),
                timed_out: std::cell::Cell::new(false),
                panicked: std::cell::Cell::new(false),
            }),
        };
        let pointer = (&*value.state as *const PrimeBudget<'a>).cast_mut().cast();
        // Only private history mutation connections reach this scope; no prior hook is installed.
        unsafe {
            rusqlite::ffi::sqlite3_progress_handler(
                value.connection.handle(),
                1000,
                Some(progress),
                pointer,
            )
        };
        value
    }
}
impl Drop for PrimeProgress<'_, '_> {
    fn drop(&mut self) {
        // All statements/transactions have dropped before this guard, including on panic/error.
        unsafe {
            rusqlite::ffi::sqlite3_progress_handler(
                self.connection.handle(),
                0,
                None,
                std::ptr::null_mut(),
            )
        };
    }
}

pub(super) fn prime(
    connection: &mut Connection,
    cursor: Option<i64>,
    only_unresolved: bool,
    cancelled: &dyn Fn() -> bool,
) -> Result<IdentityIndexReport> {
    prime_until(
        connection,
        cursor,
        only_unresolved,
        cancelled,
        std::time::Instant::now() + std::time::Duration::from_secs(5),
    )
}

fn prime_until(
    connection: &mut Connection,
    cursor: Option<i64>,
    only_unresolved: bool,
    cancelled: &dyn Fn() -> bool,
    deadline: std::time::Instant,
) -> Result<IdentityIndexReport> {
    ensure!(
        cursor.is_none_or(|id| id > 0),
        "Invalid identity index cursor"
    );
    let scope = PrimeProgress::new(connection, cancelled, deadline);
    let mut report = IdentityIndexReport {
        checked: 0,
        indexed: 0,
        missing: 0,
        unavailable: 0,
        unavailable_ids: Vec::new(),
        remaining: None,
        protected: None,
        next_cursor: cursor,
        complete: false,
        canceled: false,
        timed_out: false,
    };
    let result = prime_steps(
        scope.connection,
        cursor,
        only_unresolved,
        &scope.state,
        &mut report,
    );
    let stopped = scope.state.stopped();
    report.canceled = scope.state.canceled.get();
    report.timed_out = scope.state.timed_out.get();
    ensure!(
        !scope.state.panicked.get(),
        "Identity cancellation callback panicked; committed metadata batches may remain; image files are unchanged"
    );
    if let Err(error) = result {
        let interrupted = error
            .downcast_ref::<rusqlite::Error>()
            .is_some_and(|error| {
                error.sqlite_error_code() == Some(rusqlite::ErrorCode::OperationInterrupted)
            });
        if !(stopped && interrupted) {
            return Err(error).context(format!(
                "{} identity rows already indexed; image files are unchanged",
                report.indexed
            ));
        }
    }
    if stopped {
        report.remaining = None;
        report.protected = None;
        report.complete = false;
    }
    Ok(report)
}

fn prime_steps(
    connection: &mut Connection,
    cursor: Option<i64>,
    only_unresolved: bool,
    budget: &PrimeBudget<'_>,
    report: &mut IdentityIndexReport,
) -> Result<()> {
    let mut after = cursor.unwrap_or(0);
    loop {
        if budget.stopped() {
            return Ok(());
        }
        let selected = {
            let sql = if only_unresolved {
                UNRESOLVED_SELECT
            } else {
                REFRESH_SELECT
            };
            let mut statement = connection.prepare(sql)?;
            statement
                .query_map([after], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        if budget.stopped() {
            return Ok(());
        }
        if selected.is_empty() {
            break;
        }
        let mut inspected = Vec::with_capacity(100);
        for (id, path) in selected {
            if budget.stopped() {
                return Ok(());
            }
            after = id;
            report.checked += 1;
            let result = path
                .as_deref()
                .context("Malformed reference path")
                .and_then(|path| identity(Path::new(path)));
            if budget.stopped() {
                return Ok(());
            }
            match result {
                Ok(key) => inspected.push((id, path.unwrap(), key)),
                Err(_) => {
                    report.unavailable += 1;
                    if report.unavailable_ids.len() < 20 {
                        report.unavailable_ids.push(id);
                    }
                }
            }
        }
        if !inspected.is_empty() {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut applied = 0;
            let mut absent = 0;
            for (id, path, key) in inspected {
                if budget.stopped() {
                    return Ok(());
                }
                let changed=transaction.execute("UPDATE captures SET file_identity=?1 WHERE id=?2 AND file_path=?3
                 AND NOT EXISTS(SELECT 1 FROM pending_file_move p WHERE p.capture_id=captures.id OR captures.file_path IN(p.source_path,p.destination_path))",
                    params![key.as_deref().unwrap_or("absent"),id,path])?;
                applied += changed;
                if key.is_none() {
                    absent += changed;
                }
            }
            transaction.commit()?;
            report.indexed += applied;
            report.missing += absent;
        }
        report.next_cursor = Some(after);
        if budget.stopped() {
            return Ok(());
        }
    }
    let summary = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
    let (remaining, protected): (i64, i64) = summary.query_row(
        "SELECT count(*),coalesce(sum(EXISTS(SELECT 1 FROM pending_file_move p
        WHERE p.capture_id=c.id OR c.file_path IN(p.source_path,p.destination_path))),0)
        FROM captures c INDEXED BY captures_unresolved_id WHERE file_identity IS NULL",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if budget.stopped() {
        return Ok(());
    }
    let remaining = u64::try_from(remaining).context("Invalid unresolved identity count")?;
    let protected = u64::try_from(protected).context("Invalid deferred identity count")?;
    let protected_all:i64=summary.query_row("SELECT count(*) FROM captures WHERE id IN(SELECT capture_id FROM pending_file_move)
        OR file_path IN(SELECT source_path FROM pending_file_move UNION ALL SELECT destination_path FROM pending_file_move)",[],|row|row.get(0))?;
    if budget.stopped() {
        return Ok(());
    }
    let protected_all =
        u64::try_from(protected_all).context("Invalid protected reference count")?;
    let after_sql = if only_unresolved {
        UNRESOLVED_AFTER
    } else {
        REFRESH_AFTER
    };
    let after_remaining: i64 = summary.query_row(after_sql, [after], |row| row.get(0))?;
    if budget.stopped() {
        return Ok(());
    }
    let after_remaining =
        u64::try_from(after_remaining).context("Invalid remaining identity count")?;
    let total = if only_unresolved {
        remaining
    } else {
        let after_unresolved: i64 =
            summary.query_row(UNRESOLVED_AFTER, [after], |row| row.get(0))?;
        if budget.stopped() {
            return Ok(());
        }
        let after_unresolved =
            u64::try_from(after_unresolved).context("Invalid unresolved continuation count")?;
        remaining
            + after_remaining
                .checked_sub(after_unresolved)
                .context("Invalid continuation summary")?
            + protected_all
                .checked_sub(protected)
                .context("Invalid deferred summary")?
    };
    summary.commit()?;
    if budget.stopped() {
        return Ok(());
    }
    report.remaining = Some(total);
    report.protected = Some(protected_all);
    report.next_cursor = (after_remaining > 0 && after > 0).then_some(after);
    report.complete = report.unavailable == 0
        && if only_unresolved {
            remaining == protected
        } else {
            total == 0
        };
    Ok(())
}

pub(super) fn check_trash(
    connection: &Connection,
    path: &Path,
    except: Option<&str>,
) -> Result<()> {
    valid_path(path)?;
    ensure!(!connection.query_row("SELECT EXISTS(SELECT 1 FROM history_trash WHERE file_identity IS NULL AND trash_id IS NOT ?1)",[except],|row|row.get::<_,bool>(0))?,
            "Malformed trash metadata prevents safe alias validation");
    let id = identity(path)?;
    let candidate = trash_id(path.to_str().unwrap());
    let mut statement=connection.prepare("SELECT trash_id,CASE WHEN typeof(trash_path)='text' AND length(CAST(trash_path AS BLOB))<=4096 THEN trash_path END
        FROM history_trash WHERE trash_id IS NOT ?3 AND (trash_id=?1 OR file_identity=?2) LIMIT 101")?;
    let rows = statement.query_map(params![candidate, id, except], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for (count, row) in rows.enumerate() {
        ensure!(
            count < MAX_CANDIDATES,
            "Too many possible trash aliases; no file operation started"
        );
        let (_, candidate) = row?;
        ensure!(
            !equivalent(Path::new(&candidate), path)?,
            "Filesystem path is reserved by trash metadata; preserve it and resolve the alias"
        );
    }
    Ok(())
}

#[cfg(test)]
mod progress_tests {
    use super::*;
    use std::cell::Cell;
    use std::time::{Duration, Instant};

    const EXPENSIVE: &str = "WITH RECURSIVE work(n)AS(VALUES(1)UNION ALL SELECT n+1 FROM work WHERE n<100000)SELECT sum(n)FROM work";

    #[test]
    fn borrowed_cancellation_interrupts_sql_vm_and_is_unregistered_before_its_scope_expires() {
        let mut connection = Connection::open_in_memory().unwrap();
        let calls = Cell::new(0);
        {
            let cancelled = || {
                calls.set(calls.get() + 1);
                calls.get() >= 3
            };
            let scope = PrimeProgress::new(
                &mut connection,
                &cancelled,
                Instant::now() + Duration::from_secs(1),
            );
            let error = scope
                .connection
                .query_row(EXPENSIVE, [], |row| row.get::<_, i64>(0))
                .unwrap_err();
            assert_eq!(
                error.sqlite_error_code(),
                Some(rusqlite::ErrorCode::OperationInterrupted)
            );
            assert!(scope.state.canceled.get());
        }
        let before = calls.get();
        assert_eq!(
            connection
                .query_row(EXPENSIVE, [], |row| row.get::<_, i64>(0))
                .unwrap(),
            5_000_050_000
        );
        assert_eq!(calls.get(), before);
    }

    #[test]
    fn deadline_interrupts_sql_vm_and_callback_panics_do_not_unwind_across_sqlite() {
        let mut connection = Connection::open_in_memory().unwrap();
        {
            let scope = PrimeProgress::new(
                &mut connection,
                &|| false,
                Instant::now() - Duration::from_millis(1),
            );
            assert_eq!(
                scope
                    .connection
                    .query_row(EXPENSIVE, [], |row| row.get::<_, i64>(0))
                    .unwrap_err()
                    .sqlite_error_code(),
                Some(rusqlite::ErrorCode::OperationInterrupted)
            );
            assert!(scope.state.timed_out.get());
        }
        {
            let cancelled = || panic!("fixture cancellation callback panic");
            let scope = PrimeProgress::new(
                &mut connection,
                &cancelled,
                Instant::now() + Duration::from_secs(1),
            );
            assert_eq!(
                scope
                    .connection
                    .query_row(EXPENSIVE, [], |row| row.get::<_, i64>(0))
                    .unwrap_err()
                    .sqlite_error_code(),
                Some(rusqlite::ErrorCode::OperationInterrupted)
            );
            assert!(scope.state.panicked.get());
        }
        assert_eq!(
            connection
                .query_row("SELECT 1", [], |row| row.get::<_, i32>(0))
                .unwrap(),
            1
        );
    }

    fn empty_history() -> Connection {
        let mut connection = Connection::open_in_memory().unwrap();
        register(&connection).unwrap();
        crate::history::migrations()
            .to_latest(&mut connection)
            .unwrap();
        connection
    }

    #[test]
    fn post_query_cancellation_and_deadline_cannot_report_complete_without_a_vm_callback() {
        use rusqlite::hooks::{AuthAction, Authorization};
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let mut connection = empty_history();
        let flag = Arc::new(AtomicBool::new(false));
        let authorizer_flag = flag.clone();
        connection
            .authorizer(Some(move |context: rusqlite::hooks::AuthContext<'_>| {
                if matches!(
                    context.action,
                    AuthAction::Read {
                        table_name: "captures",
                        column_name: "file_path"
                    }
                ) {
                    authorizer_flag.store(true, Ordering::Relaxed);
                }
                Authorization::Allow
            }))
            .unwrap();
        PRIME_VM_CALLS.with(|calls| calls.set(0));
        let report = prime(&mut connection, None, true, &|| {
            flag.load(Ordering::Relaxed)
        })
        .unwrap();
        assert!(report.canceled && !report.complete);
        assert_eq!(report.remaining, None);
        assert_eq!(PRIME_VM_CALLS.with(Cell::get), 0);
        connection
            .authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> Authorization>)
            .unwrap();
        connection
            .authorizer(Some(|context: rusqlite::hooks::AuthContext<'_>| {
                if matches!(
                    context.action,
                    AuthAction::Read {
                        table_name: "captures",
                        column_name: "file_path"
                    }
                ) {
                    std::thread::sleep(Duration::from_millis(8));
                }
                Authorization::Allow
            }))
            .unwrap();
        let report = prime_until(
            &mut connection,
            None,
            true,
            &|| false,
            Instant::now() + Duration::from_millis(1),
        )
        .unwrap();
        assert!(report.timed_out && !report.complete);
        assert_eq!(report.remaining, None);
    }

    #[test]
    fn fully_indexed_large_history_uses_empty_partial_seek_before_repeated_physical_actions() {
        let mut connection = empty_history();
        let trigger: String = connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name='captures_reservation_keys_insert'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        connection
            .execute_batch("DROP TRIGGER captures_reservation_keys_insert")
            .unwrap();
        {
            let transaction = connection.transaction().unwrap();
            let mut statement=transaction.prepare("INSERT INTO captures(file_path,mime_type,width,height,file_size,created_at,source,reservation_name,file_identity)
                VALUES(?1,'image/png',1,1,64,'2020-01-01T00:00:00Z','import',screenshots_path_name(?1),'absent')").unwrap();
            for id in 0..30000 {
                statement
                    .execute([std::env::temp_dir()
                        .join(format!("indexed-reference-{id}.png"))
                        .to_str()
                        .unwrap()])
                    .unwrap();
            }
            drop(statement);
            transaction.commit().unwrap();
        }
        connection.execute_batch(&trigger).unwrap();
        for sql in [UNRESOLVED_SELECT, UNRESOLVED_AFTER] {
            let mut explain = connection
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap();
            let plans = explain
                .query_map([0], |row| row.get::<_, String>(3))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert!(
                plans
                    .iter()
                    .any(|plan| plan.contains("captures_unresolved_id") && plan.contains("id>?")),
                "{plans:?}"
            );
            assert!(
                !plans.iter().any(|plan| plan.starts_with("SCAN c")),
                "{plans:?}"
            );
            let mut statement = connection.prepare(sql).unwrap();
            let mut rows = statement.query([0]).unwrap();
            while rows.next().unwrap().is_some() {}
            drop(rows);
            assert_eq!(
                statement.get_status(rusqlite::StatementStatus::FullscanStep),
                0
            );
            assert!(statement.get_status(rusqlite::StatementStatus::VmStep) < 100);
        }
        PRIME_VM_CALLS.with(|calls| calls.set(0));
        for _ in 0..100 {
            let report = prime(&mut connection, None, true, &|| false).unwrap();
            assert!(report.complete);
            assert_eq!(report.checked, 0);
            assert_eq!(report.remaining, Some(0));
        }
        assert!(
            PRIME_VM_CALLS.with(Cell::get) < 10,
            "indexed priming executed too much SQLite work"
        );
    }
}
