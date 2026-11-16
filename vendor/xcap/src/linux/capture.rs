use image::RgbaImage;

use crate::error::{XCapError, XCapResult};

#[path = "physical_region_bounds.rs"]
mod physical_region_bounds;

use super::{
    impl_monitor::ImplMonitor,
    impl_window::ImplWindow,
    utils::{get_current_screen_buf, get_monitor_info_buf, wayland_detect},
    wayland_capture::wayland_capture,
    xorg_capture::xorg_capture,
};

pub fn capture_monitor(impl_monitor: &ImplMonitor) -> XCapResult<RgbaImage> {
    let monitor_info_buf = get_monitor_info_buf(impl_monitor.output)?;

    if wayland_detect() {
        wayland_capture(
            monitor_info_buf.x() as i32,
            monitor_info_buf.y() as i32,
            monitor_info_buf.width() as i32,
            monitor_info_buf.height() as i32,
        )
    } else {
        let screen_buf = get_current_screen_buf()?;

        xorg_capture(
            screen_buf.root(),
            monitor_info_buf.x() as i32,
            monitor_info_buf.y() as i32,
            monitor_info_buf.width() as u32,
            monitor_info_buf.height() as u32,
        )
    }
}

pub fn capture_region(
    impl_monitor: &ImplMonitor,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> XCapResult<RgbaImage> {
    let monitor_info_buf = get_monitor_info_buf(impl_monitor.output)?;

    if wayland_detect() {
        let monitor_width = impl_monitor.width()?;
        let monitor_height = impl_monitor.height()?;
        if width == 0
            || height == 0
            || x.checked_add(width).is_none_or(|end| end > monitor_width)
            || y.checked_add(height).is_none_or(|end| end > monitor_height)
        {
            return Err(XCapError::InvalidCaptureRegion(
                "Region is outside monitor bounds".into(),
            ));
        }
        wayland_capture(x as i32, y as i32, width as i32, height as i32)
    } else {
        // Bounds and root-read origin use one raw RandR observation, never Xft scale.
        let (native_x, native_y) = physical_region_bounds::physical_region_origin(
            (
                i32::from(monitor_info_buf.x()),
                i32::from(monitor_info_buf.y()),
                u32::from(monitor_info_buf.width()),
                u32::from(monitor_info_buf.height()),
            ),
            (x, y, width, height),
        )
        .ok_or_else(|| {
            XCapError::InvalidCaptureRegion(
                "Region is outside native physical monitor bounds or X11 coordinate range".into(),
            )
        })?;
        let screen_buf = get_current_screen_buf()?;
        xorg_capture(screen_buf.root(), native_x, native_y, width, height)
    }
}

pub fn capture_window(impl_window: &ImplWindow) -> XCapResult<RgbaImage> {
    let width = impl_window.width()?;
    let height = impl_window.height()?;

    xorg_capture(impl_window.window, 0, 0, width, height)
}
