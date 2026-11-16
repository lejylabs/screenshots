//! Durable schedule ownership and at-most-once task admission, separate from history.
use super::model::{MAX_SCHEDULES, Spec, valid_id};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
#[cfg(not(target_os = "windows"))]
use std::fs::OpenOptions;
use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
const APPLICATION_ID: i32 = 0x53534348;
const MAX_BYTES: u64 = 32 * 1024 * 1024;
const MAX_SEAL: usize = 256 * 1024;
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Schedule {
    pub id: String,
    pub profile: PathBuf,
    pub seal: Vec<u8>,
    pub spec: Spec,
    pub next: u64,
    pub paused: bool,
    pub reason: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Run {
    pub id: String,
    pub schedule: String,
    pub ordinal: u64,
    pub due_ms: i64,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Wake {
    pub id: String,
    pub at_ms: i64,
}
pub(super) struct Store {
    connection: Connection,
    _lock: Option<ScheduleLock>,
    origin: Instant,
    deadline: Arc<AtomicU64>,
    canceled: Arc<AtomicBool>,
    observe_cancel: Arc<AtomicBool>,
}
struct ScheduleLock(File);
impl Drop for ScheduleLock {
    fn drop(&mut self) {
        // A concurrent fork may retain this open description until exec; closing
        // the owner's descriptor alone would keep flock held during that window.
        let _ = self.0.unlock();
    }
}
fn safe_file(file: &File, max: u64) -> Result<()> {
    let m = file.metadata()?;
    ensure!(
        m.is_file() && m.len() <= max,
        "Schedule state must be a bounded regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o777 == 0o600 && m.nlink() == 1,
            "Schedule files must be current-user-owned, private (0600) and have one link"
        );
    }
    #[cfg(target_os = "windows")]
    super::private_file::validate(file)?;
    Ok(())
}
fn open_file(path: &Path, create: bool, writable: bool) -> Result<File> {
    #[cfg(target_os = "windows")]
    {
        return super::private_file::open_access(path, create, writable);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut options = OpenOptions::new();
        options.read(true).write(writable);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
        }
        if create {
            options.create_new(true);
        }
        Ok(options.open(path)?)
    }
}
fn canonical_path(path: &Path, create: bool) -> Result<PathBuf> {
    ensure!(
        path.as_os_str().as_encoded_bytes().len() <= 4096,
        "Schedule database path exceeds 4096 bytes"
    );
    let absolute = std::path::absolute(path)?;
    let chosen = absolute
        .parent()
        .context("Schedule database needs a parent")?;
    if create && !chosen.try_exists()? {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().mode(0o700).create(chosen)?;
        }
        #[cfg(target_os = "windows")]
        super::private_file::directory(chosen, true)?;
    }
    let parent = chosen.canonicalize()?;
    #[cfg(target_os = "windows")]
    super::private_file::directory(&parent, false)?;
    let name = absolute
        .file_name()
        .context("Schedule database needs a filename")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = parent.metadata()?;
        ensure!(
            m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o022 == 0,
            "Schedule directory must belong to the current user and not be writable by others"
        );
    }
    Ok(parent.join(name))
}
impl Store {
    pub fn open(path: &Path, writable: bool, create: bool) -> Result<Self> {
        let path = canonical_path(path, create && writable)?;
        let lock = if writable {
            let lock_path = path.with_file_name(format!(
                "{}.lock",
                path.file_name()
                    .context("Missing database name")?
                    .to_str()
                    .context("Schedule database filename must be UTF-8")?
            ));
            let file = match open_file(&lock_path, true, true) {
                Ok(f) => f,
                Err(e)
                    if e.downcast_ref::<std::io::Error>()
                        .is_some_and(|e| e.kind() == std::io::ErrorKind::AlreadyExists) =>
                {
                    open_file(&lock_path, false, true)?
                }
                Err(e) => return Err(e),
            };
            safe_file(&file, 0)?;
            file.try_lock().map_err(|_| {
                anyhow::anyhow!(
                    "Schedule database is busy; stop its service before editing or resolving"
                )
            })?;
            Some(ScheduleLock(file))
        } else {
            None
        };
        let mut newly_created = false;
        let file = match open_file(&path, false, writable) {
            Ok(file) => file,
            Err(error)
                if create
                    && writable
                    && error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                newly_created = true;
                open_file(&path, true, true)?
            }
            Err(error) => return Err(error.context("Could not open existing schedule database")),
        };
        safe_file(&file, MAX_BYTES)?;
        for suffix in ["-journal", "-wal", "-shm"] {
            let mut name = path.as_os_str().to_os_string();
            name.push(suffix);
            match open_file(Path::new(&name), false, false) {
                Ok(sidecar) => safe_file(&sidecar, MAX_BYTES + 65536)?,
                Err(error)
                    if error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) => {}
                Err(error) => {
                    return Err(error.context("Unsafe or unavailable schedule SQLite sidecar"));
                }
            }
        }
        // Read admission and the native lock are held before SQLite opens the same cooperative path.
        let flags = if writable {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        };
        let mut connection =
            Connection::open_with_flags(&path, flags | OpenFlags::SQLITE_OPEN_NOFOLLOW)?;
        connection.busy_timeout(Duration::from_millis(100))?;
        let origin = Instant::now();
        let deadline = Arc::new(AtomicU64::new(5000));
        let observed = deadline.clone();
        let canceled = Arc::new(AtomicBool::new(false));
        let observe_cancel = Arc::new(AtomicBool::new(true));
        let flag = canceled.clone();
        let enabled = observe_cancel.clone();
        connection.progress_handler(
            1000,
            Some(move || {
                (enabled.load(Ordering::Relaxed) && flag.load(Ordering::Relaxed))
                    || origin.elapsed().as_millis() >= u128::from(observed.load(Ordering::Relaxed))
            }),
        )?;
        let store = Self {
            connection,
            _lock: lock,
            origin,
            deadline,
            canceled,
            observe_cancel,
        };
        store.deadline();
        if newly_created {
            store.connection.execute_batch("PRAGMA page_size=4096; PRAGMA max_page_count=8192; PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA secure_delete=ON; BEGIN IMMEDIATE;
                PRAGMA application_id=1397965640; PRAGMA user_version=1;
                CREATE TABLE schedules(id TEXT PRIMARY KEY, payload TEXT NOT NULL CHECK(length(CAST(payload AS BLOB))<=1048576));
                CREATE TABLE active_run(singleton INTEGER PRIMARY KEY CHECK(singleton=1), payload TEXT NOT NULL CHECK(length(CAST(payload AS BLOB))<=1024));
                CREATE TABLE owned_wake(singleton INTEGER PRIMARY KEY CHECK(singleton=1), payload TEXT NOT NULL CHECK(length(CAST(payload AS BLOB))<=256));
                CREATE TABLE outcomes(schedule_id TEXT PRIMARY KEY, run_id TEXT NOT NULL, state TEXT NOT NULL);
                COMMIT;")?;
            file.sync_all()?;
            #[cfg(unix)]
            File::open(path.parent().context("Missing database parent")?)?.sync_all()?;
        }
        ensure!(
            store
                .connection
                .pragma_query_value(None, "application_id", |r| r.get::<_, i32>(0))?
                == APPLICATION_ID
                && store
                    .connection
                    .pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))?
                    == 1,
            "Unknown or foreign schedule database"
        );
        let pages = store
            .connection
            .pragma_query_value(None, "page_count", |r| r.get::<_, i64>(0))?;
        let page_size = store
            .connection
            .pragma_query_value(None, "page_size", |r| r.get::<_, i64>(0))?;
        ensure!(
            pages >= 0 && page_size == 4096 && pages.saturating_mul(page_size) <= MAX_BYTES as i64,
            "Schedule database exceeds its page bounds"
        );
        ensure!(
            store
                .connection
                .pragma_query_value(None, "journal_mode", |r| r.get::<_, String>(0))?
                == "delete",
            "Schedule database must use its bounded rollback journal"
        );
        if writable {
            store.connection.execute_batch(
                "PRAGMA max_page_count=8192; PRAGMA synchronous=FULL; PRAGMA secure_delete=ON;",
            )?;
        }
        // Inspect bounded rows and all payloads on explicit admission, never on app startup.
        store.list()?;
        store.pending()?;
        Ok(store)
    }
    pub fn cancellation_flag(&self) -> Arc<AtomicBool> {
        self.canceled.clone()
    }
    pub fn cleanup_mode(&self) {
        self.observe_cancel.store(false, Ordering::Relaxed);
    }
    fn deadline(&self) {
        self.deadline.store(
            (self.origin.elapsed().as_millis() as u64).saturating_add(5000),
            Ordering::Relaxed,
        );
    }
    fn check_deadline(&self) -> Result<()> {
        ensure!(
            self.origin.elapsed().as_millis() < u128::from(self.deadline.load(Ordering::Relaxed)),
            "Schedule database operation exceeded its five-second deadline"
        );
        Ok(())
    }
    pub fn list(&self) -> Result<Vec<Schedule>> {
        self.deadline();
        let mut statement = self
            .connection
            .prepare("SELECT id,payload FROM schedules ORDER BY id LIMIT 33")?;
        let mut rows = statement.query([])?;
        let mut result = Vec::new();
        while let Some(row) = rows.next()? {
            ensure!(
                result.len() < MAX_SCHEDULES,
                "Schedule database exceeds 32 schedules"
            );
            let id: String = row.get(0)?;
            let bytes: String = row.get(1)?;
            ensure!(bytes.len() <= 1048576, "Schedule entry exceeds 1 MiB");
            let s: Schedule = serde_json::from_str(&bytes).context("Invalid schedule entry")?;
            ensure!(
                s.id == id
                    && valid_id(&id)
                    && s.seal.len() <= MAX_SEAL
                    && !s.seal.is_empty()
                    && s.next <= 2534023008000
                    && s.profile.is_absolute()
                    && s.profile.as_os_str().as_encoded_bytes().len() <= 4096
                    && s.reason.as_ref().is_none_or(|r| r.len() <= 64),
                "Invalid schedule metadata"
            );
            s.spec.validate()?;
            result.push(s);
        }
        self.check_deadline()?;
        Ok(result)
    }
    pub fn pending(&self) -> Result<Option<Run>> {
        self.deadline();
        let bytes: Option<String> = self
            .connection
            .query_row(
                "SELECT payload FROM active_run WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        bytes
            .map(|b| {
                ensure!(b.len() <= 1024, "Run intent exceeds 1024 bytes");
                let r: Run = serde_json::from_str(&b)?;
                ensure!(
                    uuid::Uuid::parse_str(&r.id).is_ok()
                        && valid_id(&r.schedule)
                        && r.ordinal <= 2534023008000
                        && (0..=super::model::MAX_UTC_MS).contains(&r.due_ms),
                    "Invalid run intent"
                );
                Ok(r)
            })
            .transpose()
    }
    pub fn add(&mut self, s: &Schedule) -> Result<()> {
        ensure!(
            self.list()?.len() < MAX_SCHEDULES,
            "At most 32 schedules are supported"
        );
        ensure!(
            valid_id(&s.id) && s.seal.len() <= MAX_SEAL && !s.seal.is_empty(),
            "Invalid schedule ID or seal"
        );
        s.spec.validate()?;
        self.deadline();
        self.connection.execute(
            "INSERT INTO schedules(id,payload) VALUES(?1,?2)",
            params![s.id, serde_json::to_string(s)?],
        )?;
        Ok(())
    }
    pub fn save(&mut self, s: &Schedule) -> Result<()> {
        self.deadline();
        ensure!(
            self.connection.execute(
                "UPDATE schedules SET payload=?2 WHERE id=?1",
                params![s.id, serde_json::to_string(s)?]
            )? == 1,
            "Schedule no longer exists"
        );
        Ok(())
    }
    pub fn remove(&mut self, id: &str) -> Result<()> {
        ensure!(
            self.pending()?.is_none_or(|r| r.schedule != id),
            "Resolve the uncertain run before removing its schedule"
        );
        self.deadline();
        let tx = self.connection.transaction()?;
        ensure!(
            tx.execute("DELETE FROM schedules WHERE id=?1", [id])? == 1,
            "Unknown schedule ID"
        );
        tx.execute("DELETE FROM outcomes WHERE schedule_id=?1", [id])?;
        tx.commit()?;
        Ok(())
    }
    pub fn begin(&mut self, s: &Schedule, ordinal: u64, due_ms: i64, next: u64) -> Result<Run> {
        ensure!(
            self.pending()?.is_none(),
            "Resolve the uncertain run before executing another task"
        );
        let run = Run {
            id: uuid::Uuid::new_v4().to_string(),
            schedule: s.id.clone(),
            ordinal,
            due_ms,
        };
        let mut updated = s.clone();
        updated.next = next;
        self.deadline();
        let tx = self.connection.transaction()?;
        tx.execute(
            "INSERT INTO active_run(singleton,payload) VALUES(1,?1)",
            [serde_json::to_string(&run)?],
        )?;
        ensure!(
            tx.execute(
                "UPDATE schedules SET payload=?2 WHERE id=?1",
                params![s.id, serde_json::to_string(&updated)?]
            )? == 1,
            "Schedule disappeared before admission"
        );
        tx.commit()?;
        Ok(run)
    }
    pub fn wake(&self) -> Result<Option<Wake>> {
        self.deadline();
        let bytes: Option<String> = self
            .connection
            .query_row(
                "SELECT payload FROM owned_wake WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        bytes
            .map(|b| {
                ensure!(b.len() <= 256, "Wake record is oversized");
                let w: Wake = serde_json::from_str(&b)?;
                ensure!(
                    uuid::Uuid::parse_str(&w.id).is_ok()
                        && (0..=super::model::MAX_UTC_MS).contains(&w.at_ms),
                    "Invalid owned wake record"
                );
                Ok(w)
            })
            .transpose()
    }
    pub fn set_wake(&mut self, wake: Option<&Wake>) -> Result<()> {
        self.deadline();
        if let Some(wake) = wake {
            self.connection.execute(
                "INSERT INTO owned_wake(singleton,payload) VALUES(1,?1)",
                [serde_json::to_string(wake)?],
            )?;
        } else {
            self.connection
                .execute("DELETE FROM owned_wake WHERE singleton=1", [])?;
        }
        Ok(())
    }
    pub fn resolve(&mut self, run_id: &str, state: &str) -> Result<()> {
        ensure!(
            matches!(state, "complete" | "skip"),
            "Run resolution must be complete or skip"
        );
        let run = self.pending()?.context("No uncertain run to resolve")?;
        ensure!(run.id == run_id, "Run ID does not match the uncertain run");
        self.deadline();
        let tx = self.connection.transaction()?;
        tx.execute("INSERT INTO outcomes(schedule_id,run_id,state) VALUES(?1,?2,?3) ON CONFLICT(schedule_id) DO UPDATE SET run_id=excluded.run_id,state=excluded.state",params![run.schedule,run.id,state])?;
        ensure!(
            tx.execute("DELETE FROM active_run WHERE singleton=1", [])? == 1,
            "Run intent disappeared"
        );
        tx.commit()?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> PathBuf {
        let p = std::env::temp_dir().join(format!("screenshots-schedule-{}", uuid::Uuid::new_v4()));
        #[cfg(target_os = "windows")]
        super::super::private_file::directory(&p, true).unwrap();
        #[cfg(not(target_os = "windows"))]
        std::fs::create_dir(&p).unwrap();
        p
    }
    fn schedule() -> Schedule {
        Schedule {
            id: "fixture".into(),
            profile: std::path::absolute("fixture.json").unwrap(),
            seal: vec![1],
            spec: Spec {
                start_ms: 1000,
                interval_ms: Some(100),
                count: Some(3),
                until_ms: None,
                missed: super::super::model::Missed::Once,
                grace_ms: 5,
                wake: false,
            },
            next: 0,
            paused: false,
            reason: None,
        }
    }
    #[test]
    fn crash_after_intent_never_readmits_and_explicit_resolution_consumes_slot() -> Result<()> {
        let root = root();
        let p = root.join("state.sqlite");
        let mut s = Store::open(&p, true, true)?;
        s.add(&schedule())?;
        let run = s.begin(&schedule(), 0, 1000, 1)?;
        drop(s);
        let mut recovered = Store::open(&p, true, false)?;
        assert_eq!(recovered.pending()?, Some(run.clone()));
        assert_eq!(recovered.list()?[0].next, 1);
        assert!(recovered.begin(&recovered.list()?[0], 1, 1100, 2).is_err());
        assert!(recovered.remove("fixture").is_err());
        recovered.resolve(&run.id, "skip")?;
        assert!(recovered.pending()?.is_none());
        assert_eq!(recovered.list()?[0].next, 1);
        drop(recovered);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
    #[test]
    fn cooperative_lock_and_readonly_list() -> Result<()> {
        let root = root();
        let p = root.join("state.sqlite");
        let mut s = Store::open(&p, true, true)?;
        s.add(&schedule())?;
        assert!(Store::open(&p, true, false).is_err());
        assert_eq!(Store::open(&p, false, false)?.list()?.len(), 1);
        drop(s);
        Store::open(&p, true, false)?;
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
    #[cfg(unix)]
    #[test]
    fn owner_teardown_unlocks_even_while_a_fork_equivalent_descriptor_survives() -> Result<()> {
        let root = root();
        let path = root.join("state.sqlite");
        let state = Store::open(&path, true, true)?;
        let inherited = state._lock.as_ref().unwrap().0.try_clone()?;
        assert!(Store::open(&path, true, false).is_err());
        drop(state);
        let next_owner = Store::open(&path, true, false)?;
        drop(inherited);
        assert!(Store::open(&path, true, false).is_err());
        drop(next_owner);
        Store::open(&path, true, false)?;
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
    #[test]
    fn task_success_with_failed_confirmation_preserves_uncertain_intent() -> Result<()> {
        let root = root();
        let p = root.join("state.sqlite");
        let mut state = Store::open(&p, true, true)?;
        state.add(&schedule())?;
        let run = state.begin(&schedule(), 0, 1000, 1)?;
        let effects = std::sync::atomic::AtomicUsize::new(0);
        effects.fetch_add(1, Ordering::SeqCst);
        state.connection.commit_hook(Some(|| true))?;
        assert!(state.resolve(&run.id, "complete").is_err());
        drop(state);
        let mut recovered = Store::open(&p, true, false)?;
        assert_eq!(recovered.pending()?, Some(run.clone()));
        assert!(recovered.begin(&recovered.list()?[0], 1, 1100, 2).is_err());
        assert_eq!(effects.load(Ordering::SeqCst), 1);
        recovered.resolve(&run.id, "complete")?;
        assert_eq!(recovered.list()?[0].next, 1);
        drop(recovered);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
    #[test]
    fn sqlite_progress_observes_deadline_and_cancel_inside_a_native_query() -> Result<()> {
        let root = root();
        let path = root.join("state.sqlite");
        let state = Store::open(&path, true, true)?;
        let expensive = "WITH RECURSIVE series(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM series WHERE n<1000000000) SELECT sum(n) FROM series";
        state.deadline.store(0, Ordering::Relaxed);
        assert!(
            state
                .connection
                .query_row(expensive, [], |r| r.get::<_, i64>(0))
                .is_err()
        );
        state.deadline();
        state.cancellation_flag().store(true, Ordering::Relaxed);
        assert!(
            state
                .connection
                .query_row(expensive, [], |r| r.get::<_, i64>(0))
                .is_err()
        );
        state.cleanup_mode();
        assert!(state.list()?.is_empty());
        drop(state);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
    #[cfg(unix)]
    #[test]
    fn symlink_hardlink_and_foreign_state_are_refused() -> Result<()> {
        let root = root();
        let p = root.join("state.sqlite");
        let state = Store::open(&p, true, true)?;
        drop(state);
        let link = root.join("alias.sqlite");
        std::os::unix::fs::symlink(&p, &link)?;
        assert!(Store::open(&link, false, false).is_err());
        std::fs::remove_file(&link)?;
        std::fs::hard_link(&p, &link)?;
        assert!(Store::open(&p, false, false).is_err());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}
