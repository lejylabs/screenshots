//! User/session-scoped Windows launch forwarding. No message payload or idle polling.
use anyhow::{Context, Result, ensure};
use std::{sync::Arc, thread::JoinHandle};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::ConvertSidToStringSidW, *},
    System::Threading::*,
};

struct Handle(isize);
impl Handle {
    fn new(raw: HANDLE) -> Result<Self> {
        ensure!(
            !raw.is_null(),
            "Windows instance handle: {}",
            std::io::Error::last_os_error()
        );
        Ok(Self(raw as isize))
    }
    fn raw(&self) -> HANDLE {
        self.0 as HANDLE
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.raw());
        }
    }
}

pub struct SingleInstance {
    _ownership: Handle,
    show: Arc<Handle>,
    stop: Arc<Handle>,
    listener: Option<JoinHandle<()>>,
}

impl SingleInstance {
    /// None means the existing instance was signaled; no second app should start.
    pub fn acquire() -> Result<Option<Self>> {
        Self::acquire_named(&format!(
            "Local\\lejylabs.screenshots.{}",
            current_user_sid()?
        ))
    }

    fn acquire_named(name: &str) -> Result<Option<Self>> {
        let wide = |suffix: &str| {
            format!("{name}.{suffix}")
                .encode_utf16()
                .chain(Some(0))
                .collect::<Vec<_>>()
        };
        // Create the event first so simultaneous launches cannot lose a startup request.
        let show = Arc::new(Handle::new(unsafe {
            CreateEventW(std::ptr::null(), 0, 0, wide("show").as_ptr())
        })?);
        let ownership_raw = unsafe { CreateMutexW(std::ptr::null(), 0, wide("owner").as_ptr()) };
        let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let ownership = Handle::new(ownership_raw)?;
        if existed {
            ensure!(
                unsafe { SetEvent(show.raw()) } != 0,
                "Could not notify the running app: {}",
                std::io::Error::last_os_error()
            );
            return Ok(None);
        }
        let stop = Arc::new(Handle::new(unsafe {
            CreateEventW(std::ptr::null(), 1, 0, std::ptr::null())
        })?);
        Ok(Some(Self {
            _ownership: ownership,
            show,
            stop,
            listener: None,
        }))
    }

    /// Start only after the application event receiver exists. Callback must not block.
    pub fn listen(&mut self, notify: impl Fn() + Send + 'static) -> Result<()> {
        ensure!(
            self.listener.is_none(),
            "Instance listener is already running"
        );
        let show = self.show.clone();
        let stop = self.stop.clone();
        self.listener = Some(std::thread::Builder::new().name("launch-events".into()).stack_size(256 * 1024).spawn(move || {
            let handles = [stop.raw(), show.raw()];
            loop {
                match unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) } {
                    WAIT_OBJECT_0 => break,
                    value if value == WAIT_OBJECT_0 + 1 => notify(),
                    _ => { tracing::error!(error = %std::io::Error::last_os_error(), "Instance event wait failed"); break; }
                }
            }
        })?);
        Ok(())
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        if let Some(listener) = self.listener.take() {
            unsafe {
                SetEvent(self.stop.raw());
            }
            let _ = listener.join();
        }
    }
}

fn current_user_sid() -> Result<String> {
    let mut token = std::ptr::null_mut();
    ensure!(
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } != 0,
        "Could not identify the current user"
    );
    let token = Handle::new(token)?;
    let mut size = 0;
    unsafe {
        GetTokenInformation(token.raw(), TokenUser, std::ptr::null_mut(), 0, &mut size);
    }
    ensure!(size > 0 && size < 65536, "Invalid Windows user token size");
    let mut buffer = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
    ensure!(
        unsafe {
            GetTokenInformation(
                token.raw(),
                TokenUser,
                buffer.as_mut_ptr().cast(),
                size,
                &mut size,
            )
        } != 0,
        "Could not read the current user token"
    );
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let mut text = std::ptr::null_mut();
    ensure!(
        unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) } != 0,
        "Could not read the current user SID"
    );
    let result = (|| {
        let mut length = 0;
        while length < 256 && unsafe { *text.add(length) } != 0 {
            length += 1;
        }
        ensure!(length < 256, "Invalid user SID");
        String::from_utf16(unsafe { std::slice::from_raw_parts(text, length) })
            .context("Invalid user SID encoding")
    })();
    unsafe {
        LocalFree(text.cast());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn second_launch_forwards_even_before_listener_and_restart_releases_owner() -> Result<()> {
        assert!(current_user_sid()?.starts_with("S-1-"));
        let name = format!("Local\\screenshots-test-{}", uuid::Uuid::new_v4());
        let mut first = SingleInstance::acquire_named(&name)?.unwrap();
        assert!(SingleInstance::acquire_named(&name)?.is_none());
        let (sender, receiver) = std::sync::mpsc::channel();
        first.listen(move || {
            let _ = sender.send(());
        })?;
        receiver.recv_timeout(std::time::Duration::from_secs(2))?;
        drop(first);
        assert!(SingleInstance::acquire_named(&name)?.is_some());
        Ok(())
    }
}
