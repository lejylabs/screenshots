//! Native capture uses the common atomic job/handle-list creation boundary.
use anyhow::{Result, ensure};
use app_core::windows_child::{self, Child, Environment, Streams};
use std::{
    mem::size_of,
    os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, OwnedHandle},
    process::Command,
    time::{Duration, Instant},
};
use windows_sys::Win32::{Foundation::HANDLE, System::JobObjects::*};

pub(super) struct Process {
    child: Child,
}
pub(super) struct Container(pub OwnedHandle);
pub(super) fn container() -> Result<Container> {
    Ok(Container(windows_child::job(
        None,
        Some(1024 * 1024 * 1024),
    )?))
}
#[cfg(test)]
#[test]
fn native_worker_job_has_an_explicit_commit_ceiling() -> Result<()> {
    let container = container()?;
    let mut information = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    ensure!(
        unsafe {
            QueryInformationJobObject(
                container.0.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                std::ptr::from_mut(&mut information).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        } != 0
    );
    assert_ne!(
        information.BasicLimitInformation.LimitFlags & JOB_OBJECT_LIMIT_PROCESS_MEMORY,
        0
    );
    assert_eq!(information.ProcessMemoryLimit, 1024 * 1024 * 1024);
    Ok(())
}
pub(super) fn spawn(
    command: &Command,
    container: &Container,
    inherited: &[HANDLE],
) -> Result<Process> {
    // These handles remain owned by the prepared transport throughout creation.
    let inherited = inherited
        .iter()
        .map(|handle| unsafe { BorrowedHandle::borrow_raw(*handle) })
        .collect::<Vec<_>>();
    let mut child = windows_child::spawn(
        command,
        container.0.as_handle(),
        Streams::NULL,
        Environment::Explicit,
        &inherited,
    )?;
    child.resume(Instant::now() + Duration::from_secs(5))?;
    Ok(Process { child })
}
impl Process {
    pub fn wait_ready(&self, duration: Duration) {
        self.child.wait_ready(duration);
    }
    pub fn try_wait(&mut self) -> Result<Option<bool>> {
        Ok(self.child.try_wait()?.map(|status| status.success()))
    }
    pub fn wait(&mut self) -> Result<bool> {
        Ok(self.child.wait()?.success())
    }
    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }
}
impl Container {
    pub fn stop(&self) {
        unsafe {
            TerminateJobObject(self.0.as_raw_handle(), 1);
        }
    }
    pub fn empty(&self) -> Result<bool> {
        let mut information = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        ensure!(
            unsafe {
                QueryInformationJobObject(
                    self.0.as_raw_handle(),
                    JobObjectBasicAccountingInformation,
                    std::ptr::from_mut(&mut information).cast(),
                    size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                    std::ptr::null_mut(),
                )
            } != 0,
            "Native job status unavailable"
        );
        Ok(information.ActiveProcesses == 0)
    }
}
