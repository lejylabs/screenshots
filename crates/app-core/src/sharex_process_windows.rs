use super::{Job, RuntimeBundle, command};
use anyhow::{Context, Result, ensure};
use std::{
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    time::Instant,
};
use windows_sys::Win32::{
    Foundation::{ERROR_NO_MORE_FILES, GetLastError, INVALID_HANDLE_VALUE},
    System::{Diagnostics::ToolHelp::*, JobObjects::*, Threading::*},
};
pub(super) fn spawn(bundle: &RuntimeBundle, deadline: Instant) -> Result<Job> {
    let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    ensure!(!raw.is_null(), "Could not create SXCU interpreter job");
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        | JOB_OBJECT_LIMIT_PROCESS_MEMORY
        | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
    limits.BasicLimitInformation.ActiveProcessLimit = 1;
    limits.ProcessMemoryLimit = 256 * 1024 * 1024;
    ensure!(
        unsafe {
            SetInformationJobObject(
                handle.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        } != 0,
        "Could not bound SXCU interpreter job"
    );
    let child = command(bundle)
        .creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW)
        .spawn()
        .context("Could not create suspended SXCU interpreter")?;
    let job = Job {
        child,
        stopped: false,
        handle: Some(handle),
    };
    ensure!(
        unsafe {
            AssignProcessToJobObject(
                job.handle.as_ref().unwrap().as_raw_handle(),
                job.child.as_raw_handle(),
            )
        } != 0,
        "Could not isolate SXCU interpreter before startup"
    );
    resume(job.child.id(), deadline)?;
    ensure!(
        Instant::now() < deadline,
        "SXCU interpreter startup exceeded five seconds"
    );
    Ok(job)
}
fn resume(process: u32, deadline: Instant) -> Result<()> {
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    ensure!(
        raw != INVALID_HANDLE_VALUE,
        "Could not inspect suspended SXCU interpreter"
    );
    let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut present = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) } != 0;
    let mut target = None;
    let mut count = 0;
    while present {
        count += 1;
        ensure!(
            count <= 65536 && Instant::now() < deadline,
            "SXCU thread inspection exceeded its bound"
        );
        if entry.th32OwnerProcessID == process {
            ensure!(target.is_none(), "Ambiguous SXCU initial thread");
            target = Some(entry.th32ThreadID);
        }
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        present = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) } != 0;
    }
    ensure!(
        unsafe { GetLastError() } == ERROR_NO_MORE_FILES,
        "Could not complete SXCU thread inspection"
    );
    let id = target.context("SXCU initial thread is missing")?;
    let raw = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, id) };
    ensure!(!raw.is_null(), "Could not open SXCU initial thread");
    let thread = unsafe { OwnedHandle::from_raw_handle(raw) };
    ensure!(
        unsafe { ResumeThread(thread.as_raw_handle()) } == 1,
        "Could not resume isolated SXCU interpreter"
    );
    Ok(())
}
