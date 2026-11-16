use super::*;
use std::{
    io::{Read, Write},
    os::windows::io::AsHandle,
    process::Stdio,
};

const MODE: &str = "SCREENSHOTS_ATOMIC_CHILD_FIXTURE";
const BASE: &str = "SCREENSHOTS_ATOMIC_CHILD_DIRECTORY";
const MAGIC: &[u8] = b"SC-ATOMIC-BINARY\0";
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("screenshots-atomic-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fixture(mode: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "windows_child::tests::child_fixture",
            "--ignored",
            "--nocapture",
            "--quiet",
        ])
        .env(MODE, mode);
    command
}
fn started() {
    let mut contained = 0;
    assert_ne!(
        unsafe { IsProcessInJob(GetCurrentProcess(), null_mut(), &mut contained) },
        0
    );
    assert_ne!(contained, 0, "Child user code ran without containment");
}
fn binary(child: &mut Child) -> Vec<u8> {
    let mut bytes = Vec::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    let status = child.wait().unwrap();
    if !status.success() {
        let mut error = Vec::new();
        if let Some(mut pipe) = child.stderr.take() {
            pipe.read_to_end(&mut error).unwrap();
        }
        panic!(
            "Synthetic child failed {status}: {}",
            String::from_utf8_lossy(&error)
        );
    }
    let index = bytes
        .windows(MAGIC.len())
        .position(|value| value == MAGIC)
        .expect("fixture payload marker");
    bytes[index + MAGIC.len()..].to_vec()
}
fn run(command: &Command) -> Vec<u8> {
    let container = job(None, None).unwrap();
    let mut child = spawn(
        command,
        container.as_handle(),
        Streams::PIPED,
        Environment::Inherit,
        &[],
    )
    .unwrap();
    child.stdin.take();
    child
        .resume(Instant::now() + Duration::from_secs(5))
        .unwrap();
    binary(&mut child)
}
fn wait_file(path: &Path) -> String {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(text) = std::fs::read_to_string(path)
            && !text.is_empty()
        {
            return text;
        }
        assert!(Instant::now() < until, "fixture readiness deadline");
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn observe(pid: u32) -> OwnedHandle {
    let raw = unsafe {
        OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        )
    };
    assert!(!raw.is_null(), "owned process must still be observable");
    unsafe { OwnedHandle::from_raw_handle(raw) }
}
fn signaled(process: &OwnedHandle) {
    assert_eq!(
        unsafe { WaitForSingleObject(process.as_raw_handle(), 5000) },
        WAIT_OBJECT_0
    );
}
struct Owner(std::process::Child);
impl Drop for Owner {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "private native subprocess fixture"]
fn child_fixture() {
    let Ok(mode) = std::env::var(MODE) else {
        return;
    };
    let directory = std::env::var_os(BASE).map(PathBuf::from);
    if mode == "resolver-oracle" {
        // std's independent resolver oracle intentionally runs outside our Job.
        std::io::stdout().write_all(MAGIC).unwrap();
        std::io::stdout().write_all(b"true").unwrap();
        std::process::exit(0);
    }
    if mode.starts_with("owner-") {
        let directory = directory.unwrap();
        let container = job(None, None).unwrap();
        let mode = if mode == "owner-suspended" {
            "hold"
        } else {
            "descendant"
        };
        let mut command = fixture(mode);
        command.env(BASE, &directory);
        let mut child = spawn(
            &command,
            container.as_handle(),
            Streams::PIPED,
            Environment::Inherit,
            &[],
        )
        .unwrap();
        if mode == "descendant" {
            child
                .resume(Instant::now() + Duration::from_secs(5))
                .unwrap();
            wait_file(&directory.join("descendant"));
        }
        std::fs::write(directory.join("child"), child.id().to_string()).unwrap();
        std::thread::sleep(Duration::from_secs(60));
        std::process::exit(11);
    }
    if mode == "environment-owner" {
        let mut command = fixture("metadata");
        command
            .env_remove("SCREENSHOTS_ATOMIC_REMOVED")
            .env("screenshots_atomic_override", "child");
        std::fs::write(directory.unwrap().join("environment"), run(&command)).unwrap();
        std::process::exit(0);
    }
    if mode == "batch-dump" {
        let values = std::env::args_os()
            .skip_while(|value| value != "--")
            .skip(1)
            .map(|value| value.encode_wide().collect::<Vec<_>>())
            .collect::<Vec<_>>();
        std::fs::write(
            std::env::var_os("SCREENSHOTS_ATOMIC_BATCH_OUTPUT").unwrap(),
            serde_json::to_vec(&values).unwrap(),
        )
        .unwrap();
        std::process::exit(0);
    }
    started();
    if let Some(directory) = &directory {
        std::fs::write(directory.join("started"), b"contained").unwrap();
    }
    match mode.as_str() {
        "hold" => std::thread::sleep(Duration::from_secs(60)),
        "descendant" => {
            let mut command = fixture("hold");
            command.env(BASE, directory.as_ref().unwrap());
            let mut descendant = command.spawn().unwrap();
            std::fs::write(
                directory.unwrap().join("descendant"),
                descendant.id().to_string(),
            )
            .unwrap();
            std::thread::sleep(Duration::from_secs(60));
            let _ = descendant.wait();
        }
        "roundtrip" => {
            let mut bytes = Vec::new();
            std::io::stdin().read_to_end(&mut bytes).unwrap();
            std::io::stdout().write_all(MAGIC).unwrap();
            std::io::stdout().write_all(&bytes).unwrap();
            std::io::stderr()
                .write_all(b"PRIVATE SYNTHETIC DIAGNOSTIC")
                .unwrap();
        }
        "metadata" => {
            let metadata = serde_json::json!({
                "arguments":std::env::args_os().skip_while(|value|value!="--").skip(1).map(|value|value.encode_wide().collect::<Vec<_>>()).collect::<Vec<_>>(),
                "override":std::env::var("SCREENSHOTS_ATOMIC_OVERRIDE").ok(),
                "removed":std::env::var("SCREENSHOTS_ATOMIC_REMOVED").ok(),
                "path":std::env::var_os("PATH").map(|value|value.encode_wide().collect::<Vec<_>>()),
                "directory":std::env::current_dir().unwrap().as_os_str().encode_wide().collect::<Vec<_>>(),
                "dotnet":std::env::var_os("DOTNET_STARTUP_HOOKS").is_some(),
            });
            std::io::stdout().write_all(MAGIC).unwrap();
            std::io::stdout()
                .write_all(&serde_json::to_vec(&metadata).unwrap())
                .unwrap();
        }
        "handles" => {
            let allowed = std::env::var("SCREENSHOTS_ATOMIC_ALLOWED")
                .unwrap()
                .parse::<usize>()
                .unwrap() as HANDLE;
            let denied = std::env::var("SCREENSHOTS_ATOMIC_DENIED")
                .unwrap()
                .parse::<usize>()
                .unwrap() as HANDLE;
            assert_ne!(unsafe { SetEvent(allowed) }, 0);
            unsafe {
                SetEvent(denied);
            }
        }
        _ => panic!("unknown fixture"),
    }
    std::io::stdout().flush().unwrap();
    std::io::stderr().flush().unwrap();
    std::process::exit(0);
}

#[test]
fn suspended_child_is_contained_before_user_code_and_roundtrips_binary_pipes() {
    let directory = Directory::new();
    let container = job(None, None).unwrap();
    let mut command = fixture("roundtrip");
    command.env(BASE, &directory.0);
    let mut child = spawn(
        &command,
        container.as_handle(),
        Streams::PIPED,
        Environment::Inherit,
        &[],
    )
    .unwrap();
    let mut contained = 0;
    assert_ne!(
        unsafe {
            IsProcessInJob(
                child.as_raw_handle(),
                container.as_raw_handle(),
                &mut contained,
            )
        },
        0
    );
    assert_eq!(contained, 1);
    assert!(!directory.0.join("started").exists());
    let bytes = (0..131073)
        .map(|index| (index % 256) as u8)
        .collect::<Vec<_>>();
    let expected = bytes.clone();
    let mut input = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || input.write_all(&bytes).unwrap());
    child
        .resume(Instant::now() + Duration::from_secs(5))
        .unwrap();
    let output = binary(&mut child);
    writer.join().unwrap();
    assert_eq!(output, expected);
    let mut diagnostics = Vec::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_end(&mut diagnostics)
        .unwrap();
    assert_eq!(diagnostics, b"PRIVATE SYNTHETIC DIAGNOSTIC");
    assert_eq!(wait_file(&directory.0.join("started")), "contained");
}

#[test]
fn abrupt_owner_loss_kills_suspended_and_running_descendant_families() {
    for mode in ["owner-suspended", "owner-running"] {
        let directory = Directory::new();
        let mut command = fixture(mode);
        command
            .env(BASE, &directory.0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut owner = Owner(command.spawn().unwrap());
        let child = observe(wait_file(&directory.0.join("child")).parse().unwrap());
        let descendant = if mode == "owner-running" {
            Some(observe(
                wait_file(&directory.0.join("descendant")).parse().unwrap(),
            ))
        } else {
            None
        };
        owner.0.kill().unwrap();
        owner.0.wait().unwrap();
        signaled(&child);
        if let Some(descendant) = descendant {
            signaled(&descendant);
        }
        if mode == "owner-suspended" {
            assert!(!directory.0.join("started").exists());
        }
    }
}

#[test]
fn expired_resume_and_dropped_suspended_children_cannot_orphan() {
    let directory = Directory::new();
    let container = job(None, None).unwrap();
    let mut command = fixture("hold");
    command.env(BASE, &directory.0);
    let mut child = spawn(
        &command,
        container.as_handle(),
        Streams::NULL,
        Environment::Inherit,
        &[],
    )
    .unwrap();
    let observation = observe(child.id());
    assert!(
        child
            .resume(Instant::now() - Duration::from_secs(1))
            .is_err()
    );
    drop(child);
    signaled(&observation);
    assert!(!directory.0.join("started").exists());
    let child = spawn(
        &command,
        container.as_handle(),
        Streams::NULL,
        Environment::Inherit,
        &[],
    )
    .unwrap();
    let observation = observe(child.id());
    drop(child);
    signaled(&observation);
}

#[test]
fn only_listed_extra_handles_are_inherited_and_job_is_noninheritable() {
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    let allowed = unsafe { OwnedHandle::from_raw_handle(CreateEventW(&attributes, 1, 0, null())) };
    let denied = unsafe { OwnedHandle::from_raw_handle(CreateEventW(&attributes, 1, 0, null())) };
    let container = job(None, None).unwrap();
    let mut flags = 0;
    assert_ne!(
        unsafe { GetHandleInformation(container.as_raw_handle(), &mut flags) },
        0
    );
    assert_eq!(flags & HANDLE_FLAG_INHERIT, 0);
    let mut command = fixture("handles");
    command
        .env(
            "SCREENSHOTS_ATOMIC_ALLOWED",
            (allowed.as_raw_handle() as usize).to_string(),
        )
        .env(
            "SCREENSHOTS_ATOMIC_DENIED",
            (denied.as_raw_handle() as usize).to_string(),
        );
    let mut child = spawn(
        &command,
        container.as_handle(),
        Streams::NULL,
        Environment::Inherit,
        &[allowed.as_handle()],
    )
    .unwrap();
    child
        .resume(Instant::now() + Duration::from_secs(5))
        .unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(
        unsafe { WaitForSingleObject(allowed.as_raw_handle(), 0) },
        WAIT_OBJECT_0
    );
    assert_eq!(
        unsafe { WaitForSingleObject(denied.as_raw_handle(), 0) },
        WAIT_TIMEOUT
    );
}

#[test]
fn argument_environment_and_current_directory_semantics_are_preserved() {
    let directory = Directory::new();
    let values = [
        OsString::from(""),
        OsString::from("plain"),
        OsString::from("space value"),
        OsString::from("quote\"inside"),
        OsString::from("trailing\\"),
        OsString::from("emoji 🦊"),
    ];
    let mut command = fixture("metadata");
    command.arg("--").args(&values).current_dir(&directory.0);
    let result: serde_json::Value = serde_json::from_slice(&run(&command)).unwrap();
    assert_eq!(
        result["arguments"],
        serde_json::json!(
            values
                .iter()
                .map(|value| value.encode_wide().collect::<Vec<_>>())
                .collect::<Vec<_>>()
        )
    );
    assert_eq!(
        result["directory"],
        serde_json::json!(
            std::path::absolute(&directory.0)
                .unwrap()
                .as_os_str()
                .encode_wide()
                .collect::<Vec<_>>()
        )
    );
    let mut owner = fixture("environment-owner");
    owner
        .env(BASE, &directory.0)
        .env("SCREENSHOTS_ATOMIC_REMOVED", "synthetic")
        .env("SCREENSHOTS_ATOMIC_OVERRIDE", "parent")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    assert!(owner.status().unwrap().success());
    let result: serde_json::Value =
        serde_json::from_str(&wait_file(&directory.0.join("environment"))).unwrap();
    assert!(result["removed"].is_null());
    assert_eq!(result["override"], "child");
    let mut command = fixture("metadata");
    command
        .env_clear()
        .env(MODE, "metadata")
        .env("DOTNET_STARTUP_HOOKS", "synthetic")
        .env_remove("DOTNET_STARTUP_HOOKS");
    let container = job(None, None).unwrap();
    let mut child = spawn(
        &command,
        container.as_handle(),
        Streams::PIPED,
        Environment::Explicit,
        &[],
    )
    .unwrap();
    child.stdin.take();
    child
        .resume(Instant::now() + Duration::from_secs(5))
        .unwrap();
    let result: serde_json::Value = serde_json::from_slice(&binary(&mut child)).unwrap();
    assert!(result["path"].is_null());
    assert_eq!(result["dotnet"], false);
}

#[test]
fn job_limits_are_installed_and_reject_extra_process_before_user_code() {
    let container = job(Some(1), Some(256 * 1024 * 1024)).unwrap();
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    assert_ne!(
        unsafe {
            QueryInformationJobObject(
                container.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                std::ptr::from_mut(&mut limits).cast(),
                size_of_val(&limits) as u32,
                null_mut(),
            )
        },
        0
    );
    assert_eq!(limits.BasicLimitInformation.ActiveProcessLimit, 1);
    assert_eq!(limits.ProcessMemoryLimit, 256 * 1024 * 1024);
    assert_ne!(
        limits.BasicLimitInformation.LimitFlags & JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        0
    );
    let command = fixture("hold");
    let first = spawn(
        &command,
        container.as_handle(),
        Streams::NULL,
        Environment::Inherit,
        &[],
    )
    .unwrap();
    assert!(
        spawn(
            &command,
            container.as_handle(),
            Streams::NULL,
            Environment::Inherit,
            &[]
        )
        .is_err()
    );
    drop(first);
}

#[test]
fn invalid_job_image_and_command_bounds_fail_before_user_code() {
    let container = job(None, None).unwrap();
    let directory = Directory::new();
    let event = unsafe { OwnedHandle::from_raw_handle(CreateEventW(null(), 1, 0, null())) };
    let mut command = fixture("hold");
    command.env(BASE, &directory.0);
    assert!(
        spawn(
            &command,
            event.as_handle(),
            Streams::NULL,
            Environment::Inherit,
            &[]
        )
        .is_err()
    );
    assert!(!directory.0.join("started").exists());
    assert!(
        spawn(
            &Command::new(directory.0.join("missing.exe")),
            container.as_handle(),
            Streams::NULL,
            Environment::Inherit,
            &[]
        )
        .is_err()
    );
    let mut oversized = fixture("hold");
    oversized.arg("x".repeat(COMMAND_UNITS));
    assert!(
        spawn(
            &oversized,
            container.as_handle(),
            Streams::NULL,
            Environment::Inherit,
            &[]
        )
        .is_err()
    );
    let mut nul = fixture("hold");
    nul.arg(OsString::from_wide(&[b'a' as u16, 0, b'b' as u16]));
    assert!(
        spawn(
            &nul,
            container.as_handle(),
            Streams::NULL,
            Environment::Inherit,
            &[]
        )
        .is_err()
    );
}

#[test]
fn installed_engine_lookup_uses_the_explicit_child_path() {
    let directory = Directory::new();
    let name = "screenshots-synthetic-path-engine.exe";
    std::fs::copy(std::env::current_exe().unwrap(), directory.0.join(name)).unwrap();
    let fixture = fixture("metadata");
    let mut command = Command::new(name);
    command
        .args(fixture.get_args())
        .env(MODE, "metadata")
        .env("Path", &directory.0);
    let result: serde_json::Value = serde_json::from_slice(&run(&command)).unwrap();
    assert_eq!(
        result["path"],
        serde_json::json!(directory.0.as_os_str().encode_wide().collect::<Vec<_>>())
    );
}

#[test]
fn bare_leading_dot_engine_matches_std_even_with_an_exe_sibling() {
    let directory = Directory::new();
    let name = ".screenshots-leading-dot-engine";
    std::fs::copy(std::env::current_exe().unwrap(), directory.0.join(name)).unwrap();
    std::fs::write(
        directory.0.join(format!("{name}.exe")),
        b"must not execute this sibling",
    )
    .unwrap();
    let fixture = fixture("resolver-oracle");
    let mut command = Command::new(name);
    command
        .args(fixture.get_args())
        .env(MODE, "resolver-oracle")
        .env("Path", &directory.0);
    let expected = command.output().unwrap();
    assert!(
        expected.status.success(),
        "std failed: {}",
        String::from_utf8_lossy(&expected.stderr)
    );
    let start = expected
        .stdout
        .windows(MAGIC.len())
        .position(|bytes| bytes == MAGIC)
        .unwrap()
        + MAGIC.len();
    let expected: serde_json::Value = serde_json::from_slice(&expected.stdout[start..]).unwrap();
    let actual: serde_json::Value = serde_json::from_slice(&run(&command)).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn native_and_batch_dispatch_preserve_unpaired_utf16_arguments() {
    let directory = Directory::new();
    let value = OsString::from_wide(&[0xd800, 0x61, 0xdc00]);
    let shell = system_directory(false).unwrap().join("cmd.exe");
    let collect = |command: &Command| {
        let container = job(None, None).unwrap();
        let mut child = spawn(
            command,
            container.as_handle(),
            Streams::OUTPUT,
            Environment::Inherit,
            &[],
        )
        .unwrap();
        child
            .resume(Instant::now() + Duration::from_secs(5))
            .unwrap();
        let mut bytes = Vec::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        (child.wait().unwrap(), bytes)
    };
    let mut command = Command::new(&shell);
    command.args(["/u", "/d", "/c", "echo"]).arg(&value);
    let reference = command.output().unwrap();
    assert!(reference.status.success());
    let (status, output) = collect(&command);
    assert!(status.success());
    assert_eq!(output, reference.stdout);
    let expected = [0xd800u16, 0x61, 0xdc00, b'\r' as u16, b'\n' as u16]
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    assert_eq!(reference.stdout, expected);
    // The batch script's /u shell switch keeps its echo output in raw UTF-16,
    // avoiding libtest's UTF-8 argument parser as an unrelated test fixture limit.
    let script = directory.0.join("utf16.cmd");
    std::fs::write(&script, b"@echo off\r\ncmd.exe /u /d /c echo %~1\r\n").unwrap();
    let mut command = Command::new(script);
    command.arg(&value);
    let reference = command.output().unwrap();
    let (status, output) = collect(&command);
    assert_eq!(status.code(), reference.status.code());
    assert_eq!(output, reference.stdout);
    if status.success() {
        assert_eq!(reference.stdout, expected);
    }
}

#[test]
fn batch_dispatch_matches_stable_std_and_does_not_execute_argument_metacharacters() {
    let directory = Directory::new();
    let executable = std::env::current_exe().unwrap();
    let executable = batch::user_path(&executable).unwrap();
    let executable = String::from_utf16(&executable[..executable.len() - 1]).unwrap();
    let injection = directory.0.join("injected");
    let values = [
        OsString::from(""),
        OsString::from("space value"),
        OsString::from("trailing\\"),
        OsString::from("quote\"inside"),
        OsString::from("emoji 🦊"),
        OsString::from("%SCREENSHOTS_ATOMIC_SECRET%"),
        OsString::from("!literal!^caret|pipe<left>right(parens)"),
        OsString::from(format!("literal & echo executed > {}", injection.display())),
    ];
    for extension in ["bat", "CmD"] {
        let script = directory.0.join(format!("synthetic script.{extension}"));
        std::fs::write(&script,format!("@echo off\r\n\"{executable}\" --exact windows_child::tests::child_fixture --ignored --nocapture --quiet -- %*\r\n")).unwrap();
        let reference = directory.0.join("std.json");
        let actual = directory.0.join("native.json");
        let configure = |path: &Path| {
            let mut command = Command::new(&script);
            command
                .args(&values)
                .env(MODE, "batch-dump")
                .env("SCREENSHOTS_ATOMIC_BATCH_OUTPUT", path)
                .env("SCREENSHOTS_ATOMIC_SECRET", "SYNTHETIC_NOT_FOR_EXPANSION")
                .current_dir(&directory.0);
            command
        };
        assert!(configure(&reference).output().unwrap().status.success());
        let container = job(None, None).unwrap();
        let mut child = spawn(
            &configure(&actual),
            container.as_handle(),
            Streams::NULL,
            Environment::Inherit,
            &[],
        )
        .unwrap();
        child
            .resume(Instant::now() + Duration::from_secs(5))
            .unwrap();
        assert!(child.wait().unwrap().success());
        let reference: Vec<Vec<u16>> =
            serde_json::from_slice(&std::fs::read(&reference).unwrap()).unwrap();
        let actual: Vec<Vec<u16>> =
            serde_json::from_slice(&std::fs::read(&actual).unwrap()).unwrap();
        assert_eq!(
            actual, reference,
            "batch dispatch must preserve stable std semantics"
        );
        assert!(
            !injection.exists(),
            "argument metacharacters executed a command"
        );
        assert!(!actual.iter().any(|value| {
            value
                .windows("SYNTHETIC_NOT_FOR_EXPANSION".len())
                .any(|part| {
                    part.iter()
                        .copied()
                        .eq("SYNTHETIC_NOT_FOR_EXPANSION".encode_utf16())
                })
        }));
        for newline in ["line\nbreak", "line\rbreak"] {
            let mut command = Command::new(&script);
            command.arg(newline);
            assert!(command.output().is_err());
            assert!(
                spawn(
                    &command,
                    container.as_handle(),
                    Streams::NULL,
                    Environment::Inherit,
                    &[]
                )
                .is_err()
            );
        }
    }
}

#[test]
fn prepared_batch_identity_is_checked_again_before_suspended_start() {
    let directory = Directory::new();
    let script = directory.0.join("scheduled.cmd");
    let marker = directory.0.join("marker");
    let body = |label: &str| format!("@echo off\r\necho {label}>\"{}\"\r\n", marker.display());
    std::fs::write(&script, body("original")).unwrap();
    let (_, identity) = crate::FileIdentity::open(&script).unwrap();
    let command = Command::new(&script);
    let container = job(None, None).unwrap();
    let mut child = spawn_with_identity(
        &command,
        container.as_handle(),
        Streams::OUTPUT,
        Environment::Inherit,
        &[],
        Some(&identity),
    )
    .unwrap();
    assert!(!marker.exists());
    std::fs::rename(&script, directory.0.join("prior-command.cmd")).unwrap();
    std::fs::write(&script, body("injected")).unwrap();
    assert!(
        child
            .resume(Instant::now() + Duration::from_secs(5))
            .is_err()
    );
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(!marker.exists());
}
