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
    let _private = Private::enter(
        &station,
        matches!(mode.as_str(), "controller" | "controller-4k"),
    )
    .unwrap();
    if mode == "native" {
        try_child(&[output::CHILD.into()]).unwrap();
        return;
    }
    if mode == "controller-4k" {
        let result = (|| -> Result<()> {
            let mut seed = 0x12345678u32;
            let pixels = Arc::new(image::RgbaImage::from_fn(3840, 2160, |_, _| {
                let mut rgba = [0; 4];
                for byte in &mut rgba {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    *byte = seed as u8;
                }
                image::Rgba(rgba)
            }));
            let owner = spawn_input(
                Input::Output(app_core::NativeClipboardOutput::Image(pixels.clone())),
                false,
                command("native", &station),
                &|| Ok(()),
            )?;
            owner.retire()?;
            let formats = image_windows::read_owned_formats()?;
            ensure!(
                formats
                    .png
                    .as_ref()
                    .is_some_and(|bytes| bytes.len() > 15 * 1024 * 1024),
                "The 4K fixture did not exercise an incompressible PNG above the rejected fixed cutoff"
            );
            drop(formats);
            assert_owned_image(&pixels, true)
        })();
        drain_owned_workers();
        result.unwrap();
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
        io.write_all(b"SCOUTP02").unwrap();
        io.write_all(&nonce).unwrap();
        let mut header = [0; 42];
        io.read_exact(&mut header).unwrap();
        assert_eq!(&header[..8], b"SCOUTP02");
        assert_eq!(&header[8..24], nonce);
        assert_eq!(header[24], 1);
        assert_eq!(
            u64::from_be_bytes(header[33..41].try_into().unwrap()),
            4 * 1024 * 1024
        );
        let prepared = image_windows::prepare(
            &mut io,
            u32::from_be_bytes(header[25..29].try_into().unwrap()),
            u32::from_be_bytes(header[29..33].try_into().unwrap()),
            4 * 1024 * 1024,
        )
        .unwrap();
        io.write_all(&[3]).unwrap();
        let mut gate = [0];
        io.read_exact(&mut gate).unwrap();
        assert_eq!(gate, [1]);
        // This is a real private PNG transfer, not a fabricated setter-success acknowledgement.
        let _publishing = prepared.begin().unwrap();
        io.write_all(&[4]).unwrap();
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
        // No raw DIB byte is drained and no full-image success acknowledgement is fabricated.
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
        assert_owned_image(&pixels, true)?;
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
            let pasted_files = reader.get().file_list()?;
            assert_eq!(pasted_files.len(), 1);
            assert_eq!(
                std::fs::canonicalize(&pasted_files[0])?,
                std::fs::canonicalize(&path)?
            );
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
            let failure = spawn_input(
                Input::Output(app_core::NativeClipboardOutput::Image(image)),
                false,
                stalled,
                &check,
            )
            .err()
            .context("Blocked image output incorrectly reported success")?;
            ensure!(
                failure.to_string().contains("PNG may already be available"),
                "Partial image failure was not reported explicitly"
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
            let partial = image::RgbaImage::from_pixel(1024, 1024, image::Rgba([17, 31, 47, 83]));
            assert_owned_image(&partial, false)?;
        }
        native_preparation_and_partial_format_faults()?;
        Ok(())
    })();
    drain_owned_workers();
    result.unwrap();
}
#[test]
#[ignore = "native Windows only: dedicated subprocess, fresh private station/desktop, actual image/text/file paste and Job cancellation"]
fn windows_private_clipboard_survives_helper_reap_and_cancellation_releases_job() -> Result<()> {
    run_owned_controller("controller")
}
#[test]
#[ignore = "native Windows only: owned private station/Job, noisy 4K PNG+DIBV5 paste after helper reap"]
fn incompressible_4k_image_keeps_both_native_formats_without_a_fixed_png_cutoff() -> Result<()> {
    run_owned_controller("controller-4k")
}
fn run_owned_controller(mode: &str) -> Result<()> {
    let station = format!("screenshots-fixture-{}", uuid::Uuid::new_v4());
    // The outer controller is itself inside an owned Job. Failure/timeout retains that Job
    // through actual family cleanup, including all nested clipboard-helper Jobs.
    let container = Arc::new(windows::container()?);
    let (child, mut stderr) =
        windows::spawn_fixture_diagnostic(&command(mode, &station), &container)?;
    let mut native = Native { child, container };
    let mut diagnostic = Vec::new();
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        drain_fixture_diagnostic(&mut stderr, &mut diagnostic)?;
        if let Some(status) = native.child.fixture_status()? {
            ensure!(
                status.success(),
                "Private Windows clipboard fixture {mode} failed ({status}): {}",
                String::from_utf8_lossy(&diagnostic)
            );
            return Ok(());
        }
        if Instant::now() >= until {
            anyhow::bail!("Private Windows clipboard fixture exceeded its finite deadline");
        }
        std::thread::park_timeout(Duration::from_millis(1));
    }
}

fn drain_fixture_diagnostic(pipe: &mut std::fs::File, prefix: &mut Vec<u8>) -> Result<()> {
    use std::{io::Read, os::windows::io::AsRawHandle};
    use windows_sys::Win32::{
        Foundation::{ERROR_BROKEN_PIPE, GetLastError},
        System::Pipes::PeekNamedPipe,
    };
    // Only synthetic private-fixture stderr; drain excess without blocking its owned Job.
    for _ in 0..4 {
        let mut available = 0;
        if unsafe {
            PeekNamedPipe(
                pipe.as_raw_handle(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut available,
                std::ptr::null_mut(),
            )
        } == 0
        {
            ensure!(
                unsafe { GetLastError() } == ERROR_BROKEN_PIPE,
                "Private fixture stderr inspection failed"
            );
            return Ok(());
        }
        if available == 0 {
            return Ok(());
        }
        let mut chunk = [0; 4096];
        let count = pipe.read(&mut chunk[..(available as usize).min(4096)])?;
        let retain = count.min((16 * 1024usize).saturating_sub(prefix.len()));
        prefix.extend_from_slice(&chunk[..retain]);
        if count == 0 {
            return Ok(());
        }
    }
    Ok(())
}

fn framed_png(pixels: &image::RgbaImage) -> Result<Vec<u8>> {
    let mut png = Vec::new();
    output::encode_png_stream(pixels, &mut png, &|| Ok(()))?;
    let mut framed = Vec::new();
    for chunk in png.chunks(64 * 1024) {
        framed.extend_from_slice(&(chunk.len() as u32).to_be_bytes());
        framed.extend_from_slice(chunk);
    }
    framed.extend_from_slice(&0u32.to_be_bytes());
    Ok(framed)
}
fn publish_direct(pixels: &image::RgbaImage) -> Result<()> {
    let mut framed = std::io::Cursor::new(framed_png(pixels)?);
    let prepared = image_windows::prepare(
        &mut framed,
        pixels.width(),
        pixels.height(),
        pixels.as_raw().len(),
    )?;
    let mut publication = prepared.begin()?;
    publication.read_rows(&mut std::io::Cursor::new(pixels.as_raw()))?;
    publication.finish()
}
fn assert_owned_image(pixels: &image::RgbaImage, complete: bool) -> Result<()> {
    let formats = image_windows::read_owned_formats()?;
    let png = formats.png.context("Owned PNG format is missing")?;
    let decoded = image::load_from_memory(&png)?.into_rgba8();
    ensure!(
        &decoded == pixels,
        "Owned PNG/alpha differs after helper exit"
    );
    // The OS may synthesize formats; partial output never claims DIBV5 was transferred by us.
    if !complete {
        return Ok(());
    }
    let dib = formats.dib.context("Owned DIBV5 format is missing")?;
    let word = |offset: usize| u32::from_le_bytes(dib[offset..offset + 4].try_into().unwrap());
    ensure!(
        dib.len() >= 124 + pixels.as_raw().len()
            && word(0) == 124
            && word(4) == pixels.width()
            && word(8) == pixels.height()
            && word(16) == 3
            && word(20) as usize == pixels.as_raw().len()
            && dib[12..16] == [1, 0, 32, 0]
            && word(40) == 0x00ff0000
            && word(44) == 0x0000ff00
            && word(48) == 0x000000ff
            && word(52) == 0xff000000
            && word(56) == 0x73524742
            && word(108) == 4
            && word(112) == 0
            && word(116) == 0,
        "Owned DIBV5 extent/masks/sRGB differ"
    );
    for y in 0..pixels.height() {
        for x in 0..pixels.width() {
            let rgba = pixels.get_pixel(x, pixels.height() - 1 - y).0;
            let offset = 124 + (y as usize * pixels.width() as usize + x as usize) * 4;
            ensure!(
                dib[offset..offset + 4] == [rgba[2], rgba[1], rgba[0], rgba[3]],
                "Owned DIBV5 rows/alpha differ"
            );
        }
    }
    Ok(())
}
fn native_preparation_and_partial_format_faults() -> Result<()> {
    use image_windows::stats;
    let previous = image::RgbaImage::from_pixel(3, 2, image::Rgba([91, 71, 51, 31]));
    let next = image::RgbaImage::from_fn(3, 2, |x, y| image::Rgba([x as u8, y as u8, 47, 83]));
    for fault in 1..=8 {
        stats::FAULT.store(0, Ordering::Release);
        publish_direct(&previous)?;
        stats::FAULT.store(fault, Ordering::Release);
        let result = publish_direct(&next);
        stats::FAULT.store(0, Ordering::Release);
        ensure!(
            result.is_err(),
            "Private native publication fault did not fail"
        );
        ensure!(
            stats::FREE_FAILED.load(Ordering::Acquire) == 0
                && stats::APP_BYTES.load(Ordering::Acquire) == 0,
            "Failed native preparation retained an application-owned allocation"
        );
        ensure!(
            stats::ALLOCATED.load(Ordering::Acquire)
                == stats::FREED.load(Ordering::Acquire)
                    + stats::TRANSFERRED.load(Ordering::Acquire),
            "Native allocation did not end in exactly free or successful ownership transfer"
        );
        if fault <= 3 {
            assert_owned_image(&previous, true)?;
        } else if fault == 4 {
            let empty = image_windows::read_owned_formats()?;
            ensure!(
                empty.png.is_none() && empty.dib.is_none(),
                "Failure after EmptyClipboard fabricated a format"
            );
        } else {
            assert_owned_image(&next, fault == 8)?;
        }
    }
    Ok(())
}
