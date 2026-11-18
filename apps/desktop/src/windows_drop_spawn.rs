//! Reuses the existing authenticated pipes and atomic Windows Job boundary.
use anyhow::{Result, ensure};
use std::{
    ffi::OsString,
    os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle},
    process::Command,
    time::{Duration, Instant},
};

pub(crate) fn install() -> Result<()> {
    app_ui::install_windows_native_drop_spawner(spawn)
}

fn spawn(
    command: &mut Command,
    inherited: &[BorrowedHandle<'_>],
) -> Result<app_ui::WindowsNativeDropProcess> {
    use app_core::windows_child::{self, Environment, Streams};
    let prepared = super::capture_worker::drop_transport(command)?;
    let job = windows_child::job(Some(1), Some(1024 * 1024 * 1024))?;
    let pipe_handles = prepared.child_handles();
    let mut handles = pipe_handles
        .iter()
        .map(|handle| unsafe { BorrowedHandle::borrow_raw(*handle) })
        .collect::<Vec<_>>();
    handles.extend(inherited.iter().map(|handle| handle.as_handle()));
    let mut child = windows_child::spawn(
        command,
        job.as_handle(),
        Streams::NULL,
        Environment::Explicit,
        &handles,
    )?;
    // Clone before resume: failure cannot run an unauthenticated native receiver.
    let process =
        match unsafe { BorrowedHandle::borrow_raw(child.as_raw_handle()) }.try_clone_to_owned() {
            Ok(handle) => handle,
            Err(error) => {
                reap_startup(&mut child, &job);
                return Err(error.into());
            }
        };
    if let Err(error) = child.resume(Instant::now() + Duration::from_secs(5)) {
        reap_startup(&mut child, &job);
        return Err(error);
    }
    let (transport, nonce) = prepared.parent();
    let (read, write) = transport.into_windows_files();
    Ok(app_ui::WindowsNativeDropProcess {
        process,
        job,
        read,
        write,
        nonce,
    })
}

pub(crate) fn try_child(arguments: &[OsString]) -> Result<bool> {
    if arguments
        .first()
        .is_none_or(|value| value != app_ui::WINDOWS_NATIVE_DROP_ARGUMENT)
    {
        return Ok(false);
    }
    ensure!(
        arguments.len() == 3,
        "Invalid private native drop arguments"
    );
    let command: usize = arguments[1]
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid native event"))?
        .parse()?;
    let permit: usize = arguments[2]
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid native event"))?
        .parse()?;
    let (transport, nonce, parent) = super::capture_worker::inherited_drop_transport()?;
    let (read, write) = transport.into_windows_files();
    let _budget = app_core::native_acquisition_budget()?;
    app_ui::run_windows_native_drop_receiver(read, write, nonce, parent, command, permit)
        .map_err(|_| anyhow::anyhow!("Private native drop receiver failed"))?;
    Ok(true)
}

fn reap_startup(
    child: &mut app_core::windows_child::Child,
    job: &std::os::windows::io::OwnedHandle,
) {
    use windows_sys::Win32::{
        Foundation::WAIT_OBJECT_0,
        System::{
            JobObjects::TerminateJobObject,
            Threading::{INFINITE, WaitForSingleObject},
        },
    };
    let _ = child.kill();
    unsafe {
        let _ = TerminateJobObject(job.as_raw_handle(), 70);
    }
    if unsafe { WaitForSingleObject(child.as_raw_handle(), INFINITE) } != WAIT_OBJECT_0 {
        // Keep both owners on this background controller stack; never overlap an
        // unproved cleanup with a replacement helper or release their handles.
        loop {
            std::thread::park();
        }
    }
}
