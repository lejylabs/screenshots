//! Explicit durable scheduling. No application-startup hook or periodic wakeup.
use anyhow::{Context, Result, bail, ensure};
use std::{ffi::OsString, io::Write, path::Path, sync::atomic::Ordering};
#[path = "capture_schedule_model.rs"]
mod model;
#[cfg(target_os = "linux")]
#[path = "capture_schedule_timer_linux.rs"]
mod native;
#[cfg(target_os = "windows")]
#[path = "capture_schedule_timer_windows.rs"]
mod native;
#[cfg(target_os = "macos")]
#[path = "capture_schedule_timer_macos.rs"]
mod native;
#[cfg(target_os = "windows")]
#[path = "capture_schedule_private_windows.rs"]
mod private_file;
#[path = "capture_schedule_store.rs"]
mod store;
use model::{Decision, Missed, Spec};
use store::{Schedule, Store};
const USAGE: &str = "Usage: screenshots schedule add DATABASE ID PROFILE --at RFC3339 [--every-ms N --count N|--forever] [--until RFC3339] [--missed skip|once|pause] [--grace-ms N] [--wake] | list|serve DATABASE | pause|resume|remove DATABASE ID | resolve DATABASE RUN_ID complete|skip";
fn text(value: &OsString) -> Result<&str> {
    value.to_str().context("Schedule arguments must be UTF-8")
}
fn utc(value: &str) -> Result<i64> {
    ensure!(
        !value.ends_with("-00:00"),
        "Schedule UTC offset must be known"
    );
    if let Some((_, fraction)) = value.split_once('.') {
        ensure!(
            fraction.bytes().take_while(u8::is_ascii_digit).count() <= 3,
            "Schedule time supports at most three fractional digits"
        );
    }
    let time = chrono::DateTime::parse_from_rfc3339(value)
        .context("Schedule time must be RFC3339 with an explicit timezone")?;
    ensure!(
        time.timestamp_subsec_nanos() < 1000000000 && time.timestamp_subsec_nanos() % 1000000 == 0,
        "Schedule time supports at most millisecond precision and no leap seconds"
    );
    let ms = time.timestamp_millis();
    ensure!(
        (0..=model::MAX_UTC_MS).contains(&ms),
        "Schedule UTC must be within 1970–9999"
    );
    Ok(ms)
}
fn add(args: &[OsString]) -> Result<(String, std::path::PathBuf, Spec)> {
    ensure!(args.len() >= 5, USAGE);
    let id = text(&args[1])?.to_owned();
    ensure!(
        model::valid_id(&id),
        "Schedule ID must be 1–64 ASCII letters, digits, underscore or hyphen"
    );
    let profile = std::path::absolute(Path::new(&args[2]))?;
    let (
        mut start,
        mut interval,
        mut count,
        mut until,
        mut missed,
        mut grace,
        mut wake,
        mut forever,
    ) = (None, None, None, None, Missed::Pause, 1000, false, false);
    let mut seen = std::collections::HashSet::new();
    let mut index = 3;
    while index < args.len() {
        let key = text(&args[index])?;
        ensure!(seen.insert(key), "Duplicate schedule option");
        index += 1;
        if key == "--wake" {
            wake = true;
            continue;
        }
        if key == "--forever" {
            forever = true;
            continue;
        }
        let value = text(args.get(index).context("Missing schedule option value")?)?;
        index += 1;
        match key {
            "--at" => start = Some(utc(value)?),
            "--every-ms" => {
                interval = Some(value.parse::<u64>().context("Invalid schedule interval")?)
            }
            "--count" => count = Some(value.parse::<u64>().context("Invalid schedule count")?),
            "--until" => until = Some(utc(value)?),
            "--grace-ms" => grace = value.parse::<u64>().context("Invalid schedule grace")?,
            "--missed" => {
                missed = match value {
                    "skip" => Missed::Skip,
                    "once" => Missed::Once,
                    "pause" => Missed::Pause,
                    _ => bail!("Missed policy must be skip, once or pause"),
                }
            }
            _ => bail!("Unknown schedule option"),
        }
    }
    ensure!(
        !(forever && count.is_some()),
        "Choose count or forever, not both"
    );
    if interval.is_some() {
        ensure!(
            count.is_some() || forever,
            "Repeating schedules require explicit --count or --forever"
        );
    } else {
        ensure!(
            !forever && count.is_none_or(|n| n == 1),
            "One occurrence requires count 1 and cannot repeat forever"
        );
        count = Some(1);
    }
    let spec = Spec {
        start_ms: start.context("--at is required")?,
        interval_ms: interval,
        count,
        until_ms: until,
        missed,
        grace_ms: grace,
        wake,
    };
    spec.validate()?;
    Ok((id, profile, spec))
}
fn emit(value: serde_json::Value) -> Result<()> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{value}")?;
    out.flush()?;
    Ok(())
}
fn recover_wake_with(
    store: &mut Store,
    cancel: &impl Fn(&store::Wake) -> Result<()>,
) -> Result<()> {
    if let Some(wake) = store.wake()? {
        cancel(&wake)?;
        store.set_wake(None)?;
    }
    Ok(())
}
fn cancel_owned_wake(wake: &store::Wake) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        native::power(wake, true)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = wake;
        bail!(
            "Recovering this journaled native wake requires macOS; its owned intent was preserved"
        )
    }
}
fn clear_wake(store: &mut Store) -> Result<()> {
    recover_wake_with(store, &cancel_owned_wake)
}
fn edit_with_power<T>(
    path: &Path,
    create: bool,
    cancel: &impl Fn(&store::Wake) -> Result<()>,
    edit: impl FnOnce(&mut Store) -> Result<T>,
) -> Result<T> {
    let mut store = Store::open(path, true, create)?;
    recover_wake_with(&mut store, cancel)?;
    edit(&mut store)
}
fn edit_store<T>(
    path: &Path,
    create: bool,
    edit: impl FnOnce(&mut Store) -> Result<T>,
) -> Result<T> {
    edit_with_power(path, create, &cancel_owned_wake, edit)
}
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
fn arm(
    native: &mut native::Native,
    store: &mut Store,
    schedules: &[Schedule],
    now: i64,
) -> Result<()> {
    let next = schedules
        .iter()
        .filter(|s| !s.paused)
        .filter_map(|s| s.spec.due(s.next))
        .min();
    let wake = schedules
        .iter()
        .filter(|s| !s.paused && s.spec.wake)
        .filter_map(|s| s.spec.due(s.next))
        .filter(|due| *due > now)
        .min();
    #[cfg(target_os = "macos")]
    {
        if store.wake()?.as_ref().map(|w| w.at_ms) != wake {
            clear_wake(store)?;
            if let Some(at_ms) = wake {
                let record = store::Wake {
                    id: uuid::Uuid::new_v4().to_string(),
                    at_ms,
                };
                ensure!(
                    unsafe { libc::geteuid() } == 0,
                    "Explicit macOS wake requires native root privilege; no privilege prompt was requested"
                );
                store.set_wake(Some(&record))?;
                native::power(&record, false)?;
            }
        }
        native.arm(next, None)?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = store;
        native.arm(next, wake)?;
    }
    Ok(())
}
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
fn serve(path: &Path) -> Result<()> {
    let mut store = Store::open(path, true, false)?;
    clear_wake(&mut store)?;
    if let Some(run) = store.pending()? {
        bail!(
            "Uncertain scheduled run {}; inspect its outputs/remote effects and explicitly resolve complete or skip before serving; it was not retried",
            run.id
        );
    }
    let stop = store.cancellation_flag();
    let mut native = native::Native::new(stop.clone())?;
    let result = (|| -> Result<()> {
        let mut schedules = store.list()?;
        arm(
            &mut native,
            &mut store,
            &schedules,
            chrono::Utc::now().timestamp_millis(),
        )?;
        emit(
            serde_json::json!({"type":"schedule_ready","schedules":schedules.len(),"pid":std::process::id()}),
        )?;
        while !stop.load(Ordering::Relaxed) {
            let now = chrono::Utc::now().timestamp_millis();
            let mut ready = Vec::with_capacity(model::MAX_SCHEDULES);
            for (index, s) in schedules.iter_mut().enumerate() {
                if s.paused {
                    continue;
                }
                match s.spec.decide(s.next, now) {
                    Decision::Pause => {
                        s.paused = true;
                        s.reason = Some("missed_run".into());
                        store.save(s)?;
                    }
                    Decision::Advance(next) => {
                        s.next = next;
                        store.save(s)?;
                    }
                    Decision::Run {
                        ordinal,
                        due_ms,
                        next,
                    } => ready.push((due_ms, s.id.clone(), index, ordinal, next)),
                    Decision::Complete | Decision::Wait(_) => {}
                }
            }
            arm(&mut native, &mut store, &schedules, now)?;
            if stop.load(Ordering::Relaxed) {
                break;
            }
            ready.sort_unstable();
            let Some((due_ms, _, index, ordinal, next)) = ready.into_iter().next() else {
                native.wait()?;
                continue;
            };
            // Exact immutable prepared settings are compared before the durable intent or any effect.
            let prepared = match crate::task::PreparedScheduledTask::read(
                &schedules[index].profile,
                &schedules[index].seal,
            ) {
                Ok(task) => task,
                Err(_) => {
                    schedules[index].paused = true;
                    schedules[index].reason = Some("task_changed_or_unavailable".into());
                    store.save(&schedules[index])?;
                    continue;
                }
            };
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let run = store.begin(&schedules[index], ordinal, due_ms, next)?;
            schedules[index].next = next;
            // Reserve the following explicit wake before a long Task, with no queued capture.
            arm(
                &mut native,
                &mut store,
                &schedules,
                chrono::Utc::now().timestamp_millis(),
            )?;
            if prepared.execute(stop.clone()).is_err() {
                bail!(
                    "Scheduled run {} has an uncertain outcome; earlier outputs/uploads may exist; no retry; inspect and explicitly resolve complete or skip",
                    run.id
                );
            }
            store.resolve(&run.id,"complete").context("Scheduled task finished but durable confirmation failed; its run intent must be inspected, never automatically retried")?;
        }
        Ok(())
    })();
    store.cleanup_mode();
    let cleanup = clear_wake(&mut store);
    result?;
    cleanup?;
    emit(serde_json::json!({"type":"schedule_stopped"}))
}
pub(crate) fn run(args: &[OsString]) -> Result<()> {
    let command = text(args.first().context(USAGE)?)?;
    let path = Path::new(args.get(1).context(USAGE)?);
    match command {
        "add" => {
            let (id, profile, spec) = add(&args[1..])?;
            let seal = crate::task::TaskSeal::read(&profile)?.into_bytes()?;
            edit_store(path, true, |store| {
                store.add(&Schedule {
                    id: id.clone(),
                    profile,
                    seal,
                    spec,
                    next: 0,
                    paused: false,
                    reason: None,
                })
            })?;
            emit(serde_json::json!({"id":id,"state":"scheduled"}))
        }
        "list" => {
            ensure!(args.len() == 2, USAGE);
            let store = Store::open(path, false, false)?;
            let pending = store.pending()?;
            let schedules=store.list()?.into_iter().map(|s|serde_json::json!({"id":s.id,"next_ordinal":s.next,"next_utc_ms":s.spec.due(s.next),"paused":s.paused,"reason":s.reason,"wake":s.spec.wake,"missed":s.spec.missed,"state":if pending.as_ref().is_some_and(|r|r.schedule==s.id){"uncertain"}else if s.paused{"paused"}else if s.spec.due(s.next).is_none(){"complete"}else{"scheduled"}})).collect::<Vec<_>>();
            emit(serde_json::json!({"version":1,"schedules":schedules,"uncertain_run":pending}))
        }
        "pause" | "resume" | "remove" => {
            ensure!(args.len() == 3, USAGE);
            let id = text(&args[2])?;
            ensure!(model::valid_id(id), "Invalid schedule ID");
            edit_store(path, false, |store| {
                if command == "remove" {
                    store.remove(id)?;
                } else {
                    let mut s = store
                        .list()?
                        .into_iter()
                        .find(|s| s.id == id)
                        .context("Unknown schedule ID")?;
                    s.paused = command == "pause";
                    if !s.paused {
                        s.next = s
                            .spec
                            .resume_after(s.next, chrono::Utc::now().timestamp_millis());
                    }
                    s.reason = if s.paused {
                        Some("operator".into())
                    } else {
                        None
                    };
                    store.save(&s)?;
                }
                Ok(())
            })?;
            emit(serde_json::json!({"id":id,"state":command}))
        }
        "resolve" => {
            ensure!(args.len() == 4, USAGE);
            edit_store(path, false, |store| {
                store.resolve(text(&args[2])?, text(&args[3])?)
            })?;
            emit(serde_json::json!({"state":"resolved"}))
        }
        "serve" => {
            ensure!(args.len() == 2, USAGE);
            #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
            {
                serve(path)
            }
            #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
            {
                bail!("Native scheduling is unavailable on this platform")
            }
        }
        _ => bail!(USAGE),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn wake_fixture() -> Result<(std::path::PathBuf, store::Wake)> {
        let root = std::env::temp_dir().join(format!("screenshots-wake-{}", uuid::Uuid::new_v4()));
        #[cfg(target_os = "windows")]
        private_file::directory(&root, true)?;
        #[cfg(not(target_os = "windows"))]
        std::fs::create_dir(&root)?;
        let path = root.join("schedule.sqlite");
        let mut state = Store::open(&path, true, true)?;
        state.add(&Schedule {
            id: "fixture".into(),
            profile: root.join("task.json"),
            seal: vec![1],
            spec: Spec {
                start_ms: 1000,
                interval_ms: None,
                count: Some(1),
                until_ms: None,
                missed: Missed::Pause,
                grace_ms: 0,
                wake: true,
            },
            next: 0,
            paused: false,
            reason: None,
        })?;
        let wake = store::Wake {
            id: uuid::Uuid::new_v4().to_string(),
            at_ms: 1000,
        };
        state.set_wake(Some(&wake))?;
        drop(state);
        Ok((path, wake))
    }
    #[test]
    fn offline_pause_remove_reconcile_exact_owned_wake_after_crash_before_or_after_publication()
    -> Result<()> {
        for published in [false, true] {
            for remove in [false, true] {
                let (path, wake) = wake_fixture()?;
                let armed = std::cell::Cell::new(published);
                let calls = std::cell::Cell::new(0);
                edit_with_power(
                    &path,
                    false,
                    &|record| {
                        assert_eq!(record.id, wake.id);
                        assert_eq!(record.at_ms, wake.at_ms);
                        calls.set(calls.get() + 1);
                        armed.set(false);
                        Ok(())
                    },
                    |state| {
                        if remove {
                            state.remove("fixture")
                        } else {
                            let mut s = state.list()?.remove(0);
                            s.paused = true;
                            s.reason = Some("operator".into());
                            state.save(&s)
                        }
                    },
                )?;
                assert!(!armed.get());
                assert_eq!(calls.get(), 1);
                let state = Store::open(&path, false, false)?;
                assert!(state.wake()?.is_none());
                assert_eq!(state.list()?.is_empty(), remove);
                if !remove {
                    assert!(state.list()?[0].paused);
                }
                drop(state);
                std::fs::remove_dir_all(path.parent().unwrap())?;
            }
        }
        Ok(())
    }
    #[test]
    fn offline_mutation_never_reports_success_when_cancel_or_durable_clear_fails() -> Result<()> {
        let (path, wake) = wake_fixture()?;
        let changed = std::cell::Cell::new(false);
        let cancel = |_: &store::Wake| -> Result<()> { bail!("Injected native cancel failure") };
        assert!(
            edit_with_power(&path, false, &cancel, |_| {
                changed.set(true);
                Ok(())
            })
            .is_err()
        );
        assert!(!changed.get());
        let state = Store::open(&path, false, false)?;
        assert_eq!(state.wake()?.unwrap().id, wake.id);
        assert!(!state.list()?[0].paused);
        drop(state);
        let db = rusqlite::Connection::open(&path)?;
        db.execute_batch("CREATE TRIGGER fixture_clear_failure BEFORE DELETE ON owned_wake BEGIN SELECT RAISE(ABORT,'injected wake clear failure'); END;")?;
        drop(db);
        assert!(
            edit_with_power(&path, false, &|_| Ok(()), |_| {
                changed.set(true);
                Ok(())
            })
            .is_err()
        );
        assert!(!changed.get());
        let state = Store::open(&path, false, false)?;
        assert_eq!(state.wake()?.unwrap().id, wake.id);
        drop(state);
        let db = rusqlite::Connection::open(&path)?;
        db.execute_batch("DROP TRIGGER fixture_clear_failure;")?;
        drop(db);
        edit_with_power(&path, false, &|_| Ok(()), |state| state.remove("fixture"))?;
        let state = Store::open(&path, false, false)?;
        assert!(state.wake()?.is_none());
        drop(state);
        std::fs::remove_dir_all(path.parent().unwrap())?;
        Ok(())
    }
    #[test]
    fn precise_utc_and_explicit_repeat_admission() {
        assert_eq!(utc("1970-01-01T01:00:00.123+01:00").unwrap(), 123);
        for value in [
            "2030-01-01",
            "2030-01-01T00:00:00-00:00",
            "2030-01-01T00:00:00.0001Z",
            "1969-12-31T23:59:59Z",
        ] {
            assert!(utc(value).is_err(), "{value}");
        }
        let args = |v: &[&str]| v.iter().map(OsString::from).collect::<Vec<_>>();
        assert!(
            add(&args(&[
                "db",
                "id",
                "p",
                "--at",
                "2030-01-01T00:00:00Z",
                "--every-ms",
                "100"
            ]))
            .is_err()
        );
        assert!(
            add(&args(&[
                "db",
                "id",
                "p",
                "--at",
                "2030-01-01T00:00:00Z",
                "--every-ms",
                "100",
                "--forever"
            ]))
            .is_ok()
        );
        assert!(
            add(&args(&[
                "db",
                "id",
                "p",
                "--at",
                "2030-01-01T00:00:00Z",
                "--wake",
                "--wake"
            ]))
            .is_err()
        );
    }
}
