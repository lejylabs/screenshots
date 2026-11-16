//! Atomic job/handle-list process creation: no unassigned suspended-child gap.
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    mem::{size_of, size_of_val},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    process::Command,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::SECURITY_ATTRIBUTES,
    Storage::FileSystem::*,
    System::{JobObjects::*, Threading::*},
    UI::WindowsAndMessaging::SW_HIDE,
};

pub(super) struct Process {
    handle: OwnedHandle,
    pub pid: u32,
}
pub(super) struct Container(pub OwnedHandle);
pub(super) fn container() -> Result<Container> {
    Ok(Container(crate::ocr_windows::job_object()?))
}
fn wide(value: &OsStr) -> Result<Vec<u16>> {
    let mut result = value.encode_wide().collect::<Vec<_>>();
    ensure!(!result.contains(&0), "Invalid native process string");
    result.push(0);
    Ok(result)
}
fn quote(value: &OsStr) -> Result<Vec<u16>> {
    let value = wide(value)?;
    let mut result = vec![b'"' as u16];
    let mut slash = 0;
    for value in value.into_iter().take_while(|value| *value != 0) {
        if value == b'\\' as u16 {
            slash += 1;
            continue;
        }
        if value == b'"' as u16 {
            result.extend(std::iter::repeat_n(b'\\' as u16, slash * 2 + 1));
        } else {
            result.extend(std::iter::repeat_n(b'\\' as u16, slash));
        }
        slash = 0;
        result.push(value);
    }
    result.extend(std::iter::repeat_n(b'\\' as u16, slash * 2));
    result.push(b'"' as u16);
    Ok(result)
}
fn environment(command: &Command) -> Result<Vec<u16>> {
    // The controller explicitly supplies its sanitized environment.
    let mut values: BTreeMap<String, (OsString, OsString)> = BTreeMap::new();
    for (key, value) in command.get_envs() {
        let normalized = key.to_string_lossy().to_uppercase();
        if let Some(value) = value {
            values.insert(normalized, (key.to_owned(), value.to_owned()));
        } else {
            values.remove(&normalized);
        }
    }
    let mut block = Vec::new();
    for (_, (key, value)) in values {
        let key = wide(&key)?;
        let value = wide(&value)?;
        block.extend_from_slice(&key[..key.len() - 1]);
        block.push(b'=' as u16);
        block.extend_from_slice(&value);
        ensure!(
            block.len() <= 512 * 1024,
            "Native process environment exceeds 1 MiB"
        );
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    Ok(block)
}
struct Attributes {
    bytes: Vec<usize>,
    initialized: bool,
}
impl Attributes {
    fn pointer(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.bytes.as_mut_ptr().cast()
    }
    fn new() -> Result<Self> {
        let mut length = 0;
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), 2, 0, &mut length);
        }
        ensure!(
            length > 0 && length <= 65536,
            "Invalid process attribute extent"
        );
        let mut value = Self {
            bytes: vec![0; length.div_ceil(size_of::<usize>())],
            initialized: false,
        };
        ensure!(
            unsafe { InitializeProcThreadAttributeList(value.pointer(), 2, 0, &mut length) } != 0,
            "Cannot initialize native process attributes"
        );
        value.initialized = true;
        Ok(value)
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        if self.initialized {
            unsafe {
                DeleteProcThreadAttributeList(self.pointer());
            }
        }
    }
}

pub(super) fn spawn(
    command: &Command,
    container: &Container,
    inherited: &[HANDLE],
) -> Result<Process> {
    let program = wide(command.get_program())?;
    let mut arguments = quote(command.get_program())?;
    for argument in command.get_args() {
        arguments.push(b' ' as u16);
        arguments.extend(quote(argument)?);
    }
    arguments.push(0);
    let environment = environment(command)?;
    let current_directory = command
        .get_current_dir()
        .map(|path| wide(path.as_os_str()))
        .transpose()?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    let null_name = [b'N' as u16, b'U' as u16, b'L' as u16, 0];
    let handle = unsafe {
        CreateFileW(
            null_name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            &attributes,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    };
    ensure!(
        handle != INVALID_HANDLE_VALUE && !handle.is_null(),
        "Cannot discard native worker stdio"
    );
    let nul = unsafe { OwnedHandle::from_raw_handle(handle) };
    let mut handles = inherited.to_vec();
    handles.push(nul.as_raw_handle());
    let jobs = [container.0.as_raw_handle()];
    let mut attributes = Attributes::new()?;
    ensure!(
        unsafe {
            UpdateProcThreadAttribute(
                attributes.pointer(),
                0,
                PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
                jobs.as_ptr().cast(),
                size_of_val(&jobs),
                null_mut(),
                null_mut(),
            )
        } != 0,
        "Cannot atomically contain native worker"
    );
    ensure!(
        unsafe {
            UpdateProcThreadAttribute(
                attributes.pointer(),
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast(),
                size_of_val(handles.as_slice()),
                null_mut(),
                null_mut(),
            )
        } != 0,
        "Cannot restrict native handle inheritance"
    );
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES | STARTF_USESHOWWINDOW;
    startup.StartupInfo.wShowWindow = SW_HIDE as u16;
    startup.StartupInfo.hStdInput = nul.as_raw_handle();
    startup.StartupInfo.hStdOutput = nul.as_raw_handle();
    startup.StartupInfo.hStdError = nul.as_raw_handle();
    startup.lpAttributeList = attributes.pointer();
    let mut info = PROCESS_INFORMATION::default();
    ensure!(
        unsafe {
            CreateProcessW(
                program.as_ptr(),
                arguments.as_mut_ptr(),
                null(),
                null(),
                1,
                CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
                environment.as_ptr().cast(),
                current_directory
                    .as_ref()
                    .map_or(null(), |value| value.as_ptr()),
                &startup.StartupInfo,
                &mut info,
            )
        } != 0,
        "Cannot create contained native worker"
    );
    let process = Process {
        handle: unsafe { OwnedHandle::from_raw_handle(info.hProcess) },
        pid: info.dwProcessId,
    };
    drop(unsafe { OwnedHandle::from_raw_handle(info.hThread) });
    Ok(process)
}
impl Process {
    pub fn wait_ready(&self, duration: std::time::Duration) {
        unsafe {
            WaitForSingleObject(
                self.handle.as_raw_handle(),
                duration.as_millis().min(u32::MAX as u128) as u32,
            );
        }
    }
    pub fn try_wait(&self) -> Result<Option<bool>> {
        let status = unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 0) };
        if status == WAIT_TIMEOUT {
            return Ok(None);
        }
        ensure!(status == WAIT_OBJECT_0, "Native process wait failed");
        let mut code = 0;
        ensure!(
            unsafe { GetExitCodeProcess(self.handle.as_raw_handle(), &mut code) } != 0,
            "Native process status unavailable"
        );
        Ok(Some(code == 0))
    }
    pub fn wait(&self) -> Result<bool> {
        ensure!(
            unsafe { WaitForSingleObject(self.handle.as_raw_handle(), INFINITE) } == WAIT_OBJECT_0,
            "Native process reap failed"
        );
        self.try_wait()?.context("Native process did not finish")
    }
    pub fn kill(&self) {
        unsafe {
            TerminateProcess(self.handle.as_raw_handle(), 1);
        }
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
                    (&mut information as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                    size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                    null_mut(),
                )
            } != 0,
            "Native job status unavailable"
        );
        Ok(information.ActiveProcesses == 0)
    }
}
