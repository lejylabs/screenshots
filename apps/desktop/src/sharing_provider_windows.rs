//! A private nonblocking configuration pipe and existing Windows Job isolation.

use super::sharing_provider::{self as provider, Process};
use anyhow::{Context, Result, ensure};
use std::{
    fs::File,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    process::{Child, Stdio},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{
        ERROR_NO_DATA, ERROR_PIPE_CONNECTED, ERROR_PIPE_LISTENING, INVALID_HANDLE_VALUE, LocalFree,
    },
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            SDDL_REVISION_1,
        },
        GetTokenInformation, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
    },
    Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_OUTBOUND, WriteFile},
    System::{
        JobObjects::AssignProcessToJobObject,
        Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, PIPE_NOWAIT,
            PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
        },
        Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED, GetCurrentProcess, OpenProcessToken},
    },
};
use zeroize::{Zeroize, Zeroizing};

fn descriptor() -> Result<*mut std::ffi::c_void> {
    let mut token = std::ptr::null_mut();
    ensure!(
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } != 0,
        "Could not identify provider user"
    );
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut size = 0;
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            std::ptr::null_mut(),
            0,
            &mut size,
        );
    }
    ensure!(
        size >= std::mem::size_of::<TOKEN_USER>() as u32 && size <= 65536,
        "Invalid provider user token"
    );
    let mut buffer = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
    ensure!(
        unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                buffer.as_mut_ptr().cast(),
                size,
                &mut size,
            )
        } != 0,
        "Could not read provider user token"
    );
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let mut text = std::ptr::null_mut();
    ensure!(
        unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) } != 0,
        "Could not read provider user SID"
    );
    let result = (|| -> Result<String> {
        let mut length = 0;
        while length < 256 && unsafe { *text.add(length) } != 0 {
            length += 1;
        }
        ensure!(length < 256, "Invalid provider user SID");
        Ok(String::from_utf16(unsafe {
            std::slice::from_raw_parts(text, length)
        })?)
    })();
    unsafe {
        LocalFree(text.cast());
    }
    let sddl: Vec<u16> = format!("D:P(A;;FA;;;{})", result?)
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut descriptor = std::ptr::null_mut();
    ensure!(
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        } != 0,
        "Could not prepare private provider permissions"
    );
    Ok(descriptor)
}

struct ConfigPipe {
    handle: Option<OwnedHandle>,
    name: std::ffi::OsString,
    connected: bool,
}
impl ConfigPipe {
    fn new() -> Result<Self> {
        let name = std::ffi::OsString::from(format!(
            r"\\.\pipe\screenshots-provider-{}",
            uuid::Uuid::new_v4()
        ));
        let wide: Vec<u16> = name.encode_wide().chain(Some(0)).collect();
        let descriptor = descriptor()?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let handle = unsafe {
            CreateNamedPipeW(
                wide.as_ptr(),
                PIPE_ACCESS_OUTBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                8192,
                8192,
                0,
                &attributes,
            )
        };
        unsafe {
            LocalFree(descriptor);
        }
        ensure!(
            handle != INVALID_HANDLE_VALUE,
            "Could not create private provider configuration pipe"
        );
        Ok(Self {
            handle: Some(unsafe { OwnedHandle::from_raw_handle(handle) }),
            name,
            connected: false,
        })
    }
    fn send(&mut self, child: u32, bytes: &[u8]) -> Result<usize> {
        let handle = self
            .handle
            .as_ref()
            .context("Provider configuration pipe is closed")?
            .as_raw_handle();
        if !self.connected {
            if unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) } == 0 {
                match std::io::Error::last_os_error()
                    .raw_os_error()
                    .map(|value| value as u32)
                {
                    Some(ERROR_PIPE_LISTENING) => return Ok(0),
                    Some(ERROR_PIPE_CONNECTED) => {}
                    _ => anyhow::bail!("Could not connect private provider configuration pipe"),
                }
            }
            let mut pid = 0;
            ensure!(
                unsafe { GetNamedPipeClientProcessId(handle, &mut pid) } != 0 && pid == child,
                "Provider configuration client identity mismatch"
            );
            self.connected = true;
        }
        let mut written = 0;
        if unsafe {
            WriteFile(
                handle,
                bytes.as_ptr(),
                bytes.len() as u32,
                &mut written,
                std::ptr::null_mut(),
            )
        } == 0
        {
            if std::io::Error::last_os_error().raw_os_error() == Some(ERROR_NO_DATA as i32) {
                return Ok(0);
            }
            anyhow::bail!("Could not deliver provider configuration");
        }
        Ok(written as usize)
    }
}

struct Curl {
    child: Child,
    job: Option<OwnedHandle>,
    pipe: ConfigPipe,
    config: Zeroizing<Vec<u8>>,
    sent: usize,
}
impl Process for Curl {
    fn child(&mut self) -> &mut Child {
        &mut self.child
    }
    fn config(&mut self) -> Result<bool> {
        if self.pipe.handle.is_none() {
            return Ok(true);
        }
        self.sent += self.pipe.send(self.child.id(), &self.config[self.sent..])?;
        if self.sent == self.config.len() {
            self.pipe.handle.take();
            self.config.zeroize();
            Ok(true)
        } else {
            Ok(false)
        }
    }
    fn stop(&mut self) {
        self.job.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.pipe.handle.take();
    }
}
impl Drop for Curl {
    fn drop(&mut self) {
        self.stop();
    }
}

pub(super) fn transfer(
    configuration: Zeroizing<Vec<u8>>,
    source: File,
    length: u64,
    check: &impl Fn() -> Result<()>,
    native_sftp: bool,
) -> Result<Vec<u8>> {
    check()?;
    ensure!(
        configuration.len() <= 8192,
        "Provider configuration exceeds 8 KiB"
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    let pipe = ConfigPipe::new()?;
    let job = crate::ocr_windows::job_object().context("Could not isolate provider engine")?;
    let mut command = if native_sftp {
        let mut command = std::process::Command::new(
            std::env::current_exe().context("Could not locate private provider engine")?,
        );
        command.arg("provider-engine");
        provider::scrub_environment(&mut command);
        command
    } else {
        provider::curl()
    };
    // Defense in depth; the native helper also forbids AGENT in SSH_AUTH_TYPES.
    command.env(
        "SSH_AUTH_SOCK",
        format!(
            r"\\.\pipe\screenshots-provider-no-agent-{}",
            uuid::Uuid::new_v4()
        ),
    );
    let child = command
        .arg("--config")
        .arg(&pipe.name)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED)
        .spawn()
        .map_err(|_| {
            anyhow::anyhow!(
                "Provider engine is unavailable; install curl with the required protocol separately"
            )
        })?;
    let process = Curl {
        child,
        job: Some(job),
        pipe,
        config: configuration,
        sent: 0,
    };
    ensure!(
        unsafe {
            AssignProcessToJobObject(
                process.job.as_ref().unwrap().as_raw_handle(),
                process.child.as_raw_handle(),
            )
        } != 0,
        "Could not isolate provider engine before starting it"
    );
    crate::ocr_windows::resume_initial_thread(process.child.id(), deadline)
        .context("Could not start isolated provider engine")?;
    provider::execute(process, source, length, deadline, check)
        .context("Provider upload failed; a remote object may remain and no retry was attempted")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn private_nonblocking_config_pipe_preserves_all_bytes_and_eof() {
        let mut pipe = ConfigPipe::new().unwrap();
        let name = pipe.name.clone();
        let expected = b"synthetic-config\nuser = \"synthetic:NOT_A_REAL_SECRET\"\n";
        let reader = std::thread::spawn(move || {
            let mut file = File::open(name).unwrap();
            let mut bytes = Vec::new();
            file.take(8193).read_to_end(&mut bytes).unwrap();
            bytes
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut sent = 0;
        while sent < expected.len() {
            if Instant::now() >= deadline {
                pipe.handle.take();
                panic!("Configuration pipe did not connect");
            }
            sent += pipe.send(std::process::id(), &expected[sent..]).unwrap();
            std::thread::sleep(provider::INTERVAL);
        }
        pipe.handle.take();
        assert_eq!(reader.join().unwrap(), expected);
    }

    #[test]
    fn config_pipe_refuses_a_different_client_process_identity() {
        let mut pipe = ConfigPipe::new().unwrap();
        let name = pipe.name.clone();
        let reader = std::thread::spawn(move || {
            let mut file = File::open(name).unwrap();
            let mut bytes = Vec::new();
            file.take(8193).read_to_end(&mut bytes).unwrap();
            bytes
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if Instant::now() >= deadline {
                pipe.handle.take();
                panic!("Configuration pipe did not connect");
            }
            match pipe.send(std::process::id().wrapping_add(1), b"must not be released") {
                Ok(0) => std::thread::sleep(provider::INTERVAL),
                Ok(_) => panic!("Config escaped identity admission"),
                Err(error) => {
                    assert!(error.to_string().contains("identity"));
                    break;
                }
            }
        }
        pipe.handle.take();
        assert!(reader.join().unwrap().is_empty());
    }
}
