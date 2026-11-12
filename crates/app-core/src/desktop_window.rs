//! Native visibility for persistent Windows overlay windows, called on the UI thread.
use anyhow::{Result, ensure};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IsWindow, IsWindowVisible, SW_HIDE, SW_SHOWNOACTIVATE, ShowWindowAsync,
};

/// Snapshot the foreground target before hiding UI or attaching graphics.
pub fn foreground_window_capture() -> Result<crate::CaptureRequest> {
    window_capture_request(unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow()
    })
}

fn window_capture_request(
    handle: windows_sys::Win32::Foundation::HWND,
) -> Result<crate::CaptureRequest> {
    let process_id = capture_window_process(handle)?;
    // XCap's Windows window ID is the low 32 bits of HWND.
    Ok(crate::CaptureRequest::ForegroundWindow {
        window_id: (handle as usize as u32).to_string(),
        process_id,
    })
}

/// Snapshot the active application's display before hiding UI or attaching graphics.
pub fn foreground_display_capture() -> Result<crate::CaptureRequest> {
    display_capture_request(unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow()
    })
}

fn display_capture_request(handle: windows_sys::Win32::Foundation::HWND) -> Result<crate::CaptureRequest> {
    capture_window_process(handle)?;
    use windows_sys::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONULL};
    let monitor = unsafe { MonitorFromWindow(handle, MONITOR_DEFAULTTONULL) };
    ensure!(!monitor.is_null(), "The active window does not intersect an available display");
    // XCap's Windows monitor ID is the low 32 bits of HMONITOR.
    Ok(crate::CaptureRequest::Display((monitor as usize as u32).to_string()))
}

fn capture_window_process(handle: windows_sys::Win32::Foundation::HWND) -> Result<u32> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetDesktopWindow, GetShellWindow, GetWindowThreadProcessId, IsIconic,
    };
    ensure!(
        !handle.is_null() && unsafe { IsWindow(handle) } != 0,
        "No active window is available"
    );
    ensure!(
        unsafe { IsWindowVisible(handle) } != 0 && unsafe { IsIconic(handle) } == 0,
        "The active window is hidden or minimized"
    );
    let mut class = [0u16; 64];
    let class_length = unsafe { GetClassNameW(handle, class.as_mut_ptr(), class.len() as i32) };
    ensure!(class_length > 0, "The active window is no longer available");
    ensure!(
        handle != unsafe { GetDesktopWindow() }
            && handle != unsafe { GetShellWindow() }
            && ![
                "Progman",
                "WorkerW",
                "Shell_TrayWnd",
                "Shell_SecondaryTrayWnd"
            ]
            .iter()
            .any(|name| class[..class_length as usize]
                .iter()
                .copied()
                .eq(name.encode_utf16())),
        "Focus an application window instead of the desktop or taskbar"
    );
    let mut process_id = 0;
    ensure!(
        unsafe { GetWindowThreadProcessId(handle, &mut process_id) } != 0 && process_id != 0,
        "The active window is no longer available"
    );
    ensure!(
        process_id != std::process::id(),
        "Focus another application's window before capturing"
    );
    Ok(process_id)
}

#[derive(Clone, Copy)]
pub struct DesktopWindow(isize);

pub fn cursor_position() -> Option<[i32; 2]> {
    let mut point = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
    (unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut point) } != 0)
        .then_some([point.x, point.y])
}

impl DesktopWindow {
    pub fn from_handle(handle: isize) -> Self {
        Self(handle)
    }

    pub fn request_visible(&self, visible: bool) -> Result<()> {
        let handle = self.0 as windows_sys::Win32::Foundation::HWND;
        // Queue visibility messages so they cannot re-enter borrowed GPUI state.
        unsafe {
            ensure!(IsWindow(handle) != 0, "application window no longer exists");
            ensure!(
                ShowWindowAsync(handle, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE }) != 0,
                "could not request application window visibility change: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(())
    }

    /// Wait on the capture worker, keeping the UI thread free to process the request.
    pub fn wait_until_hidden(&self) -> Result<()> {
        let started = std::time::Instant::now();
        loop {
            let handle = self.0 as windows_sys::Win32::Foundation::HWND;
            unsafe {
                ensure!(
                    IsWindow(handle) != 0,
                    "application window disappeared before capture"
                );
                if IsWindowVisible(handle) == 0 {
                    return Ok(());
                }
            }
            ensure!(
                started.elapsed() < std::time::Duration::from_secs(2),
                "application window did not hide before capture"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// GPUI destroys HWNDs asynchronously; finish that work before detaching the platform.
    pub fn wait_until_destroyed(&self) -> Result<()> {
        let started = std::time::Instant::now();
        while unsafe { IsWindow(self.0 as windows_sys::Win32::Foundation::HWND) } != 0 {
            ensure!(
                started.elapsed() < std::time::Duration::from_secs(2),
                "application window did not finish closing"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Ok(())
    }
}

/// Call on the capture worker after the UI has hidden its windows.
pub fn wait_for_desktop_compositor() -> Result<()> {
    let status = unsafe { windows_sys::Win32::Graphics::Dwm::DwmFlush() };
    ensure!(status >= 0, "DwmFlush failed: 0x{:08X}", status as u32);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::*,
    };

    #[test]
    #[ignore = "requires an explicitly supplied external solid-color window fixture; captures only that window into RAM"]
    fn captures_only_the_explicit_window_fixture() -> Result<()> {
        capture_fixture(false)
    }

    #[test]
    #[ignore = "requires an explicitly supplied external fixture that can acquire foreground focus; captures only that window into RAM"]
    fn captures_only_the_expected_foreground_fixture() -> Result<()> {
        capture_fixture(true)
    }

    fn capture_fixture(require_foreground: bool) -> Result<()> {
        let expected_process: u32 =
            std::env::var("SCREENSHOTS_ACTIVE_WINDOW_FIXTURE_PID")?.parse()?;
        let handle =
            std::env::var("SCREENSHOTS_ACTIVE_WINDOW_FIXTURE_HANDLE")?.parse::<isize>()? as HWND;
        let request = if require_foreground {
            let ready = std::time::Instant::now();
            while unsafe { GetForegroundWindow() } != handle {
                ensure!(
                    ready.elapsed() < std::time::Duration::from_secs(2),
                    "The explicit fixture is not foreground; no capture attempted"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            foreground_window_capture()?
        } else {
            window_capture_request(handle)?
        };
        let crate::CaptureRequest::ForegroundWindow {
            window_id,
            process_id,
        } = request
        else {
            anyhow::bail!("Unexpected capture request");
        };
        ensure!(
            process_id == expected_process,
            "The foreground window is not the explicit fixture; no capture attempted"
        );
        let backend =
            crate::XcapBackend::new(std::env::temp_dir().join("screenshots-unused-fixture-output"));
        let started = std::time::Instant::now();
        let shots = backend.capture_batch(vec![crate::CaptureRequest::ForegroundWindow {
            window_id: window_id.clone(),
            process_id,
        }])?;
        let capture_elapsed = started.elapsed();
        let shot = &shots[0];
        ensure!(
            shot.window_id.as_ref() == Some(&window_id),
            "Wrong captured window"
        );
        ensure!(
            shot.width > 0 && shot.width <= 1024 && shot.height > 0 && shot.height <= 1024,
            "Unexpected fixture dimensions"
        );
        ensure!(!shot.png_path.exists(), "Capture persisted a source frame");
        ensure!(
            backend
                .capture_batch(vec![crate::CaptureRequest::ForegroundWindow {
                    window_id: window_id.clone(),
                    process_id: process_id.wrapping_add(1)
                }])
                .is_err(),
            "A different process was accepted"
        );
        let handle = window_id.parse::<u32>()? as usize as HWND;
        let mut current_process = 0;
        ensure!(
            unsafe { GetWindowThreadProcessId(handle, &mut current_process) } != 0
                && current_process == expected_process,
            "Fixture ownership changed; refusing to close it"
        );
        ensure!(
            unsafe { PostMessageW(handle, WM_CLOSE, 0, 0) } != 0,
            "Could not close the synthetic fixture"
        );
        DesktopWindow::from_handle(handle as isize).wait_until_destroyed()?;
        ensure!(
            backend
                .capture_batch(vec![crate::CaptureRequest::ForegroundWindow {
                    window_id,
                    process_id
                }])
                .is_err(),
            "A closed foreground target was accepted"
        );
        eprintln!(
            "Synthetic window capture/preparation: {:?}; complete fixture checks: {:?}",
            capture_elapsed,
            started.elapsed()
        );
        Ok(())
    }

    thread_local! {
        static REQUESTING: Cell<bool> = const { Cell::new(false) };
        static REENTERED: Cell<bool> = const { Cell::new(false) };
    }

    unsafe extern "system" fn test_window_proc(
        hwnd: HWND,
        message: u32,
        w: WPARAM,
        l: LPARAM,
    ) -> LRESULT {
        if message == WM_SHOWWINDOW && REQUESTING.with(Cell::get) {
            REENTERED.with(|value| value.set(true));
        }
        unsafe { DefWindowProcW(hwnd, message, w, l) }
    }

    #[test]
    #[ignore = "creates an off-screen native window and pumps Windows messages"]
    fn visibility_requests_do_not_reenter_and_worker_observes_hide() -> Result<()> {
        let class: Vec<u16> = "screenshots-visibility-test\0".encode_utf16().collect();
        let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
        let descriptor = WNDCLASSW {
            lpfnWndProc: Some(test_window_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..unsafe { std::mem::zeroed() }
        };
        ensure!(
            unsafe { RegisterClassW(&descriptor) } != 0,
            "register test window: {}",
            std::io::Error::last_os_error()
        );
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class.as_ptr(),
                class.as_ptr(),
                WS_POPUP,
                -32000,
                -32000,
                1,
                1,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            )
        };
        if hwnd.is_null() {
            unsafe {
                UnregisterClassW(class.as_ptr(), instance);
            }
            anyhow::bail!("create test window: {}", std::io::Error::last_os_error());
        }
        struct Cleanup(HWND);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                unsafe {
                    DestroyWindow(self.0);
                }
            }
        }
        let cleanup = Cleanup(hwnd);
        let window = DesktopWindow::from_handle(hwnd as isize);
        let result = (|| -> Result<()> {
            for visible in [true, false] {
                REQUESTING.with(|value| value.set(true));
                let request = window.request_visible(visible);
                REQUESTING.with(|value| value.set(false));
                request?;
                ensure!(
                    !REENTERED.with(Cell::get),
                    "visibility request synchronously re-entered the window callback"
                );
                let worker =
                    (!visible).then(|| std::thread::spawn(move || window.wait_until_hidden()));
                let started = std::time::Instant::now();
                loop {
                    let mut message: MSG = unsafe { std::mem::zeroed() };
                    while unsafe {
                        PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE)
                    } != 0
                    {
                        unsafe {
                            TranslateMessage(&message);
                            DispatchMessageW(&message);
                        }
                    }
                    if (unsafe { IsWindowVisible(hwnd) } != 0) == visible {
                        break;
                    }
                    ensure!(
                        started.elapsed() < std::time::Duration::from_secs(2),
                        "test visibility did not change"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                if visible {
                    ensure!(
                        window_capture_request(hwnd).err().is_some_and(|error| error
                            .to_string()
                            .contains("Focus another application's window")),
                        "Own window was not rejected"
                    );
                }
                if let Some(worker) = worker {
                    worker.join().expect("visibility worker")?;
                }
            }
            Ok(())
        })();
        drop(cleanup);
        ensure!(
            unsafe { IsWindow(hwnd) } == 0,
            "test window was not destroyed"
        );
        ensure!(
            unsafe { UnregisterClassW(class.as_ptr(), instance) } != 0,
            "test window class was not released"
        );
        result
    }

    #[test]
    fn rejects_invalid_handle() {
        assert!(window_capture_request(std::ptr::null_mut()).is_err());
        assert!(window_capture_request(unsafe { GetDesktopWindow() }).is_err());
        assert!(window_capture_request(unsafe { GetShellWindow() }).is_err());
        assert!(display_capture_request(std::ptr::null_mut()).is_err());
        assert!(display_capture_request(unsafe { GetDesktopWindow() }).is_err());
        assert!(display_capture_request(unsafe { GetShellWindow() }).is_err());
        let window = DesktopWindow::from_handle(0);
        assert!(window.request_visible(false).is_err());
        assert!(window.request_visible(true).is_err());
        assert!(window.wait_until_hidden().is_err());
        assert!(window.wait_until_destroyed().is_ok());
    }

    #[test]
    #[ignore = "requires an explicitly supplied external foreground-window fixture and expected monitor ID; metadata only"]
    fn foreground_display_resolves_only_the_explicit_fixture_monitor() -> Result<()> {
        let handle = std::env::var("SCREENSHOTS_ACTIVE_WINDOW_FIXTURE_HANDLE")?.parse::<isize>()? as HWND;
        let expected_monitor = std::env::var("SCREENSHOTS_ACTIVE_DISPLAY_FIXTURE_MONITOR_ID")?;
        ensure!(unsafe { GetForegroundWindow() } == handle, "Explicit fixture is not foreground");
        let crate::CaptureRequest::Display(id) = foreground_display_capture()? else {
            anyhow::bail!("Unexpected capture request");
        };
        ensure!(id == expected_monitor, "Unexpected display; no pixels captured");
        Ok(())
    }

    #[test]
    #[ignore = "requires an active Windows desktop compositor"]
    fn synchronizes_desktop_compositor() -> Result<()> {
        wait_for_desktop_compositor()
    }
}
