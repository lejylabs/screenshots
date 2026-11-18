//! Native Windows fixtures create and open only a fresh private window station/desktop.
//! No fixture falls back to the interactive station or the user's clipboard.
use super::*;
use std::ffi::c_void;
type Handle = *mut c_void;
#[link(name = "user32")]
unsafe extern "system" {
    fn CreateWindowStationW(
        name: *const u16,
        flags: u32,
        access: u32,
        attributes: *const c_void,
    ) -> Handle;
    fn OpenWindowStationW(name: *const u16, inherit: i32, access: u32) -> Handle;
    fn GetProcessWindowStation() -> Handle;
    fn SetProcessWindowStation(station: Handle) -> i32;
    fn CloseWindowStation(station: Handle) -> i32;
    fn CreateDesktopW(
        name: *const u16,
        device: *const u16,
        mode: *const c_void,
        flags: u32,
        access: u32,
        attributes: *const c_void,
    ) -> Handle;
    fn OpenDesktopW(name: *const u16, flags: u32, inherit: i32, access: u32) -> Handle;
    fn GetThreadDesktop(thread: u32) -> Handle;
    fn SetThreadDesktop(desktop: Handle) -> i32;
    fn CloseDesktop(desktop: Handle) -> i32;
}
struct Private {
    station: Handle,
    desktop: Handle,
    previous_station: Handle,
    previous_desktop: Handle,
}
impl Private {
    fn enter(name: &str, create: bool) -> Result<Self> {
        ensure!(
            name.starts_with("screenshots-fixture-") && name.len() < 96,
            "Invalid private station"
        );
        let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        let name = wide(name);
        let desktop_name = wide("clipboard");
        let previous_station = unsafe { GetProcessWindowStation() };
        let previous_desktop = unsafe {
            GetThreadDesktop(windows_sys::Win32::System::Threading::GetCurrentThreadId())
        };
        ensure!(
            !previous_station.is_null() && !previous_desktop.is_null(),
            "Private desktop origin unavailable"
        );
        let station = if create {
            unsafe { CreateWindowStationW(name.as_ptr(), 0, 0x37f, std::ptr::null()) }
        } else {
            unsafe { OpenWindowStationW(name.as_ptr(), 0, 0x37f) }
        };
        ensure!(!station.is_null(), "Private clipboard station unavailable");
        let mut value = Self {
            station,
            desktop: std::ptr::null_mut(),
            previous_station,
            previous_desktop,
        };
        ensure!(
            unsafe { SetProcessWindowStation(station) } != 0,
            "Private station selection failed"
        );
        value.desktop = if create {
            unsafe {
                CreateDesktopW(
                    desktop_name.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    0x1ff,
                    std::ptr::null(),
                )
            }
        } else {
            unsafe { OpenDesktopW(desktop_name.as_ptr(), 0, 0, 0x1ff) }
        };
        ensure!(
            !value.desktop.is_null() && unsafe { SetThreadDesktop(value.desktop) } != 0,
            "Private clipboard desktop selection failed"
        );
        Ok(value)
    }
}
impl Drop for Private {
    fn drop(&mut self) {
        unsafe {
            let _ = SetThreadDesktop(self.previous_desktop);
            let _ = SetProcessWindowStation(self.previous_station);
            if !self.desktop.is_null() {
                let _ = CloseDesktop(self.desktop);
            }
            let _ = CloseWindowStation(self.station);
        }
    }
}
fn command(mode: &str, station: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "file_clipboard::windows_tests::fixture",
            "--nocapture",
        ])
        .env_clear()
        .env("PRIVATE_OUTPUT_MODE", mode)
        .env("PRIVATE_OUTPUT_STATION", station)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for key in ["SystemRoot", "WINDIR", "PATH", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
}
#[test]
fn fixture() {
    let Ok(mode) = std::env::var("PRIVATE_OUTPUT_MODE") else {
        return;
    };
    let station = std::env::var("PRIVATE_OUTPUT_STATION").unwrap();
    let _private = Private::enter(&station, mode == "controller").unwrap();
    if mode == "native" {
        try_child(&[output::CHILD.into()]).unwrap();
        return;
    }
    if mode == "stall-payload" {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::{
            Pipes::{GetNamedPipeInfo, PeekNamedPipe},
            Threading::{EVENT_MODIFY_STATE, OpenEventW, SetEvent},
        };
        let (channel, nonce, _) = transport::Transport::inherited().unwrap();
        let mut io = transport::ActiveIo {
            transport: channel,
            deadline: Instant::now() + Duration::from_secs(5),
            check: &|| Ok(()),
        };
        io.write_all(b"SCOUTP01").unwrap();
        io.write_all(&nonce).unwrap();
        let mut header = [0; 42];
        io.read_exact(&mut header).unwrap();
        assert_eq!(&header[..8], b"SCOUTP01");
        assert_eq!(&header[8..24], nonce);
        assert_eq!(header[24], 1);
        assert_eq!(
            u64::from_be_bytes(header[33..41].try_into().unwrap()),
            4 * 1024 * 1024
        );
        let (read, _write) = io.transport.into_windows_files();
        let mut capacity = 0;
        assert_ne!(
            unsafe {
                GetNamedPipeInfo(
                    read.as_raw_handle(),
                    std::ptr::null_mut(),
                    &mut capacity,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            },
            0
        );
        assert!(capacity >= 65536 && capacity < 4 * 1024 * 1024);
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            let mut available = 0;
            assert_ne!(
                unsafe {
                    PeekNamedPipe(
                        read.as_raw_handle(),
                        std::ptr::null_mut(),
                        0,
                        std::ptr::null_mut(),
                        &mut available,
                        std::ptr::null_mut(),
                    )
                },
                0
            );
            if available >= capacity {
                break;
            }
            assert!(Instant::now() < until, "Private payload pipe did not fill");
            std::thread::park_timeout(Duration::from_millis(1));
        }
        let name = std::env::var("PRIVATE_OUTPUT_STARTED_EVENT")
            .unwrap()
            .encode_utf16()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
        assert!(!event.is_null());
        assert_ne!(unsafe { SetEvent(event) }, 0);
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(event);
        }
        // No payload byte is drained and no native clipboard setter/acknowledgement is fabricated.
        loop {
            std::thread::park();
        }
    }
    if mode == "stall" {
        let (_channel, _, _) = transport::Transport::inherited().unwrap();
        use windows_sys::Win32::System::Threading::{EVENT_MODIFY_STATE, OpenEventW, SetEvent};
        let name = std::env::var("PRIVATE_OUTPUT_STARTED_EVENT")
            .unwrap()
            .encode_utf16()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
        assert!(!event.is_null());
        assert_ne!(unsafe { SetEvent(event) }, 0);
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(event);
        }
        loop {
            std::thread::park();
        }
    }
    assert_eq!(mode, "controller");
    let result = (|| -> Result<()> {
        let pixels =
            image::RgbaImage::from_fn(3, 2, |x, y| image::Rgba([x as u8, y as u8, 47, 83]));
        let owner = spawn_input(
            Input::Output(app_core::NativeClipboardOutput::Image(Arc::new(
                pixels.clone(),
            ))),
            false,
            command("native", &station),
            &|| Ok(()),
        )?;
        owner.retire()?; // Windows data must survive actual helper/Job reap.
        let mut reader = arboard::Clipboard::new()?;
        let pasted = reader.get_image()?;
        assert_eq!((pasted.width, pasted.height), (3, 2));
        assert_eq!(pasted.bytes.as_ref(), pixels.as_raw());
        drop(pasted);
        drop(reader);
        let text = "owned Unicode ä 🐾";
        let owner = spawn_input(
            Input::Output(app_core::NativeClipboardOutput::Text(text)),
            false,
            command("native", &station),
            &|| Ok(()),
        )?;
        owner.retire()?;
        let mut reader = arboard::Clipboard::new()?;
        assert_eq!(reader.get_text()?, text);
        drop(reader);
        let missing =
            std::env::temp_dir().join(format!("missing-clipboard-{}", uuid::Uuid::new_v4()));
        assert!(
            spawn_input(
                Input::Output(app_core::NativeClipboardOutput::File(&missing)),
                false,
                command("native", &station),
                &|| Ok(())
            )
            .is_err()
        );
        let mut reader = arboard::Clipboard::new()?;
        assert_eq!(reader.get_text()?, text);
        drop(reader);
        let path =
            std::env::temp_dir().join(format!("owned-clipboard-{}.bin", uuid::Uuid::new_v4()));
        std::fs::write(&path, b"private unchanged source")?;
        let copied = (|| -> Result<()> {
            let owner = spawn_input(
                Input::Output(app_core::NativeClipboardOutput::File(&path)),
                false,
                command("native", &station),
                &|| Ok(()),
            )?;
            owner.retire()?;
            let mut reader = arboard::Clipboard::new()?;
            assert_eq!(reader.get().file_list()?, [path.clone()]);
            assert_eq!(std::fs::read(&path)?, b"private unchanged source");
            Ok(())
        })();
        std::fs::remove_file(path)?;
        copied?;
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::{
            Foundation::WAIT_OBJECT_0,
            System::Threading::{CreateEventW, WaitForSingleObject},
        };
        let event_name = format!("Local\\screenshots-clipboard-{}", uuid::Uuid::new_v4());
        let wide = event_name.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, wide.as_ptr()) };
        ensure!(!event.is_null(), "Private provider-start event unavailable");
        let event = unsafe { OwnedHandle::from_raw_handle(event) };
        let check = || {
            if unsafe { WaitForSingleObject(event.as_raw_handle(), 0) } == WAIT_OBJECT_0 {
                Err(app_core::CaptureError::NativeCanceled)
            } else {
                Ok(())
            }
        };
        let mut stalled = command("stall", &station);
        stalled.env("PRIVATE_OUTPUT_STARTED_EVENT", &event_name);
        assert!(
            spawn_input(
                Input::Output(app_core::NativeClipboardOutput::Text("cancel")),
                false,
                stalled,
                &check
            )
            .is_err()
        );
        ensure!(
            unsafe { WaitForSingleObject(event.as_raw_handle(), 0) } == WAIT_OBJECT_0,
            "Private provider never reached the pre-hello transport boundary"
        );
        let until = Instant::now() + Duration::from_secs(5);
        while BUSY.load(Ordering::Acquire) {
            ensure!(
                Instant::now() < until,
                "Windows clipboard Job was not actually reaped"
            );
            std::thread::park_timeout(Duration::from_millis(1));
        }
        let previous_files = arboard::Clipboard::new()?.get().file_list()?;
        for cancel_blocked_write in [true, false] {
            let event_name = format!("Local\\screenshots-payload-full-{}", uuid::Uuid::new_v4());
            let wide = event_name.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
            let raw = unsafe { CreateEventW(std::ptr::null(), 1, 0, wide.as_ptr()) };
            ensure!(!raw.is_null(), "Private full-pipe event unavailable");
            let full = unsafe { OwnedHandle::from_raw_handle(raw) };
            let probe = Arc::new(transport::WriteProbe::default());
            *transport::WRITE_PROBE
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()) = Some(probe.clone());
            struct ResetProbe;
            impl Drop for ResetProbe {
                fn drop(&mut self) {
                    transport::WRITE_PROBE
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .take();
                }
            }
            let _reset = ResetProbe;
            let blocked_since = std::cell::Cell::new(None::<Instant>);
            let observed_blocked = std::cell::Cell::new(false);
            let canceled_write = std::cell::Cell::new(false);
            let check = || {
                let full = unsafe { WaitForSingleObject(full.as_raw_handle(), 0) } == WAIT_OBJECT_0;
                let pending =
                    probe.entered.load(Ordering::Acquire) > probe.finished.load(Ordering::Acquire);
                if full && pending {
                    observed_blocked.set(true);
                    let since = blocked_since.get().unwrap_or_else(|| {
                        let now = Instant::now();
                        blocked_since.set(Some(now));
                        now
                    });
                    if cancel_blocked_write && since.elapsed() >= Duration::from_millis(100) {
                        canceled_write.set(true);
                        return Err(app_core::CaptureError::NativeCanceled);
                    }
                } else {
                    blocked_since.set(None);
                }
                Ok(())
            };
            let mut stalled = command("stall-payload", &station);
            stalled.env("PRIVATE_OUTPUT_STARTED_EVENT", &event_name);
            let image = Arc::new(image::RgbaImage::from_pixel(
                1024,
                1024,
                image::Rgba([17, 31, 47, 83]),
            ));
            let started = Instant::now();
            assert!(
                spawn_input(
                    Input::Output(app_core::NativeClipboardOutput::Image(image)),
                    false,
                    stalled,
                    &check
                )
                .is_err()
            );
            ensure!(
                observed_blocked.get(),
                "Fixture never observed a full pipe with an unfinished synchronous write"
            );
            if cancel_blocked_write {
                ensure!(
                    canceled_write.get() && started.elapsed() < Duration::from_secs(2),
                    "Blocked payload write did not cancel through the owned Job"
                );
            } else {
                ensure!(
                    started.elapsed() >= Duration::from_millis(4800)
                        && started.elapsed() < Duration::from_secs(6),
                    "Blocked payload write did not return at its operation deadline"
                );
            }
            let until = Instant::now() + Duration::from_secs(5);
            while BUSY.load(Ordering::Acquire) {
                ensure!(
                    Instant::now() < until,
                    "Blocked payload Job was not actually reaped"
                );
                std::thread::park_timeout(Duration::from_millis(1));
            }
            ensure!(
                probe.entered.load(Ordering::Acquire) == probe.finished.load(Ordering::Acquire),
                "Synchronous native write remained outstanding after actual Job cleanup"
            );
            let current_files = arboard::Clipboard::new()?.get().file_list()?;
            ensure!(
                current_files == previous_files,
                "Blocked payload fixture changed the private clipboard"
            );
        }
        Ok(())
    })();
    drain_owned_workers();
    result.unwrap();
}
#[test]
#[ignore = "native Windows only: dedicated subprocess, fresh private station/desktop, actual image/text/file paste and Job cancellation"]
fn windows_private_clipboard_survives_helper_reap_and_cancellation_releases_job() -> Result<()> {
    let station = format!("screenshots-fixture-{}", uuid::Uuid::new_v4());
    // The outer controller is itself inside an owned Job. Failure/timeout retains that Job
    // through actual family cleanup, including all nested clipboard-helper Jobs.
    let container = Arc::new(windows::container()?);
    let child = windows::spawn(&command("controller", &station), &container, &[])?;
    let mut native = Native { child, container };
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(success) = native.child.try_wait()? {
            ensure!(success, "Private Windows clipboard fixture failed");
            return Ok(());
        }
        if Instant::now() >= until {
            anyhow::bail!("Private Windows clipboard fixture exceeded its finite deadline");
        }
        std::thread::park_timeout(Duration::from_millis(1));
    }
}
