#![cfg(target_os = "windows")]
//! Explicit DWM gate. Creates/captures only its external child; never reads the desktop DC.
use anyhow::{Context, Result, ensure};
use app_core::{
    CaptureBackend, CaptureCursor, CaptureOptions, CaptureWindowFrame, CaptureWindowShadow,
    CaptureWindowTransparency, XcapBackend,
};
use std::{
    io::{BufRead, BufReader, Read, Write},
    mem::size_of,
    process::{Child, Command, Stdio},
    ptr::{null, null_mut},
    sync::mpsc,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::{Dwm::*, Gdi::*},
    System::LibraryLoader::GetModuleHandleW,
    UI::{HiDpi::*, WindowsAndMessaging::*},
};

const CHILD: &str = "SCREENSHOTS_OWN_DWM_WINDOW_FIXTURE";
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
fn color(x: u32, y: u32) -> [u8; 4] {
    [
        (x * 17 + y * 3) as u8,
        (x * 5 + y * 11) as u8,
        (x * 7 + y * 19) as u8,
        255,
    ]
}
fn rectangle(rect: RECT) -> [i32; 4] {
    [rect.left, rect.top, rect.right, rect.bottom]
}
struct Dpi(DPI_AWARENESS_CONTEXT);
impl Dpi {
    fn new() -> Result<Self> {
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        ensure!(
            !previous.is_null(),
            "Per-monitor-v2 fixture context is unavailable"
        );
        Ok(Self(previous))
    }
}
impl Drop for Dpi {
    fn drop(&mut self) {
        unsafe {
            SetThreadDpiAwarenessContext(self.0);
        }
    }
}
fn paint(hwnd: HWND, dc: HDC) {
    let mut client = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut client) } == 0 {
        return;
    }
    let width = client.right - client.left;
    let height = client.bottom - client.top;
    if !(1..=1024).contains(&width) || !(1..=768).contains(&height) {
        return;
    }
    let mut bytes = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height as u32 {
        for x in 0..width as u32 {
            let p = color(x, y);
            bytes.extend_from_slice(&[p[2], p[1], p[0], 255]);
        }
    }
    let mut info = BITMAPINFO::default();
    info.bmiHeader = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width,
        biHeight: -height,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..Default::default()
    };
    unsafe {
        SetDIBitsToDevice(
            dc,
            0,
            0,
            width as u32,
            height as u32,
            0,
            0,
            0,
            height as u32,
            bytes.as_ptr().cast(),
            &info,
            DIB_RGB_COLORS,
        );
    }
}
unsafe extern "system" fn procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_PAINT => {
            let mut state = PAINTSTRUCT::default();
            let dc = unsafe { BeginPaint(hwnd, &mut state) };
            if !dc.is_null() {
                paint(hwnd, dc);
            }
            unsafe {
                EndPaint(hwnd, &state);
            }
            0
        }
        WM_PRINTCLIENT => {
            paint(hwnd, wparam as HDC);
            0
        }
        WM_ERASEBKGND => 1,
        WM_CLOSE => {
            unsafe {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_DESTROY => {
            unsafe {
                PostQuitMessage(0);
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

#[test]
fn owned_window_child() -> Result<()> {
    if std::env::var_os(CHILD).is_none() {
        return Ok(());
    }
    let _dpi = Dpi::new()?;
    let class = wide("ScreenshotsOwnedDwmCaptureFixture");
    let title = wide("Owned DWM capture fixture");
    let instance = unsafe { GetModuleHandleW(null()) };
    ensure!(!instance.is_null(), "Fixture module unavailable");
    let definition = WNDCLASSW {
        lpfnWndProc: Some(procedure),
        hInstance: instance,
        lpszClassName: class.as_ptr(),
        ..Default::default()
    };
    ensure!(
        unsafe { RegisterClassW(&definition) } != 0,
        "Cannot register owned fixture class"
    );
    let mut work = RECT::default();
    ensure!(
        unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, (&mut work as *mut RECT).cast(), 0) }
            != 0,
        "Fixture work area is unavailable"
    );
    ensure!(
        work.right - work.left >= 640 && work.bottom - work.top >= 480,
        "Native fixture requires a 640×480 work area"
    );
    let mut cursor = CURSORINFO {
        cbSize: size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    ensure!(
        unsafe { GetCursorInfo(&mut cursor) } != 0,
        "Cannot inspect cursor without moving it"
    );
    let width = 320;
    let height = 240;
    let left = if cursor.ptScreenPos.x < (work.left + work.right) / 2 {
        work.right - width - 24
    } else {
        work.left + 24
    };
    let top = if cursor.ptScreenPos.y < (work.top + work.bottom) / 2 {
        work.bottom - height - 24
    } else {
        work.top + 24
    };
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_NOACTIVATE,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            left,
            top,
            width,
            height,
            null_mut(),
            null_mut(),
            instance,
            null(),
        )
    };
    ensure!(
        !hwnd.is_null(),
        "Cannot create own decorated fixture window"
    );
    let disable_transitions = 1i32;
    ensure!(
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_TRANSITIONS_FORCEDISABLED as u32,
                (&disable_transitions as *const i32).cast(),
                size_of::<i32>() as u32,
            )
        } >= 0,
        "Cannot disable animation on the owned fixture"
    );
    unsafe {
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        UpdateWindow(hwnd);
    }
    ensure!(
        unsafe { GetDpiForWindow(hwnd) } == 96,
        "This exact native gate requires 100% display scaling"
    );
    ensure!(
        unsafe { DwmFlush() } >= 0,
        "Cannot synchronize the DWM fixture"
    );
    println!(
        "{}",
        serde_json::json!({"fixture":true,"window":hwnd as usize as u32,"pid":std::process::id()})
    );
    std::io::stdout().flush()?;
    let target = hwnd as usize;
    let stop = std::thread::spawn(move || {
        let mut byte = [0];
        let _ = std::io::stdin().read_exact(&mut byte);
        unsafe {
            PostMessageW(target as HWND, WM_CLOSE, 0, 0);
        }
    });
    let mut message = MSG::default();
    loop {
        let result = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
        ensure!(result >= 0, "Fixture native event loop failed");
        if result == 0 {
            break;
        }
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    stop.join()
        .map_err(|_| anyhow::anyhow!("Fixture stop thread failed"))?;
    unsafe {
        UnregisterClassW(class.as_ptr(), instance);
    }
    Ok(())
}
struct FixtureChild {
    child: Child,
    reader: Option<std::thread::JoinHandle<()>>,
}
impl Drop for FixtureChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
fn child() -> Result<(FixtureChild, HWND, u32)> {
    let mut child = Command::new(std::env::current_exe()?)
        .args(["--exact", "owned_window_child", "--nocapture"])
        .env(CHILD, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let stdout = child.stdout.take().context("Fixture stdout unavailable")?;
    let (tx, ready) = mpsc::sync_channel(1);
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else {
                break;
            };
            if let Some(start) = line.find('{').filter(|_| line.len() <= 4096) {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line[start..]) {
                    if value["fixture"] == true {
                        let _ = tx.try_send(value);
                    }
                }
            }
        }
    });
    let guard = FixtureChild {
        child,
        reader: Some(reader),
    };
    let value = ready
        .recv_timeout(Duration::from_secs(10))
        .context("Own DWM fixture startup deadline")?;
    let handle = value["window"]
        .as_u64()
        .context("Invalid fixture window ID")?;
    let process = value["pid"].as_u64().context("Invalid fixture PID")?;
    ensure!(
        process == u64::from(guard.child.id()) && handle > 0 && handle <= u64::from(u32::MAX),
        "Unexpected fixture identity"
    );
    let hwnd = handle as usize as HWND;
    let mut actual = 0;
    ensure!(
        unsafe { GetWindowThreadProcessId(hwnd, &mut actual) } != 0
            && actual == guard.child.id()
            && actual != std::process::id(),
        "No capture: fixture ownership is not proven"
    );
    Ok((guard, hwnd, actual))
}
struct Dc(HDC);
impl Drop for Dc {
    fn drop(&mut self) {
        unsafe {
            DeleteDC(self.0);
        }
    }
}
struct WindowDc(HWND, HDC);
impl Drop for WindowDc {
    fn drop(&mut self) {
        unsafe {
            ReleaseDC(self.0, self.1);
        }
    }
}
struct Bitmap(HBITMAP);
impl Drop for Bitmap {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.0);
        }
    }
}
struct Selection(HDC, HGDIOBJ);
impl Drop for Selection {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.0, self.1);
        }
    }
}

fn cursor_is_away(pointer: &CURSORINFO, raw: RECT) -> Result<bool> {
    if pointer.flags & CURSOR_SHOWING == 0 {
        return Ok(true);
    }
    let mut info = ICONINFO::default();
    ensure!(
        unsafe { GetIconInfo(pointer.hCursor, &mut info) } != 0,
        "Cannot inspect cursor extent without moving it"
    );
    let _mask = Bitmap(info.hbmMask);
    let _color = Bitmap(info.hbmColor);
    let handle = if info.hbmColor.is_null() {
        info.hbmMask
    } else {
        info.hbmColor
    };
    let mut bitmap = BITMAP::default();
    ensure!(
        unsafe {
            GetObjectW(
                handle,
                size_of::<BITMAP>() as i32,
                (&mut bitmap as *mut BITMAP).cast(),
            )
        } == size_of::<BITMAP>() as i32,
        "Cannot inspect cursor bitmap dimensions"
    );
    let width = i64::from(bitmap.bmWidth);
    let height = i64::from(bitmap.bmHeight) / if info.hbmColor.is_null() { 2 } else { 1 };
    ensure!(
        (1..=1024).contains(&width) && (1..=1024).contains(&height),
        "Native fixture cursor exceeds its bound"
    );
    let left = i64::from(pointer.ptScreenPos.x) - i64::from(info.xHotspot);
    let top = i64::from(pointer.ptScreenPos.y) - i64::from(info.yHotspot);
    Ok(left + width <= i64::from(raw.left)
        || left >= i64::from(raw.right)
        || top + height <= i64::from(raw.top)
        || top >= i64::from(raw.bottom))
}

/// Independent BitBlt reference: the source DC belongs only to the proven fixture HWND.
fn visible_reference(hwnd: HWND, raw: RECT, visible: RECT) -> Result<image::RgbaImage> {
    let width = visible.right - visible.left;
    let height = visible.bottom - visible.top;
    ensure!(
        (1..=1024).contains(&width) && (1..=768).contains(&height),
        "Fixture frame exceeds its bound"
    );
    let source = WindowDc(hwnd, unsafe { GetWindowDC(hwnd) });
    ensure!(!source.1.is_null(), "Own window DC unavailable");
    let destination = Dc(unsafe { CreateCompatibleDC(source.1) });
    ensure!(
        !destination.0.is_null(),
        "Fixture bitmap context unavailable"
    );
    let mut info = BITMAPINFO::default();
    info.bmiHeader = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width,
        biHeight: -height,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..Default::default()
    };
    let mut data = null_mut();
    let bitmap = Bitmap(unsafe {
        CreateDIBSection(
            destination.0,
            &info,
            DIB_RGB_COLORS,
            &mut data,
            null_mut(),
            0,
        )
    });
    ensure!(
        !bitmap.0.is_null() && !data.is_null(),
        "Fixture bitmap allocation failed"
    );
    let old = unsafe { SelectObject(destination.0, bitmap.0) };
    ensure!(
        !old.is_null() && old as isize != -1,
        "Cannot select fixture bitmap"
    );
    let _selection = Selection(destination.0, old);
    let bytes = unsafe {
        std::slice::from_raw_parts_mut(data.cast::<u8>(), width as usize * height as usize * 4)
    };
    bytes.fill(0);
    ensure!(
        unsafe {
            BitBlt(
                destination.0,
                0,
                0,
                width,
                height,
                source.1,
                visible.left - raw.left,
                visible.top - raw.top,
                SRCCOPY,
            )
        } != 0,
        "Owned-window reference BitBlt failed"
    );
    ensure!(unsafe { GdiFlush() } != 0, "Fixture reference flush failed");
    let mut output = bytes.to_vec();
    for pixel in output.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        pixel[3] = 255;
    }
    image::RgbaImage::from_raw(width as u32, height as u32, output)
        .context("Invalid fixture reference pixels")
}
fn equal(actual: &image::RgbaImage, expected: &image::RgbaImage, label: &str) -> Result<()> {
    ensure!(
        actual.dimensions() == expected.dimensions(),
        "{label}: incorrect physical dimensions"
    );
    ensure!(
        actual.as_raw() == expected.as_raw(),
        "{label}: exact owned-window pixels differ"
    );
    Ok(())
}

#[test]
#[ignore = "explicit native Windows DWM gate; creates/captures only its own child window, never the desktop or input"]
fn decorated_window_policies_preserve_exact_dwm_frame_and_client_pixels() -> Result<()> {
    let _dpi = Dpi::new()?;
    let mut enabled = 0;
    ensure!(
        unsafe { DwmIsCompositionEnabled(&mut enabled) } >= 0 && enabled != 0,
        "Genuine DWM composition is unavailable; this gate has not passed"
    );
    let (mut fixture, hwnd, _pid) = child()?;
    let mut raw = RECT::default();
    let mut visible = RECT::default();
    let mut client = RECT::default();
    ensure!(
        unsafe { GetWindowRect(hwnd, &mut raw) } != 0
            && unsafe { GetClientRect(hwnd, &mut client) } != 0,
        "Own fixture geometry unavailable"
    );
    ensure!(
        unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS as u32,
                (&mut visible as *mut RECT).cast(),
                size_of::<RECT>() as u32,
            )
        } >= 0,
        "Own fixture DWM frame unavailable"
    );
    ensure!(
        raw.left < visible.left
            || raw.top < visible.top
            || raw.right > visible.right
            || raw.bottom > visible.bottom,
        "Fixture has no invisible DWM resize border; the regression is not exercised"
    );
    ensure!(
        visible.left >= raw.left
            && visible.top >= raw.top
            && visible.right <= raw.right
            && visible.bottom <= raw.bottom,
        "Unexpected own fixture DWM geometry"
    );
    let mut start = POINT {
        x: client.left,
        y: client.top,
    };
    let mut end = POINT {
        x: client.right,
        y: client.bottom,
    };
    ensure!(
        unsafe { ClientToScreen(hwnd, &mut start) } != 0
            && unsafe { ClientToScreen(hwnd, &mut end) } != 0,
        "Cannot map own fixture client coordinates"
    );
    ensure!(
        start.x >= visible.left
            && start.y >= visible.top
            && end.x <= visible.right
            && end.y <= visible.bottom
            && start.x < end.x
            && start.y < end.y,
        "Own fixture client does not fit its visible frame"
    );
    let reference = visible_reference(hwnd, raw, visible)?;
    let expected_client = image::RgbaImage::from_fn(
        (client.right - client.left) as u32,
        (client.bottom - client.top) as u32,
        |x, y| image::Rgba(color(x, y)),
    );
    let reference_client = image::imageops::crop_imm(
        &reference,
        (start.x - visible.left) as u32,
        (start.y - visible.top) as u32,
        expected_client.width(),
        expected_client.height(),
    )
    .to_image();
    equal(
        &reference_client,
        &expected_client,
        "Independent fixture client reference",
    )?;
    let mut pointer = CURSORINFO {
        cbSize: size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    ensure!(
        unsafe { GetCursorInfo(&mut pointer) } != 0,
        "Cannot inspect native pointer without moving it"
    );
    ensure!(
        cursor_is_away(&pointer, raw)?,
        "Cursor raster overlaps the fixture; no test input will be injected"
    );
    let id = (hwnd as usize as u32).to_string();
    for (label, options, client_only) in [
        (
            "included frame",
            CaptureOptions {
                window_frame: CaptureWindowFrame::Include,
                window_shadow: CaptureWindowShadow::Exclude,
                ..Default::default()
            },
            false,
        ),
        (
            "client-only",
            CaptureOptions {
                window_frame: CaptureWindowFrame::Client,
                ..Default::default()
            },
            true,
        ),
        (
            "cursor included outside fixture",
            CaptureOptions {
                cursor: CaptureCursor::Include,
                ..Default::default()
            },
            false,
        ),
        (
            "opaque background",
            CaptureOptions {
                window_transparency: CaptureWindowTransparency::Opaque,
                background: Some([31, 47, 59]),
                ..Default::default()
            },
            false,
        ),
        ("default GDI capture", CaptureOptions::default(), false),
    ] {
        let frame = XcapBackend::new(std::path::PathBuf::new())
            .with_options(options)?
            .capture_window(&id)?;
        equal(
            &frame.pixels,
            if client_only {
                &expected_client
            } else {
                &reference
            },
            label,
        )?;
    }
    let mut current = RECT::default();
    ensure!(
        unsafe { GetWindowRect(hwnd, &mut current) } != 0 && rectangle(current) == rectangle(raw),
        "Fixture moved/resized during acceptance"
    );
    fixture
        .child
        .stdin
        .as_mut()
        .context("Fixture control unavailable")?
        .write_all(b"Q")?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let exit = loop {
        if let Some(exit) = fixture.child.try_wait()? {
            break exit;
        }
        ensure!(Instant::now() < deadline, "Own window teardown deadline");
        std::thread::sleep(Duration::from_millis(10));
    };
    ensure!(exit.success(), "Own fixture child failed");
    Ok(())
}
