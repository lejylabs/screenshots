//! Kernel wakeups only. No timed or periodic idle wake.
use anyhow::{Result, ensure};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows::Win32::{Foundation::HANDLE, Security::SECURITY_ATTRIBUTES, System::Threading::*};
pub(super) fn raw(handle: &impl AsRawHandle) -> HANDLE {
    HANDLE(handle.as_raw_handle())
}
pub(super) fn event(inherit: bool) -> Result<OwnedHandle> {
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: inherit.into(),
    };
    let handle = unsafe { CreateEventW(Some(&attributes), false, false, None) }?;
    Ok(unsafe { OwnedHandle::from_raw_handle(handle.0) })
}
pub(super) fn signal(event: &impl AsRawHandle) -> Result<()> {
    unsafe { SetEvent(raw(event)) }?;
    Ok(())
}
pub(super) fn wait(handles: &[HANDLE], milliseconds: u32) -> Result<u32> {
    let value = unsafe { WaitForMultipleObjects(handles, false, milliseconds) }.0;
    ensure!(
        value < handles.len() as u32 || value == 258,
        "Native drop wait failed"
    );
    Ok(value)
}
