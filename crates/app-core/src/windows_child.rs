//! One operation-scoped child, atomically placed in its caller-owned Windows job.
use anyhow::{Context, Result, ensure};
use std::{
    cmp::Ordering,
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs::File,
    io,
    mem::{size_of, size_of_val},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle, RawHandle},
        process::ExitStatusExt,
    },
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    ptr::{null, null_mut},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Globalization::CompareStringOrdinal,
    Security::SECURITY_ATTRIBUTES,
    Storage::FileSystem::*,
    System::{
        JobObjects::*,
        Pipes::CreatePipe,
        SystemInformation::{GetSystemDirectoryW, GetWindowsDirectoryW},
        Threading::*,
    },
};
use zeroize::Zeroizing;

const ENVIRONMENT_UNITS: usize = 512 * 1024;
const COMMAND_UNITS: usize = 32767;
#[path = "windows_child_batch.rs"]
mod batch;

/// Stable Command getters do not expose whether env_clear was called.
#[derive(Clone, Copy)]
pub enum Environment {
    Inherit,
    Explicit,
}
#[derive(Clone, Copy)]
pub enum Stream {
    Null,
    Pipe,
}
#[derive(Clone, Copy)]
pub struct Streams {
    pub stdin: Stream,
    pub stdout: Stream,
    pub stderr: Stream,
}
impl Streams {
    pub const PIPED: Self = Self {
        stdin: Stream::Pipe,
        stdout: Stream::Pipe,
        stderr: Stream::Pipe,
    };
    pub const OUTPUT: Self = Self {
        stdin: Stream::Null,
        stdout: Stream::Pipe,
        stderr: Stream::Pipe,
    };
    pub const NULL: Self = Self {
        stdin: Stream::Null,
        stdout: Stream::Null,
        stderr: Stream::Null,
    };
}

/// The caller retains the sole noninheritable job handle for the whole operation.
pub fn job(processes: Option<u32>, memory: Option<usize>) -> Result<OwnedHandle> {
    let raw = unsafe { CreateJobObjectW(null(), null()) };
    ensure!(!raw.is_null(), "Could not create child isolation job");
    let job = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    if let Some(count) = processes {
        ensure!(count > 0, "Invalid child process limit");
        limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        limits.BasicLimitInformation.ActiveProcessLimit = count;
    }
    if let Some(bytes) = memory {
        ensure!(bytes > 0, "Invalid child memory limit");
        limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
        limits.ProcessMemoryLimit = bytes;
    }
    ensure!(
        unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                size_of_val(&limits) as u32,
            )
        } != 0,
        "Could not configure child isolation job"
    );
    Ok(job)
}

fn wide(value: &OsStr, limit: usize) -> Result<Vec<u16>> {
    let mut result = Vec::new();
    for unit in value.encode_wide() {
        ensure!(
            unit != 0 && result.len() < limit,
            "Invalid native process string"
        );
        result.push(unit);
    }
    result.push(0);
    Ok(result)
}
fn quote(value: &OsStr, always: bool) -> Result<Vec<u16>> {
    let value = wide(value, COMMAND_UNITS)?;
    let quoted = always
        || value.len() == 1
        || value.contains(&(b' ' as u16))
        || value.contains(&(b'\t' as u16));
    let mut result = Vec::new();
    if quoted {
        result.push(b'"' as u16);
    }
    let mut slashes = 0;
    for value in value.into_iter().take_while(|unit| *unit != 0) {
        if value == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        let count = if value == b'"' as u16 {
            slashes * 2 + 1
        } else {
            slashes
        };
        result.extend(std::iter::repeat_n(b'\\' as u16, count));
        slashes = 0;
        result.push(value);
    }
    result.extend(std::iter::repeat_n(
        b'\\' as u16,
        slashes * if quoted { 2 } else { 1 },
    ));
    if quoted {
        result.push(b'"' as u16);
    }
    ensure!(
        result.len() < COMMAND_UNITS,
        "Native process argument exceeds its limit"
    );
    Ok(result)
}

#[derive(Eq)]
struct Key(Vec<u16>);
impl Key {
    fn new(value: &OsStr) -> Result<Self> {
        let mut value = wide(value, COMMAND_UNITS)?;
        value.pop();
        ensure!(!value.is_empty(), "Empty native environment name");
        ensure!(
            !value[1..].contains(&(b'=' as u16)),
            "Invalid native environment name"
        );
        Ok(Self(value))
    }
}
impl Ord for Key {
    fn cmp(&self, other: &Self) -> Ordering {
        // Valid, bounded UTF-16 buffers make the native comparison infallible.
        match unsafe {
            CompareStringOrdinal(
                self.0.as_ptr(),
                self.0.len() as i32,
                other.0.as_ptr(),
                other.0.len() as i32,
                1,
            )
        } {
            1 => Ordering::Less,
            2 => Ordering::Equal,
            3 => Ordering::Greater,
            _ => unreachable!("Valid native environment-key comparison failed"),
        }
    }
}
impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
fn environment(command: &Command, mode: Environment) -> Result<Zeroizing<Vec<u16>>> {
    let mut values = BTreeMap::new();
    let mut total = 0usize;
    if matches!(mode, Environment::Inherit) {
        for (key, value) in std::env::vars_os() {
            let key = Key::new(&key)?;
            let value = wide(&value, ENVIRONMENT_UNITS)?;
            total = total.saturating_add(key.0.len() + value.len() + 1);
            ensure!(
                total < ENVIRONMENT_UNITS,
                "Native process environment exceeds 1 MiB"
            );
            values.insert(key, Zeroizing::new(value));
        }
    }
    for (key, value) in command.get_envs() {
        let key = Key::new(key)?;
        if let Some(previous) = values.remove(&key) {
            total -= key.0.len() + previous.len() + 1;
        }
        if let Some(value) = value {
            let value = wide(value, ENVIRONMENT_UNITS)?;
            total = total.saturating_add(key.0.len() + value.len() + 1);
            ensure!(
                total < ENVIRONMENT_UNITS,
                "Native process environment exceeds 1 MiB"
            );
            values.insert(key, Zeroizing::new(value));
        }
    }
    let mut block = Zeroizing::new(Vec::with_capacity(total.max(1) + 1));
    for (key, value) in values {
        block.extend_from_slice(&key.0);
        block.push(b'=' as u16);
        block.extend_from_slice(&value);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    Ok(block)
}

fn system_directory(windows: bool) -> Result<PathBuf> {
    let mut bytes = vec![0u16; 32768];
    let size = if windows {
        unsafe { GetWindowsDirectoryW(bytes.as_mut_ptr(), bytes.len() as u32) }
    } else {
        unsafe { GetSystemDirectoryW(bytes.as_mut_ptr(), bytes.len() as u32) }
    } as usize;
    ensure!(
        size > 0 && size < bytes.len(),
        "Could not locate native engine search directory"
    );
    Ok(PathBuf::from(OsString::from_wide(&bytes[..size])))
}
fn executable(command: &Command) -> Result<PathBuf> {
    let name = command.get_program();
    let path = Path::new(name);
    ensure!(
        !name.is_empty() && path.file_name().is_some(),
        "Invalid native engine path"
    );
    let extension = path.extension().and_then(OsStr::to_str);
    let append_exe = |path: &Path| {
        let mut value = path.as_os_str().to_os_string();
        value.push(".exe");
        PathBuf::from(value)
    };
    if path.components().count() > 1 {
        let mut path = std::path::absolute(path)?;
        if !extension.is_some_and(|value| value.eq_ignore_ascii_case("exe")) {
            let candidate = append_exe(&path);
            if candidate.is_file() {
                path = candidate;
            }
        }
        return Ok(path);
    }
    let child_path = command.get_envs().find_map(|(key, value)| {
        (Key::new(key).ok()? == Key::new(OsStr::new("PATH")).ok()?)
            .then_some(value)
            .flatten()
    });
    let mut paths = Vec::new();
    if let Some(value) = child_path {
        paths.extend(std::env::split_paths(value));
    }
    if let Ok(path) = std::env::current_exe()
        && let Some(parent) = path.parent()
    {
        paths.push(parent.to_owned());
    }
    paths.push(system_directory(false)?);
    paths.push(system_directory(true)?);
    if let Some(value) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&value));
    }
    ensure!(
        paths.len() <= 16384,
        "Native engine search path exceeds its bound"
    );
    // std's bare-name search treats any dot, including a leading dot, as an extension.
    let bare_has_extension = name.encode_wide().any(|unit| unit == u16::from(b'.'));
    for directory in paths
        .into_iter()
        .filter(|path| !path.as_os_str().is_empty())
    {
        let candidate = directory.join(path);
        let candidate = if !bare_has_extension {
            append_exe(&candidate)
        } else {
            candidate
        };
        if candidate.is_file() {
            return Ok(std::path::absolute(candidate)?);
        }
    }
    anyhow::bail!("Native engine executable is unavailable")
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
            "Invalid native process attribute extent"
        );
        let mut value = Self {
            bytes: vec![0; length.div_ceil(size_of::<usize>())],
            initialized: false,
        };
        ensure!(
            unsafe { InitializeProcThreadAttributeList(value.pointer(), 2, 0, &mut length) } != 0,
            "Could not initialize native process attributes"
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
struct Pipe {
    parent: Option<File>,
    child: OwnedHandle,
}
fn pipe(mode: Stream, input: bool) -> Result<Pipe> {
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    if matches!(mode, Stream::Null) {
        let name = [b'N' as u16, b'U' as u16, b'L' as u16, 0];
        let raw = unsafe {
            CreateFileW(
                name.as_ptr(),
                if input { GENERIC_READ } else { GENERIC_WRITE },
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                &attributes,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                null_mut(),
            )
        };
        ensure!(
            !raw.is_null() && raw != INVALID_HANDLE_VALUE,
            "Could not open native process null stream"
        );
        return Ok(Pipe {
            parent: None,
            child: unsafe { OwnedHandle::from_raw_handle(raw) },
        });
    }
    let (mut read, mut write) = (null_mut(), null_mut());
    ensure!(
        unsafe { CreatePipe(&mut read, &mut write, &attributes, 65536) } != 0,
        "Could not create native process pipe"
    );
    let read = unsafe { OwnedHandle::from_raw_handle(read) };
    let write = unsafe { OwnedHandle::from_raw_handle(write) };
    let (parent, child) = if input { (write, read) } else { (read, write) };
    ensure!(
        unsafe { SetHandleInformation(parent.as_raw_handle(), HANDLE_FLAG_INHERIT, 0) } != 0,
        "Could not protect native parent pipe"
    );
    Ok(Pipe {
        parent: Some(File::from(parent)),
        child,
    })
}

pub struct Child {
    handle: OwnedHandle,
    thread: Option<OwnedHandle>,
    pid: u32,
    status: Option<ExitStatus>,
    launch_identity: Option<(PathBuf, crate::FileIdentity)>,
    pub stdin: Option<File>,
    pub stdout: Option<File>,
    pub stderr: Option<File>,
}
impl AsRawHandle for Child {
    fn as_raw_handle(&self) -> RawHandle {
        self.handle.as_raw_handle()
    }
}
impl Child {
    pub fn id(&self) -> u32 {
        self.pid
    }
    pub fn resume(&mut self, deadline: Instant) -> Result<()> {
        ensure!(
            Instant::now() < deadline,
            "Native child startup exceeded its deadline"
        );
        // Windows startup is suspended: ordinary replacements before actual start
        // must be rejected as well. Script/interpreter reads after native start are
        // still pathname operations, not an atomic executable/content pin.
        let _admitted_program = if let Some((path, identity)) = &self.launch_identity {
            let (file, _) = crate::FileIdentity::open(path)?;
            identity.verify_file(&file)?;
            Some(file)
        } else {
            None
        };
        let thread = self
            .thread
            .as_ref()
            .context("Native initial thread was already resumed")?;
        ensure!(
            unsafe { ResumeThread(thread.as_raw_handle()) } == 1,
            "Could not resume contained native child"
        );
        self.thread.take();
        self.launch_identity.take();
        Ok(())
    }
    pub fn kill(&mut self) -> io::Result<()> {
        if self.try_wait()?.is_some() {
            return Ok(());
        }
        if unsafe { TerminateProcess(self.handle.as_raw_handle(), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if self.status.is_some() {
            return Ok(self.status);
        }
        match unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 0) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => {
                let mut code = 0;
                if unsafe { GetExitCodeProcess(self.handle.as_raw_handle(), &mut code) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                self.status = Some(ExitStatus::from_raw(code));
                Ok(self.status)
            }
            _ => Err(io::Error::last_os_error()),
        }
    }
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.stdin.take();
        if let Some(status) = self.try_wait()? {
            return Ok(status);
        }
        if unsafe { WaitForSingleObject(self.handle.as_raw_handle(), INFINITE) } != WAIT_OBJECT_0 {
            return Err(io::Error::last_os_error());
        }
        self.try_wait()?
            .ok_or_else(|| io::Error::other("Native process did not finish"))
    }
    pub fn wait_ready(&self, duration: Duration) {
        unsafe {
            WaitForSingleObject(
                self.handle.as_raw_handle(),
                duration.as_millis().min(u32::MAX as u128) as u32,
            );
        }
    }
}
impl Drop for Child {
    fn drop(&mut self) {
        // A fallible pipe/bridge setup must never leave a permanently suspended child.
        if self.thread.is_some() {
            let _ = self.kill();
            let _ = self.wait();
        }
    }
}

/// Uses the same platform/script resolution as native launch; no process is created.
pub fn resolved_program(command: &Command) -> Result<PathBuf> {
    executable(command)
}

pub fn spawn(
    command: &Command,
    job: BorrowedHandle<'_>,
    streams: Streams,
    mode: Environment,
    inherited: &[BorrowedHandle<'_>],
) -> Result<Child> {
    spawn_with_identity(command, job, streams, mode, inherited, None)
}
pub fn spawn_with_identity(
    command: &Command,
    job: BorrowedHandle<'_>,
    streams: Streams,
    mode: Environment,
    inherited: &[BorrowedHandle<'_>],
    identity: Option<&crate::FileIdentity>,
) -> Result<Child> {
    ensure!(
        inherited.len() <= 32,
        "Native child handle list exceeds its bound"
    );
    let resolved = executable(command)?;
    let image = batch::user_path(&resolved)?;
    let suffix = image[..image.len() - 1]
        .iter()
        .rev()
        .take(4)
        .map(|unit| *unit | 0x20)
        .collect::<Vec<_>>();
    let is_batch = suffix.iter().copied().eq("tab.".encode_utf16())
        || suffix.iter().copied().eq("dmc.".encode_utf16());
    let (program, mut arguments) = if is_batch {
        (
            wide(
                system_directory(false)?.join("cmd.exe").as_os_str(),
                COMMAND_UNITS,
            )?,
            batch::command_line(&image, command)?,
        )
    } else {
        let mut arguments = quote(command.get_program(), true)?;
        for value in command.get_args() {
            arguments.push(b' ' as u16);
            arguments.extend(quote(value, false)?);
            ensure!(
                arguments.len() < COMMAND_UNITS,
                "Native command line exceeds 32767 UTF-16 units"
            );
        }
        (image, arguments)
    };
    arguments.push(0);
    let environment = environment(command, mode)?;
    let directory = command
        .get_current_dir()
        .map(batch::user_path)
        .transpose()?;
    let input = pipe(streams.stdin, true)?;
    let output = pipe(streams.stdout, false)?;
    let error = pipe(streams.stderr, false)?;
    let mut handles = vec![
        input.child.as_raw_handle(),
        output.child.as_raw_handle(),
        error.child.as_raw_handle(),
    ];
    for handle in inherited {
        let handle = handle.as_raw_handle();
        ensure!(
            handle != job.as_raw_handle() && !handle.is_null() && handle != INVALID_HANDLE_VALUE,
            "Invalid inherited native child handle"
        );
        if !handles.contains(&handle) {
            handles.push(handle);
        }
    }
    let jobs = [job.as_raw_handle()];
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
        "Could not atomically contain native child"
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
        "Could not restrict native child handle inheritance"
    );
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = input.child.as_raw_handle();
    startup.StartupInfo.hStdOutput = output.child.as_raw_handle();
    startup.StartupInfo.hStdError = error.child.as_raw_handle();
    startup.lpAttributeList = attributes.pointer();
    // Verify the resolved executable or batch script at the native start boundary.
    // The handle is retained through CreateProcessW; pathname retarget during that native call
    // is not an atomic/hostile-user executable pin, and cmd.exe remains the OS interpreter.
    let _admitted_program = if let Some(identity) = identity {
        let (file, _) = crate::FileIdentity::open(&resolved)?;
        identity.verify_file(&file)?;
        Some(file)
    } else {
        None
    };
    let mut info = PROCESS_INFORMATION::default();
    ensure!(
        unsafe {
            CreateProcessW(
                program.as_ptr(),
                arguments.as_mut_ptr(),
                null(),
                null(),
                1,
                CREATE_SUSPENDED
                    | CREATE_NO_WINDOW
                    | CREATE_UNICODE_ENVIRONMENT
                    | EXTENDED_STARTUPINFO_PRESENT,
                environment.as_ptr().cast(),
                directory.as_ref().map_or(null(), |value| value.as_ptr()),
                &startup.StartupInfo,
                &mut info,
            )
        } != 0,
        "Could not create atomically contained native child"
    );
    Ok(Child {
        handle: unsafe { OwnedHandle::from_raw_handle(info.hProcess) },
        thread: Some(unsafe { OwnedHandle::from_raw_handle(info.hThread) }),
        pid: info.dwProcessId,
        status: None,
        launch_identity: identity.map(|identity| (resolved, identity.clone())),
        stdin: input.parent,
        stdout: output.parent,
        stderr: error.parent,
    })
}

#[cfg(test)]
#[path = "windows_child_tests.rs"]
mod tests;
