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
