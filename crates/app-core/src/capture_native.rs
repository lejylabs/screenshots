//! On-demand native cursor and persistent monitor identity. No resident connection.
use crate::{
    CaptureMonitorIdentity, CaptureOptions, CaptureWindowFrame, CaptureWindowShadow,
    CaptureWindowTransparency, DisplayInfo,
};
use anyhow::{Result, ensure};
use image::RgbaImage;

#[cfg(target_os = "linux")]
#[path = "capture_native_linux.rs"]
mod platform;
#[cfg(target_os = "windows")]
#[path = "capture_native_windows.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "capture_native_macos.rs"]
mod platform;

pub(crate) fn display_identities(displays: &[DisplayInfo]) -> Result<Vec<CaptureMonitorIdentity>> {
    if let Some(executor) = crate::native_capture_job::executor() {
        let call = crate::NativeCaptureCall::Identities {
            displays: displays.to_vec(),
        };
        call.validate()?;
        let reply = executor(call.clone(), &|| Ok(()))?;
        call.validate_reply(&reply)?;
        let crate::NativeCaptureReply::Identities(identities) = reply else {
            anyhow::bail!("Unexpected native identity reply");
        };
        return Ok(identities);
    }
    platform::display_identities(displays)
}
#[cfg(target_os = "windows")]
pub(crate) fn dpi_scope() -> Result<platform::DpiScope> {
    platform::DpiScope::new()
}
#[cfg(target_os = "windows")]
pub(crate) fn topology_guard() -> Result<platform::TopologyGuard> {
    platform::TopologyGuard::new()
}
#[cfg(target_os = "windows")]
pub(crate) fn physical_monitor_geometry(monitor: &xcap::Monitor) -> Result<crate::DisplayGeometry> {
    platform::monitor_geometry(monitor)
}
pub(crate) fn validate_options(options: &CaptureOptions, window: bool) -> Result<()> {
    options.validate()?;
    platform::validate_options(options, window)
}
pub(crate) fn validate_template_session() -> Result<()> {
    platform::validate_template_session()
}

pub(crate) fn capture_window(window: &xcap::Window, options: &CaptureOptions) -> Result<RgbaImage> {
    validate_options(options, true)?;
    let native = CaptureOptions {
        delay_ms: 0,
        ..*options
    };
    if !cfg!(target_os = "macos") && native == CaptureOptions::default() {
        return Ok(window.capture_image()?);
    }
    platform::capture_window(window, &native)
}

pub(crate) fn apply_cursor(
    options: &CaptureOptions,
    image: &mut RgbaImage,
    x: i32,
    y: i32,
) -> Result<()> {
    if options.cursor == crate::CaptureCursor::Include {
        platform::apply_cursor(image, x, y)?;
    }
    Ok(())
}

pub(crate) fn validate_native_window_policy(
    options: &CaptureOptions,
    client: bool,
    include: bool,
    shadow: bool,
    transparency: bool,
) -> Result<()> {
    ensure!(
        options.window_frame != CaptureWindowFrame::Client || client,
        "Native client-only window capture is unsupported on this platform"
    );
    ensure!(
        options.window_frame != CaptureWindowFrame::Include || include,
        "Native decorated window capture is unsupported on this platform"
    );
    ensure!(
        options.window_shadow != CaptureWindowShadow::Include || shadow,
        "Native window shadow inclusion is unsupported on this platform"
    );
    ensure!(
        options.window_transparency != CaptureWindowTransparency::Preserve || transparency,
        "Native window transparency preservation is unsupported on this platform"
    );
    Ok(())
}
