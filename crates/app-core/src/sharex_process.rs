//! Trusted operation-scoped interpreter process; no .NET runtime exists in the desktop.
#[cfg(target_os = "windows")]
use crate::windows_child::Child;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2_10::{Digest, Sha256};
#[cfg(not(target_os = "windows"))]
use std::process::Child;
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

const INPUT: usize = 4 * 1024 * 1024;
const OUTPUT: usize = 2 * 1024 * 1024;
const DIAGNOSTICS: usize = 16 * 1024;
const TICK: Duration = Duration::from_millis(25);
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Member {
    path: PathBuf,
    bytes: u64,
    sha256: String,
    executable: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    protocol: u32,
    rid: String,
    sdk: String,
    runtime: String,
    entrypoint: String,
    files: Vec<Member>,
}
pub(crate) struct RuntimeBundle {
    directory: PathBuf,
    entrypoint: PathBuf,
    manifest: Manifest,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallState {
    owner: String,
    current: String,
    previous: Option<String>,
    target: String,
    key_sha256: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InstalledMember {
    path: PathBuf,
    sha256: String,
    size: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InstalledVersion {
    owner: String,
    version: String,
    target: String,
    executable: String,
    files: Vec<InstalledMember>,
    archive_sha256: String,
}
#[derive(Deserialize)]
struct BundleInfo {
    version: String,
    target: String,
    binary_sha256: String,
}
fn metadata<T: for<'de> Deserialize<'de>>(path: &Path, limit: u64) -> Result<T> {
    let (file, length) = regular(path, limit)?;
    let mut bytes = Vec::with_capacity(length as usize);
    file.take(length + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 == length,
        "SXCU installation metadata changed while reading"
    );
    serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("Invalid SXCU installation ownership metadata"))
}
fn private_directory(path: &Path) -> Result<()> {
    let metadata =
        std::fs::symlink_metadata(path).context("Private SXCU runtime directory is missing")?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "Private SXCU directory cannot be a symlink"
    );
    Ok(())
}
fn owned_install_directory(path: &Path) -> Result<()> {
    private_directory(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::symlink_metadata(path)?;
        ensure!(
            meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o022 == 0,
            "SXCU user-install directory is not privately owned by the current user"
        );
    }
    Ok(())
}
fn digest_text(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn version_identifier(value: &str, target: &str) -> bool {
    value.len() <= 256
        && value
            .strip_suffix(&format!("-{target}"))
            .is_some_and(|version| {
                version.split('.').count() == 3
                    && version.split('.').all(|part| {
                        !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
                    })
            })
}
fn native_target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => {
            if cfg!(target_env = "gnu") {
                "x86_64-pc-windows-gnu"
            } else {
                "x86_64-pc-windows-msvc"
            }
        }
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        _ => "unsupported",
    }
}
fn regular(path: &Path, limit: u64) -> Result<(File, u64)> {
    let named = std::fs::symlink_metadata(path).context("Private SXCU runtime file is missing")?;
    ensure!(
        named.is_file() && !named.file_type().is_symlink() && named.len() <= limit,
        "Private SXCU runtime member is not a bounded regular file"
    );
    let mut options = File::options();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .context("Could not open private SXCU runtime member")?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() == named.len(),
        "Private SXCU runtime member changed during admission"
    );
    Ok((file, metadata.len()))
}
impl RuntimeBundle {
    pub(crate) fn config_digest(&self) -> Result<[u8; 32]> {
        let mut hash = Sha256::new();
        hash.update(b"screenshots-sxcu-runtime-config-v1\0");
        hash.update(serde_json::to_vec(&self.manifest)?);
        // The manifest is retained by the prepared executor; verify() checks its members before use.
        hash.update(serde_json::to_vec(&self.directory)?);
        Ok(hash.finalize().into())
    }
    pub(crate) fn installed() -> Result<Self> {
        let executable =
            std::env::current_exe().context("Could not locate the application private runtime")?;
        Self::for_executable(&executable, env!("CARGO_PKG_VERSION"), native_target())
    }
    fn for_executable(executable: &Path, version: &str, target: &str) -> Result<Self> {
        let parent = executable
            .parent()
            .context("Application has no directory")?;
        let installed = match std::fs::symlink_metadata(parent.join(".screenshots-install.json")) {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(_) => anyhow::bail!("Could not inspect private SXCU installation ownership"),
        };
        if installed {
            owned_install_directory(parent)?;
            let state: InstallState = metadata(&parent.join(".screenshots-install.json"), 16384)?;
            ensure!(
                state.owner == "screenshots-user-install-v1"
                    && state.target == target
                    && version_identifier(&state.current, target)
                    && state
                        .previous
                        .as_ref()
                        .is_none_or(|value| version_identifier(value, target))
                    && state
                        .key_sha256
                        .as_ref()
                        .is_none_or(|value| digest_text(value)),
                "Invalid owned user-install metadata for SXCU runtime"
            );
            let versions = parent.join("versions");
            owned_install_directory(&versions)?;
            let folder = versions.join(format!("{version}-{target}"));
            owned_install_directory(&folder)?;
            let manifest: InstalledVersion =
                metadata(&folder.join(".screenshots-version.json"), 4 * 1024 * 1024)?;
            let expected = if cfg!(target_os = "windows") {
                "screenshots.exe"
            } else {
                "screenshots"
            };
            ensure!(
                manifest.owner == "screenshots-user-install-v1"
                    && manifest.version == version
                    && manifest.target == target
                    && manifest.executable == expected
                    && digest_text(&manifest.archive_sha256)
                    && manifest.files.len() <= 10000,
                "Retained installation does not match the running SXCU application version"
            );
            let mut seen = std::collections::HashSet::new();
            let mut anchor = None;
            let mut binary = false;
            for member in manifest.files {
                ensure!(
                    !member.path.is_absolute()
                        && member
                            .path
                            .components()
                            .all(|part| matches!(part, std::path::Component::Normal(_)))
                        && member.path.as_os_str().as_encoded_bytes().len() <= 4096
                        && digest_text(&member.sha256)
                        && seen.insert(member.path.clone()),
                    "Invalid retained installation member"
                );
                if member.path == Path::new(expected) {
                    binary = true;
                }
                if member.path == Path::new("lib/screenshots-sxcu/RUNTIME-MANIFEST.json") {
                    anchor = Some(member);
                }
            }
            ensure!(
                binary,
                "Retained installation executable is absent from its ownership manifest"
            );
            let anchor =
                anchor.context("Retained application version has no owned SXCU runtime")?;
            let lib = folder.join("lib");
            private_directory(&lib)?;
            return Self::read_anchored(&lib.join("screenshots-sxcu"), Some(&anchor));
        }
        if parent.file_name().is_some_and(|name| name == "MacOS")
            && parent
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == "Contents")
        {
            let resources = parent.parent().unwrap().join("Resources");
            private_directory(&resources)?;
            let info: BundleInfo = metadata(&resources.join("BUILD-INFO.json"), 16384)?;
            ensure!(
                info.version == version
                    && info.target == target
                    && digest_text(&info.binary_sha256),
                "macOS app resource runtime does not match the running application"
            );
            let lib = resources.join("lib");
            private_directory(&lib)?;
            return Self::read(&lib.join("screenshots-sxcu"));
        }
        let lib = parent.join("lib");
        private_directory(&lib)?;
        Self::read(&lib.join("screenshots-sxcu"))
    }
    fn read(directory: &Path) -> Result<Self> {
        Self::read_anchored(directory, None)
    }
    fn read_anchored(directory: &Path, anchor: Option<&InstalledMember>) -> Result<Self> {
        let meta = std::fs::symlink_metadata(directory)
            .context("Full SXCU execution requires the packaged private interpreter runtime")?;
        ensure!(
            meta.is_dir() && !meta.file_type().is_symlink(),
            "Private SXCU runtime directory cannot be a symlink"
        );
        let (file, length) = regular(&directory.join("RUNTIME-MANIFEST.json"), 128 * 1024)?;
        let mut bytes = Vec::with_capacity(length as usize);
        file.take(length + 1).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 == length,
            "Private SXCU manifest changed while reading"
        );
        if let Some(anchor) = anchor {
            let actual: String = Sha256::digest(&bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            ensure!(
                length == anchor.size && actual == anchor.sha256,
                "Retained SXCU runtime manifest failed its installation ownership hash"
            );
        }
        let manifest: Manifest = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Invalid private SXCU runtime manifest"))?;
        let rid = match (std::env::consts::OS, std::env::consts::ARCH) {
            ("windows", "x86_64") => "win-x64",
            ("linux", "x86_64") => "linux-x64",
            ("linux", "aarch64") => "linux-arm64",
            ("macos", "aarch64") => "osx-arm64",
            ("macos", "x86_64") => "osx-x64",
            _ => anyhow::bail!("Full SXCU runtime is unavailable for this target"),
        };
        let expected = if cfg!(target_os = "windows") {
            "screenshots-sxcu.exe"
        } else {
            "screenshots-sxcu"
        };
        ensure!(
            manifest.version == 1
                && manifest.protocol == 1
                && manifest.rid == rid
                && manifest.sdk == "10.0.404"
                && manifest.runtime == "10.0.15"
                && manifest.entrypoint == expected
                && (1..=512).contains(&manifest.files.len()),
            "Private SXCU runtime version or target mismatch"
        );
        let mut total = 0u64;
        let mut paths = std::collections::HashSet::new();
        let mut entrypoint = false;
        for member in &manifest.files {
            ensure!(
                member.path.as_os_str().as_encoded_bytes().len() <= 256
                    && !member.path.as_os_str().is_empty()
                    && member
                        .path
                        .components()
                        .all(|part| matches!(part, std::path::Component::Normal(_)))
                    && member.bytes <= 64 * 1024 * 1024
                    && member.sha256.len() == 64
                    && member
                        .sha256
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                    && paths.insert(member.path.clone()),
                "Invalid private SXCU runtime member"
            );
            total = total
                .checked_add(member.bytes)
                .context("Private SXCU runtime size overflow")?;
            ensure!(
                total <= 256 * 1024 * 1024,
                "Private SXCU runtime exceeds 256 MiB"
            );
            let mut ancestor = directory.to_path_buf();
            for component in member.path.components() {
                ancestor.push(component);
                let metadata = std::fs::symlink_metadata(&ancestor)
                    .context("Private SXCU runtime member is missing")?;
                ensure!(
                    !metadata.file_type().is_symlink(),
                    "Private SXCU runtime symlinks are forbidden"
                );
            }
            let (_, length) = regular(&directory.join(&member.path), 64 * 1024 * 1024)?;
            ensure!(
                length == member.bytes,
                "Private SXCU runtime member length mismatch"
            );
            #[cfg(unix)]
            if member.executable {
                use std::os::unix::fs::PermissionsExt;
                ensure!(
                    std::fs::metadata(directory.join(&member.path))?
                        .permissions()
                        .mode()
                        & 0o111
                        != 0,
                    "Private SXCU executable permission is missing"
                );
            }
            if member.path == Path::new(expected) {
                entrypoint = true;
                ensure!(
                    member.executable,
                    "Private SXCU entrypoint is not executable"
                );
            }
        }
        let mut directories = vec![(directory.to_path_buf(), 0usize)];
        let mut entries = 0;
        while let Some((folder, depth)) = directories.pop() {
            ensure!(
                depth <= 8,
                "Private SXCU runtime directory nesting exceeded its bound"
            );
            for item in std::fs::read_dir(folder)? {
                let item = item?;
                entries += 1;
                ensure!(
                    entries <= 1024,
                    "Private SXCU runtime contains excessive entries"
                );
                let metadata = std::fs::symlink_metadata(item.path())?;
                ensure!(
                    !metadata.file_type().is_symlink(),
                    "Private SXCU runtime symlinks are forbidden"
                );
                if metadata.is_dir() {
                    directories.push((item.path(), depth + 1));
                } else {
                    ensure!(
                        metadata.is_file(),
                        "Private SXCU runtime contains a special file"
                    );
                    let relative = item.path().strip_prefix(directory)?.to_path_buf();
                    ensure!(
                        relative == Path::new("RUNTIME-MANIFEST.json") || paths.contains(&relative),
                        "Private SXCU runtime contains an unowned file"
                    );
                }
            }
        }
        ensure!(
            entrypoint,
            "Private SXCU runtime entrypoint is absent from its manifest"
        );
        Ok(Self {
            directory: directory.to_path_buf(),
            entrypoint: directory.join(expected),
            manifest,
        })
    }
    pub(crate) fn verify(&self, cancel: &crate::UploadCancellation) -> Result<()> {
        let mut buffer = [0u8; 65536];
        for member in &self.manifest.files {
            cancel.check()?;
            let (mut file, length) = regular(&self.directory.join(&member.path), 64 * 1024 * 1024)?;
            ensure!(
                length == member.bytes,
                "Private SXCU runtime member changed"
            );
            let mut remaining = length;
            let mut hash = Sha256::new();
            while remaining > 0 {
                cancel.check()?;
                let want = buffer.len().min(remaining as usize);
                let read = file.read(&mut buffer[..want])?;
                ensure!(read > 0, "Private SXCU runtime member ended early");
                hash.update(&buffer[..read]);
                remaining -= read as u64;
            }
            let digest: String = hash
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            ensure!(
                digest == member.sha256,
                "Private SXCU runtime integrity check failed"
            );
        }
        Ok(())
    }
}

fn command(bundle: &RuntimeBundle) -> Command {
    let mut command = Command::new(&bundle.entrypoint);
    command
        .current_dir(&bundle.directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, _) in std::env::vars_os() {
        let key_upper = key.to_string_lossy().to_ascii_uppercase();
        if [
            "DOTNET_",
            "COMPLUS_",
            "CORECLR_",
            "COREHOST_",
            "COR_",
            "LD_",
            "DYLD_",
        ]
        .iter()
        .any(|prefix| key_upper.starts_with(prefix))
            || matches!(
                key_upper.as_str(),
                "DEVPATH" | "PATH" | "SSLKEYLOGFILE" | "MONO_ENV_OPTIONS" | "MONO_PATH"
            )
        {
            command.env_remove(key);
        }
    }
    for (key, value) in [
        ("DOTNET_EnableDiagnostics", "0"),
        ("DOTNET_DbgEnableMiniDump", "0"),
        ("DOTNET_GCHeapHardLimit", "4000000"),
        ("DOTNET_gcServer", "0"),
        ("DOTNET_gcConcurrent", "0"),
        ("DOTNET_TieredCompilation", "0"),
        ("DOTNET_TieredPGO", "0"),
        ("DOTNET_MULTILEVEL_LOOKUP", "0"),
    ] {
        command.env(key, value);
    }
    command
}
struct Job {
    child: Child,
    stopped: bool,
    #[cfg(unix)]
    group: i32,
    #[cfg(target_os = "windows")]
    handle: Option<std::os::windows::io::OwnedHandle>,
}
impl Job {
    fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        #[cfg(unix)]
        unsafe {
            libc::kill(-self.group, libc::SIGKILL);
        }
        #[cfg(target_os = "windows")]
        drop(self.handle.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
    fn finish(&mut self) {
        self.stopped = true;
        #[cfg(target_os = "windows")]
        drop(self.handle.take());
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.stop();
    }
}
#[cfg(unix)]
fn spawn(bundle: &RuntimeBundle, deadline: Instant) -> Result<Job> {
    use std::os::unix::process::CommandExt;
    let mut command = command(bundle);
    command.process_group(0);
    #[cfg(target_os = "linux")]
    let parent = std::process::id() as libc::pid_t;
    unsafe {
        command.pre_exec(move || {
            #[cfg(target_os = "linux")]
            {
                if libc::getppid() != parent
                    || libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0
                    || libc::getppid() != parent
                {
                    return Err(std::io::Error::other("SXCU parent isolation failed"));
                }
            }
            let limit = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            if libc::setrlimit(libc::RLIMIT_CORE, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .context("Could not start the private SXCU interpreter")?;
    let group = match i32::try_from(child.id()) {
        Ok(value) => value,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("Invalid SXCU interpreter identifier")
        }
    };
    let job = Job {
        child,
        group,
        stopped: false,
    };
    ensure!(
        Instant::now() < deadline,
        "SXCU interpreter startup exceeded five seconds"
    );
    Ok(job)
}
#[cfg(target_os = "windows")]
#[path = "sharex_process_windows.rs"]
mod windows;
#[cfg(target_os = "windows")]
fn spawn(bundle: &RuntimeBundle, deadline: Instant) -> Result<Job> {
    windows::spawn(bundle, deadline)
}

#[cfg(unix)]
fn ready(pipe: &impl std::os::fd::AsRawFd, write: bool, stop: &AtomicBool) -> Result<bool> {
    let mut descriptor = libc::pollfd {
        fd: pipe.as_raw_fd(),
        events: if write { libc::POLLOUT } else { libc::POLLIN },
        revents: 0,
    };
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let result = unsafe { libc::poll(&mut descriptor, 1, 25) };
        if result > 0 {
            return Ok(true);
        }
        if result < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            anyhow::bail!("Private SXCU pipe wait failed");
        }
    }
}
#[cfg(unix)]
fn nonblocking(pipe: &impl std::os::fd::AsRawFd) -> Result<()> {
    let fd = pipe.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    ensure!(
        flags >= 0 && unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == 0,
        "Could not bound private SXCU pipe"
    );
    Ok(())
}
fn send(
    mut pipe: impl Write + Pipe,
    bytes: Zeroizing<Vec<u8>>,
    stop: &AtomicBool,
) -> Result<Zeroizing<Vec<u8>>> {
    #[cfg(unix)]
    nonblocking(&pipe)?;
    let mut sent = 0;
    while sent < bytes.len() {
        ensure!(!stop.load(Ordering::Relaxed), "SXCU pipe canceled");
        #[cfg(unix)]
        if !ready(&pipe, true, stop)? {
            anyhow::bail!("SXCU pipe canceled");
        }
        match pipe.write(&bytes[sent..]) {
            Ok(0) => anyhow::bail!("Private SXCU input ended early"),
            Ok(count) => sent += count,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) => {}
            Err(_) => anyhow::bail!("Private SXCU input delivery failed"),
        }
    }
    Ok(Zeroizing::new(Vec::new()))
}
fn drain(
    mut pipe: impl Read + Pipe,
    limit: usize,
    retain: bool,
    stop: &AtomicBool,
) -> Result<Zeroizing<Vec<u8>>> {
    #[cfg(unix)]
    nonblocking(&pipe)?;
    let mut bytes = Zeroizing::new(Vec::new());
    let mut buffer = Zeroizing::new([0u8; 8192]);
    let mut count = 0usize;
    loop {
        ensure!(!stop.load(Ordering::Relaxed), "SXCU pipe canceled");
        #[cfg(unix)]
        if !ready(&pipe, false, stop)? {
            anyhow::bail!("SXCU pipe canceled");
        }
        match pipe.read(&mut *buffer) {
            Ok(0) => return Ok(bytes),
            Ok(read) => {
                count = count.saturating_add(read);
                ensure!(count <= limit, "Private SXCU pipe exceeded its byte bound");
                if retain {
                    bytes.extend_from_slice(&buffer[..read]);
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) => {}
            Err(_) => anyhow::bail!("Private SXCU pipe read failed"),
        }
    }
}
#[cfg(unix)]
trait Pipe: std::os::fd::AsRawFd {}
#[cfg(unix)]
impl<T: std::os::fd::AsRawFd> Pipe for T {}
#[cfg(not(unix))]
trait Pipe {}
#[cfg(not(unix))]
impl<T> Pipe for T {}
struct Active {
    job: Job,
    stop: Arc<AtomicBool>,
    workers: Vec<JoinHandle<()>>,
}
impl Drop for Active {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.job.stop();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}
pub(crate) fn interpret(
    bundle: &RuntimeBundle,
    request: Zeroizing<Vec<u8>>,
    cancel: &crate::UploadCancellation,
) -> Result<Zeroizing<Vec<u8>>> {
    ensure!(
        request.len() <= INPUT,
        "Private SXCU request exceeds four MiB"
    );
    cancel.check()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let job = spawn(bundle, deadline)?;
    let mut active = Active {
        job,
        stop: Arc::new(AtomicBool::new(false)),
        workers: Vec::with_capacity(3),
    };
    let input = active
        .job
        .child
        .stdin
        .take()
        .context("SXCU input pipe is missing")?;
    let output = active
        .job
        .child
        .stdout
        .take()
        .context("SXCU output pipe is missing")?;
    let diagnostics = active
        .job
        .child
        .stderr
        .take()
        .context("SXCU diagnostic pipe is missing")?;
    let (tx, rx) = mpsc::sync_channel(3);
    {
        let tx = tx.clone();
        let stop = active.stop.clone();
        active.workers.push(
            std::thread::Builder::new()
                .name("sxcu-input".into())
                .stack_size(256 * 1024)
                .spawn(move || {
                    let _ = tx.send((0, send(input, request, &stop)));
                })?,
        );
    }
    {
        let tx = tx.clone();
        let stop = active.stop.clone();
        active.workers.push(
            std::thread::Builder::new()
                .name("sxcu-output".into())
                .stack_size(256 * 1024)
                .spawn(move || {
                    let _ = tx.send((1, drain(output, OUTPUT, true, &stop)));
                })?,
        );
    }
    {
        let stop = active.stop.clone();
        active.workers.push(
            std::thread::Builder::new()
                .name("sxcu-diagnostics".into())
                .stack_size(256 * 1024)
                .spawn(move || {
                    let _ = tx.send((2, drain(diagnostics, DIAGNOSTICS, false, &stop)));
                })?,
        );
    }
    cancel.check()?;
    #[cfg(target_os = "windows")]
    active.job.child.resume(deadline)?;
    let mut completed = 0;
    let mut result = None;
    let mut status = None;
    loop {
        cancel.check()?;
        ensure!(
            Instant::now() < deadline,
            "Private SXCU interpreter exceeded five seconds"
        );
        if completed == 3 {
            std::thread::sleep(TICK);
        } else {
            match rx.recv_timeout(TICK) {
                Ok((kind, value)) => {
                    let value = value?;
                    completed += 1;
                    if kind == 1 {
                        result = Some(value);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => ensure!(
                    completed == 3,
                    "Private SXCU pipe worker ended unexpectedly"
                ),
            }
        }
        if status.is_none() {
            status = active.job.child.try_wait()?;
        }
        if completed == 3 && status.is_some() {
            break;
        }
    }
    ensure!(
        status.unwrap().success(),
        "Private SXCU interpreter failed; raw diagnostics were discarded"
    );
    cancel.check()?;
    active.job.finish();
    result.context("Private SXCU interpreter returned no result")
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("screenshots-sxcu-process-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn directory(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::create_dir_all(&path).unwrap();
            path
        }
        fn runtime(&self, directory: &Path, program: &[u8]) -> RuntimeBundle {
            let entrypoint = if cfg!(target_os = "windows") {
                "screenshots-sxcu.exe"
            } else {
                "screenshots-sxcu"
            };
            let path = directory.join(entrypoint);
            std::fs::write(&path, program).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
            let sha: String = Sha256::digest(program)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            let rid = match (std::env::consts::OS, std::env::consts::ARCH) {
                ("windows", _) => "win-x64",
                ("linux", "aarch64") => "linux-arm64",
                ("linux", _) => "linux-x64",
                ("macos", "aarch64") => "osx-arm64",
                ("macos", _) => "osx-x64",
                _ => panic!("unsupported fixture"),
            };
            let manifest = serde_json::json!({"version":1,"protocol":1,"rid":rid,"sdk":"10.0.404","runtime":"10.0.15","entrypoint":entrypoint,"files":[{"path":entrypoint,"bytes":program.len(),"sha256":sha,"executable":true}]});
            std::fs::write(
                directory.join("RUNTIME-MANIFEST.json"),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
            RuntimeBundle::read(directory).unwrap()
        }
        fn installed(&self, root: &Path, version: &str, program: &[u8]) {
            let target = native_target();
            let folder = root.join("versions").join(format!("{version}-{target}"));
            std::fs::create_dir_all(folder.join("lib/screenshots-sxcu")).unwrap();
            self.runtime(&folder.join("lib/screenshots-sxcu"), program);
            let manifest = folder.join("lib/screenshots-sxcu/RUNTIME-MANIFEST.json");
            let bytes = std::fs::read(&manifest).unwrap();
            let sha: String = Sha256::digest(&bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            let executable = if cfg!(target_os = "windows") {
                "screenshots.exe"
            } else {
                "screenshots"
            };
            let owned = serde_json::json!({"owner":"screenshots-user-install-v1","version":version,"target":target,"executable":executable,"archive_sha256":"0".repeat(64),"files":[{"path":executable,"size":1,"sha256":"0".repeat(64)},{"path":"lib/screenshots-sxcu/RUNTIME-MANIFEST.json","size":bytes.len(),"sha256":sha}]});
            std::fs::write(
                folder.join(".screenshots-version.json"),
                serde_json::to_vec(&owned).unwrap(),
            )
            .unwrap();
            let state = serde_json::json!({"owner":"screenshots-user-install-v1","current":format!("{version}-{target}"),"previous":null,"target":target,"key_sha256":null});
            std::fs::write(
                root.join(".screenshots-install.json"),
                serde_json::to_vec(&state).unwrap(),
            )
            .unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn runtime_integrity_rejects_changed_missing_unowned_and_escaping_members() {
        let fixture = Fixture::new();
        let dir = fixture.directory("runtime");
        let bundle = fixture.runtime(&dir, b"synthetic-runtime");
        bundle
            .verify(&crate::UploadCancellation::default())
            .unwrap();
        std::fs::write(
            dir.join("screenshots-sxcu.runtimeconfig.dev.json"),
            b"unowned loader config",
        )
        .unwrap();
        assert!(RuntimeBundle::read(&dir).is_err());
        std::fs::remove_file(dir.join("screenshots-sxcu.runtimeconfig.dev.json")).unwrap();
        std::fs::write(&bundle.entrypoint, b"changed-runtime!!").unwrap();
        assert!(
            bundle
                .verify(&crate::UploadCancellation::default())
                .is_err()
        );
        std::fs::remove_file(&bundle.entrypoint).unwrap();
        assert!(RuntimeBundle::read(&dir).is_err());
        let _ = fixture.runtime(&dir, b"synthetic-runtime");
        let path = dir.join("RUNTIME-MANIFEST.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value["files"][0]["path"] = "../outside".into();
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(RuntimeBundle::read(&dir).is_err());
    }
    #[test]
    fn portable_owned_install_and_macos_resource_layouts_select_only_known_roots() {
        let fixture = Fixture::new();
        let portable = fixture.directory("portable/lib/screenshots-sxcu");
        fixture.runtime(&portable, b"synthetic-runtime");
        let binary = fixture.0.join("portable/screenshots");
        assert!(RuntimeBundle::for_executable(&binary, "0.1.0", native_target()).is_ok());
        let install = fixture.directory("installed");
        fixture.installed(&install, "0.1.0", b"synthetic-runtime");
        let state = serde_json::json!({"owner":"screenshots-user-install-v1","current":format!("9.9.9-{}",native_target()),"previous":format!("0.1.0-{}",native_target()),"target":native_target(),"key_sha256":null});
        std::fs::write(
            install.join(".screenshots-install.json"),
            serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        let retained =
            RuntimeBundle::for_executable(&install.join("screenshots"), "0.1.0", native_target())
                .unwrap();
        assert!(
            retained.directory.starts_with(
                install
                    .join("versions")
                    .join(format!("0.1.0-{}", native_target()))
            )
        );
        assert!(
            RuntimeBundle::for_executable(&install.join("screenshots"), "9.9.9", native_target())
                .is_err()
        );
        let resources =
            fixture.directory("Screenshots.app/Contents/Resources/lib/screenshots-sxcu");
        fixture.runtime(&resources, b"synthetic-runtime");
        let info = serde_json::json!({"version":"0.1.0","target":native_target(),"binary_sha256":"0".repeat(64)});
        std::fs::write(
            fixture
                .0
                .join("Screenshots.app/Contents/Resources/BUILD-INFO.json"),
            serde_json::to_vec(&info).unwrap(),
        )
        .unwrap();
        let binary = fixture.0.join("Screenshots.app/Contents/MacOS/screenshots");
        assert!(RuntimeBundle::for_executable(&binary, "0.1.0", native_target()).is_ok());
    }
    #[cfg(unix)]
    #[test]
    fn anonymous_pipe_protocol_has_no_secret_argv_and_cancellation_reaps_blocked_child() {
        let fixture = Fixture::new();
        let dir = fixture.directory("runtime");
        let bundle=fixture.runtime(&dir,b"#!/bin/sh\n[ $# -eq 0 ] || exit 7\n/bin/cat >/dev/null\nprintf '{\"ok\":true,\"result\":{\"received\":true}}'\n");
        let cancel = crate::UploadCancellation::default();
        bundle.verify(&cancel).unwrap();
        let bytes = interpret(
            &bundle,
            Zeroizing::new(b"SYNTHETIC_PRIVATE_CONFIGURATION".to_vec()),
            &cancel,
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["result"]["received"], true);
        let bundle = fixture.runtime(&dir, b"#!/bin/sh\nexec /bin/sleep 30\n");
        let cancel = crate::UploadCancellation::default();
        let signal = cancel.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            signal.cancel();
        });
        let start = Instant::now();
        assert!(interpret(&bundle, Zeroizing::new(vec![b'x'; INPUT]), &cancel).is_err());
        thread.join().unwrap();
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[cfg(unix)]
    #[test]
    fn five_second_deadline_bounds_a_child_that_never_reads_its_input() {
        let fixture = Fixture::new();
        let dir = fixture.directory("runtime");
        let bundle = fixture.runtime(&dir, b"#!/bin/sh\nexec /bin/sleep 30\n");
        let start = Instant::now();
        let error = interpret(
            &bundle,
            Zeroizing::new(vec![b'x'; INPUT]),
            &crate::UploadCancellation::default(),
        )
        .err()
        .unwrap();
        assert!(error.to_string().contains("five seconds"));
        assert!(start.elapsed() < Duration::from_secs(7));
    }
    #[cfg(unix)]
    #[test]
    fn old_installed_process_resolves_its_retained_runtime_after_upgrade() {
        const CHILD: &str = "SCREENSHOTS_SXCU_INSTALLED_CHILD";
        if let Some(marker) = std::env::var_os(CHILD) {
            std::fs::write(marker, b"ready").unwrap();
            std::io::stdin().read_exact(&mut [0u8; 1]).unwrap();
            let runtime = RuntimeBundle::installed().unwrap();
            runtime
                .verify(&crate::UploadCancellation::default())
                .unwrap();
            let result = interpret(
                &runtime,
                Zeroizing::new(b"{}".to_vec()),
                &crate::UploadCancellation::default(),
            )
            .unwrap();
            assert_eq!(&*result, b"old-runtime");
            return;
        }
        let fixture = Fixture::new();
        let root = fixture.directory("installed");
        let version = env!("CARGO_PKG_VERSION");
        fixture.installed(
            &root,
            version,
            b"#!/bin/sh\n/bin/cat >/dev/null\nprintf old-runtime\n",
        );
        let launcher = root.join("screenshots");
        let executable = std::env::current_exe().unwrap();
        if std::fs::hard_link(&executable, &launcher).is_err() {
            std::fs::copy(&executable, &launcher).unwrap();
        }
        let marker = fixture.0.join("ready");
        let mut child=Command::new(&launcher).args(["--exact","sharex_process::tests::old_installed_process_resolves_its_retained_runtime_after_upgrade","--nocapture"]).env(CHILD,&marker).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("old installed child did not become ready");
            }
            std::thread::sleep(TICK);
        }
        let state = serde_json::json!({"owner":"screenshots-user-install-v1","current":format!("9.9.9-{}",native_target()),"previous":format!("{version}-{}",native_target()),"target":native_target(),"key_sha256":null});
        std::fs::write(
            root.join(".screenshots-install.json"),
            serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        std::fs::remove_file(&launcher).unwrap();
        std::fs::write(&launcher, b"new launcher bytes").unwrap();
        child.stdin.take().unwrap().write_all(b"x").unwrap();
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("old process failed to use retained runtime after upgrade");
            }
            std::thread::sleep(TICK);
        }
    }
}
