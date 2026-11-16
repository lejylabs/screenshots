use super::{Job, RuntimeBundle, command};
use crate::windows_child::{self, Environment, Streams};
use anyhow::{Result, ensure};
use std::{os::windows::io::AsHandle, time::Instant};

pub(super) fn spawn(bundle: &RuntimeBundle, deadline: Instant) -> Result<Job> {
    ensure!(
        Instant::now() < deadline,
        "SXCU interpreter startup exceeded five seconds"
    );
    let handle = windows_child::job(Some(1), Some(256 * 1024 * 1024))?;
    let child = windows_child::spawn(
        &command(bundle),
        handle.as_handle(),
        Streams::PIPED,
        Environment::Inherit,
        &[],
    )?;
    let job = Job {
        child,
        stopped: false,
        handle: Some(handle),
    };
    Ok(job)
}
