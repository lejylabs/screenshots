//! Compiled only with --cfg screenshots_native_drop_acceptance in an owned CI build.
//! No ordinary build contains the entry, DLL loader, probes or environment forwarding.
use super::{
    native_drop_broker::NativeDropBroker, native_drop_events as event,
    native_drop_process as process, native_drop_wire as wire,
};
use anyhow::{Result, ensure};
use std::{
    ffi::{OsStr, c_void},
    fs::File,
    io::Write,
    os::windows::{
        ffi::OsStrExt,
        io::{AsHandle, AsRawHandle},
    },
    process::Command,
    sync::{OnceLock, mpsc},
    thread,
    time::Duration,
};
use windows::{
    Win32::{
        Foundation::*,
        System::{
            JobObjects::TerminateJobObject, LibraryLoader::*, StationsAndDesktops::*, Threading::*,
        },
        UI::WindowsAndMessaging::*,
    },
    core::*,
};

const ENVIRONMENT: [&str; 5] = [
    "SCREENSHOTS_DROP_ACCEPTANCE_CONTEXT",
    "SCREENSHOTS_DROP_ACCEPTANCE_DLL",
    "SCREENSHOTS_DROP_ACCEPTANCE_SOURCE",
    "SCREENSHOTS_DROP_ACCEPTANCE_CASE",
    "SCREENSHOTS_DROP_ACCEPTANCE_CONTROL",
];
fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}
fn value(name: &str) -> Result<Vec<u16>> {
    Ok(wide(&std::env::var_os(name).ok_or_else(|| {
        anyhow::anyhow!("Missing explicit acceptance setting")
    })?))
}
pub(super) fn decorate(command: &mut Command) {
    for name in ENVIRONMENT {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    if std::env::var(ENVIRONMENT[3]).is_ok_and(|case| case.starts_with("ci-")) {
        for name in [
            "GITHUB_ACTIONS",
            "RUNNER_OS",
            "RUNNER_ENVIRONMENT",
            "SCREENSHOTS_OWNED_HOSTED_CI_INPUT",
        ] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
    }
}
fn symbol(module: HMODULE, name: PCSTR) -> unsafe extern "system" fn() -> isize {
    unsafe { GetProcAddress(module, name) }.expect("owned acceptance DLL export")
}
static MODULE: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);
fn library() -> Result<HMODULE> {
    let mut module = MODULE.lock().unwrap();
    if let Some(module) = *module {
        return Ok(HMODULE(module as _));
    }
    let dll = std::env::var_os(ENVIRONMENT[1])
        .ok_or_else(|| anyhow::anyhow!("Missing acceptance DLL"))?;
    ensure!(
        std::path::Path::new(&dll).is_absolute(),
        "Acceptance DLL must be absolute"
    );
    let loaded = unsafe { LoadLibraryW(PCWSTR(wide(&dll).as_ptr())) }?;
    *module = Some(loaded.0 as usize);
    Ok(loaded)
}

type Hook = unsafe extern "C" fn(u32, u64, *mut c_void);
static HOOK: OnceLock<Option<Hook>> = OnceLock::new();
pub(super) fn hook(code: u32, sequence: u64, pointer: *mut c_void) {
    if let Some(hook) = HOOK.get_or_init(|| {
        if std::env::var_os(ENVIRONMENT[0]).is_none() {
            return None;
        }
        let module = library().expect("explicit owned fixture DLL");
        let attach: unsafe extern "C" fn(*const u16, *const u16) =
            unsafe { std::mem::transmute(symbol(module, s!("acceptance_attach"))) };
        unsafe {
            attach(
                value(ENVIRONMENT[0]).unwrap().as_ptr(),
                value(ENVIRONMENT[3]).unwrap().as_ptr(),
            );
        }
        Some(unsafe { std::mem::transmute(symbol(module, s!("acceptance_hook"))) })
    }) {
        unsafe {
            hook(code, sequence, pointer);
        }
    }
}

/// Must run before COM, GPUI, logging, account/config or single-instance startup.
/// The external owned-process watchdog is mandatory; a stuck assertion cannot escape it.
pub fn run(case: &str) -> Result<()> {
    ensure!(
        std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
            && std::env::var("RUNNER_OS").as_deref() == Ok("Windows")
            && std::env::var("RUNNER_ENVIRONMENT").as_deref() == Ok("github-hosted")
            && std::env::var("SCREENSHOTS_OWNED_PRODUCTION_ACCEPTANCE").as_deref() == Ok("1"),
        "Owned hosted Windows CI opt-in is required"
    );
    let _desktop = PrivateDesktop::new(case.starts_with("ci-"), false)?;
    let window = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("STATIC"),
            w!("Owned synthetic drop target"),
            WS_POPUP | WS_VISIBLE,
            0,
            0,
            128,
            128,
            None,
            None,
            None,
            None,
        )
    }?;
    let result = if case.starts_with("protocol-") {
        protocol(case, window)
    } else {
        provider(case, window)
    };
    if unsafe { IsWindow(Some(window)) }.as_bool() {
        unsafe { DestroyWindow(window) }?;
    }
    result?;
    println!(
        "{{\"acceptance_case\":\"{case}\",\"passed\":true,\"owned_synthetic_input\":{},\"user_desktop\":false,\"user_clipboard\":false,\"private_owned_desktop\":true}}",
        case.starts_with("ci-")
    );
    Ok(())
}
struct PrivateDesktop {
    old_input: Option<HDESK>,
}
impl PrivateDesktop {
    fn new(owned_input: bool, graphical: bool) -> Result<Self> {
        let expected = std::env::var("SCREENSHOTS_DROP_ACCEPTANCE_DESKTOP").map_err(|_| {
            anyhow::anyhow!("Outer owned controller must launch the app on its startup desktop")
        })?;
        let owner = std::env::var("SCREENSHOTS_DROP_ACCEPTANCE_DESKTOP_OWNER")?.parse::<u32>()?;
        let station = unsafe { GetProcessWindowStation() }?;
        let desktop = unsafe { GetThreadDesktop(GetCurrentThreadId()) }?;
        let name = |handle: HANDLE| -> Result<String> {
            let mut text = [0u16; 256];
            let mut actual = 0;
            unsafe {
                GetUserObjectInformationW(
                    handle,
                    UOI_NAME,
                    Some(text.as_mut_ptr().cast()),
                    (text.len() * 2) as u32,
                    Some(&mut actual),
                )
            }?;
            let end = text
                .iter()
                .position(|unit| *unit == 0)
                .ok_or_else(|| anyhow::anyhow!("Owned desktop identity overflow"))?;
            Ok(String::from_utf16(&text[..end])?)
        };
        let actual = format!("{}\\{}", name(HANDLE(station.0))?, name(HANDLE(desktop.0))?);
        ensure!(
            actual == expected,
            "Startup desktop does not match exact owned controller"
        );
        let required = if owned_input || graphical {
            format!("WinSta0\\ScreenshotsDropProbe-{owner}")
        } else {
            format!("ScreenshotsProductionAcceptance-{owner}\\owned")
        };
        ensure!(actual == required, "Startup desktop is not fixture-owned");
        let old_input = if owned_input {
            ensure!(
                std::env::var("SCREENSHOTS_OWNED_HOSTED_CI_INPUT").as_deref() == Ok("1"),
                "Separate owned hosted input opt-in required"
            );
            let original = unsafe {
                OpenInputDesktop(
                    DESKTOP_CONTROL_FLAGS(0),
                    false,
                    DESKTOP_ACCESS_FLAGS(
                        DESKTOP_SWITCHDESKTOP.0 | DESKTOP_READOBJECTS.0 | DESKTOP_WRITEOBJECTS.0,
                    ),
                )
            }?;
            unsafe { SwitchDesktop(desktop) }?;
            Some(original)
        } else {
            None
        };
        Ok(Self { old_input })
    }
}
impl Drop for PrivateDesktop {
    fn drop(&mut self) {
        if let Some(input) = self.old_input {
            unsafe {
                SwitchDesktop(input).expect("restore owned CI input desktop");
                CloseDesktop(input).expect("close original input guard");
            }
        }
    }
}

struct Session {
    child: process::NativeDropProcess,
    command: std::os::windows::io::OwnedHandle,
    permit: std::os::windows::io::OwnedHandle,
    packets: Option<mpsc::Receiver<Result<wire::Packet>>>,
    reader: Option<thread::JoinHandle<()>>,
    sequence: u64,
}
impl Session {
    fn new() -> Result<Self> {
        let command = event::event(true)?;
        let permit = event::event(true)?;
        let mut invocation = Command::new(std::env::current_exe()?);
        invocation
            .arg(process::CHILD_ARGUMENT)
            .arg((command.as_raw_handle() as usize).to_string())
            .arg((permit.as_raw_handle() as usize).to_string())
            .env_clear();
        for name in ["SystemRoot", "WINDIR", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(name) {
                invocation.env(name, value);
            }
        }
        let child = process::spawn(&mut invocation, &[command.as_handle(), permit.as_handle()])?;
        let mut input = child.read.try_clone()?;
        let (tx, packets) = mpsc::sync_channel(1);
        let reader = thread::Builder::new()
            .name("acceptance-owned-reader".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                loop {
                    let packet = wire::read(&mut input);
                    let failed = packet.is_err();
                    if tx.send(packet).is_err() || failed {
                        break;
                    }
                }
            })?;
        Ok(Self {
            child,
            command,
            permit,
            packets: Some(packets),
            reader: Some(reader),
            sequence: 0,
        })
    }
    fn receive(&self, kind: u32) -> Result<wire::Packet> {
        let packet = self
            .packets
            .as_ref()
            .unwrap()
            .recv_timeout(Duration::from_secs(7))??;
        ensure!(
            packet.header.kind == kind,
            "Unexpected actual receiver packet"
        );
        Ok(packet)
    }
    fn permit(&self) -> Result<()> {
        event::signal(&self.permit)
    }
    fn begin(&mut self) -> Result<()> {
        wire::write(
            &mut self.child.write,
            wire::Header {
                kind: wire::HELLO,
                ..Default::default()
            },
            &self.child.nonce,
        )?;
        ensure!(
            self.receive(wire::ACK)?.bytes == self.child.nonce,
            "Actual child authentication"
        );
        ensure!(
            self.receive(30)?.header.reader == 1,
            "Actual child startup ARM"
        );
        self.permit()?;
        self.receive(32)?;
        self.receive(31)?;
        self.permit()
    }
    fn command(
        &mut self,
        kind: u32,
        window: u64,
        bytes: &[u8],
        response: Option<u32>,
    ) -> Result<()> {
        self.sequence += 1;
        wire::write(
            &mut self.child.write,
            wire::Header {
                kind,
                window,
                version: 1,
                reader: self.sequence,
                ..Default::default()
            },
            bytes,
        )?;
        event::signal(&self.command)?;
        self.receive(30)?;
        self.permit()?;
        if let Some(kind) = response {
            self.receive(kind)?;
        }
        let done = self.receive(31)?;
        ensure!(
            done.header.generation == self.sequence,
            "Command completed by exact sequence"
        );
        self.permit()
    }
    fn exited(&self, expected: Option<u32>, timeout: u32) -> Result<()> {
        ensure!(
            unsafe { WaitForSingleObject(event::raw(&self.child.process), timeout) }
                == WAIT_OBJECT_0,
            "Actual process not reaped before deadline"
        );
        let mut code = 0;
        unsafe { GetExitCodeProcess(event::raw(&self.child.process), &mut code) }?;
        if let Some(expected) = expected {
            ensure!(code == expected, "Unexpected actual helper exit code");
        }
        Ok(())
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        unsafe {
            let _ = TerminateJobObject(event::raw(&self.child.job), 70);
        }
        self.exited(None, 2000).expect("owned helper cleanup");
        self.packets.take();
        self.reader
            .take()
            .unwrap()
            .join()
            .expect("owned reader joined");
    }
}
fn protocol(case: &str, hwnd: HWND) -> Result<()> {
    if case == "protocol-broken-response-pipe" {
        let command = event::event(true)?;
        let permit = event::event(true)?;
        let mut invocation = Command::new(std::env::current_exe()?);
        invocation
            .arg(process::CHILD_ARGUMENT)
            .arg((command.as_raw_handle() as usize).to_string())
            .arg((permit.as_raw_handle() as usize).to_string())
            .env_clear();
        for name in ["SystemRoot", "WINDIR", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(name) {
                invocation.env(name, value);
            }
        }
        let mut child =
            process::spawn(&mut invocation, &[command.as_handle(), permit.as_handle()])?;
        drop(std::mem::replace(&mut child.read, File::open("NUL")?));
        wire::write(
            &mut child.write,
            wire::Header {
                kind: wire::HELLO,
                ..Default::default()
            },
            &child.nonce,
        )?;
        ensure!(
            unsafe { WaitForSingleObject(event::raw(&child.process), 7000) } == WAIT_OBJECT_0,
            "Broken response helper was not reaped"
        );
        let mut code = 0;
        unsafe { GetExitCodeProcess(event::raw(&child.process), &mut code) }?;
        ensure!(code == 1, "Broken response pipe did not fail private entry");
        return Ok(());
    }
    let mut session = Session::new()?;
    if case == "protocol-bad-auth" {
        let mut nonce = session.child.nonce;
        nonce[0] ^= 1;
        wire::write(
            &mut session.child.write,
            wire::Header {
                kind: wire::HELLO,
                ..Default::default()
            },
            &nonce,
        )?;
        session.exited(Some(1), 2000)?;
        return Ok(());
    }
    if case == "protocol-withheld-startup-permit" {
        wire::write(
            &mut session.child.write,
            wire::Header {
                kind: wire::HELLO,
                ..Default::default()
            },
            &session.child.nonce,
        )?;
        session.receive(wire::ACK)?;
        session.receive(30)?;
        session.exited(Some(1), 7000)?;
        return Ok(());
    }
    session.begin()?;
    if case == "protocol-parent-death" {
        println!("{{\"owned_helper_ready\":true,\"pid\":{}}}", unsafe {
            GetProcessId(event::raw(&session.child.process))
        });
        std::io::stdout().flush()?;
        loop {
            std::thread::park();
        }
    }
    if case == "protocol-register-remove-stop" {
        unsafe {
            SetPropW(
                hwnd,
                super::native_drop_receiver::PROPERTY,
                Some(HANDLE(41usize as _)),
            )
        }?;
        session.command(
            wire::REGISTER,
            41,
            &(hwnd.0 as usize as u64).to_le_bytes(),
            Some(wire::REGISTERED),
        )?;
        session.command(wire::REMOVE, 41, &[], Some(wire::REMOVED))?;
        session.command(wire::STOP, 0, &[], None)?;
        session.exited(Some(0), 2000)?;
        unsafe { RemovePropW(hwnd, super::native_drop_receiver::PROPERTY) }?;
    } else if case == "protocol-job-kill-reap" {
        unsafe { TerminateJobObject(event::raw(&session.child.job), 97) }?;
        session.exited(Some(97), 2000)?;
    } else if case == "protocol-broken-command-pipe" {
        drop(std::mem::replace(
            &mut session.child.write,
            File::options().write(true).open("NUL")?,
        ));
        event::signal(&session.command)?;
        session.receive(30)?;
        session.permit()?;
        session.exited(Some(70), 2000)?;
    } else {
        let mut raw = [0u8; 64];
        raw[..8].copy_from_slice(b"SCODR001");
        raw[8..12].copy_from_slice(&wire::STOP.to_le_bytes());
        raw[32..40].copy_from_slice(&1u64.to_le_bytes());
        match case {
            "protocol-bad-magic" => raw[0] ^= 1,
            "protocol-reserved-bits" => raw[63] = 1,
            "protocol-overflow-length" => raw[56..60].copy_from_slice(&u32::MAX.to_le_bytes()),
            "protocol-invalid-kind" => raw[8..12].copy_from_slice(&u32::MAX.to_le_bytes()),
            "protocol-skipped-sequence" => raw[32..40].copy_from_slice(&2u64.to_le_bytes()),
            "protocol-truncated-body" => {
                raw[8..12].copy_from_slice(&wire::REGISTER.to_le_bytes());
                raw[56..60].copy_from_slice(&8u32.to_le_bytes());
            }
            _ => anyhow::bail!("Unknown explicit acceptance case"),
        }
        session.child.write.write_all(&raw)?;
        if case == "protocol-truncated-body" {
            session.child.write.write_all(&[0; 4])?;
        }
        session.child.write.flush()?;
        if case == "protocol-truncated-body" {
            drop(std::mem::replace(
                &mut session.child.write,
                File::options().write(true).open("NUL")?,
            ));
        }
        event::signal(&session.command)?;
        session.receive(30)?;
        session.permit()?;
        session.exited(Some(70), 2000)?;
    }
    Ok(())
}
fn provider(case: &str, hwnd: HWND) -> Result<()> {
    let requested_case = case;
    let special_control = match case {
        "control-late-submit" => Some(("normal", "late-submit")),
        "control-noop-barrier" => Some(("query-reenter", "omit-barrier-reply")),
        "ordered-enter-submit" => Some(("normal", "ordered-enter-submit")),
        "ordered-replacement-enter-submit" => Some(("a-close-b-live", "ordered-replacement")),
        "ordered-failure-publish" => Some(("helper-drop-stall", "delay-failure-publish")),
        "control-early-failure-ack" => Some(("helper-drop-stall", "early-failure-ack")),
        "control-wrong-terminal" => Some(("normal", "wrong-terminal")),
        "control-failed-as-leave-source-death" => Some(("source-death", "failed-as-leave")),
        "control-failed-as-leave-unmarshal" => Some(("custom-unmarshal-memory", "failed-as-leave")),
        "control-helper-unknown" => Some(("normal", "helper-unknown")),
        "control-source-error" => Some(("normal", "mutate-source-error")),
        "control-source-none" => Some(("normal", "mutate-source-none")),
        "control-duplicate-acquire" => Some(("normal", "mutate-duplicate-get")),
        "control-duplicate-release" => Some(("normal", "mutate-duplicate-release")),
        "control-busy-copy" => Some(("withheld-body-256", "mutate-busy-copy")),
        "control-busy-acquire" => Some(("withheld-body-256", "mutate-busy-get")),
        "control-custom-source-error" => Some(("custom-normal", "mutate-source-error")),
        "ci-control-source-error" => Some(("ci-ole-normal", "mutate-ole-source-error")),
        "ci-control-source-none" => Some(("ci-ole-normal", "mutate-ole-source-none")),
        "ci-control-duplicate-acquire" => Some(("ci-ole-normal", "mutate-duplicate-get")),
        _ => None,
    };
    let case = special_control.map_or_else(
        || case.strip_prefix("noop-").unwrap_or(case),
        |(scenario, _)| scenario,
    );
    if let Some((_, control)) = special_control {
        unsafe {
            std::env::set_var(ENVIRONMENT[4], control);
        }
    }
    let rejection_control = matches!(
        requested_case,
        "control-late-submit" | "control-noop-barrier"
    );
    let ordered_first = requested_case == "ordered-enter-submit";
    let ordered_replacement = requested_case == "ordered-replacement-enter-submit";
    let failure_ordering = matches!(
        requested_case,
        "ordered-failure-publish" | "control-early-failure-ack"
    );
    let allowed = [
        "normal",
        "shell-private-normal",
        "ci-ole-shell-private-normal",
        "custom-normal",
        "query-stall",
        "get-stall",
        "release-stall",
        "over-memory",
        "custom-unmarshal-stall",
        "custom-unmarshal-memory",
        "cancel-pending-body",
        "a-close-b-live",
        "same-hwnd-reclaim",
        "withheld-body-256",
        "hwnd-reuse",
        "image-normal",
        "url-normal",
        "query-reenter",
        "get-reenter",
        "medium-reenter",
        "helper-factory-stall",
        "helper-enter-stall",
        "helper-drop-stall",
        "helper-release-stall",
        "helper-enter-reenter",
        "helper-drop-reenter",
        "helper-release-reenter",
        "error-release-stall",
        "error-release-reenter",
        "source-death",
        "startup-stall",
        "register-stall",
        "revoke-stall",
        "uninitialize-stall",
        "ci-ole-normal",
        "ci-ole-query-stall",
        "ci-ole-get-stall",
        "ci-ole-release-stall",
        "ci-ole-over-memory",
        "ci-custom-ole-normal",
        "ci-custom-ole-unmarshal-stall",
        "ci-custom-ole-unmarshal-memory",
    ];
    ensure!(allowed.contains(&case), "Unknown provider acceptance case");
    let module = library()?;
    let begin: unsafe extern "C" fn(*const u16, *const u16, *const u16, HWND) =
        unsafe { std::mem::transmute(symbol(module, s!("acceptance_begin"))) };
    let drive: unsafe extern "C" fn() -> i32 =
        unsafe { std::mem::transmute(symbol(module, s!("acceptance_drive"))) };
    let end: unsafe extern "C" fn() =
        unsafe { std::mem::transmute(symbol(module, s!("acceptance_end"))) };
    let context = format!(
        "Local\\ScreenshotsProductionAcceptance-{}",
        std::process::id()
    );
    // No other threads exist yet; these exact values are forwarded only in this cfg build.
    unsafe {
        std::env::set_var(ENVIRONMENT[0], &context);
        std::env::set_var(
            ENVIRONMENT[3],
            if ordered_first {
                "ordered-enter-submit"
            } else if requested_case == "control-wrong-terminal" {
                "terminal-leave"
            } else if [
                "cancel-pending-body",
                "a-close-b-live",
                "same-hwnd-reclaim",
                "withheld-body-256",
                "hwnd-reuse",
            ]
            .contains(&case)
            {
                "normal"
            } else {
                if special_control.is_some() {
                    case
                } else {
                    requested_case
                }
            },
        );
    }
    unsafe {
        begin(
            wide(OsStr::new(&context)).as_ptr(),
            value(ENVIRONMENT[2])?.as_ptr(),
            value(ENVIRONMENT[3])?.as_ptr(),
            hwnd,
        );
    }
    let mut expected_point = POINT { x: 48, y: 40 };
    unsafe { windows::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut expected_point) }.ok()?;
    let broker = NativeDropBroker::default();
    let mut registration = broker.register(hwnd.0 as usize, 1)?;
    let mut handle = registration.handle();
    let mut additional_window = None;
    ensure!(
        unsafe { drive() } == 0,
        "Actual Rust provider boundary failed"
    );
    if ordered_first {
        finish_ordered(module, &mut registration, &handle)?;
    }
    if failure_ordering {
        let until = std::time::Instant::now() + Duration::from_secs(7);
        while registration.acceptance_pending_kind() != Some(wire::ENTER) {
            pump_owner(&[event::raw(own_event(&PUBLICATION_EVENT))], until)?;
        }
        println!("{{\"actual_enter_published_before_fault\":true}}");
        let finish: unsafe extern "C" fn() -> i32 =
            unsafe { std::mem::transmute(symbol(module, s!("acceptance_finish_ordered_failure"))) };
        ensure!(
            unsafe { finish() } == 0,
            "Actual ordered Drop fault/deadline/reap failed"
        );
    }
    if !rejection_control
        && matches!(
            case,
            "normal"
                | "shell-private-normal"
                | "ci-ole-shell-private-normal"
                | "custom-normal"
                | "cancel-pending-body"
                | "a-close-b-live"
                | "same-hwnd-reclaim"
                | "withheld-body-256"
                | "hwnd-reuse"
                | "image-normal"
                | "url-normal"
                | "ci-ole-normal"
                | "ci-custom-ole-normal"
                | "revoke-stall"
                | "uninitialize-stall"
        )
    {
        if case == "cancel-pending-body" {
            let queued = wait_completion(&broker)?;
            ensure!(
                queued.acknowledged && queued.submits > 0,
                "Real pending body was not published before cancellation"
            );
            broker.cancel();
        }
        let packet = if case == "cancel-pending-body" {
            wait_delivery(&mut registration, &handle, 0)?
        } else {
            successful_body(&broker, &mut registration, &handle)?
        };
        if case == "cancel-pending-body" {
            ensure!(
                packet.packet.bytes.is_empty() && packet.packet.header.kind == wire::FAILED,
                "Canceled queued body reached foreground"
            );
        } else {
            ensure!(
                packet.packet.header.kind == wire::SUBMIT
                    && (if case.starts_with("ci-") {
                        (expected_point.x..=expected_point.x + 1).contains(&packet.packet.header.x)
                            && (expected_point.y..=expected_point.y + 1).contains(&packet.packet.header.y)
                    } else {
                        packet.packet.header.x == expected_point.x && packet.packet.header.y == expected_point.y
                    })
                    && handle.current(&packet),
                "Actual submitted final point/generation"
            );
            let decoded = wire::decode(wire::Packet {
                header: packet.packet.header,
                bytes: packet.packet.bytes.clone(),
            })?;
            match decoded {
                wire::Data::Files(paths) => ensure!(
                    paths.paths() == [std::path::PathBuf::from(r"C:\owned-fixture.png")],
                    "Exact native file path"
                ),
                wire::Data::Image(gpui::ImageFormat::Png, bytes) if case == "image-normal" => {
                    ensure!(
                        bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
                        "Actual bounded native PNG"
                    )
                }
                wire::Data::Url(url) if case == "url-normal" => ensure!(
                    &*url == "https://example.invalid/owned.png",
                    "Actual canonical URL"
                ),
                _ => anyhow::bail!("Wrong actual native payload kind"),
            }
        }
        if case == "withheld-body-256" {
            let busy: unsafe extern "C" fn() -> i32 =
                unsafe { std::mem::transmute(symbol(module, s!("acceptance_busy_drive"))) };
            for _ in 0..256 {
                ensure!(
                    unsafe { busy() } == 0 && handle.current(&packet),
                    "Withheld actual body admitted another acquisition"
                );
            }
        }
        if case == "hwnd-reuse" {
            wait_future(
                handle.close(),
                std::time::Instant::now() + Duration::from_secs(7),
            )??;
            drop(registration);
            unsafe { RemovePropW(hwnd, super::native_drop_receiver::PROPERTY) }?;
            let reaped: unsafe extern "C" fn() -> i32 =
                unsafe { std::mem::transmute(symbol(module, s!("acceptance_wait_reaped"))) };
            ensure!(
                unsafe { reaped() } == 0,
                "Old HWND helper must be reaped before destruction"
            );
            unsafe { DestroyWindow(hwnd) }?;
            let mut reused = None;
            for _ in 0..131072 {
                let candidate = unsafe {
                    CreateWindowExW(
                        WINDOW_EX_STYLE::default(),
                        w!("STATIC"),
                        w!("Owned recycled target"),
                        WS_POPUP,
                        0,
                        0,
                        128,
                        128,
                        None,
                        None,
                        None,
                        None,
                    )
                }?;
                if candidate == hwnd {
                    reused = Some(candidate);
                    break;
                }
                unsafe { DestroyWindow(candidate) }?;
            }
            let reused = reused.ok_or_else(|| {
                anyhow::anyhow!("Exact HWND value did not recycle within bounded owned fixture")
            })?;
            let rearm: unsafe extern "C" fn(HWND) =
                unsafe { std::mem::transmute(symbol(module, s!("acceptance_rearm"))) };
            unsafe {
                rearm(reused);
            }
            registration = broker.register(reused.0 as usize, 2)?;
            let replacement = registration.handle();
            ensure!(
                replacement.token() != handle.token()
                    && !replacement.current(&packet)
                    && !handle.current(&packet),
                "Old body was retagged onto recycled HWND"
            );
            handle = replacement;
            additional_window = Some(reused);
        }
        drop(packet);
    }
    if rejection_control
        || !matches!(
            case,
            "normal"
                | "shell-private-normal"
                | "ci-ole-shell-private-normal"
                | "custom-normal"
                | "cancel-pending-body"
                | "a-close-b-live"
                | "same-hwnd-reclaim"
                | "withheld-body-256"
                | "hwnd-reuse"
                | "image-normal"
                | "url-normal"
                | "ci-ole-normal"
                | "ci-custom-ole-normal"
                | "revoke-stall"
                | "uninitialize-stall"
        )
    {
        if matches!(case, "source-death" | "custom-unmarshal-memory") {
            // A failed argument unmarshal has no target callback to emit a wire terminal.
            // Complete the actual cancellation/failure-publication path before requiring FAILED.
            broker.cancel();
        }
        let proof = if failure_ordering {
            controlled_failure_completion(&broker, &registration)?
        } else {
            wait_completion(&broker)?
        };
        let effect: unsafe extern "C" fn() -> i32 =
            unsafe { std::mem::transmute(symbol(module, s!("acceptance_rejection_effect"))) };
        let effect_verified = unsafe { effect() } == 0;
        let terminal_evidence =
            matches!(proof.terminal, wire::LEAVE | wire::FAILED) || proof.reaps > 0;
        let rejected = proof.acknowledged
            && proof.submits == 0
            && proof.reader_submits == 0
            && effect_verified
            && terminal_evidence;
        println!(
            "{{\"completion_barrier\":true,\"barrier_acknowledged\":{},\"reader_reaped_barrier\":{},\"enters\":{},\"submits\":{},\"reader_submits\":{},\"terminal_kind\":{},\"rejection_verified\":{}}}",
            proof.acknowledged,
            proof.reaped,
            proof.enters,
            proof.submits,
            proof.reader_submits,
            proof.terminal,
            rejected
        );
        ensure!(rejected, "Completion/effect barrier rejected source oracle");
        let expected_terminal = if matches!(case, "source-death" | "custom-unmarshal-memory") {
            wire::FAILED
        } else {
            0
        };
        let packet = wait_delivery(&mut registration, &handle, expected_terminal)?;
        ensure!(
            (packet.packet.header.kind == expected_terminal
                || expected_terminal == 0
                    && matches!(packet.packet.header.kind, wire::LEAVE | wire::FAILED))
                && packet.packet.bytes.is_empty()
                && packet.packet.header.window == handle.token()
                && (packet.packet.header.generation > 0
                    || packet.packet.header.version >= proof.failure_version),
            "Completion did not deliver the current exact terminal rejection"
        );
        println!(
            "{{\"terminal_rejection_consumed\":true,\"terminal_kind\":{},\"terminal_generation\":{},\"terminal_version\":{}}}",
            packet.packet.header.kind,
            packet.packet.header.generation,
            packet.packet.header.version
        );
        drop(packet);
    }
    if case == "a-close-b-live" || case == "same-hwnd-reclaim" || case == "hwnd-reuse" {
        let rearm: unsafe extern "C" fn(HWND) =
            unsafe { std::mem::transmute(symbol(module, s!("acceptance_rearm"))) };
        if case == "a-close-b-live" {
            let other = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("Owned second target"),
                    WS_POPUP | WS_VISIBLE,
                    256,
                    0,
                    128,
                    128,
                    None,
                    None,
                    None,
                    None,
                )
            }?;
            unsafe {
                rearm(other);
            }
            let second = broker.register(other.0 as usize, 2)?;
            wait_future(
                handle.close(),
                std::time::Instant::now() + Duration::from_secs(7),
            )??;
            drop(registration);
            unsafe { RemovePropW(hwnd, super::native_drop_receiver::PROPERTY) }?;
            registration = second;
            handle = registration.handle();
            additional_window = Some(other);
        } else if case == "same-hwnd-reclaim" {
            unsafe {
                rearm(hwnd);
                let probe: unsafe extern "C" fn() =
                    std::mem::transmute(symbol(module, s!("acceptance_probe_reclaim")));
                probe();
            }
            broker.cancel();
        }
        if ordered_replacement {
            let next: unsafe extern "C" fn() =
                unsafe { std::mem::transmute(symbol(module, s!("acceptance_order_next"))) };
            unsafe {
                next();
            }
        }
        ensure!(
            unsafe { drive() } == 0,
            "Actual remaining/reclaimed target failed"
        );
        if ordered_replacement {
            finish_ordered(module, &mut registration, &handle)?;
        }
        let packet = successful_body(&broker, &mut registration, &handle)?;
        ensure!(
            packet.packet.header.kind == wire::SUBMIT && handle.current(&packet),
            "Old A/window generation affected live target B/reclaim"
        );
        drop(packet);
    }
    let teardown = matches!(case, "revoke-stall" | "uninitialize-stall");
    if teardown {
        let begin: unsafe extern "C" fn() -> i32 =
            unsafe { std::mem::transmute(symbol(module, s!("acceptance_begin_teardown"))) };
        ensure!(
            unsafe { begin() } == 0,
            "Initial delivery was not complete/alive before teardown"
        );
    }
    wait_future(
        handle.close(),
        std::time::Instant::now() + Duration::from_secs(7),
    )??;
    drop(registration);
    join_controllers()?;
    if teardown {
        let finish: unsafe extern "C" fn() -> i32 =
            unsafe { std::mem::transmute(symbol(module, s!("acceptance_finish_teardown"))) };
        ensure!(
            unsafe { finish() } == 0,
            "Post-close broker deadline/injection/reap proof failed"
        );
    }
    let final_hwnd = additional_window.unwrap_or(hwnd);
    unsafe { RemovePropW(final_hwnd, super::native_drop_receiver::PROPERTY) }?;
    if additional_window.is_some() {
        unsafe { DestroyWindow(final_hwnd) }?;
    }
    let reaped: unsafe extern "C" fn() -> i32 =
        unsafe { std::mem::transmute(symbol(module, s!("acceptance_wait_reaped"))) };
    ensure!(
        unsafe { reaped() } == 0,
        "Exact normal/fault receiver not reaped"
    );
    join_controllers()?;
    unsafe {
        end();
    }
    Ok(())
}
pub(super) fn error_reference() {
    if std::env::var_os(ENVIRONMENT[0]).is_none() {
        return;
    }
    let module = library().expect("explicit acceptance DLL");
    let function: unsafe extern "C" fn() =
        unsafe { std::mem::transmute(symbol(module, s!("acceptance_error"))) };
    unsafe {
        function();
    }
}
pub(super) fn helper() -> Result<Option<windows::Win32::UI::Shell::IDropTargetHelper>> {
    if std::env::var_os(ENVIRONMENT[0]).is_none() {
        return Ok(None);
    }
    let module = library()?;
    let function: unsafe extern "C" fn(*mut *mut c_void) -> HRESULT =
        unsafe { std::mem::transmute(symbol(module, s!("acceptance_helper"))) };
    let mut pointer = std::ptr::null_mut();
    let result = unsafe { function(&mut pointer) };
    if result == S_FALSE {
        return Ok(None);
    }
    result.ok()?;
    ensure!(!pointer.is_null(), "Owned helper interface is missing");
    Ok(Some(unsafe {
        windows::Win32::UI::Shell::IDropTargetHelper::from_raw(pointer)
    }))
}
/// Keeps the private station/desktop alive while an actual GPUI acceptance loop runs.
pub fn run_ui(case: &str, work: impl FnOnce() -> Result<()>) -> Result<()> {
    ensure!(
        std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
            && std::env::var("RUNNER_OS").as_deref() == Ok("Windows")
            && std::env::var("RUNNER_ENVIRONMENT").as_deref() == Ok("github-hosted")
            && std::env::var("SCREENSHOTS_OWNED_PRODUCTION_ACCEPTANCE").as_deref() == Ok("1"),
        "Owned hosted Windows CI opt-in is required"
    );
    let _desktop = PrivateDesktop::new(case.starts_with("gpui-ole-"), true)?;
    let module = library()?;
    let context = format!(
        "Local\\ScreenshotsProductionAcceptance-{}",
        std::process::id()
    );
    let selected = match case {
        "gpui-ole-image" => "ci-ole-image-normal",
        "gpui-image" | "gpui-busy" | "gpui-cancel" | "gpui-hide-show" => "image-normal",
        "gpui-url" => "url-normal",
        "gpui-file" => "normal",
        _ => anyhow::bail!("Unknown actual GPUI acceptance case"),
    };
    unsafe {
        std::env::set_var(ENVIRONMENT[0], &context);
        std::env::set_var(ENVIRONMENT[3], selected);
    }
    let begin: unsafe extern "C" fn(*const u16, *const u16, *const u16, HWND) =
        unsafe { std::mem::transmute(symbol(module, s!("acceptance_begin"))) };
    let end: unsafe extern "C" fn() =
        unsafe { std::mem::transmute(symbol(module, s!("acceptance_end"))) };
    unsafe {
        begin(
            wide(OsStr::new(&context)).as_ptr(),
            value(ENVIRONMENT[2])?.as_ptr(),
            wide(OsStr::new(selected)).as_ptr(),
            HWND::default(),
        );
    }
    let result = work();
    unsafe {
        end();
    }
    result
}
pub fn ui_input(hwnd: usize, path: &std::path::Path, url: &str, x: i32, y: i32) -> Result<()> {
    let module = library()?;
    let input: unsafe extern "C" fn(HWND, *const u16, *const i8, i32, i32) =
        unsafe { std::mem::transmute(symbol(module, s!("acceptance_input"))) };
    let url = std::ffi::CString::new(url)?;
    unsafe {
        input(
            HWND(hwnd as _),
            wide(path.as_os_str()).as_ptr(),
            url.as_ptr(),
            x,
            y,
        );
    }
    Ok(())
}
pub fn ui_drive() -> Result<()> {
    let module = library()?;
    let drive: unsafe extern "C" fn() -> i32 =
        unsafe { std::mem::transmute(symbol(module, s!("acceptance_drive"))) };
    ensure!(
        unsafe { drive() } == 0,
        "Actual helper/source acceptance failed"
    );
    Ok(())
}
static DELIVERY_GATE: std::sync::Mutex<Option<futures::channel::oneshot::Receiver<()>>> =
    std::sync::Mutex::new(None);
static DELIVERY_RELEASE: std::sync::Mutex<Option<futures::channel::oneshot::Sender<()>>> =
    std::sync::Mutex::new(None);
pub fn hold_delivery() {
    let (sender, receiver) = futures::channel::oneshot::channel();
    *DELIVERY_GATE.lock().unwrap() = Some(receiver);
    *DELIVERY_RELEASE.lock().unwrap() = Some(sender);
}
pub fn release_delivery() {
    if let Some(sender) = DELIVERY_RELEASE.lock().unwrap().take() {
        let _ = sender.send(());
    }
}

pub fn wait_reaped() -> Result<()> {
    let function: unsafe extern "C" fn() -> i32 =
        unsafe { std::mem::transmute(symbol(library()?, s!("acceptance_wait_reaped"))) };
    ensure!(
        unsafe { function() } == 0,
        "Exact actual receiver was not reaped"
    );
    join_controllers()
}

static CONTROLLERS: std::sync::Mutex<Vec<thread::JoinHandle<()>>> =
    std::sync::Mutex::new(Vec::new());
pub(super) fn own_controller(controller: thread::JoinHandle<()>) {
    let mut controllers = CONTROLLERS.lock().unwrap();
    assert!(
        controllers.len() < 64,
        "Owned acceptance controller handle bound"
    );
    controllers.push(controller);
}
fn join_controllers() -> Result<()> {
    let controllers = std::mem::take(&mut *CONTROLLERS.lock().unwrap());
    for controller in controllers {
        let until = std::time::Instant::now() + Duration::from_secs(7);
        while pump_owner(&[event::raw(&controller)], until)? != 0 {}
        controller
            .join()
            .map_err(|_| anyhow::anyhow!("Actual controller thread panicked"))?;
    }
    Ok(())
}
pub fn ui_rearm(hwnd: usize) -> Result<()> {
    let function: unsafe extern "C" fn(HWND) =
        unsafe { std::mem::transmute(symbol(library()?, s!("acceptance_rearm"))) };
    unsafe {
        function(HWND(hwnd as _));
    }
    Ok(())
}

pub(super) fn deadline_expired() {
    if std::env::var_os(ENVIRONMENT[0]).is_none() {
        return;
    }
    let function: unsafe extern "C" fn() = unsafe {
        std::mem::transmute(symbol(
            library().expect("owned acceptance DLL"),
            s!("acceptance_deadline"),
        ))
    };
    unsafe {
        function();
    }
}
#[derive(Clone, Copy, Default)]
pub(crate) struct CompletionProof {
    pub acknowledged: bool,
    pub reaped: bool,
    pub enters: u64,
    pub submits: u64,
    pub reader_submits: u64,
    pub terminal: u32,
    pub reaps: u64,
    pub failure_version: u64,
}
static COMPLETION: std::sync::Mutex<CompletionProof> = std::sync::Mutex::new(CompletionProof {
    acknowledged: false,
    reaped: false,
    enters: 0,
    submits: 0,
    reader_submits: 0,
    terminal: 0,
    reaps: 0,
    failure_version: 0,
});
static COMPLETION_WAITER: std::sync::Mutex<
    Option<futures::channel::oneshot::Sender<CompletionProof>>,
> = std::sync::Mutex::new(None);
static BARRIER_ISSUED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static COMPLETION_REQUEST: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static CHILD_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub(super) fn record_read(header: wire::Header) {
    if header.kind == wire::SUBMIT {
        COMPLETION.lock().unwrap().reader_submits += 1;
    }
}
pub(super) fn record_publish(header: wire::Header) {
    let mut state = COMPLETION.lock().unwrap();
    match header.kind {
        wire::ENTER => state.enters += 1,
        wire::SUBMIT => {
            state.submits += 1;
            state.terminal = wire::SUBMIT;
        }
        wire::LEAVE | wire::FAILED => state.terminal = header.kind,
        _ => {}
    }
}
pub(super) fn child_started() {
    FAILURE_READY.store(false, std::sync::atomic::Ordering::Release);
    CHILD_ACTIVE.store(true, std::sync::atomic::Ordering::Release);
}
pub(super) fn request_completion() -> futures::channel::oneshot::Receiver<CompletionProof> {
    BARRIER_ISSUED.store(false, std::sync::atomic::Ordering::Release);
    let (tx, rx) = futures::channel::oneshot::channel();
    let mut waiter = COMPLETION_WAITER.lock().unwrap();
    assert!(waiter.is_none(), "One bounded completion request");
    *waiter = Some(tx);
    let reaps = COMPLETION.lock().unwrap().reaps;
    if !CHILD_ACTIVE.load(std::sync::atomic::Ordering::Acquire)
        && reaps > 0
        && FAILURE_READY.load(std::sync::atomic::Ordering::Acquire)
    {
        let sender = waiter.take().unwrap();
        drop(waiter);
        let mut proof = *COMPLETION.lock().unwrap();
        proof.acknowledged = FAILURE_ACK.load(std::sync::atomic::Ordering::Acquire);
        proof.reaped = true;
        let _ = sender.send(proof);
    } else {
        // Arm before publication can take the waiter and clear this request.
        COMPLETION_REQUEST.store(true, std::sync::atomic::Ordering::Release);
        drop(waiter);
    }
    rx
}
pub(super) fn take_completion() -> bool {
    let requested = COMPLETION_REQUEST.swap(false, std::sync::atomic::Ordering::AcqRel);
    if requested {
        BARRIER_ISSUED.store(true, std::sync::atomic::Ordering::Release);
    }
    requested
}
pub(super) fn completion_done(acknowledged: bool, reaped: bool) {
    if let Some(sender) = COMPLETION_WAITER.lock().unwrap().take() {
        let mut proof = *COMPLETION.lock().unwrap();
        proof.acknowledged = acknowledged;
        proof.reaped = reaped;
        let _ = sender.send(proof);
    }
}
pub(super) fn reader_reaped() {
    CHILD_ACTIVE.store(false, std::sync::atomic::Ordering::Release);
    COMPLETION.lock().unwrap().reaps += 1;
    if std::env::var(ENVIRONMENT[4]).as_deref() == Ok("early-failure-ack") {
        FAILURE_ACK.store(true, std::sync::atomic::Ordering::Release);
        FAILURE_READY.store(true, std::sync::atomic::Ordering::Release);
        completion_done(true, true);
    }
}

pub(super) fn omit_completion_reply() -> bool {
    std::env::var(ENVIRONMENT[4]).as_deref() == Ok("omit-barrier-reply")
}
#[derive(Clone, Copy)]
pub struct ConsumerProof {
    pub kind: &'static str,
    pub token: u64,
    pub generation: u64,
    pub acknowledged: bool,
}
static CONSUMER: std::sync::Mutex<
    Option<(
        &'static str,
        futures::channel::oneshot::Sender<ConsumerProof>,
    )>,
> = std::sync::Mutex::new(None);
thread_local! {static SUBMIT_STAMP:std::cell::Cell<Option<(u64,u64)>>=const{std::cell::Cell::new(None)};}
pub fn arm_consumer(kind: &'static str) -> futures::channel::oneshot::Receiver<ConsumerProof> {
    let (tx, rx) = futures::channel::oneshot::channel();
    let mut pending = CONSUMER.lock().unwrap();
    assert!(pending.is_none(), "One bounded consumer acknowledgment");
    *pending = Some((kind, tx));
    rx
}
pub async fn await_consumer<T: std::future::Future<Output = ()>>(
    receiver: futures::channel::oneshot::Receiver<ConsumerProof>,
    timeout: T,
) -> Result<ConsumerProof> {
    futures::pin_mut!(receiver, timeout);
    let result = match futures::future::select(receiver, timeout).await {
        futures::future::Either::Left((result, _)) => result?,
        _ => anyhow::bail!("Actual consumer acknowledgment deadline"),
    };
    println!(
        "{{\"consumer_path\":\"{}\",\"consumer_visited\":true,\"consumer_token\":{},\"consumer_generation\":{},\"consumer_acknowledged\":{}}}",
        result.kind, result.token, result.generation, result.acknowledged
    );
    ensure!(
        result.acknowledged && result.token > 0 && result.generation > 0,
        "Actual foreground consumer acknowledgment omitted"
    );
    Ok(result)
}
fn consumed(kind: &'static str, token: u64, generation: u64) {
    let mut pending = CONSUMER.lock().unwrap();
    if pending
        .as_ref()
        .is_some_and(|(expected, _)| *expected == kind)
    {
        let (_, sender) = pending.take().unwrap();
        let omitted = std::env::var(ENVIRONMENT[4])
            .is_ok_and(|control| control == format!("omit-{kind}-ack"));
        let _ = sender.send(ConsumerProof {
            kind,
            token,
            generation,
            acknowledged: !omitted,
        });
    }
}
pub(super) async fn hold_body(token: u64, generation: u64) {
    let gate = DELIVERY_GATE.lock().unwrap().take();
    if let Some(gate) = gate {
        consumed("body-held", token, generation);
        let _ = gate.await;
    }
}
pub(super) fn canceled_body(token: u64, generation: u64) {
    consumed("cancel-discard", token, generation);
}
pub(super) fn submit_scope<R>(token: u64, generation: u64, callback: impl FnOnce() -> R) -> R {
    SUBMIT_STAMP.with(|stamp| {
        let old = stamp.replace(Some((token, generation)));
        let result = callback();
        stamp.set(old);
        result
    })
}
pub fn busy_admission() {
    SUBMIT_STAMP.with(|stamp| {
        if let Some((token, generation)) = stamp.get() {
            consumed("busy-admission", token, generation);
        }
    })
}
static FAILURE_READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static FAILURE_ACK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static PUBLICATION_EVENT: OnceLock<std::os::windows::io::OwnedHandle> = OnceLock::new();
static FAILURE_HELD: OnceLock<std::os::windows::io::OwnedHandle> = OnceLock::new();
static FAILURE_RELEASE: OnceLock<std::os::windows::io::OwnedHandle> = OnceLock::new();
fn own_event(
    slot: &OnceLock<std::os::windows::io::OwnedHandle>,
) -> &std::os::windows::io::OwnedHandle {
    slot.get_or_init(|| event::event(false).expect("owned fixture event"))
}
pub(super) fn publication_changed() {
    let _ = event::signal(own_event(&PUBLICATION_EVENT));
}
pub(super) fn before_failure_transition() {
    static ONCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if std::env::var(ENVIRONMENT[4]).is_ok_and(|value| {
        matches!(
            value.as_str(),
            "delay-failure-publish" | "early-failure-ack"
        )
    }) && !ONCE.swap(true, std::sync::atomic::Ordering::AcqRel)
    {
        event::signal(own_event(&FAILURE_HELD)).unwrap();
        assert_eq!(
            event::wait(&[event::raw(own_event(&FAILURE_RELEASE))], 7000).unwrap(),
            0,
            "Owned failure-publication release deadline"
        );
    }
}
pub(super) fn failure_published(version: u64) {
    // Serialize failure readiness/request clearing with waiter installation and arming.
    let mut waiter = COMPLETION_WAITER.lock().unwrap();
    let acknowledged = {
        let mut proof = COMPLETION.lock().unwrap();
        proof.failure_version = version;
        !CHILD_ACTIVE.load(std::sync::atomic::Ordering::Acquire) && proof.reaps > 0
    };
    // The broker invokes this only after exact process reap, pipe-reader join and
    // protected target invalidation/version publication. That fence supersedes
    // a native barrier that could not reply before cancellation/native failure.
    // A live reader's omitted barrier still completes with acknowledged=false.
    BARRIER_ISSUED.store(false, std::sync::atomic::Ordering::Release);
    FAILURE_ACK.store(acknowledged, std::sync::atomic::Ordering::Release);
    FAILURE_READY.store(true, std::sync::atomic::Ordering::Release);
    COMPLETION_REQUEST.store(false, std::sync::atomic::Ordering::Release);
    if let Some(sender) = waiter.take() {
        let mut proof = *COMPLETION.lock().unwrap();
        proof.acknowledged = acknowledged;
        proof.reaped = true;
        let _ = sender.send(proof);
    }
    drop(waiter);
    publication_changed();
}
struct OwnerWake;
impl futures::task::ArcWake for OwnerWake {
    fn wake_by_ref(_: &std::sync::Arc<Self>) {
        publication_changed();
    }
}
fn pump_owner(handles: &[HANDLE], until: std::time::Instant) -> Result<u32> {
    let remaining = until.saturating_duration_since(std::time::Instant::now());
    ensure!(!remaining.is_zero(), "Owned publication/cleanup deadline");
    let ready = unsafe {
        MsgWaitForMultipleObjectsEx(
            Some(handles),
            remaining.as_millis().min(7000) as u32,
            QS_ALLINPUT,
            MWMO_INPUTAVAILABLE,
        )
    }
    .0;
    ensure!(
        ready <= handles.len() as u32,
        "Owned publication/cleanup wait expired"
    );
    if ready == handles.len() as u32 {
        let mut message = MSG::default();
        for _ in 0..32 {
            if !unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                break;
            }
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
    Ok(ready)
}
fn wait_future<F: std::future::Future>(future: F, until: std::time::Instant) -> Result<F::Output> {
    let mut future = Box::pin(future);
    let owner = std::sync::Arc::new(OwnerWake);
    let waker = futures::task::waker(owner);
    let mut context = std::task::Context::from_waker(&waker);
    loop {
        if let std::task::Poll::Ready(result) = future.as_mut().poll(&mut context) {
            return Ok(result);
        }
        pump_owner(&[event::raw(own_event(&PUBLICATION_EVENT))], until)?;
    }
}
pub(super) fn failure_terminal_kind() -> u32 {
    if std::env::var(ENVIRONMENT[4]).as_deref() == Ok("failed-as-leave") {
        println!("{{\"failure_terminal_mutated\":true,\"expected_kind\":14,\"actual_kind\":12}}");
        wire::LEAVE
    } else {
        wire::FAILED
    }
}
fn wait_delivery(
    registration: &mut super::native_drop_broker::Registration,
    handle: &super::native_drop_broker::TargetHandle,
    expected: u32,
) -> Result<super::native_drop_broker::Delivery> {
    let until = std::time::Instant::now() + Duration::from_secs(7);
    let mut intermediates = 0u32;
    loop {
        let delivery = wait_future(registration.next(), until)?
            .ok_or_else(|| anyhow::anyhow!("Actual registration closed before terminal"))?;
        let kind = delivery.packet.header.kind;
        if handle.current(&delivery)
            && (kind == expected || expected == 0 && matches!(kind, wire::LEAVE | wire::FAILED))
        {
            println!(
                "{{\"ordered_delivery\":true,\"expected_kind\":{expected},\"actual_kind\":{kind},\"intermediates_consumed\":{intermediates},\"current\":true,\"actual_version\":{},\"actual_generation\":{}}}",
                delivery.packet.header.version, delivery.packet.header.generation
            );
            return Ok(delivery);
        }
        if handle.current(&delivery)
            && (matches!(kind, wire::SUBMIT | wire::LEAVE)
                || kind == wire::FAILED && delivery.packet.header.generation > 0)
        {
            println!(
                "{{\"wrong_terminal_rejected\":true,\"expected_kind\":{expected},\"actual_kind\":{kind}}}"
            );
            anyhow::bail!("Wrong actual terminal delivery");
        }
        ensure!(
            matches!(kind, wire::ENTER | wire::MOTION | wire::FAILED),
            "Unexpected actual intermediate"
        );
        drop(delivery);
        intermediates += 1;
        ensure!(intermediates <= 1024, "Bounded actual intermediate count");
    }
}
fn wait_completion(broker: &NativeDropBroker) -> Result<CompletionProof> {
    Ok(wait_future(
        broker.acceptance_begin_completion()?,
        std::time::Instant::now() + Duration::from_secs(7),
    )??)
}

fn successful_body(
    broker: &NativeDropBroker,
    registration: &mut super::native_drop_broker::Registration,
    handle: &super::native_drop_broker::TargetHandle,
) -> Result<super::native_drop_broker::Delivery> {
    let proof = wait_completion(broker)?;
    ensure!(
        proof.acknowledged,
        "Actual success publication barrier failed"
    );
    wait_delivery(registration, handle, wire::SUBMIT)
}
fn finish_ordered(
    module: HMODULE,
    registration: &mut super::native_drop_broker::Registration,
    handle: &super::native_drop_broker::TargetHandle,
) -> Result<()> {
    let enter = wait_delivery(registration, handle, wire::ENTER)?;
    let generation = enter.packet.header.generation;
    ensure!(generation > 0, "Actual ordered ENTER generation");
    drop(enter);
    println!("{{\"real_enter_consumed_before_drop\":true,\"generation\":{generation}}}");
    let finish: unsafe extern "C" fn() -> i32 =
        unsafe { std::mem::transmute(symbol(module, s!("acceptance_finish_ordered"))) };
    ensure!(
        unsafe { finish() } == 0,
        "Actual ordered source Drop/reap failed"
    );
    Ok(())
}
fn controlled_failure_completion(
    broker: &NativeDropBroker,
    registration: &super::native_drop_broker::Registration,
) -> Result<CompletionProof> {
    use futures::FutureExt as _;
    ensure!(
        event::wait(&[event::raw(own_event(&FAILURE_HELD))], 7000)? == 0,
        "Actual post-reader/pre-failure gate not reached"
    );
    let retained = registration.acceptance_pending_kind() == Some(wire::ENTER);
    ensure!(
        retained,
        "Control requires actual retained ENTER before failure publication"
    );
    let mut completion = Box::pin(broker.acceptance_begin_completion()?);
    let early = completion.as_mut().now_or_never();
    let early_ack = early
        .as_ref()
        .is_some_and(|result| result.as_ref().is_ok_and(|proof| proof.acknowledged));
    println!(
        "{{\"reader_reaped_before_failure_publication\":true,\"retained_enter_before_transition\":{retained},\"completion_pending_before_transition\":{},\"early_ack_rejected\":{early_ack}}}",
        early.is_none()
    );
    ensure!(
        early.is_none(),
        "Completion was acknowledged before actual failure publication"
    );
    event::signal(own_event(&FAILURE_RELEASE))?;
    let proof = wait_future(
        completion,
        std::time::Instant::now() + Duration::from_secs(7),
    )??;
    ensure!(
        proof.acknowledged && proof.failure_version > 0,
        "Missing completed broker failure publication"
    );
    println!(
        "{{\"failure_publication_completed\":true,\"failure_version\":{}}}",
        proof.failure_version
    );
    Ok(proof)
}
pub(super) struct PayloadCall {
    end: u32,
    finished: bool,
}
impl PayloadCall {
    pub(super) fn begin(kind: u32, format: u16) -> Self {
        let end = match kind {
            1 => 51,
            2 => 53,
            3 => 55,
            _ => unreachable!(),
        };
        hook(end - 1, format as u64, std::ptr::null_mut());
        Self {
            end,
            finished: false,
        }
    }
    pub(super) fn finish(&mut self, result: i32) {
        hook(self.end, result as u32 as u64, std::ptr::null_mut());
        self.finished = true;
    }
}
impl Drop for PayloadCall {
    fn drop(&mut self) {
        if !self.finished {
            hook(self.end, 0x80004004, std::ptr::null_mut());
        }
    }
}
pub(super) fn source_mutation() -> Option<&'static str> {
    match std::env::var(ENVIRONMENT[4]).ok().as_deref() {
        Some("mutate-source-error") => Some("error"),
        Some("mutate-source-none") => Some("none"),
        Some("mutate-duplicate-get") => Some("get"),
        Some("mutate-duplicate-release") => Some("release"),
        Some("mutate-busy-copy") => Some("busy-copy"),
        Some("mutate-busy-get") => Some("busy-get"),
        _ => None,
    }
}
pub(super) fn mutate_drop_effect(effect: *mut windows::Win32::System::Ole::DROPEFFECT) -> bool {
    match source_mutation() {
        Some("error") => {
            hook(70, 1, std::ptr::null_mut());
            true
        }
        Some("none") => {
            unsafe {
                *effect = windows::Win32::System::Ole::DROPEFFECT_NONE;
            }
            hook(70, 2, std::ptr::null_mut());
            false
        }
        _ => false,
    }
}
pub(super) fn mutate_busy_effect(effect: *mut windows::Win32::System::Ole::DROPEFFECT) {
    if source_mutation() == Some("busy-copy")
        && unsafe { *effect } == windows::Win32::System::Ole::DROPEFFECT_NONE
    {
        unsafe {
            *effect = windows::Win32::System::Ole::DROPEFFECT_COPY;
        }
        hook(70, 5, std::ptr::null_mut());
    }
}
pub(super) fn duplicate_release(
    medium: &mut windows::Win32::System::Com::STGMEDIUM,
) -> Result<Option<windows::Win32::System::Com::STGMEDIUM>> {
    if source_mutation() != Some("release") {
        return Ok(None);
    }
    let mut copy = windows::Win32::System::Com::STGMEDIUM::default();
    let function: unsafe extern "C" fn(
        *mut windows::Win32::System::Com::STGMEDIUM,
        *mut windows::Win32::System::Com::STGMEDIUM,
    ) -> HRESULT =
        unsafe { std::mem::transmute(symbol(library()?, s!("acceptance_duplicate_release"))) };
    unsafe { function(medium, &mut copy) }.ok()?;
    Ok(Some(copy))
}
