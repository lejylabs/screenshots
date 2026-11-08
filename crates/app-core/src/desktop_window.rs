//! Native visibility for persistent Windows overlay windows, called on the UI thread.
use anyhow::{Result, ensure};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IsWindow, IsWindowVisible, SW_HIDE, SW_SHOWNOACTIVATE, ShowWindowAsync,
};

#[derive(Clone, Copy)]
pub struct DesktopWindow(isize);

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
        let window = DesktopWindow::from_handle(0);
        assert!(window.request_visible(false).is_err());
        assert!(window.request_visible(true).is_err());
        assert!(window.wait_until_hidden().is_err());
    }

    #[test]
    #[ignore = "requires an active Windows desktop compositor"]
    fn synchronizes_desktop_compositor() -> Result<()> {
        wait_for_desktop_compositor()
    }
}
