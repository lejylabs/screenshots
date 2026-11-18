//! These fixtures own every protocol socket/display. They never read the user's clipboard.
use super::*;
use std::{
    os::{
        fd::AsRawFd,
        unix::{ffi::OsStrExt, fs::DirBuilderExt},
    },
    process::Child,
};
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
pub(super) struct CleanupProbe {
    stream: std::os::unix::net::UnixStream,
    entered: bool,
}
impl CleanupProbe {
    pub(super) fn before_reap(&mut self, group: i32) {
        if self.entered {
            return;
        }
        self.entered = true;
        if frame(&mut self.stream, b"START001", group as u32).is_err() {
            return;
        }
        let mut release = [0];
        // A failed fixture channel still permits the real destructor to reap its child.
        // The controller's missing marker/minimum-delay oracle fails independently.
        let _ = self.stream.read_exact(&mut release);
    }
    pub(super) fn after_reap(&mut self, group: i32) {
        let _ = frame(&mut self.stream, b"REAPED01", group as u32);
    }
}
thread_local! {
    pub(super) static CLEANUP_PROBE: std::cell::RefCell<Option<CleanupProbe>> = const { std::cell::RefCell::new(None) };
    static RESULT_PROBE: std::cell::RefCell<Option<std::os::unix::net::UnixStream>> = const { std::cell::RefCell::new(None) };
}
fn frame(stream: &mut std::os::unix::net::UnixStream, magic: &[u8; 8], value: u32) -> Result<()> {
    let mut bytes = [0; 12];
    bytes[..8].copy_from_slice(magic);
    bytes[8..].copy_from_slice(&value.to_be_bytes());
    stream.write_all(&bytes)?;
    Ok(())
}
pub(super) fn standalone_result_ready(result: &Result<()>) {
    RESULT_PROBE.with(|probe| {
        if let Some(mut probe) = probe.borrow_mut().take() {
            frame(&mut probe, b"APIERR01", u32::from(result.is_err())).unwrap();
        }
    });
}
fn fixture_spawner(
    path: &Path,
    check: &app_core::NativeCaptureCheck<'_>,
) -> Result<Box<dyn app_core::FileClipboardOwner>> {
    let mode = if std::env::var("FILE_CLIPBOARD_DRAIN_CASE").as_deref() == Ok("publication") {
        "stall"
    } else {
        "provider"
    };
    spawn_with_command_checked(path, false, child(mode), check)
}
fn child(mode: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "file_clipboard::tests::owned_file_writer_child_fixture",
            "--nocapture",
        ])
        .env_clear()
        .env("FILE_CLIPBOARD_TEST_CHILD", mode)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}
fn cleaned() {
    let until = Instant::now() + Duration::from_secs(5);
    while BUSY.load(Ordering::Acquire) {
        assert!(
            Instant::now() < until,
            "Private file owner cleanup did not release admission"
        );
        std::thread::park_timeout(Duration::from_millis(1));
    }
}
#[test]
fn owned_file_writer_child_fixture() {
    let Ok(mode) = std::env::var("FILE_CLIPBOARD_TEST_CHILD") else {
        return;
    };
    #[cfg(target_os = "linux")]
    if mode == "cli-drain-controller" {
        delayed_cleanup_controller().unwrap();
        return;
    }
    if mode == "cli-drain" {
        let socket = std::env::var_os("FILE_CLIPBOARD_DRAIN_SOCKET").unwrap();
        let stream = std::os::unix::net::UnixStream::connect(socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        RESULT_PROBE.with(|probe| *probe.borrow_mut() = Some(stream.try_clone().unwrap()));
        CLEANUP_PROBE.with(|probe| {
            *probe.borrow_mut() = Some(CleanupProbe {
                stream,
                entered: false,
            })
        });
        app_core::install_file_clipboard_spawner(fixture_spawner, quiescent).unwrap();
        let result = command(&["/owned-fixture".into(), "--hold-ms".into(), "1000".into()]);
        // Mirrors the ordinary main error exit, after the standalone command's completion drain.
        std::process::exit(if result.is_err() { 19 } else { 20 });
    }
    if matches!(mode.as_str(), "hold-before-poll" | "hold-other-thread") {
        let cancel = crate::record::unix::Cancellation::new().unwrap();
        let wake = HoldWake::new().unwrap();
        let other_thread = mode == "hold-other-thread";
        BEFORE_HOLD_POLL.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                if other_thread {
                    std::thread::spawn(|| {
                        signal_hook::low_level::raise(signal_hook::consts::SIGTERM).unwrap()
                    })
                    .join()
                    .unwrap();
                } else {
                    signal_hook::low_level::raise(signal_hook::consts::SIGINT).unwrap();
                }
            }))
        });
        let started = Instant::now();
        wake.wait(&cancel, Instant::now() + Duration::from_secs(300))
            .unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "Signal did not wake the five-minute hold"
        );
        assert!(
            cancel
                .check(Instant::now() + Duration::from_secs(1))
                .is_err()
        );
        return;
    }
    if mode == "output-native" {
        try_child(&[output::CHILD.into()]).unwrap();
        return;
    }
    if mode.starts_with("output-") {
        let (channel, nonce, _) = transport::Transport::inherited().unwrap();
        let mut io = transport::ActiveIo {
            transport: channel,
            deadline: Instant::now() + Duration::from_secs(20),
            check: &|| Ok(()),
        };
        if mode == "output-stall" {
            let mut never = [0];
            let _ = io.read_exact(&mut never);
            return;
        }
        io.write_all(b"SCOUTP01").unwrap();
        io.write_all(&nonce).unwrap();
        let mut header = [0; 42];
        io.read_exact(&mut header).unwrap();
        assert_eq!(&header[..8], b"SCOUTP01");
        assert_eq!(&header[8..24], nonce);
        assert!(matches!(header[24], 1 | 2));
        let length =
            usize::try_from(u64::from_be_bytes(header[33..41].try_into().unwrap())).unwrap();
        assert!(length <= 1024 * 1024);
        let mut bytes = Vec::new();
        if header[24] == 1 {
            loop {
                let mut count = [0; 4];
                io.read_exact(&mut count).unwrap();
                let count = u32::from_be_bytes(count) as usize;
                if count == 0 {
                    break;
                }
                assert!(count <= 65536 && bytes.len() + count <= 1024 * 1024);
                let start = bytes.len();
                bytes.resize(start + count, 0);
                io.read_exact(&mut bytes[start..]).unwrap();
            }
            assert!(image::load_from_memory(&bytes).is_ok());
        } else {
            bytes.resize(length, 0);
            io.read_exact(&mut bytes).unwrap();
        }
        let mut gate = [0];
        io.read_exact(&mut gate).unwrap();
        assert_eq!(gate, [1]);
        if mode == "output-late" {
            let marker = std::path::PathBuf::from(std::env::var_os("OUTPUT_STARTED").unwrap());
            std::fs::write(&marker, b"entered-private-native-publication").unwrap();
            std::thread::park_timeout(Duration::from_millis(500));
            std::fs::write(marker.with_extension("published"), b"forbidden-late-result").unwrap();
        }
        io.write_all(&[1]).unwrap();
        let mut retired = [0];
        let _ = io.read(&mut retired);
        return;
    }
    if mode == "native" {
        try_child(&[CHILD.into()]).unwrap();
        return;
    }
    let (channel, nonce, _) = transport::Transport::inherited().unwrap();
    let mut io = transport::ActiveIo {
        transport: channel,
        deadline: Instant::now() + Duration::from_secs(20),
        check: &|| Ok(()),
    };
    if mode == "stall" {
        let mut never = [0];
        let _ = io.read_exact(&mut never);
        return;
    }
    io.write_all(MAGIC).unwrap();
    io.write_all(&nonce).unwrap();
    let mut header = [0; 29];
    io.read_exact(&mut header).unwrap();
    assert_eq!(&header[..8], MAGIC);
    assert_eq!(&header[8..24], nonce);
    let length = u32::from_be_bytes(header[24..28].try_into().unwrap()) as usize;
    assert!((1..=4096).contains(&length));
    let mut path = vec![0; length];
    io.read_exact(&mut path).unwrap();
    assert_eq!(path, b"/owned-fixture");
    io.write_all(&[1]).unwrap();
    let mut retired = [0];
    let _ = io.read(&mut retired);
}
#[test]
fn file_owner_retains_admission_and_drop_never_waits_for_native_teardown() -> Result<()> {
    let _serial = SERIAL.lock().unwrap();
    let owner = spawn_with_command(Path::new("/owned-fixture"), false, child("provider"))?;
    #[cfg(target_os = "linux")]
    assert!(
        !crate::capture_worker::reserve_signal_shutdown(),
        "a live file owner must prevent process exit"
    );
    #[cfg(target_os = "linux")]
    assert!(spawn_with_command(Path::new("/owned-fixture"), false, child("provider")).is_err());
    let started = Instant::now();
    drop(owner);
    assert!(started.elapsed() < Duration::from_millis(50));
    cleaned();
    spawn_with_command(Path::new("/owned-fixture"), false, child("provider"))?.retire()?;
    assert!(!BUSY.load(Ordering::Acquire));
    Ok(())
}
#[test]
#[ignore = "five-second private stalled-provider deadline fixture; never opens a native clipboard"]
fn stalled_file_owner_is_contained_and_never_late_publishes() {
    let _serial = SERIAL.lock().unwrap();
    let started = Instant::now();
    let result = spawn_with_command(Path::new("/owned-fixture"), false, child("stall"));
    assert!(result.is_err());
    assert!(started.elapsed() < Duration::from_secs(6));
    cleaned();
}
#[test]
fn cancelled_file_publication_stops_its_private_provider_without_a_late_success() {
    let _serial = SERIAL.lock().unwrap();
    let started = Instant::now();
    let check = || {
        if started.elapsed() >= Duration::from_millis(50) {
            Err(app_core::CaptureError::NativeCanceled)
        } else {
            Ok(())
        }
    };
    assert!(
        spawn_with_command_checked(Path::new("/owned-fixture"), false, child("stall"), &check)
            .is_err()
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    cleaned();
}
#[test]
fn file_hold_wakes_for_a_signal_before_poll_and_on_another_thread() -> Result<()> {
    let _serial = SERIAL.lock().unwrap();
    for mode in ["hold-before-poll", "hold-other-thread"] {
        let mut process = child(mode).spawn()?;
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(status) = process.try_wait()? {
                ensure!(status.success(), "Private hold-signal fixture failed");
                break;
            }
            if Instant::now() >= until {
                // Only this exact owned fixture is killed; a five-minute regression cannot hang the suite.
                let _ = process.kill();
                let _ = process.wait();
                anyhow::bail!("Private hold-signal fixture exceeded its deadline");
            }
            std::thread::park_timeout(Duration::from_millis(1));
        }
    }
    Ok(())
}
#[cfg(target_os = "linux")]
struct DrainProcess(Child);
#[cfg(target_os = "linux")]
impl Drop for DrainProcess {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
        // This dedicated private controller is a subreaper and creates no unrelated children.
        // A failing regression may exit CLI early, but its adopted provider is still actually reaped.
        loop {
            let mut status = 0;
            let result = unsafe { libc::waitpid(-1, &mut status, 0) };
            if result > 0 {
                continue;
            }
            if result < 0
                && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
            {
                continue;
            }
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ECHILD)
            );
            break;
        }
    }
}
#[cfg(target_os = "linux")]
fn delayed_cleanup_controller() -> Result<()> {
    use std::os::unix::net::UnixListener;
    ensure!(
        unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1) } == 0,
        "Could not own private fixture descendants"
    );
    let case = std::env::var("FILE_CLIPBOARD_DRAIN_CASE")?;
    ensure!(
        matches!(case.as_str(), "publication" | "retirement"),
        "Invalid private cleanup case"
    );
    let root = Root::new()?;
    let socket = root.0.join("cleanup-probe");
    let listener = UnixListener::bind(&socket)?;
    listener.set_nonblocking(true)?;
    let mut launch = child("cli-drain");
    launch
        .env("FILE_CLIPBOARD_DRAIN_CASE", &case)
        .env("FILE_CLIPBOARD_DRAIN_SOCKET", &socket);
    let mut process = DrainProcess(launch.spawn()?);
    let result = (|| -> Result<()> {
        let startup = Instant::now() + Duration::from_secs(15);
        let mut stream = loop {
            if let Some(status) = process.0.try_wait()? {
                anyhow::bail!("Standalone fixture exited before native cleanup began: {status}");
            }
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error.into()),
            }
            ensure!(
                Instant::now() < startup,
                "Private standalone fixture did not connect"
            );
            std::thread::park_timeout(Duration::from_millis(1));
        };
        stream.set_read_timeout(Some(Duration::from_secs(15)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut started = None;
        let mut provider = None;
        let mut bounded_api_error = false;
        // The actual supervisor sends START before it is allowed to perform child.wait.
        // The actual standalone command sends APIERR after its bounded API call returns.
        for _ in 0..2 {
            let mut message = [0; 12];
            stream.read_exact(&mut message)?;
            let value = u32::from_be_bytes(message[8..].try_into()?);
            match &message[..8] {
                b"START001" => {
                    ensure!(
                        provider.is_none() && value > 1,
                        "Invalid owned provider marker"
                    );
                    provider = Some(value);
                    started = Some(Instant::now());
                }
                b"APIERR01" => {
                    ensure!(
                        !bounded_api_error && value == 1,
                        "Bounded API result did not remain an error"
                    );
                    bounded_api_error = true;
                }
                _ => anyhow::bail!("Unexpected cleanup marker before actual reap"),
            }
        }
        let started = started.context("Actual supervisor cleanup did not begin")?;
        ensure!(
            bounded_api_error,
            "Bounded publication/retirement API did not return its error"
        );
        // Seven real seconds exceeds both the unchanged 250ms publication cleanup grace and
        // the unchanged five-second retirement wait. No shorter timeout is injected.
        while started.elapsed() < Duration::from_secs(7) {
            ensure!(
                process.0.try_wait()?.is_none(),
                "CLI exited while its actual provider remained unreaped"
            );
            std::thread::park_timeout(Duration::from_millis(5));
        }
        ensure!(
            process.0.try_wait()?.is_none(),
            "CLI escaped the actual-reap completion drain"
        );
        stream.write_all(b"R")?;
        let mut reaped = [0; 12];
        stream.read_exact(&mut reaped)?;
        ensure!(
            &reaped[..8] == b"REAPED01"
                && Some(u32::from_be_bytes(reaped[8..].try_into()?)) == provider,
            "Actual supervisor did not acknowledge the same owned child.wait completion"
        );
        let exit_deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = process.0.try_wait()? {
                break status;
            }
            ensure!(
                Instant::now() < exit_deadline,
                "Standalone CLI stayed parked after its actual provider was reaped"
            );
            std::thread::park_timeout(Duration::from_millis(5));
        };
        ensure!(
            status.code() == Some(19),
            "Standalone error result was hidden or changed after cleanup"
        );
        Ok(())
    })();
    // Cleanup is performed before returning any failed oracle, including the old premature-exit bug.
    drop(process);
    result
}
#[cfg(target_os = "linux")]
#[test]
#[ignore = "two real supervisor delayed-reap subprocesses (>7 seconds each), no native clipboard access"]
fn standalone_errors_wait_for_actual_reap_beyond_publication_and_retirement_thresholds()
-> Result<()> {
    let _serial = SERIAL.lock().unwrap();
    for case in ["publication", "retirement"] {
        let status = child("cli-drain-controller")
            .env("FILE_CLIPBOARD_DRAIN_CASE", case)
            .status()?;
        ensure!(
            status.success(),
            "Private actual-supervisor cleanup controller failed"
        );
    }
    Ok(())
}
struct Root(std::path::PathBuf);
impl Root {
    fn new() -> Result<Self> {
        let path =
            std::env::temp_dir().join(format!("owned-file-clipboard-{}", uuid::Uuid::new_v4()));
        std::fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[cfg(target_os = "linux")]
fn authority(path: &Path, number: &str, cookie: &[u8]) -> Result<()> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&65535u16.to_be_bytes()); // FamilyWild, never TCP.
    for field in [
        b"".as_slice(),
        number.as_bytes(),
        b"MIT-MAGIC-COOKIE-1".as_slice(),
        cookie,
    ] {
        bytes.extend_from_slice(&(field.len() as u16).to_be_bytes());
        bytes.extend_from_slice(field);
    }
    std::fs::write(path, bytes)?;
    Ok(())
}
#[cfg(target_os = "linux")]
#[test]
#[ignore = "starts authenticated private Xvfb; SCREENSHOTS_TEST_BINARY must name the optimized CLI reader"]
fn x11_owned_file_roundtrip_uses_the_existing_isolated_native_file_reader() -> Result<()> {
    let _serial = SERIAL.lock().unwrap();
    let binary =
        std::env::var_os("SCREENSHOTS_TEST_BINARY").context("Set SCREENSHOTS_TEST_BINARY")?;
    let root = Root::new()?;
    let auth = root.0.join("authority");
    let cookie = *uuid::Uuid::new_v4().as_bytes();
    authority(&auth, "", &cookie)?;
    let mut server = Server(
        Command::new("Xvfb")
            .args([
                "-displayfd",
                "1",
                "-screen",
                "0",
                "64x64x24",
                "-nolisten",
                "tcp",
                "-auth",
            ])
            .arg(&auth)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let mut output = server.0.stdout.take().unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    let mut display = String::new();
    loop {
        ensure!(
            Instant::now() < until && display.len() < 8,
            "Private Xvfb startup failed"
        );
        let mut fd = libc::pollfd {
            fd: output.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let timeout = until.saturating_duration_since(Instant::now()).as_millis() as i32;
        ensure!(
            unsafe { libc::poll(&mut fd, 1, timeout) } > 0,
            "Private Xvfb startup timed out"
        );
        let mut byte = [0];
        output.read_exact(&mut byte)?;
        if byte[0] == b'\n' {
            break;
        }
        ensure!(byte[0].is_ascii_digit(), "Invalid private Xvfb display");
        display.push(byte[0] as char);
    }
    authority(&auth, &display, &cookie)?;
    let display = format!(":{display}");
    let path = root.0.join(std::ffi::OsStr::from_bytes(
        b"Gr\xc3\xbc\xc3\x9fe #?% \xff.bin",
    ));
    std::fs::write(&path, b"source unchanged; no image decoder")?;
    let mut command = child("native");
    command
        .env("DISPLAY", &display)
        .env("XAUTHORITY", &auth)
        .env("XDG_SESSION_TYPE", "x11");
    let owner = spawn_with_command(&path, false, command)?;
    let output = Command::new(binary)
        .args(["clipboard", "files"])
        .env_clear()
        .env("DISPLAY", &display)
        .env("XAUTHORITY", &auth)
        .env("XDG_SESSION_TYPE", "x11")
        .stdin(Stdio::null())
        .output()?;
    ensure!(
        output.status.success(),
        "Owned native file-list reader failed"
    );
    let app_core::ClipboardSnapshot::Files { files } = serde_json::from_slice(&output.stdout)?
    else {
        anyhow::bail!("Native file-list reader returned the wrong representation");
    };
    assert_eq!(files.len(), 1);
    assert_eq!(
        files[0].native,
        std::fs::canonicalize(&path)?.as_os_str().as_bytes()
    );
    assert_eq!(std::fs::read(&path)?, b"source unchanged; no image decoder");
    owner.retire()?;
    Ok(())
}

#[cfg(target_os = "linux")]
#[path = "file_clipboard_wayland_tests.rs"]
mod wayland;

#[test]
fn image_and_text_cancellation_reap_late_providers_before_replacement() -> Result<()> {
    let _serial = SERIAL.lock().unwrap();
    let root = Root::new()?;
    let image = image::RgbaImage::from_pixel(3, 2, image::Rgba([17, 31, 47, 83]));
    for is_image in [true, false] {
        let started = root.0.join(if is_image {
            "image.started"
        } else {
            "text.started"
        });
        let mut command = child("output-late");
        command.env("OUTPUT_STARTED", &started);
        let check = || {
            if started.exists() {
                Err(app_core::CaptureError::NativeCanceled)
            } else {
                Ok(())
            }
        };
        let value = if is_image {
            app_core::NativeClipboardOutput::Image(Arc::new(image.clone()))
        } else {
            app_core::NativeClipboardOutput::Text("private-UTF8-ä")
        };
        let now = Instant::now();
        assert!(spawn_input(Input::Output(value), false, command, &check).is_err());
        assert!(now.elapsed() < Duration::from_secs(2));
        assert!(
            started.exists(),
            "Provider never reached the native-publication boundary"
        );
        cleaned();
        assert!(
            !started.with_extension("published").exists(),
            "Retired provider published late"
        );
        let owner = spawn_input(
            Input::Output(app_core::NativeClipboardOutput::Text("replacement")),
            false,
            child("output-provider"),
            &|| Ok(()),
        )?;
        #[cfg(target_os = "linux")]
        assert!(
            spawn_input(
                Input::Output(app_core::NativeClipboardOutput::Image(Arc::new(
                    image.clone()
                ))),
                false,
                child("output-provider"),
                &|| Ok(())
            )
            .is_err()
        );
        owner.retire()?;
        cleaned();
        assert!(!started.with_extension("published").exists());
    }
    Ok(())
}
#[test]
#[ignore = "five-second private stalled output-provider fixture; never accesses a native clipboard"]
fn image_and_text_outer_deadline_contains_a_stalled_native_setter() -> Result<()> {
    let _serial = SERIAL.lock().unwrap();
    let image = image::RgbaImage::new(1, 1);
    for value in [
        app_core::NativeClipboardOutput::Image(Arc::new(image.clone())),
        app_core::NativeClipboardOutput::Text("synthetic"),
    ] {
        let now = Instant::now();
        let result = spawn_input(Input::Output(value), false, child("output-stall"), &|| {
            Ok(())
        });
        assert!(result.is_err());
        assert!(now.elapsed() < Duration::from_secs(6));
        cleaned();
    }
    Ok(())
}
#[cfg(target_os = "linux")]
fn bounded_cli(mut command: Command) -> Result<std::process::Output> {
    let mut process = Server(
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?,
    );
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        if process.0.try_wait()?.is_some() {
            break;
        }
        ensure!(
            Instant::now() < until,
            "Owned clipboard reader exceeded its finite deadline"
        );
        std::thread::park_timeout(Duration::from_millis(1));
    }
    let mut out = Vec::new();
    process
        .0
        .stdout
        .take()
        .unwrap()
        .take(65536)
        .read_to_end(&mut out)?;
    let mut err = Vec::new();
    process
        .0
        .stderr
        .take()
        .unwrap()
        .take(65536)
        .read_to_end(&mut err)?;
    Ok(std::process::Output {
        status: process.0.wait()?,
        stdout: out,
        stderr: err,
    })
}
#[cfg(target_os = "linux")]
#[test]
#[ignore = "private authenticated Xvfb only; SCREENSHOTS_TEST_BINARY names the optimized isolated image/text reader"]
fn x11_owned_image_incr_and_text_paste_survive_publication_until_explicit_replacement() -> Result<()>
{
    let _serial = SERIAL.lock().unwrap();
    let binary =
        std::env::var_os("SCREENSHOTS_TEST_BINARY").context("Set SCREENSHOTS_TEST_BINARY")?;
    let root = Root::new()?;
    let auth = root.0.join("authority");
    let cookie = *uuid::Uuid::new_v4().as_bytes();
    authority(&auth, "", &cookie)?;
    let mut server = Server(
        Command::new("Xvfb")
            .args([
                "-displayfd",
                "1",
                "-screen",
                "0",
                "64x64x24",
                "-nolisten",
                "tcp",
                "-auth",
            ])
            .arg(&auth)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let mut output = server.0.stdout.take().unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    let mut display = String::new();
    loop {
        ensure!(
            Instant::now() < until && display.len() < 8,
            "Private Xvfb startup failed"
        );
        let mut fd = libc::pollfd {
            fd: output.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let timeout = until.saturating_duration_since(Instant::now()).as_millis() as i32;
        ensure!(
            unsafe { libc::poll(&mut fd, 1, timeout) } > 0,
            "Private Xvfb startup timed out"
        );
        let mut byte = [0];
        output.read_exact(&mut byte)?;
        if byte[0] == b'\n' {
            break;
        }
        ensure!(byte[0].is_ascii_digit(), "Invalid private display");
        display.push(byte[0] as char);
    }
    authority(&auth, &display, &cookie)?;
    let display = format!(":{display}");
    let native = || {
        let mut command = child("output-native");
        command
            .env("DISPLAY", &display)
            .env("XAUTHORITY", &auth)
            .env("XDG_SESSION_TYPE", "x11");
        command
    };
    let reader = |arguments: &[&std::ffi::OsStr]| {
        let mut command = Command::new(&binary);
        command
            .args(arguments)
            .env_clear()
            .env("DISPLAY", &display)
            .env("XAUTHORITY", &auth)
            .env("XDG_SESSION_TYPE", "x11");
        command
    };
    let mut seed = 0x12345678u32;
    let image = image::RgbaImage::from_fn(256, 256, |_, _| {
        let mut pixel = [0; 4];
        for b in &mut pixel {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            *b = seed as u8;
        }
        image::Rgba(pixel)
    });
    let owner = spawn_input(
        Input::Output(app_core::NativeClipboardOutput::Image(Arc::new(
            image.clone(),
        ))),
        false,
        native(),
        &|| Ok(()),
    )?;
    for index in 0..2 {
        let path = root.0.join(format!("pasted-{index}.png"));
        let result = bounded_cli(reader(&[
            "clipboard".as_ref(),
            "image".as_ref(),
            path.as_os_str(),
        ]))?;
        ensure!(result.status.success(), "Owned X11 PNG paste failed");
        assert!(
            std::fs::metadata(&path)?.len() > 64 * 1024,
            "Fixture did not exercise INCR"
        );
        assert_eq!(image::open(path)?.into_rgba8(), image);
    }
    owner.retire()?;
    let text = "private UTF-8 ä and emoji 🐾";
    let owner = spawn_input(
        Input::Output(app_core::NativeClipboardOutput::Text(text)),
        false,
        native(),
        &|| Ok(()),
    )?;
    let result = bounded_cli(reader(&["clipboard".as_ref(), "text".as_ref()]))?;
    ensure!(result.status.success(), "Owned X11 text paste failed");
    let app_core::ClipboardSnapshot::Text { text: pasted } =
        serde_json::from_slice(&result.stdout)?
    else {
        anyhow::bail!("Wrong text representation");
    };
    assert_eq!(pasted, text);
    owner.retire()?;
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn cancellation_and_deadline_leave_actual_retirement_owned_until_reap() -> Result<()> {
    let _serial = SERIAL.lock().unwrap();
    for canceled in [false, true] {
        let (mut controller, provider) = std::os::unix::net::UnixStream::pair()?;
        for stream in [&controller, &provider] {
            stream.set_read_timeout(Some(Duration::from_secs(3)))?;
            stream.set_write_timeout(Some(Duration::from_secs(3)))?;
        }
        CLEANUP_PROBE.with(|probe| {
            *probe.borrow_mut() = Some(CleanupProbe {
                stream: provider,
                entered: false,
            })
        });
        let owner = spawn_with_command(Path::new("/owned-fixture"), false, child("provider"))?;
        let started = Instant::now();
        let deadline = started
            + if canceled {
                Duration::from_secs(5)
            } else {
                Duration::from_millis(50)
            };
        let check = || {
            if canceled && started.elapsed() >= Duration::from_millis(50) {
                Err(app_core::CaptureError::NativeCanceled)
            } else {
                Ok(())
            }
        };
        assert!(owner.retire_with_cancel(deadline, &check).is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
        let mut marker = [0; 12];
        controller.read_exact(&mut marker)?;
        assert_eq!(&marker[..8], b"START001");
        assert!(
            BUSY.load(Ordering::Acquire),
            "Canceled retirement released admission before actual reap"
        );
        assert!(spawn_with_command(Path::new("/owned-fixture"), false, child("provider")).is_err());
        controller.write_all(&[1])?;
        controller.read_exact(&mut marker)?;
        assert_eq!(&marker[..8], b"REAPED01");
        cleaned();
    }
    Ok(())
}
