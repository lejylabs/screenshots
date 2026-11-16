//! Bounded target-window wheel delivery without global input injection.
use super::{Geometry, Region};
use anyhow::{Context, Result, ensure};
use windows_sys::Win32::{
    Foundation::RECT,
    Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute},
    UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId, IsIconic, IsWindowVisible, SMTO_ABORTIFHUNG,
        SMTO_BLOCK, SendMessageTimeoutW, WM_MOUSEWHEEL,
    },
};

pub(super) struct Native {
    window: windows_sys::Win32::Foundation::HWND,
    _dpi: crate::region_analysis::DpiContext,
}
impl Native {
    pub(super) fn new(id: u32) -> Result<Self> {
        Ok(Self {
            window: id as usize as _,
            _dpi: crate::region_analysis::DpiContext::enter()?,
        })
    }
    pub(super) fn geometry(&self) -> Result<Geometry> {
        let mut pid = 0;
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        unsafe {
            ensure!(
                GetForegroundWindow() == self.window,
                "Scrolling target lost foreground focus"
            );
            ensure!(
                IsWindowVisible(self.window) != 0 && IsIconic(self.window) == 0,
                "Scrolling target is unavailable or minimized"
            );
            ensure!(
                GetWindowThreadProcessId(self.window, &mut pid) != 0
                    && pid != 0
                    && pid != std::process::id(),
                "Scrolling requires a different identified application window"
            );
            ensure!(
                DwmGetWindowAttribute(
                    self.window,
                    DWMWA_EXTENDED_FRAME_BOUNDS as u32,
                    (&mut rect as *mut RECT).cast(),
                    std::mem::size_of::<RECT>() as u32
                ) >= 0,
                "Could not inspect scrolling window geometry"
            );
        }
        let width = rect
            .right
            .checked_sub(rect.left)
            .filter(|v| *v > 0)
            .context("Invalid scrolling window width")?;
        let height = rect
            .bottom
            .checked_sub(rect.top)
            .filter(|v| *v > 0)
            .context("Invalid scrolling window height")?;
        Ok(Geometry {
            x: rect.left,
            y: rect.top,
            width: width as u32,
            height: height as u32,
            pid,
        })
    }
    pub(super) fn wheel(&self, expected: Geometry, region: Region, notches: u32) -> Result<()> {
        ensure!(
            self.geometry()? == expected,
            "Scrolling target geometry or process changed"
        );
        let (wparam, lparam) = wheel_message(expected, region, notches)?;
        let mut result = 0;
        ensure!(
            unsafe {
                SendMessageTimeoutW(
                    self.window,
                    WM_MOUSEWHEEL,
                    wparam,
                    lparam,
                    SMTO_ABORTIFHUNG | SMTO_BLOCK,
                    250,
                    &mut result,
                )
            } != 0,
            "Scrolling target did not accept the wheel message within 250 ms"
        );
        Ok(())
    }
}

fn wheel_message(expected: Geometry, region: Region, notches: u32) -> Result<(usize, isize)> {
    ensure!(
        (1..=10).contains(&notches),
        "Scrolling wheel count is out of range"
    );
    let x = expected
        .x
        .checked_add(i32::try_from(region.x + region.width / 2)?)
        .context("Scrolling point overflow")?;
    let y = expected
        .y
        .checked_add(i32::try_from(region.y + region.height / 2)?)
        .context("Scrolling point overflow")?;
    ensure!(
        i16::try_from(x).is_ok() && i16::try_from(y).is_ok(),
        "Scrolling point exceeds native wheel coordinates"
    );
    let delta = -(i32::try_from(notches)? * 120);
    let wparam = usize::from(delta as u16) << 16;
    let lparam = (u32::from(x as u16) | (u32::from(y as u16) << 16)) as isize;
    Ok((wparam, lparam))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wheel_arguments_use_physical_captured_frame_origin_and_signed_coordinates() {
        let geometry = Geometry {
            x: -1920,
            y: -200,
            width: 1280,
            height: 720,
            pid: 42,
        };
        let region = Region {
            x: 20,
            y: 30,
            width: 100,
            height: 80,
        };
        let (wheel, point) = wheel_message(geometry, region, 3).unwrap();
        assert_eq!(((wheel >> 16) as u16) as i16, -360);
        assert_eq!((point as u16) as i16, -1850);
        assert_eq!(((point as u32 >> 16) as u16) as i16, -130);
        assert_eq!(wheel & 0xffff, 0);
        assert!(wheel_message(geometry, region, 0).is_err());
        assert!(
            wheel_message(
                Geometry {
                    x: 32760,
                    ..geometry
                },
                region,
                1
            )
            .is_err()
        );
    }
}
