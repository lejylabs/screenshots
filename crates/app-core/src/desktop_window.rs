//! Native visibility for persistent Windows overlay windows, called on the UI thread.
use anyhow::{Result, ensure};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IsWindow, IsWindowVisible, SW_HIDE, SW_SHOWNOACTIVATE, ShowWindow,
};

pub struct DesktopWindow(isize);

impl DesktopWindow {
    pub fn from_handle(handle: isize) -> Self {
        Self(handle)
    }

    pub fn set_visible(&self, visible: bool) -> Result<()> {
        let handle = self.0 as windows_sys::Win32::Foundation::HWND;
        // ShowWindow returns previous visibility, not a success code.
        unsafe {
            ensure!(IsWindow(handle) != 0, "application window no longer exists");
            ShowWindow(handle, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
            ensure!(
                (IsWindowVisible(handle) != 0) == visible,
                "could not change application window visibility"
            );
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

    #[test]
    fn rejects_invalid_handle() {
        let window = DesktopWindow::from_handle(0);
        assert!(window.set_visible(false).is_err());
        assert!(window.set_visible(true).is_err());
    }

    #[test]
    #[ignore = "requires an active Windows desktop compositor"]
    fn synchronizes_desktop_compositor() -> Result<()> {
        wait_for_desktop_compositor()
    }
}
