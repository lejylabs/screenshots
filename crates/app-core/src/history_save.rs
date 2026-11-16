//! Explicit capture-save intent and recovery. All entry points run on a worker.
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2_10::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[cfg(test)]
#[path = "history_save_format_tests.rs"]
mod format_tests;
#[path = "history_save_native.rs"]
pub(super) mod native;
#[cfg(test)]
#[path = "history_save_tests.rs"]
mod tests;

pub(super) const MIGRATION: &str = "
CREATE TABLE pending_capture_save (
 singleton INTEGER PRIMARY KEY CHECK(singleton=1),
 operation_id TEXT NOT NULL UNIQUE CHECK(length(operation_id)=36),
 target_path TEXT NOT NULL CHECK(length(CAST(target_path AS BLOB))<=4096),
 stage_path TEXT NOT NULL CHECK(length(CAST(stage_path AS BLOB))<=4096),
 payload TEXT NOT NULL CHECK(length(CAST(payload AS BLOB))<=16384)
);
CREATE TRIGGER save_reserves_physical_operation BEFORE INSERT ON pending_file_move
 WHEN EXISTS(SELECT 1 FROM pending_capture_save)
 BEGIN SELECT RAISE(ABORT,'Capture save recovery is pending; recover it first'); END;
CREATE TRIGGER physical_operation_reserves_save BEFORE INSERT ON pending_capture_save
 WHEN EXISTS(SELECT 1 FROM pending_file_move)
 BEGIN SELECT RAISE(ABORT,'History file recovery is pending; recover it first'); END;
CREATE TRIGGER captures_save_reserve_insert BEFORE INSERT ON captures
 WHEN EXISTS(SELECT 1 FROM pending_capture_save WHERE screenshots_paths_alias(new.file_path,target_path) OR screenshots_paths_alias(new.file_path,stage_path))
 BEGIN SELECT RAISE(ABORT,'Path is reserved by a capture save; recover it first'); END;
CREATE TRIGGER captures_save_reserve_update BEFORE UPDATE OF file_path,id ON captures
 WHEN EXISTS(SELECT 1 FROM pending_capture_save WHERE screenshots_paths_alias(new.file_path,target_path) OR screenshots_paths_alias(new.file_path,stage_path))
 BEGIN SELECT RAISE(ABORT,'Path is reserved by a capture save; recover it first'); END;
";

const MAX_ENCODED: u64 = 272 * 1024 * 1024;
const MAX_PAYLOAD: usize = 16384;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ready {
    sha256: String,
    bytes: u64,
    width: u32,
    height: u32,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CancelStamp {
    bytes: u64,
    sha256: String,
    zero_flushed: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    version: u8,
    #[serde(default, skip_serializing_if = "crate::capture_format::is_png")]
    format: crate::ImageExportFormat,
    operation_id: String,
    target: PathBuf,
    stage: PathBuf,
    parent_identity: String,
    object_identity: String,
    created_at: String,
    ready: Option<Ready>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    canceling: Option<CancelStamp>,
}
#[derive(Debug, Serialize)]
pub struct PendingCaptureSave {
    pub operation_id: String,
    pub path: PathBuf,
    pub staging_path: PathBuf,
    pub ready: bool,
    pub canceling: bool,
}
#[derive(Debug, Serialize)]
pub struct CaptureSaveOutcome {
    pub operation_id: String,
    pub path: PathBuf,
    pub status: &'static str,
    pub capture_id: Option<i64>,
    pub pending: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preserved_staging: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}
fn outcome(intent: &Intent, status: &'static str, pending: bool) -> CaptureSaveOutcome {
    CaptureSaveOutcome {
        operation_id: intent.operation_id.clone(),
        path: intent.target.clone(),
        status,
        capture_id: None,
        pending,
        preserved_staging: None,
        warning: None,
    }
}
fn text_path(path: &Path) -> Result<&str> {
    let text = path.to_str().context("Save journal path must be UTF-8")?;
    ensure!(
        path.is_absolute()
            && !text.is_empty()
            && text.len() <= 4096
            && !text.chars().any(char::is_control),
        "Invalid bounded save journal path"
    );
    Ok(text)
}
fn names(intent: &Intent) -> Result<(&str, &str)> {
    Ok((
        intent
            .stage
            .file_name()
            .and_then(|p| p.to_str())
            .context("Invalid staging name")?,
        intent
            .target
            .file_name()
            .and_then(|p| p.to_str())
            .context("Invalid target name")?,
    ))
}
fn validate(intent: &Intent) -> Result<()> {
    ensure!(
        matches!(intent.version, 1 | 2)
            && (intent.version != 1 || intent.format == crate::ImageExportFormat::Png)
            && uuid::Uuid::parse_str(&intent.operation_id)
                .is_ok_and(|id| id.hyphenated().to_string() == intent.operation_id),
        "Invalid save intent version or operation ID"
    );
    text_path(&intent.target)?;
    text_path(&intent.stage)?;
    let (stage, target) = names(intent)?;
    ensure!(
        stage == format!(".screenshots-save-{}.part", intent.operation_id)
            && intent.stage.parent() == intent.target.parent()
            && stage != target
            && crate::ImageExportFormat::from_path(&intent.target).ok() == Some(intent.format),
        "Invalid save intent paths"
    );
    ensure!(
        intent.parent_identity.len() <= 80
            && intent.object_identity.len() <= 80
            && !intent.parent_identity.is_empty()
            && !intent.object_identity.is_empty()
            && intent.created_at.len() <= 64
            && chrono::DateTime::parse_from_rfc3339(&intent.created_at).is_ok(),
        "Invalid save intent identity or date"
    );
    if let Some(ready) = &intent.ready {
        ensure!(
            ((if intent.format == crate::ImageExportFormat::Png {
                33
            } else {
                12
            })..=crate::capture_format::encoded_limit(intent.format))
                .contains(&ready.bytes)
                && ready.sha256.len() == 64
                && ready
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "Invalid save intent fingerprint"
        );
        crate::validate_image_size(ready.width, ready.height)?;
    }
    if let Some(canceling) = &intent.canceling {
        ensure!(
            canceling.bytes <= MAX_ENCODED
                && canceling.sha256.len() == 64
                && canceling
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "Invalid cancellation fingerprint"
        );
    }
    Ok(())
}
fn load(connection: &rusqlite::Connection) -> Result<Option<Intent>> {
    let value=connection.query_row("SELECT CASE WHEN length(CAST(operation_id AS BLOB))=36 THEN operation_id END,CASE WHEN length(CAST(target_path AS BLOB))<=4096 THEN target_path END,CASE WHEN length(CAST(stage_path AS BLOB))<=4096 THEN stage_path END,CASE WHEN length(CAST(payload AS BLOB))<=16384 THEN payload END FROM pending_capture_save WHERE singleton=1",[],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?))).optional()?;
    value
        .map(|(id, target, stage, value)| {
            let intent: Intent = serde_json::from_str(&value)
                .context("Malformed capture save intent; files preserved")?;
            validate(&intent)?;
            ensure!(
                intent.operation_id == id
                    && intent.target == Path::new(&target)
                    && intent.stage == Path::new(&stage),
                "Capture save intent columns disagree; files preserved"
            );
            Ok(intent)
        })
        .transpose()
}
pub fn pending_capture_save(database: &Path) -> Result<Option<PendingCaptureSave>> {
    let connection = crate::storage::open_read_only(database)?;
    let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    ensure!(
        (1..=10).contains(&version),
        "Unsupported capture history schema"
    );
    connection.prepare("SELECT id,file_path,mime_type,width,height,file_size,created_at,source FROM captures LIMIT 0")?;
    if version < 10 {
        return Ok(None);
    }
    Ok(load(&connection)?.map(|i| PendingCaptureSave {
        operation_id: i.operation_id,
        path: i.target,
        staging_path: i.stage,
        ready: i.ready.is_some(),
        canceling: i.canceling.is_some(),
    }))
}
fn clear(connection: &rusqlite::Connection, intent: &Intent) -> Result<()> {
    ensure!(
        connection.execute(
            "DELETE FROM pending_capture_save WHERE singleton=1 AND operation_id=?1 AND payload=?2",
            params![intent.operation_id, serde_json::to_string(intent)?]
        )? == 1,
        "Capture save intent changed; files preserved"
    );
    Ok(())
}
fn encoded_intent(intent: &Intent) -> Result<String> {
    let payload = serde_json::to_string(intent)?;
    ensure!(
        payload.len() <= MAX_PAYLOAD,
        "Save intent exceeds metadata limit"
    );
    Ok(payload)
}
#[cfg(any(target_os = "windows", test))]
fn replace_intent(database: &Path, old: &Intent, new: &Intent) -> Result<()> {
    let connection = super::open_existing(database)?;
    ensure!(connection.execute("UPDATE pending_capture_save SET payload=?1 WHERE singleton=1 AND operation_id=?2 AND payload=?3",params![encoded_intent(new)?,old.operation_id,encoded_intent(old)?])?==1,"Capture save changed before cancellation");
    Ok(())
}
fn canceled_outcome(intent: &Intent) -> CaptureSaveOutcome {
    let mut report = outcome(intent, "canceled", false);
    if cfg!(target_os = "windows") {
        report.warning=Some("Owned staging contents were truncated and flushed, owned handles closed, and deletion observed. Windows namespace power-loss durability remains unverified; an empty entry may reappear.".into());
    }
    report
}
#[cfg(any(target_os = "windows", test))]
fn fingerprint(mut file: &File) -> Result<(u64, String)> {
    let version = crate::FileIdentity::from_file(file)?;
    let bytes = file.metadata()?.len();
    ensure!(
        bytes <= MAX_ENCODED,
        "Cancellation staging exceeds encoded limit"
    );
    file.rewind()?;
    let mut remaining = bytes;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let start = Instant::now();
    while remaining > 0 {
        ensure!(
            start.elapsed() < Duration::from_secs(30),
            "Cancellation inspection exceeded 30 seconds"
        );
        let limit = remaining.min(buffer.len() as u64) as usize;
        let count = file.read(&mut buffer[..limit])?;
        ensure!(count > 0, "Cancellation staging changed while reading");
        hash.update(&buffer[..count]);
        remaining -= count as u64;
    }
    version.verify_file(file)?;
    Ok((bytes, format!("{:x}", hash.finalize())))
}
#[cfg(any(target_os = "windows", test))]
fn cancel_with_content_flush(
    database: &Path,
    mut intent: Intent,
    directory: &native::Directory,
    file: File,
) -> Result<CaptureSaveOutcome> {
    let named = directory
        .open_file(names(&intent)?.0, false)?
        .context("Cancellation staging is missing")?;
    ensure!(
        native::identity(&named)? == intent.object_identity
            && native::identity(&file)? == intent.object_identity,
        "Cancellation staging was replaced; all files preserved"
    );
    drop(named);
    native::require_single_link(&file)?;
    if intent.canceling.is_none() {
        if intent.ready.is_some() {
            verify_ready(&file, &intent)?;
        }
        let (bytes, sha256) = fingerprint(&file)?;
        let mut next = intent.clone();
        next.canceling = Some(CancelStamp {
            bytes,
            sha256,
            zero_flushed: false,
        });
        replace_intent(database, &intent, &next)?;
        intent = next;
        checkpoint("cancel_intent")?;
    }
    let stamp = intent.canceling.as_ref().unwrap();
    if file.metadata()?.len() != 0 {
        let (bytes, sha256) = fingerprint(&file)?;
        ensure!(
            bytes == stamp.bytes && sha256 == stamp.sha256,
            "Cancellation staging changed; all bytes preserved"
        );
        if stamp.zero_flushed {
            let mut next = intent.clone();
            next.canceling.as_mut().unwrap().zero_flushed = false;
            replace_intent(database, &intent, &next)?;
            intent = next;
        }
    }
    if !intent.canceling.as_ref().unwrap().zero_flushed {
        native::require_single_link(&file)?;
        let named = directory
            .open_file(names(&intent)?.0, false)?
            .context("Cancellation staging disappeared")?;
        ensure!(
            native::identity(&named)? == intent.object_identity,
            "Cancellation staging name changed; preserved"
        );
        drop(named);
        file.set_len(0)?;
        checkpoint("cancel_truncated")?;
        file.sync_all()?;
        checkpoint("cancel_flushed")?;
        let mut next = intent.clone();
        next.canceling.as_mut().unwrap().zero_flushed = true;
        replace_intent(database, &intent, &next)?;
        intent = next;
        checkpoint("cancel_zero_recorded")?;
    }
    ensure!(
        file.metadata()?.len() == 0,
        "Cancellation no longer names an empty owned stage; preserved"
    );
    directory.remove_owned(names(&intent)?.0, file)?;
    checkpoint("cancel_deleted")?;
    clear(&super::open_existing(database)?, &intent)?;
    Ok(canceled_outcome(&intent))
}
fn cancel_owned(
    database: &Path,
    intent: Intent,
    directory: &native::Directory,
    file: File,
) -> Result<CaptureSaveOutcome> {
    #[cfg(target_os = "windows")]
    {
        cancel_with_content_flush(database, intent, directory, file)
    }
    #[cfg(not(target_os = "windows"))]
    {
        directory.remove_owned(names(&intent)?.0, file)?;
        clear(&super::open_existing(database)?, &intent)?;
        Ok(canceled_outcome(&intent))
    }
}
fn cleanup_failed_save(
    database: &Path,
    intent: &Intent,
    directory: &native::Directory,
    file: File,
) -> Result<()> {
    let stored = load(&super::open_existing(database)?)?
        .context("Save intent is unavailable; staged bytes preserved")?;
    ensure!(
        stored.operation_id == intent.operation_id,
        "Save operation changed; staged bytes preserved"
    );
    cancel_owned(database, stored, directory, file)?;
    Ok(())
}
#[cfg(test)]
fn prepare(database: &Path, destination: &Path) -> Result<(Intent, native::Directory, File)> {
    prepare_format(database, destination, crate::ImageExportFormat::Png)
}
fn prepare_format(
    database: &Path,
    destination: &Path,
    format: crate::ImageExportFormat,
) -> Result<(Intent, native::Directory, File)> {
    let absolute = std::path::absolute(destination)?;
    text_path(&absolute)?;
    ensure!(
        crate::ImageExportFormat::from_path(&absolute)? == format,
        "Capture history destination extension does not match its encoding"
    );
    let parent = absolute.parent().context("Save needs a parent directory")?;
    super::ensure_storage_directory(parent)?;
    let canonical_parent = parent.canonicalize()?;
    let directory = native::Directory::open(&canonical_parent, None)?;
    // Preserve native user spelling (/var aliases and ordinary Windows paths)
    // only after proving it opens the same held parent object.
    let directory = match native::Directory::open(parent, Some(directory.identity())) {
        Ok(original) => original,
        Err(_) => directory,
    };
    let parent = directory.path.clone();
    let target = parent.join(absolute.file_name().context("Save needs a filename")?);
    text_path(&target)?;
    let operation_id = uuid::Uuid::new_v4().to_string();
    let stage = parent.join(format!(".screenshots-save-{operation_id}.part"));
    // Reserve an empty, locked native object before intent commit; no pixels are
    // written until that commit succeeds. A crash here can leave only an empty file.
    let stage_name = stage.file_name().unwrap().to_str().unwrap();
    let file = directory.open_file(stage_name, true)?.unwrap();
    file.try_lock().context("Capture save staging is busy")?;
    let intent = Intent {
        version: if format == crate::ImageExportFormat::Png {
            1
        } else {
            2
        },
        format,
        operation_id,
        target,
        stage,
        parent_identity: directory.identity().into(),
        object_identity: native::identity(&file)?,
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        ready: None,
        canceling: None,
    };
    let result: Result<()> = (|| {
        ensure!(
            directory.open_file(names(&intent)?.1, false)?.is_none(),
            "Capture destination already exists; preserved"
        );
        let mut connection = super::open(database)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure!(
            load(&transaction)?.is_none(),
            "Capture save recovery is pending; recover it first"
        );
        // Indexed name candidates bound alias checks for absent destination names.
        let mut statement=transaction.prepare("SELECT CASE WHEN length(CAST(file_path AS BLOB))<=4096 THEN file_path END FROM captures WHERE reservation_name=screenshots_path_name(?1) OR reservation_name IS NULL LIMIT 101")?;
        for (index, row) in statement
            .query_map([text_path(&intent.target)?], |r| r.get::<_, String>(0))?
            .enumerate()
        {
            ensure!(
                index < 100,
                "Too many possible save aliases; no image written"
            );
            let candidate = row?;
            let aliases: bool = transaction.query_row(
                "SELECT screenshots_paths_alias(?1,?2)",
                params![text_path(&intent.target)?, candidate],
                |r| r.get(0),
            )?;
            ensure!(
                !aliases,
                "Capture destination is already referenced by history; preserved"
            );
        }
        drop(statement);
        ensure!(!transaction.query_row("SELECT EXISTS(SELECT 1 FROM history_trash WHERE trash_id=screenshots_trash_id(?1))",[text_path(&intent.target)?],|r|r.get::<_,bool>(0))?,"Capture destination is reserved by trash; preserved");
        let mut largest = intent.clone();
        largest.ready = Some(Ready {
            sha256: "f".repeat(64),
            bytes: crate::capture_format::encoded_limit(format),
            width: u32::MAX,
            height: u32::MAX,
        });
        largest.canceling = Some(CancelStamp {
            bytes: MAX_ENCODED,
            sha256: "f".repeat(64),
            zero_flushed: false,
        });
        encoded_intent(&largest)?;
        let payload = encoded_intent(&intent)?;
        transaction.execute("INSERT INTO pending_capture_save(singleton,operation_id,target_path,stage_path,payload) VALUES(1,?1,?2,?3,?4)",params![intent.operation_id,text_path(&intent.target)?,text_path(&intent.stage)?,payload])?;
        directory.sync()?;
        transaction.commit()?;
        Ok(())
    })();
    if let Err(error) = result {
        let cleaned = directory.remove_owned(names(&intent)?.0, file);
        return Err(error).context(if cleaned.is_ok() {
            "Capture was not saved"
        } else {
            "Capture was not saved; empty staging cleanup could not be confirmed"
        });
    }
    Ok((intent, directory, file))
}
struct HashWriter<'a> {
    limit: u64,
    file: &'a mut File,
    hash: Sha256,
    bytes: u64,
}
impl Write for HashWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() as u64 > self.limit.saturating_sub(self.bytes) {
            return Err(std::io::Error::other(
                "Capture exceeds its format encoded limit",
            ));
        }
        let count = self.file.write(bytes)?;
        self.hash.update(&bytes[..count]);
        self.bytes += count as u64;
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}
fn checked(cancelled: &impl Fn() -> bool, start: Instant) -> Result<()> {
    ensure!(!cancelled(), "Capture save canceled");
    ensure!(
        start.elapsed() < Duration::from_secs(60),
        "Capture save deadline exceeded"
    );
    Ok(())
}
fn save(
    database: &Path,
    destination: &Path,
    cancelled: impl Fn() -> bool,
    encode: impl FnOnce(&mut dyn Write) -> Result<(u32, u32)>,
) -> Result<CaptureSaveOutcome> {
    save_format(
        database,
        destination,
        crate::ImageExportFormat::Png,
        cancelled,
        encode,
    )
}
fn save_format(
    database: &Path,
    destination: &Path,
    format: crate::ImageExportFormat,
    cancelled: impl Fn() -> bool,
    encode: impl FnOnce(&mut dyn Write) -> Result<(u32, u32)>,
) -> Result<CaptureSaveOutcome> {
    let start = Instant::now();
    checked(&cancelled, start)?;
    let (mut intent, directory, mut file) = prepare_format(database, destination, format)?;
    let written: Result<crate::FileIdentity> = (|| {
        let mut writer = HashWriter {
            limit: crate::capture_format::encoded_limit(format),
            file: &mut file,
            hash: Sha256::new(),
            bytes: 0,
        };
        let stop = || cancelled() || start.elapsed() >= Duration::from_secs(60);
        let (width, height) = {
            let mut buffered = crate::image_cancel::CheckedImageWriter::new(&mut writer, &stop)?;
            let dimensions = encode(&mut buffered)?;
            buffered.flush()?;
            dimensions
        };
        checked(&cancelled, start)?;
        crate::validate_image_size(width, height)?;
        ensure!(
            ((if format == crate::ImageExportFormat::Png {
                33
            } else {
                12
            })..=crate::capture_format::encoded_limit(format))
                .contains(&writer.bytes),
            "Capture output has invalid length"
        );
        let ready = Ready {
            sha256: format!("{:x}", writer.hash.finalize()),
            bytes: writer.bytes,
            width,
            height,
        };
        ensure!(
            file.metadata()?.len() == ready.bytes,
            "Encoded save length changed"
        );
        file.sync_all()?;
        directory.sync()?;
        let admitted = crate::FileIdentity::from_file(&file)?;
        let old = serde_json::to_string(&intent)?;
        intent.ready = Some(ready);
        let connection = super::open_existing(database)?;
        ensure!(connection.execute("UPDATE pending_capture_save SET payload=?1 WHERE singleton=1 AND operation_id=?2 AND payload=?3",params![serde_json::to_string(&intent)?,intent.operation_id,old])?==1,"Capture save intent changed before publication");
        Ok(admitted)
    })();
    let admitted = match written {
        Ok(admitted) => admitted,
        Err(error) => {
            // Use the actual committed form if the ready update failed. Never forget an
            // intent until its owned staging removal and parent barrier both succeed.
            let _cleanup = cleanup_failed_save(database, &intent, &directory, file);
            return Err(error).context(format!(
                "Capture not published; inspect pending save {} if cleanup is pending",
                intent.operation_id
            ));
        }
    };
    checkpoint("ready")?;
    let published = (|| {
        checked(&cancelled, start)?;
        admitted.verify_file(&file)?;
        directory.publish(names(&intent)?.0, names(&intent)?.1, &file)
    })();
    if let Err(error) = published {
        let _cleanup = cleanup_failed_save(database, &intent, &directory, file);
        return Err(error).context("Capture not published; existing destination preserved");
    }
    checkpoint("published")?;
    match finish(database, &intent, &directory, &file, false) {
        Ok(report) => Ok(report),
        Err(error) => {
            let mut report = outcome(&intent, "saved_unindexed", true);
            report.warning = Some(format!(
                "Image was published; history recovery is pending for operation {}: {error:#}",
                intent.operation_id
            ));
            Ok(report)
        }
    }
}
/// Encode finalized pixels once. Clipboard and image ownership are unchanged.
pub fn save_capture_with_history(
    database: &Path,
    image: &crate::CapturedImage,
    destination: &Path,
    cancelled: impl Fn() -> bool,
) -> Result<CaptureSaveOutcome> {
    crate::validate_image_size(image.pixels.width(), image.pixels.height())?;
    save(database, destination, cancelled, |writer| {
        use image::ImageEncoder;
        use image::codecs::png::{CompressionType, FilterType, PngEncoder};
        PngEncoder::new_with_quality(writer, CompressionType::Fast, FilterType::Adaptive)
            .write_image(
                image.pixels.as_raw(),
                image.pixels.width(),
                image.pixels.height(),
                image::ExtendedColorType::Rgba8,
            )?;
        Ok(image.pixels.dimensions())
    })
}
/// Explicit normal-capture format. Validate before reserving files or storage.
pub fn save_capture_with_history_policy(
    database: &Path,
    image: &crate::CapturedImage,
    destination: &Path,
    policy: crate::CaptureFormatPolicy,
    cancelled: impl Fn() -> bool,
) -> Result<CaptureSaveOutcome> {
    let cancelled = crate::image_cancel::latch(cancelled);
    policy.validate_destination(destination)?;
    policy.validate_pixels(&image.pixels, &cancelled)?;
    save_format(
        database,
        destination,
        policy.format(),
        &cancelled,
        |writer| {
            policy.encode_to(&image.pixels, writer, &cancelled)?;
            Ok(image.pixels.dimensions())
        },
    )
}

/// Explicit conversion from a saved image into a history output.
pub fn save_saved_image_with_history_policy(
    database: &Path,
    source: &Path,
    destination: &Path,
    policy: crate::CaptureFormatPolicy,
    cancelled: impl Fn() -> bool,
) -> Result<CaptureSaveOutcome> {
    let cancelled = crate::image_cancel::latch(cancelled);
    policy.validate_destination(destination)?;
    ensure!(
        source.metadata()?.is_file(),
        "Saved image source must be regular"
    );
    let (file, admitted) = crate::FileIdentity::open(source)?;
    crate::saved_image::inspect_saved_image_file(&file, &cancelled)?;
    let pixels = crate::load_editor_image_with_identity_and_limit_with_cancel(
        source,
        &admitted,
        crate::MAX_CAPTURE_BYTES,
        &cancelled,
    )?;
    admitted.verify_file(&file)?;
    policy.validate_pixels(&pixels, &cancelled)?;
    save_format(
        database,
        destination,
        policy.format(),
        &cancelled,
        |writer| {
            policy.encode_to(&pixels, writer, &cancelled)?;
            Ok(pixels.dimensions())
        },
    )
}

/// Copy an admitted PNG/JPEG/WebP exactly; no decode, metadata loss or new lossy generation.
pub fn copy_saved_image_with_history(
    database: &Path,
    source: &Path,
    destination: &Path,
    cancelled: impl Fn() -> bool,
) -> Result<CaptureSaveOutcome> {
    let cancelled = crate::image_cancel::latch(cancelled);
    ensure!(!cancelled(), crate::CaptureError::Canceled);
    ensure!(
        source.metadata()?.is_file(),
        "Saved image source must be regular"
    );
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut source = options.open(source)?;
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            source.metadata()?.file_attributes()
                & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
                == 0,
            "Saved image source refuses reparse points"
        );
    }
    let admitted = crate::FileIdentity::from_file(&source)?;
    let info = crate::saved_image::inspect_saved_image_file(&source, &cancelled)?;
    ensure!(
        crate::ImageExportFormat::from_path(destination)? == info.format,
        "Copy destination extension does not match source contents"
    );
    save_format(database, destination, info.format, &cancelled, |writer| {
        crate::saved_image::copy_saved_image_to(&mut source, &admitted, info, writer, &cancelled)?;
        Ok((info.width, info.height))
    })
}

/// Original-size PNG Save As preserves encoded bytes; never decodes source pixels.
pub fn copy_saved_png_with_history(
    database: &Path,
    source: &Path,
    destination: &Path,
    cancelled: impl Fn() -> bool,
) -> Result<CaptureSaveOutcome> {
    ensure!(!cancelled(), "Capture save canceled before source I/O");
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut source = options.open(source)?;
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            source.metadata()?.file_attributes()
                & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
                == 0,
            "Saved PNG source refuses reparse points"
        );
    }
    let admitted = crate::FileIdentity::from_file(&source)?;
    ensure!(
        (33..=MAX_ENCODED).contains(&source.metadata()?.len()),
        "Saved PNG exceeds the encoded limit"
    );
    let mut header = [0; 33];
    source.read_exact(&mut header)?;
    let dimensions = png_header(&header)?;
    source.rewind()?;
    let length = source.metadata()?.len();
    save(database, destination, cancelled, |writer| {
        let mut remaining = length;
        let mut bytes = [0u8; 64 * 1024];
        while remaining > 0 {
            let limit = remaining.min(bytes.len() as u64) as usize;
            let count = source.read(&mut bytes[..limit])?;
            ensure!(count > 0, "Saved PNG changed while reading");
            writer.write_all(&bytes[..count])?;
            remaining -= count as u64;
        }
        admitted.verify_file(&source)?;
        Ok(dimensions)
    })
}
fn png_header(header: &[u8; 33]) -> Result<(u32, u32)> {
    ensure!(
        &header[..8] == b"\x89PNG\r\n\x1a\n"
            && header[8..12] == [0, 0, 0, 13]
            && &header[12..16] == b"IHDR"
            && crc32fast::hash(&header[12..29])
                == u32::from_be_bytes(header[29..33].try_into().unwrap()),
        "Invalid saved PNG header"
    );
    let width = u32::from_be_bytes(header[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(header[20..24].try_into().unwrap());
    crate::validate_image_size(width, height)?;
    Ok((width, height))
}
fn verify_ready(mut file: &File, intent: &Intent) -> Result<()> {
    ensure!(
        native::identity(file)? == intent.object_identity,
        "Journaled image was replaced; preserved"
    );
    let ready = intent
        .ready
        .as_ref()
        .context("Image encoding did not complete; staging is preserved")?;
    let version = crate::FileIdentity::from_file(file)?;
    ensure!(
        file.metadata()?.len() == ready.bytes,
        "Journaled image length changed; preserved"
    );
    file.rewind()?;
    let mut hash = Sha256::new();
    let mut remaining = ready.bytes;
    let mut bytes = [0; 64 * 1024];
    let start = Instant::now();
    while remaining > 0 {
        ensure!(
            start.elapsed() < Duration::from_secs(30),
            "Save recovery inspection exceeded 30 seconds"
        );
        let limit = remaining.min(bytes.len() as u64) as usize;
        let count = file.read(&mut bytes[..limit])?;
        ensure!(count > 0, "Journaled image changed while reading");
        hash.update(&bytes[..count]);
        remaining -= count as u64;
    }
    version.verify_file(file)?;
    ensure!(
        format!("{:x}", hash.finalize()) == ready.sha256,
        "Journaled image content changed; preserved"
    );
    let actual = crate::saved_image::inspect_saved_image_file(file, &|| {
        start.elapsed() >= Duration::from_secs(30)
    })?;
    ensure!(
        actual.format == intent.format
            && actual.width == ready.width
            && actual.height == ready.height
            && actual.encoded_bytes == ready.bytes,
        "Journaled image format or dimensions disagree; preserved"
    );
    version.verify_file(file)?;
    Ok(())
}
fn finish(
    database: &Path,
    intent: &Intent,
    directory: &native::Directory,
    file: &File,
    recovery: bool,
) -> Result<CaptureSaveOutcome> {
    let ready = intent
        .ready
        .as_ref()
        .context("Staged image did not finish encoding")?;
    ensure!(
        native::identity(file)? == intent.object_identity,
        "Journaled image identity changed"
    );
    directory.sync()?;
    let named = directory
        .open_file(names(intent)?.1, false)?
        .context("Published capture is missing")?;
    ensure!(
        native::identity(&named)? == intent.object_identity
            && named.metadata()?.len() == ready.bytes,
        "Published capture changed; preserved"
    );
    if recovery {
        // Windows byte-range locks belong to the held handle. Named reopens
        // establish identity/length only; hash through the locked handle itself.
        verify_ready(file, intent)?;
    }
    checkpoint("before_metadata")?;
    let mut connection = super::open_existing(database)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    clear(&transaction, intent)?;
    let changed = super::insert_capture(
        &transaction,
        &intent.target,
        (
            ready.width,
            ready.height,
            i64::try_from(ready.bytes)?,
            crate::capture_format::image_mime_type(intent.format),
        ),
        "capture",
    )?;
    ensure!(
        changed == 1,
        "Capture path is already indexed; existing history preserved"
    );
    transaction.execute(
        "UPDATE captures SET created_at=?1 WHERE file_path=?2",
        params![intent.created_at, text_path(&intent.target)?],
    )?;
    let id = transaction.query_row(
        "SELECT id FROM captures WHERE file_path=?1",
        [text_path(&intent.target)?],
        |r| r.get(0),
    )?;
    directory.verify()?;
    transaction.commit()?;
    checkpoint("metadata_committed")?;
    let mut report = outcome(intent, "indexed", false);
    report.capture_id = Some(id);
    Ok(report)
}
pub fn recover_capture_save(
    database: &Path,
    operation: &str,
    action: super::MoveRecovery,
) -> Result<CaptureSaveOutcome> {
    let connection = super::open_existing(database)?;
    let intent = load(&connection)?.context("No capture save recovery is pending")?;
    ensure!(
        intent.canceling.is_none() || matches!(action, super::MoveRecovery::Cancel),
        "Cancellation has started; use cancel to complete it"
    );
    ensure!(
        intent.operation_id == operation,
        "Capture save operation changed; no files modified"
    );
    drop(connection);
    let directory = native::Directory::open(
        intent.target.parent().unwrap(),
        Some(&intent.parent_identity),
    )?;
    let (stage, target) = names(&intent)?;
    let staging = if cfg!(target_os = "windows") && matches!(action, super::MoveRecovery::Cancel) {
        directory.open_writable(stage)?
    } else {
        directory.open_file(stage, false)?
    };
    let published = directory.open_file(target, false)?;
    let file = match (staging, published) {
        (Some(stage), None) => stage,
        (None, Some(target)) => target,
        (None, None) => {
            ensure!(
                matches!(action, super::MoveRecovery::Cancel),
                "Journaled save files are missing; cancel clears metadata only"
            );
            if cfg!(target_os = "windows")
                && !intent
                    .canceling
                    .as_ref()
                    .is_some_and(|stamp| stamp.zero_flushed)
            {
                let mut report = outcome(&intent, "cancellation_absence_unconfirmed", true);
                report.warning=Some("Windows cannot establish durable namespace absence without a prior flushed-empty proof; recovery intent is preserved. Native power-loss acceptance remains open.".into());
                return Ok(report);
            }
            // An earlier unlink may have succeeded before its parent barrier
            // failed. Confirm durable absence before forgetting the only intent.
            directory.sync()?;
            clear(&super::open_existing(database)?, &intent)?;
            return Ok(canceled_outcome(&intent));
        }
        (Some(_), Some(_)) => {
            anyhow::bail!("Both staging and destination exist; all files preserved")
        }
    };
    ensure!(
        native::identity(&file)? == intent.object_identity,
        "Journaled save file was replaced; all files preserved"
    );
    file.try_lock()
        .context("Capture save is still active; recovery refused")?;
    // Reload after acquiring the native lock so a concurrent operation cannot
    // finish or change the intent between inspection and this recovery.
    let current =
        load(&super::open_existing(database)?)?.context("Capture save already finished")?;
    ensure!(
        serde_json::to_string(&current)? == serde_json::to_string(&intent)?,
        "Capture save changed during recovery"
    );
    let target_exists = directory.open_file(target, false)?.is_some();
    match action {
        super::MoveRecovery::Finish => {
            verify_ready(&file, &intent)?;
            if !target_exists {
                directory.publish(stage, target, &file)?;
            }
            finish(database, &intent, &directory, &file, true)
        }
        super::MoveRecovery::Cancel => {
            if target_exists {
                verify_ready(&file, &intent)?;
                let mut report = outcome(&intent, "saved_unindexed", true);
                report.warning=Some("The image is already published and was preserved. Finish this intent to index it; cancellation cannot undo a completed save.".into());
                return Ok(report);
            }
            if intent.canceling.is_some() {
                #[cfg(any(target_os = "windows", test))]
                {
                    // Tests exercise this state machine on private Unix files;
                    // Windows uses the same writable locked native handle.
                    #[cfg(not(target_os = "windows"))]
                    let file = {
                        drop(file);
                        let file = directory
                            .open_writable(stage)?
                            .context("Cancellation stage disappeared")?;
                        file.try_lock()?;
                        file
                    };
                    return cancel_with_content_flush(database, intent, &directory, file);
                }
                #[cfg(not(any(target_os = "windows", test)))]
                anyhow::bail!(
                    "This cancellation state needs its original Windows backend; files preserved"
                );
            }
            if intent.ready.is_some() {
                verify_ready(&file, &intent)?;
                cancel_owned(database, intent, &directory, file)
            } else if file.metadata()?.len() == 0 {
                cancel_owned(database, intent, &directory, file)
            } else {
                let mut report = outcome(&intent, "incomplete_staging_preserved", true);
                report.preserved_staging = Some(intent.stage.clone());
                report.warning=Some(if cfg!(target_os="windows") {"Encoding was interrupted before its content fingerprint committed. This exact staging file is preserved. Windows cannot clear an absent stage without a flushed-empty proof; native namespace/power-loss acceptance remains open."} else {"Encoding was interrupted before its content fingerprint committed. Preserve or move this exact staging file explicitly, then cancel the now-missing intent; no image was published."}.into());
                Ok(report)
            }
        }
    }
}
#[cfg(test)]
thread_local! {static STOP_AT:std::cell::RefCell<Option<&'static str>>=const{std::cell::RefCell::new(None)};}
pub(super) fn checkpoint(_stage: &str) -> Result<()> {
    #[cfg(test)]
    if STOP_AT.with(|s| s.borrow().is_some_and(|v| v == _stage)) {
        #[cfg(unix)]
        if matches!(
            _stage,
            "parent_sync" | "ancestor_parent_sync" | "ancestor_created"
        ) {
            return Err(std::io::Error::from_raw_os_error(libc::EIO).into());
        }
        anyhow::bail!("Injected capture save interruption");
    }
    Ok(())
}
