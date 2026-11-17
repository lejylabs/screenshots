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
        let reply = executor(
            call.clone(),
            &|| Ok(()),
            crate::NativeCaptureContext::default(),
        )?;
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
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
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

#[cfg(target_os = "macos")]
pub(crate) use crate::capture_macos_image::PreparedWindow as MacPreparedWindow;
#[cfg(not(target_os = "macos"))]
pub(crate) fn capture_window(
    window: &xcap::Window,
    options: &CaptureOptions,
    #[cfg(target_os = "windows")] admission: &WindowsCaptureAdmission,
    #[cfg(target_os = "windows")] cancel: &crate::NativeCaptureCheck<'_>,
) -> Result<RgbaImage> {
    validate_options(options, true)?;
    let native = CaptureOptions {
        delay_ms: 0,
        ..*options
    };
    #[cfg(not(target_os = "windows"))]
    if native == CaptureOptions::default() {
        return Ok(window.capture_image()?);
    }
    #[cfg(target_os = "windows")]
    {
        platform::capture_window(window, &native, admission, cancel)
    }
    #[cfg(not(target_os = "windows"))]
    {
        platform::capture_window(window, &native)
    }
}
#[cfg(target_os = "macos")]
pub(crate) fn mac_source_admission_bytes(monitor: &xcap::Monitor) -> Result<u64> {
    platform::source_admission_bytes(monitor)
}
#[cfg(target_os = "macos")]
pub(crate) fn mac_display_dimensions(monitor: &xcap::Monitor) -> Result<(u32, u32)> {
    platform::display_dimensions(monitor)
}
pub(crate) fn capture_display(
    monitor: &xcap::Monitor,
    options: &CaptureOptions,
    #[cfg(target_os = "windows")] admission: &WindowsCaptureAdmission,
    #[cfg(target_os = "windows")] cancel: &crate::NativeCaptureCheck<'_>,
) -> Result<RgbaImage> {
    #[cfg(target_os = "windows")]
    {
        platform::capture_display(monitor, options, admission, cancel)
    }
    #[cfg(target_os = "macos")]
    {
        platform::capture_display(monitor, options)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = options;
        Ok(monitor.capture_image()?)
    }
}
pub(crate) fn capture_region(
    monitor: &xcap::Monitor,
    region: crate::PixelRegion,
    options: &CaptureOptions,
    #[cfg(target_os = "windows")] admission: &WindowsCaptureAdmission,
    #[cfg(target_os = "windows")] cancel: &crate::NativeCaptureCheck<'_>,
) -> Result<RgbaImage> {
    #[cfg(target_os = "windows")]
    {
        platform::capture_region(monitor, region, options, admission, cancel)
    }
    #[cfg(target_os = "macos")]
    {
        platform::capture_region(monitor, region, options)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = options;
        Ok(monitor.capture_region(region.x, region.y, region.width, region.height)?)
    }
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

#[cfg(target_os = "windows")]
pub(crate) use crate::capture_windows_admission::Admission as WindowsCaptureAdmission;
#[cfg(target_os = "windows")]
pub(crate) fn windows_display_admission(
    monitor: &xcap::Monitor,
    area: Option<crate::PixelRegion>,
) -> Result<WindowsCaptureAdmission> {
    platform::prepare_display_admission(monitor, area)
}
#[cfg(target_os = "windows")]
pub(crate) fn windows_window_admission(window: &xcap::Window) -> Result<WindowsCaptureAdmission> {
    platform::prepare_window_admission(window)
}
#[cfg(target_os = "windows")]
pub(crate) fn check_windows_display_admission(
    monitor: &xcap::Monitor,
    area: Option<crate::PixelRegion>,
    admission: &WindowsCaptureAdmission,
) -> Result<()> {
    admission
        .verify(&platform::display_proof(monitor, area)?)
        .map_err(anyhow::Error::from)
}
#[cfg(target_os = "windows")]
pub(crate) fn check_windows_window_admission(
    window: &xcap::Window,
    admission: &WindowsCaptureAdmission,
) -> Result<()> {
    admission
        .verify(&platform::window_proof(window)?)
        .map_err(anyhow::Error::from)
}
