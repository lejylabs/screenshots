//! Phase 0 capture types and an XCap still-image backend.
//!
//! Capture stays synchronous and `Send`. Call it from a GPUI background
//! worker, not from the foreground executor.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use crate::ExportSize;
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use xcap::{Monitor, Window};

pub const MAX_CAPTURE_BYTES: u64 = 256 * 1024 * 1024;

pub fn validate_image_size(width: u32, height: u32) -> Result<(), CaptureError> {
    CaptureBudget::default().admit(width, height)
}

/// Admit an entire group before allocating pixels; failed admission leaves the budget intact.
#[derive(Default)]
struct CaptureBudget(u64);

impl CaptureBudget {
    fn admit(&mut self, width: u32, height: u32) -> Result<(), CaptureError> {
        if width == 0 || height == 0 {
            return Err(CaptureError::EmptyFrame);
        }
        let total = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or(CaptureError::MemoryLimit)?;
        self.admit_bytes(total)
    }
    fn admit_bytes(&mut self, bytes: u64) -> Result<(), CaptureError> {
        if bytes == 0 {
            return Err(CaptureError::EmptyFrame);
        }
        let total = self
            .0
            .checked_add(bytes)
            .filter(|total| *total <= MAX_CAPTURE_BYTES)
            .ok_or(CaptureError::MemoryLimit)?;
        self.0 = total;
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CaptureRequest {
    Display(String),
    Window(String),
    ForegroundWindow {
        window_id: String,
        process_id: u32,
    },
    RepeatRegion(RememberedRegion),
    ExactRegion(RememberedRegion),
    Region {
        display_id: String,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
}

enum PreparedCapture {
    Display(Monitor, String),
    #[cfg(not(target_os = "macos"))]
    Window(Window, String, String, Option<u32>),
    #[cfg(target_os = "macos")]
    MacWindow(crate::capture_native::MacPreparedWindow, String, String),
    Region(Monitor, String, u32, u32, u32, u32, Option<DisplayGeometry>),
}

impl PreparedCapture {
    fn admission_bytes(&self) -> Result<u64, CaptureError> {
        #[cfg(target_os = "macos")]
        if let Self::MacWindow(source, ..) = self {
            return Ok(source.admission_bytes());
        }
        let (width, height) = self.dimensions()?;
        validate_image_size(width, height)?;
        Ok(u64::from(width) * u64::from(height) * 4)
    }
    fn dimensions(&self) -> Result<(u32, u32), CaptureError> {
        match self {
            Self::Display(monitor, _) => {
                #[cfg(target_os = "macos")]
                {
                    return crate::capture_native::mac_display_dimensions(monitor)
                        .map_err(backend_error);
                }
                #[cfg(target_os = "linux")]
                {
                    let geometry = monitor_geometry(monitor)?;
                    Ok((geometry.width, geometry.height))
                }
                #[cfg(not(any(target_os = "macos", target_os = "linux")))]
                {
                    Ok((
                        monitor.width().map_err(backend_error)?,
                        monitor.height().map_err(backend_error)?,
                    ))
                }
            }
            #[cfg(not(target_os = "macos"))]
            Self::Window(window, _, _, _) => Ok((
                window.width().map_err(backend_error)?,
                window.height().map_err(backend_error)?,
            )),
            #[cfg(target_os = "macos")]
            Self::MacWindow(..) => Err(CaptureError::Unsupported(
                "Native window admission uses physical byte reservation",
            )),
            Self::Region(_, _, _, _, width, height, _) => Ok((*width, *height)),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CaptureCapabilities {
    pub list_displays: bool,
    pub list_windows: bool,
    pub capture_display: bool,
    pub capture_window: bool,
    pub capture_region: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayInfo {
    pub id: String,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f32,
    pub rotation: f32,
    pub is_primary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f32,
    pub rotation: f32,
}

impl DisplayGeometry {
    fn valid(self) -> bool {
        self.width > 0
            && self.height > 0
            && self.scale_factor.is_finite()
            && self.scale_factor > 0.0
            && matches!(self.rotation, 0.0 | 90.0 | 180.0 | 270.0)
    }
}

/// Validate coordinates returned by this platform's native display adapter.
/// Windows observations are calibrated and X11 uses raw RandR physical pixels;
/// other adapters keep their unscaled, unrotated region-coordinate policy.
pub fn validate_display_pixel_coordinates(display: &DisplayInfo) -> Result<(), CaptureError> {
    validate_image_size(display.width, display.height)?;
    if !display.scale_factor.is_finite()
        || display.scale_factor <= 0.0
        || !matches!(display.rotation, 0.0 | 90.0 | 180.0 | 270.0)
    {
        return Err(CaptureError::StaleRegion);
    }
    if !cfg!(any(target_os = "windows", target_os = "linux"))
        && (display.scale_factor != 1.0 || display.rotation != 0.0)
    {
        return Err(CaptureError::Unsupported(
            "This native adapter requires unrotated 100% physical-pixel region coordinates",
        ));
    }
    Ok(())
}

/// Session metadata only: never retains the source frame or a texture.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RememberedRegion {
    display_id: String,
    geometry: DisplayGeometry,
    area: crate::PixelRegion,
}

impl RememberedRegion {
    pub(crate) fn native_parts(&self) -> (&str, DisplayGeometry, crate::PixelRegion) {
        (&self.display_id, self.geometry, self.area)
    }
    pub fn from_display(
        display: &DisplayInfo,
        area: crate::PixelRegion,
    ) -> Result<Self, CaptureError> {
        if display.id.is_empty() || display.id.len() > 64 {
            return Err(CaptureError::StaleRegion);
        }
        validate_image_size(display.width, display.height)?;
        let geometry = DisplayGeometry {
            x: display.x,
            y: display.y,
            width: display.width,
            height: display.height,
            scale_factor: display.scale_factor,
            rotation: display.rotation,
        };
        let region = Self {
            display_id: display.id.clone(),
            geometry,
            area,
        };
        region.validate_geometry(geometry)?;
        Ok(region)
    }
    pub fn from_snapshot(
        source: &CapturedImage,
        area: crate::PixelRegion,
    ) -> Result<Self, CaptureError> {
        let geometry = source.display_geometry.ok_or(CaptureError::StaleRegion)?;
        if !geometry.valid()
            || source.window_id.is_some()
            || source.display_id.is_empty()
            || source.display_id.len() > 64
            || (source.width, source.height) != (geometry.width, geometry.height)
            || source.pixels.dimensions() != (source.width, source.height)
        {
            return Err(CaptureError::StaleRegion);
        }
        validate_image_size(source.width, source.height)?;
        let remembered = Self {
            display_id: source.display_id.clone(),
            geometry,
            area,
        };
        remembered.validate_geometry(geometry)?;
        Ok(remembered)
    }

    pub fn area(&self) -> crate::PixelRegion {
        self.area
    }

    fn validate_geometry(&self, current: DisplayGeometry) -> Result<(), CaptureError> {
        if !current.valid() || current != self.geometry {
            return Err(CaptureError::StaleRegion);
        }
        if self.area.width == 0
            || self.area.height == 0
            || self
                .area
                .x
                .checked_add(self.area.width)
                .is_none_or(|x| x > current.width)
            || self
                .area
                .y
                .checked_add(self.area.height)
                .is_none_or(|y| y > current.height)
        {
            return Err(CaptureError::InvalidRegion);
        }
        validate_image_size(self.area.width, self.area.height)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowInfo {
    pub id: String,
    pub pid: u32,
    pub title: String,
    pub app_name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub is_minimized: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelFormat {
    /// XCap returns `image::RgbaImage` (8-bit RGBA). Color space is not
    /// reported; treat as sRGB until a later spike measures otherwise.
    Rgba8UnormSrgb,
}

#[derive(Debug, Clone)]
pub struct CapturedImage {
    pub display_id: String,
    pub window_id: Option<String>,
    pub width: u32,
    pub height: u32,
    /// Intended export path; capture alone does not create a file.
    pub png_path: PathBuf,
    pub pixels: Arc<RgbaImage>,
    pub elapsed_ms: u128,
    pub source_format: PixelFormat,
    /// Present only for an unmodified full-display snapshot with stable native geometry.
    pub display_geometry: Option<DisplayGeometry>,
}

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("macOS Screen Recording permission is not authorized; no permission request was made")]
    MacScreenPermissionDenied,
    #[error("macOS Accessibility permission is not authorized; no permission request was made")]
    MacAccessibilityPermissionDenied,
    #[error("macOS microphone permission is not authorized; no permission request was made")]
    MacMicrophonePermissionDenied,
    #[error(
        "Clipboard read failed, was too large, or became unavailable; Wayland reads require focused application Paste"
    )]
    Clipboard,
    #[error("Remote image transfer or decoding failed")]
    ImageUrlFailed,
    #[error("Capture permission was denied or the selected portal source was closed")]
    PermissionDenied,
    #[error("Native capture canceled")]
    NativeCanceled,
    #[error("Native capture deadline exceeded")]
    NativeTimeout,
    #[error("Another native capture operation is active")]
    NativeBusy,
    #[error("Native helper termination is still pending; new native work remains blocked")]
    NativeCleanupPending,
    #[error("unsupported: {0}")]
    Unsupported(&'static str),
    #[error("display not found: {0}")]
    DisplayNotFound(String),
    #[error("window not found: {0}")]
    WindowNotFound(String),
    #[error("empty frame")]
    EmptyFrame,
    #[error("capture group exceeds the 256 MiB decoded RGBA limit")]
    MemoryLimit,
    #[error("region is outside the display")]
    InvalidRegion,
    #[error("The display geometry changed or is unavailable. Select a new region.")]
    StaleRegion,
    #[error("Export canceled")]
    Canceled,
    #[error("{0}")]
    Backend(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Image(#[from] image::ImageError),
}

pub trait CaptureBackend: Send {
    fn capabilities(&self) -> CaptureCapabilities;
    fn list_displays(&self) -> Result<Vec<DisplayInfo>, CaptureError>;
    fn list_windows(&self) -> Result<Vec<WindowInfo>, CaptureError>;
    fn capture_display(&self, display_id: &str) -> Result<CapturedImage, CaptureError>;
    fn capture_window(&self, window_id: &str) -> Result<CapturedImage, CaptureError>;
    fn capture_region(
        &self,
        display_id: &str,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<CapturedImage, CaptureError>;
}

#[derive(Clone)]
pub struct XcapBackend {
    output_dir: PathBuf,
    options: crate::CaptureOptions,
    cancellation: Option<crate::NativeCaptureCancellation>,
    originating_pid: u32,
}

impl XcapBackend {
    pub fn capture_focused_window(&self) -> Result<CapturedImage, CaptureError> {
        self.capture_focused_window_with_cancel(&|| Ok(()))
    }
    pub fn capture_focused_window_with_cancel(
        &self,
        check: &crate::NativeCaptureCheck<'_>,
    ) -> Result<CapturedImage, CaptureError> {
        #[cfg(target_os = "windows")]
        let _dpi = crate::capture_native::dpi_scope().map_err(backend_error)?;
        crate::capture_native::validate_options(&self.options, true).map_err(backend_error)?;
        let immediate = self.wait_native_delay(check)?;
        if crate::native_capture_job::executor().is_some() {
            return immediate
                .native_frames(
                    crate::NativeCaptureCall::FocusedWindow {
                        options: immediate.options,
                    },
                    check,
                )?
                .pop()
                .ok_or(CaptureError::EmptyFrame);
        }
        let window = focused_window()?;
        let process_id = window.pid().map_err(backend_error)?;
        let id = window.id().map_err(backend_error)?.to_string();
        let frame = immediate
            .capture_batch_with_cancel(
                vec![CaptureRequest::ForegroundWindow {
                    window_id: id,
                    process_id,
                }],
                check,
            )?
            .pop()
            .ok_or(CaptureError::EmptyFrame)?;
        if window.pid().ok() != Some(process_id) || !window.is_focused().unwrap_or(false) {
            return Err(CaptureError::Unsupported(
                "foreground window changed during capture",
            ));
        }
        Ok(frame)
    }

    pub fn capture_focused_display(&self) -> Result<CapturedImage, CaptureError> {
        self.capture_focused_display_with_cancel(&|| Ok(()))
    }
    pub fn capture_focused_display_with_cancel(
        &self,
        check: &crate::NativeCaptureCheck<'_>,
    ) -> Result<CapturedImage, CaptureError> {
        #[cfg(target_os = "windows")]
        let _dpi = crate::capture_native::dpi_scope().map_err(backend_error)?;
        crate::capture_native::validate_options(&self.options, false).map_err(backend_error)?;
        let immediate = self.wait_native_delay(check)?;
        if crate::native_capture_job::executor().is_some() {
            return immediate
                .native_frames(
                    crate::NativeCaptureCall::FocusedDisplay {
                        options: immediate.options,
                    },
                    check,
                )?
                .pop()
                .ok_or(CaptureError::EmptyFrame);
        }
        let window = focused_window()?;
        let process_id = window.pid().map_err(backend_error)?;
        #[cfg(not(target_os = "linux"))]
        let id = monitor_id(&window.current_monitor().map_err(backend_error)?)?;
        #[cfg(target_os = "linux")]
        let id = {
            let x = i64::from(window.x().map_err(backend_error)?);
            let y = i64::from(window.y().map_err(backend_error)?);
            let right = x + i64::from(window.width().map_err(backend_error)?);
            let bottom = y + i64::from(window.height().map_err(backend_error)?);
            immediate
                .list_displays_with_cancel(check)?
                .into_iter()
                .filter(|d| d.scale_factor == 1.0 && d.rotation == 0.0)
                .map(|d| {
                    let area = i128::from(
                        (right.min(i64::from(d.x) + i64::from(d.width)) - x.max(i64::from(d.x)))
                            .max(0),
                    ) * i128::from(
                        (bottom.min(i64::from(d.y) + i64::from(d.height)) - y.max(i64::from(d.y)))
                            .max(0),
                    );
                    (area, d.id)
                })
                .filter(|(area, _)| *area > 0)
                .max_by_key(|(area, _)| *area)
                .map(|(_, id)| id)
                .ok_or(CaptureError::Unsupported(
                    "focused window has no matching unscaled display",
                ))?
        };
        if !window.is_focused().unwrap_or(false) {
            return Err(CaptureError::Unsupported(
                "foreground window changed before capture",
            ));
        }
        let frame = immediate
            .capture_batch_with_cancel(vec![CaptureRequest::Display(id)], check)?
            .pop()
            .ok_or(CaptureError::EmptyFrame)?;
        if window.pid().ok() != Some(process_id) || !window.is_focused().unwrap_or(false) {
            return Err(CaptureError::Unsupported(
                "foreground window changed during capture",
            ));
        }
        Ok(frame)
    }
    pub fn new(output_dir: impl Into<PathBuf>) -> Self {
        Self {
            output_dir: output_dir.into(),
            options: crate::CaptureOptions::default(),
            cancellation: None,
            originating_pid: std::process::id(),
        }
    }

    pub fn with_cancellation(mut self, cancellation: crate::NativeCaptureCancellation) -> Self {
        self.cancellation = Some(cancellation);
        self
    }
    pub(crate) fn with_originating_process(mut self, pid: u32) -> Self {
        self.originating_pid = pid;
        self
    }
    fn check_native(&self, extra: &crate::NativeCaptureCheck<'_>) -> Result<(), CaptureError> {
        if let Some(cancellation) = &self.cancellation {
            cancellation.check()?;
        }
        extra()
    }
    fn wait_native_delay(
        &self,
        check: &crate::NativeCaptureCheck<'_>,
    ) -> Result<Self, CaptureError> {
        self.options.validate().map_err(backend_error)?;
        let deadline = Instant::now() + std::time::Duration::from_millis(self.options.delay_ms);
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            self.check_native(check)?;
            std::thread::sleep(remaining.min(std::time::Duration::from_millis(20)));
        }
        self.check_native(check)?;
        let mut immediate = self.clone();
        immediate.options.delay_ms = 0;
        Ok(immediate)
    }
    pub(crate) fn native_frames(
        &self,
        call: crate::NativeCaptureCall,
        check: &crate::NativeCaptureCheck<'_>,
    ) -> Result<Vec<CapturedImage>, CaptureError> {
        call.validate()?;
        let executor = crate::native_capture_job::executor()
            .ok_or_else(|| backend_error("Native capture executor unavailable"))?;
        self.check_native(check)?;
        let reply = executor(call.clone(), &|| self.check_native(check))?;
        call.validate_reply(&reply)?;
        let crate::NativeCaptureReply::Frames(frames) = reply else {
            return Err(backend_error("Unexpected native capture reply"));
        };
        let mut result = Vec::with_capacity(frames.len());
        for (index, frame) in frames.into_iter().enumerate() {
            self.check_native(check)?;
            let pixels = Arc::try_unwrap(frame.pixels)
                .map_err(|_| backend_error("Native capture pixels were unexpectedly shared"))?;
            let label = if let Some(window) = &frame.window_id {
                format!("window-{window}")
            } else if matches!(&call, crate::NativeCaptureCall::FocusedDisplay { .. })
                || matches!(&call, crate::NativeCaptureCall::Batch { requests, .. } if matches!(requests.get(index), Some(CaptureRequest::Display(_))))
            {
                "display".into()
            } else {
                "region".into()
            };
            let mut owned = self.frame(&frame.display_id, pixels, frame.elapsed_ms, &label)?;
            owned.window_id = frame.window_id;
            owned.display_geometry = frame.display_geometry;
            owned.source_format = frame.source_format;
            result.push(owned);
        }
        self.check_native(check)?;
        Ok(result)
    }

    /// Explicit operation options. Defaults keep the existing native capture path.
    pub fn with_options(mut self, options: crate::CaptureOptions) -> Result<Self, CaptureError> {
        options.validate().map_err(backend_error)?;
        self.options = options;
        Ok(self)
    }

    pub(crate) fn capture_physical_desktop_with_cancel(
        &self,
        check: &crate::NativeCaptureCheck<'_>,
    ) -> Result<CapturedImage, CaptureError> {
        if crate::native_capture_job::executor().is_none() {
            return Err(CaptureError::Unsupported(
                "Desktop capture requires the isolated native operation executor",
            ));
        }
        let immediate = self.wait_native_delay(check)?;
        immediate
            .native_frames(
                crate::NativeCaptureCall::Desktop {
                    options: immediate.options,
                },
                check,
            )?
            .pop()
            .ok_or(CaptureError::EmptyFrame)
    }

    /// One admission group, with no partial outputs if any request fails.
    pub fn capture_batch(
        &self,
        requests: Vec<CaptureRequest>,
    ) -> Result<Vec<CapturedImage>, CaptureError> {
        self.capture_batch_with_cancel(requests, &|| Ok(()))
    }
    pub fn capture_batch_with_cancel(
        &self,
        requests: Vec<CaptureRequest>,
        check: &crate::NativeCaptureCheck<'_>,
    ) -> Result<Vec<CapturedImage>, CaptureError> {
        #[cfg(target_os = "linux")]
        if linux_wayland_session()
            && requests.iter().any(|request| {
                !matches!(
                    request,
                    CaptureRequest::Window(_) | CaptureRequest::ForegroundWindow { .. }
                )
            })
        {
            return Err(CaptureError::Unsupported(
                "Named XWayland display/region capture is unavailable; select an explicit capture-portal source",
            ));
        }
        for request in &requests {
            crate::capture_native::validate_options(
                &self.options,
                matches!(
                    request,
                    CaptureRequest::Window(_) | CaptureRequest::ForegroundWindow { .. }
                ),
            )
            .map_err(backend_error)?;
        }
        let immediate = self.wait_native_delay(check)?;
        if crate::native_capture_job::executor().is_some() {
            return immediate.native_frames(
                crate::NativeCaptureCall::Batch {
                    requests,
                    options: immediate.options,
                },
                check,
            );
        }
        immediate.capture_batch_inline(requests, check)
    }
    fn capture_batch_inline(
        &self,
        requests: Vec<CaptureRequest>,
        check: &crate::NativeCaptureCheck<'_>,
    ) -> Result<Vec<CapturedImage>, CaptureError> {
        self.check_native(check)?;
        #[cfg(target_os = "windows")]
        let topology = crate::capture_native::topology_guard().map_err(backend_error)?;
        let mut budget = CaptureBudget::default();
        let prepared = requests
            .into_iter()
            .map(|request| {
                self.check_native(check)?;
                let prepared = self.prepare(request)?;
                budget.admit_bytes(prepared.admission_bytes()?)?;
                Ok(prepared)
            })
            .collect::<Result<Vec<_>, CaptureError>>()?;
        let mut actual = CaptureBudget::default();
        let frames = prepared
            .into_iter()
            .map(|prepared| {
                self.check_native(check)?;
                CaptureBudget(actual.0).admit_bytes(prepared.admission_bytes()?)?;
                let started = Instant::now();
                let mut display_geometry = None;
                let (display_id, window_id, image, label) = match prepared {
                    PreparedCapture::Display(monitor, id) => {
                        let cursor_geometry = cursor_geometry(&monitor, &self.options)?;
                        let before = if cfg!(any(target_os = "windows", target_os = "linux")) {
                            Some(monitor_geometry(&monitor)?)
                        } else {
                            None
                        };
                        let mut image =
                            crate::capture_native::capture_display(&monitor, &self.options)
                                .map_err(backend_error)?;
                        if let Some(geometry) = cursor_geometry {
                            crate::capture_native::apply_cursor(
                                &self.options,
                                &mut image,
                                geometry.x,
                                geometry.y,
                            )
                            .map_err(backend_error)?;
                            if monitor_geometry(&monitor)? != geometry {
                                return Err(CaptureError::StaleRegion);
                            }
                        }
                        if let Some(geometry) = before {
                            if (geometry.width, geometry.height) != image.dimensions()
                                || monitor_geometry(&monitor)? != geometry
                            {
                                return Err(CaptureError::StaleRegion);
                            }
                            display_geometry = Some(geometry);
                        }
                        (id, None, image, "display".to_owned())
                    }
                    #[cfg(not(target_os = "macos"))]
                    PreparedCapture::Window(window, display, id, expected_process) => {
                        if expected_process
                            .is_some_and(|expected| window.pid().ok() != Some(expected))
                        {
                            return Err(CaptureError::WindowNotFound(id));
                        }
                        let image = crate::capture_native::capture_window(&window, &self.options)
                            .map_err(backend_error)?;
                        if expected_process
                            .is_some_and(|expected| window.pid().ok() != Some(expected))
                        {
                            return Err(CaptureError::WindowNotFound(id));
                        }
                        (display, Some(id.clone()), image, format!("window-{id}"))
                    }
                    #[cfg(target_os = "macos")]
                    PreparedCapture::MacWindow(source, display, id) => {
                        let image = source.capture().map_err(backend_error)?;
                        (display, Some(id.clone()), image, format!("window-{id}"))
                    }
                    PreparedCapture::Region(monitor, id, x, y, width, height, expected) => {
                        let cursor_geometry = cursor_geometry(&monitor, &self.options)?;
                        if let Some(expected) = expected {
                            if monitor_geometry(&monitor)? != expected {
                                return Err(CaptureError::StaleRegion);
                            }
                        }
                        let mut image = crate::capture_native::capture_region(
                            &monitor,
                            crate::PixelRegion {
                                x,
                                y,
                                width,
                                height,
                            },
                            &self.options,
                        )
                        .map_err(backend_error)?;
                        if let Some(geometry) = cursor_geometry {
                            let origin_x = i32::try_from(i64::from(geometry.x) + i64::from(x))
                                .map_err(|_| CaptureError::InvalidRegion)?;
                            let origin_y = i32::try_from(i64::from(geometry.y) + i64::from(y))
                                .map_err(|_| CaptureError::InvalidRegion)?;
                            crate::capture_native::apply_cursor(
                                &self.options,
                                &mut image,
                                origin_x,
                                origin_y,
                            )
                            .map_err(backend_error)?;
                            if monitor_geometry(&monitor)? != geometry {
                                return Err(CaptureError::StaleRegion);
                            }
                        }
                        if let Some(expected) = expected {
                            if monitor_geometry(&monitor)? != expected
                                || image.dimensions() != (width, height)
                            {
                                return Err(CaptureError::StaleRegion);
                            }
                        }
                        (id, None, image, "region".to_owned())
                    }
                };
                // shortcut: XCap allocates before returning; isolate/cap the backend for a hard native allocation bound.
                actual.admit(image.width(), image.height())?;
                self.check_native(check)?;
                let mut shot =
                    self.frame(&display_id, image, started.elapsed().as_millis(), &label)?;
                shot.window_id = window_id;
                shot.display_geometry = display_geometry;
                Ok(shot)
            })
            .collect::<Result<Vec<_>, CaptureError>>()?;
        #[cfg(target_os = "windows")]
        topology.check().map_err(backend_error)?;
        self.check_native(check)?;
        Ok(frames)
    }

    fn prepare(&self, request: CaptureRequest) -> Result<PreparedCapture, CaptureError> {
        match request {
            CaptureRequest::Display(id) => {
                Ok(PreparedCapture::Display(self.find_monitor(&id)?, id))
            }
            CaptureRequest::RepeatRegion(remembered) | CaptureRequest::ExactRegion(remembered) => {
                let monitor = self.find_monitor(&remembered.display_id)?;
                remembered.validate_geometry(monitor_geometry(&monitor)?)?;
                let area = remembered.area;
                Ok(PreparedCapture::Region(
                    monitor,
                    remembered.display_id,
                    area.x,
                    area.y,
                    area.width,
                    area.height,
                    Some(remembered.geometry),
                ))
            }
            CaptureRequest::Region {
                display_id,
                x,
                y,
                width,
                height,
            } => {
                let monitor = self.find_monitor(&display_id)?;
                let x = u32::try_from(x).map_err(|_| CaptureError::InvalidRegion)?;
                let y = u32::try_from(y).map_err(|_| CaptureError::InvalidRegion)?;
                #[cfg(target_os = "macos")]
                let (display_width, display_height) =
                    crate::capture_native::mac_display_dimensions(&monitor)
                        .map_err(backend_error)?;
                #[cfg(target_os = "linux")]
                let (display_width, display_height) = {
                    let geometry = monitor_geometry(&monitor)?;
                    (geometry.width, geometry.height)
                };
                #[cfg(not(any(target_os = "macos", target_os = "linux")))]
                let (display_width, display_height) = (
                    monitor.width().map_err(backend_error)?,
                    monitor.height().map_err(backend_error)?,
                );
                if width == 0
                    || height == 0
                    || x.checked_add(width).is_none_or(|end| end > display_width)
                    || y.checked_add(height).is_none_or(|end| end > display_height)
                {
                    return Err(CaptureError::InvalidRegion);
                }
                let expected = if cfg!(any(target_os = "windows", target_os = "linux")) {
                    Some(monitor_geometry(&monitor)?)
                } else {
                    None
                };
                Ok(PreparedCapture::Region(
                    monitor, display_id, x, y, width, height, expected,
                ))
            }
            CaptureRequest::Window(id) => self.prepare_window(id, None),
            CaptureRequest::ForegroundWindow {
                window_id,
                process_id,
            } => self.prepare_window(window_id, Some(process_id)),
        }
    }

    fn prepare_window(
        &self,
        id: String,
        expected_process: Option<u32>,
    ) -> Result<PreparedCapture, CaptureError> {
        if !self.capabilities().capture_window {
            return Err(CaptureError::Unsupported("window capture"));
        }
        let numeric_id = id
            .parse::<u32>()
            .map_err(|_| CaptureError::WindowNotFound(id.clone()))?;
        let window = Window::all()
            .map_err(backend_error)?
            .into_iter()
            .find(|window| window.id().is_ok_and(|value| value == numeric_id))
            .ok_or_else(|| CaptureError::WindowNotFound(id.clone()))?;
        if window.pid().map_err(backend_error)? == std::process::id()
            || window.pid().map_err(backend_error)? == self.originating_pid
        {
            return Err(CaptureError::Unsupported(
                "capturing this application's own window",
            ));
        }
        if expected_process.is_some_and(|expected| window.pid().ok() != Some(expected)) {
            return Err(CaptureError::WindowNotFound(id));
        }
        if window.is_minimized().map_err(backend_error)? {
            return Err(CaptureError::Unsupported("capturing a minimized window"));
        }
        // XCap's Linux monitor association mixes coordinates and can overflow before admission.
        #[cfg(target_os = "linux")]
        let display = String::new();
        #[cfg(not(target_os = "linux"))]
        let display = monitor_id(&window.current_monitor().map_err(backend_error)?)?;
        #[cfg(target_os = "macos")]
        {
            let source = crate::capture_native::MacPreparedWindow::new(&window, &self.options)
                .map_err(backend_error)?;
            Ok(PreparedCapture::MacWindow(source, display, id))
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(PreparedCapture::Window(
                window,
                display,
                id,
                expected_process,
            ))
        }
    }

    fn find_monitor(&self, display_id: &str) -> Result<Monitor, CaptureError> {
        Monitor::all()
            .map_err(backend_error)?
            .into_iter()
            .find(|monitor| monitor_id(monitor).is_ok_and(|id| id == display_id))
            .ok_or_else(|| CaptureError::DisplayNotFound(display_id.to_string()))
    }

    pub fn list_displays_with_cancel(
        &self,
        check: &crate::NativeCaptureCheck<'_>,
    ) -> Result<Vec<DisplayInfo>, CaptureError> {
        self.check_native(check)?;
        if let Some(executor) = crate::native_capture_job::executor() {
            let call = crate::NativeCaptureCall::Displays;
            let reply = executor(call.clone(), &|| self.check_native(check))?;
            call.validate_reply(&reply)?;
            let crate::NativeCaptureReply::Displays(displays) = reply else {
                return Err(backend_error("Unexpected native display reply"));
            };
            self.check_native(check)?;
            return Ok(displays);
        }
        #[cfg(target_os = "windows")]
        let topology = crate::capture_native::topology_guard().map_err(backend_error)?;
        let displays = Monitor::all()
            .map_err(backend_error)?
            .iter()
            .map(display_info)
            .collect::<Result<Vec<_>, _>>()?;
        #[cfg(target_os = "windows")]
        topology.check().map_err(backend_error)?;
        self.check_native(check)?;
        Ok(displays)
    }
    pub fn list_sources_with_cancel(
        &self,
        check: &crate::NativeCaptureCheck<'_>,
    ) -> Result<crate::NativeSourceInventory, CaptureError> {
        self.check_native(check)?;
        if let Some(executor) = crate::native_capture_job::executor() {
            let call = crate::NativeCaptureCall::Inventory;
            let reply = executor(call.clone(), &|| self.check_native(check))?;
            call.validate_reply(&reply)?;
            let crate::NativeCaptureReply::Inventory(inventory) = reply else {
                return Err(backend_error("Unexpected native inventory reply"));
            };
            self.check_native(check)?;
            return Ok(inventory);
        }
        let displays = self.list_displays_with_cancel(check);
        self.check_native(check)?;
        let windows = self.list_windows_with_cancel(check);
        self.check_native(check)?;
        Ok(crate::NativeSourceInventory { displays, windows })
    }
    pub fn list_windows_with_cancel(
        &self,
        check: &crate::NativeCaptureCheck<'_>,
    ) -> Result<Vec<WindowInfo>, CaptureError> {
        self.check_native(check)?;
        if !self.capabilities().list_windows {
            return Err(CaptureError::Unsupported("window listing"));
        }
        if let Some(executor) = crate::native_capture_job::executor() {
            let call = crate::NativeCaptureCall::Windows;
            let reply = executor(call.clone(), &|| self.check_native(check))?;
            call.validate_reply(&reply)?;
            let crate::NativeCaptureReply::Windows(windows) = reply else {
                return Err(backend_error("Unexpected native window reply"));
            };
            self.check_native(check)?;
            return Ok(windows);
        }
        #[cfg(target_os = "windows")]
        let _dpi = crate::capture_native::dpi_scope().map_err(backend_error)?;
        let windows = Window::all()
            .map_err(backend_error)?
            .iter()
            .map(window_info)
            .collect::<Result<Vec<_>, _>>()?;
        self.check_native(check)?;
        Ok(windows)
    }

    fn frame(
        &self,
        display_id: &str,
        image: RgbaImage,
        elapsed_ms: u128,
        label: &str,
    ) -> Result<CapturedImage, CaptureError> {
        if frame_is_unusable(&image) {
            return Err(CaptureError::EmptyFrame);
        }

        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0);
        let png_path = self.output_dir.join(format!(
            "{label}-{display_id}-{stamp}-{}.png",
            uuid::Uuid::new_v4()
        ));

        Ok(CapturedImage {
            display_id: display_id.to_string(),
            window_id: None,
            width: image.width(),
            height: image.height(),
            pixels: Arc::new(image),
            png_path,
            elapsed_ms,
            source_format: PixelFormat::Rgba8UnormSrgb,
            display_geometry: None,
        })
    }
}

fn focused_window() -> Result<Window, CaptureError> {
    let mut focused = Window::all()
        .map_err(backend_error)?
        .into_iter()
        .filter(|window| window.is_focused().unwrap_or(false));
    let window = focused
        .next()
        .ok_or(CaptureError::Unsupported("no focused window is available"))?;
    if focused.next().is_some() {
        return Err(CaptureError::Unsupported(
            "focused window identity is ambiguous",
        ));
    }
    Ok(window)
}

impl CaptureBackend for XcapBackend {
    fn capabilities(&self) -> CaptureCapabilities {
        #[cfg(target_os = "linux")]
        let windows = linux_x11_windows_available(std::env::var_os("DISPLAY").as_deref());
        #[cfg(not(target_os = "linux"))]
        let windows = true;
        CaptureCapabilities {
            list_displays: true,
            list_windows: windows,
            capture_display: {
                #[cfg(target_os = "linux")]
                {
                    !linux_wayland_session()
                }
                #[cfg(not(target_os = "linux"))]
                {
                    true
                }
            },
            capture_window: windows,
            capture_region: {
                #[cfg(target_os = "linux")]
                {
                    !linux_wayland_session()
                }
                #[cfg(not(target_os = "linux"))]
                {
                    true
                }
            },
        }
    }

    fn list_displays(&self) -> Result<Vec<DisplayInfo>, CaptureError> {
        self.list_displays_with_cancel(&|| Ok(()))
    }

    fn list_windows(&self) -> Result<Vec<WindowInfo>, CaptureError> {
        self.list_windows_with_cancel(&|| Ok(()))
    }

    fn capture_display(&self, display_id: &str) -> Result<CapturedImage, CaptureError> {
        Ok(self
            .capture_batch(vec![CaptureRequest::Display(display_id.into())])?
            .remove(0))
    }

    fn capture_region(
        &self,
        display_id: &str,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<CapturedImage, CaptureError> {
        Ok(self
            .capture_batch(vec![CaptureRequest::Region {
                display_id: display_id.into(),
                x,
                y,
                width,
                height,
            }])?
            .remove(0))
    }

    fn capture_window(&self, window_id: &str) -> Result<CapturedImage, CaptureError> {
        Ok(self
            .capture_batch(vec![CaptureRequest::Window(window_id.into())])?
            .remove(0))
    }
}

#[cfg(target_os = "linux")]
#[doc(hidden)]
pub fn linux_wayland_markers(
    display: Option<&std::ffi::OsStr>,
    socket: Option<&std::ffi::OsStr>,
    session: Option<&std::ffi::OsStr>,
) -> bool {
    socket.is_some()
        || display.is_some_and(|value| !value.is_empty())
        || session
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|value| value.eq_ignore_ascii_case("wayland"))
}
#[cfg(target_os = "linux")]
#[doc(hidden)]
pub fn linux_wayland_session() -> bool {
    linux_wayland_markers(
        std::env::var_os("WAYLAND_DISPLAY").as_deref(),
        std::env::var_os("WAYLAND_SOCKET").as_deref(),
        std::env::var_os("XDG_SESSION_TYPE").as_deref(),
    )
}

#[cfg(target_os = "linux")]
fn linux_x11_windows_available(display: Option<&std::ffi::OsStr>) -> bool {
    // XCap's window path is X11-only, including XWayland; it cannot list native Wayland clients.
    display
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|display| !display.trim().is_empty())
}

fn backend_error(error: impl std::fmt::Display) -> CaptureError {
    CaptureError::Backend(error.to_string())
}

fn monitor_id(monitor: &Monitor) -> Result<String, CaptureError> {
    Ok(monitor.id().map_err(backend_error)?.to_string())
}

fn cursor_geometry(
    monitor: &Monitor,
    options: &crate::CaptureOptions,
) -> Result<Option<DisplayGeometry>, CaptureError> {
    if cfg!(target_os = "macos") {
        return Ok(None);
    }
    if options.cursor != crate::CaptureCursor::Include {
        return Ok(None);
    }
    let geometry = monitor_geometry(monitor)?;
    if !cfg!(target_os = "windows") && (geometry.scale_factor != 1.0 || geometry.rotation != 0.0) {
        return Err(CaptureError::Unsupported(
            "still cursor composition requires unrotated 100% physical-pixel display coordinates",
        ));
    }
    Ok(Some(geometry))
}

fn monitor_geometry(monitor: &Monitor) -> Result<DisplayGeometry, CaptureError> {
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    {
        crate::capture_native::physical_monitor_geometry(monitor).map_err(backend_error)
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        Ok(DisplayGeometry {
            x: monitor.x().map_err(backend_error)?,
            y: monitor.y().map_err(backend_error)?,
            width: monitor.width().map_err(backend_error)?,
            height: monitor.height().map_err(backend_error)?,
            scale_factor: monitor.scale_factor().map_err(backend_error)?,
            rotation: monitor.rotation().map_err(backend_error)?,
        })
    }
}

fn display_info(monitor: &Monitor) -> Result<DisplayInfo, CaptureError> {
    let geometry = monitor_geometry(monitor)?;
    Ok(DisplayInfo {
        id: monitor_id(monitor)?,
        name: monitor
            .friendly_name()
            .or_else(|_| monitor.name())
            .map_err(backend_error)?,
        x: geometry.x,
        y: geometry.y,
        width: geometry.width,
        height: geometry.height,
        scale_factor: geometry.scale_factor,
        rotation: geometry.rotation,
        is_primary: monitor.is_primary().map_err(backend_error)?,
    })
}

fn window_info(window: &Window) -> Result<WindowInfo, CaptureError> {
    Ok(WindowInfo {
        id: window.id().map_err(backend_error)?.to_string(),
        pid: window.pid().unwrap_or(0),
        title: window.title().unwrap_or_default(),
        app_name: window.app_name().unwrap_or_default(),
        x: window.x().unwrap_or(0),
        y: window.y().unwrap_or(0),
        width: window.width().unwrap_or(0),
        height: window.height().unwrap_or(0),
        is_minimized: window.is_minimized().unwrap_or(false),
    })
}

fn frame_is_unusable(image: &RgbaImage) -> bool {
    image.width() == 0 || image.height() == 0
}

pub fn capture_output_dir() -> PathBuf {
    Path::new("target").join("captures")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoded_png_publication_preserves_bytes_and_refuses_replacement() {
        let root =
            std::env::temp_dir().join(format!("screenshots-encoded-png-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let path = root.join("output.png");
        let pixels = RgbaImage::from_pixel(2, 1, Rgba([13, 27, 44, 71]));
        let bytes = encode_png_bytes(&pixels).unwrap();
        save_png_bytes(&path, &bytes).unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(image::open(&path).unwrap().into_rgba8(), pixels);
        assert!(save_png_bytes(&path, &bytes).is_err());
        assert!(save_png_bytes(&root.join("invalid.png"), b"not png").is_err());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scaled_exports_use_final_pixels_preserve_sources_and_refuse_overwrite() -> anyhow::Result<()>
    {
        let root = std::env::temp_dir().join(format!(
            "screenshots-scaled-export-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root)?;
        let shot = CapturedImage {
            width: 4,
            height: 4,
            pixels: Arc::new(RgbaImage::from_pixel(4, 4, image::Rgba([12, 34, 56, 255]))),
            png_path: root.join("source.png"),
            display_id: "fixture".into(),
            window_id: None,
            elapsed_ms: 0,
            source_format: PixelFormat::Rgba8UnormSrgb,
            display_geometry: None,
        };
        shot.save_png()?;
        let encoded = shot.png_bytes()?;
        assert!(encoded.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10]));
        assert_eq!(
            image::load_from_memory(&encoded)?.into_rgba8(),
            *shot.pixels
        );
        let before = fs::read(&shot.png_path)?;
        for extension in ["png", "jpg", "webp"] {
            let target = root.join(format!("half.{extension}"));
            let saved = root.join(format!("quarter.{extension}"));
            shot.save_image_as(&target, 73, ExportSize::Half)?;
            export_saved_png_as(&shot.png_path, &saved, 73, ExportSize::Quarter)?;
            let bytes = fs::read(&target)?;
            assert_eq!(image::open(&target)?.into_rgba8().dimensions(), (2, 2));
            assert_eq!(image::open(&saved)?.into_rgba8().dimensions(), (1, 1));
            let custom = root.join(format!("custom.{extension}"));
            export_saved_png_as(&shot.png_path, &custom, 73, ExportSize::Width(3))?;
            assert_eq!(image::open(&custom)?.into_rgba8().dimensions(), (3, 3));
            fs::remove_file(custom)?;
            let custom = root.join(format!("height.{extension}"));
            shot.save_image_as(&custom, 73, ExportSize::Height(3))?;
            assert_eq!(image::open(&custom)?.into_rgba8().dimensions(), (3, 3));
            fs::remove_file(&custom)?;
            export_saved_png_as(&shot.png_path, &custom, 73, ExportSize::Height(2))?;
            assert_eq!(image::open(&custom)?.into_rgba8().dimensions(), (2, 2));
            fs::remove_file(custom)?;
            let custom = root.join(format!("exact.{extension}"));
            shot.save_image_as(&custom, 73, ExportSize::Exact(3, 1))?;
            assert_eq!(image::open(&custom)?.into_rgba8().dimensions(), (3, 1));
            fs::remove_file(&custom)?;
            export_saved_png_as(&shot.png_path, &custom, 73, ExportSize::Exact(1, 3))?;
            assert_eq!(image::open(&custom)?.into_rgba8().dimensions(), (1, 3));
            fs::remove_file(custom)?;
            assert!(shot.save_image_as(&target, 73, ExportSize::Half).is_err());
            assert_eq!(fs::read(&target)?, bytes);
            fs::remove_file(target)?;
            fs::remove_file(saved)?;
        }
        let mut pixels = shot.pixels.as_ref().clone();
        pixels.put_pixel(0, 0, image::Rgba([12, 34, 56, 254]));
        let transparent = CapturedImage {
            pixels: Arc::new(pixels),
            ..shot.clone()
        };
        let rejected = root.join("transparent.jpg");
        assert!(
            transparent
                .save_image_as(&rejected, 73, ExportSize::Quarter)
                .is_err()
        );
        assert!(!rejected.exists());
        assert!(
            shot.save_image_as(&rejected, 73, ExportSize::Width(0))
                .is_err()
        );
        assert_eq!(fs::read(&shot.png_path)?, before);
        assert_eq!(fs::read_dir(&root)?.count(), 1);
        fs::remove_file(shot.png_path)?;
        fs::remove_dir(root)?;
        Ok(())
    }

    #[test]
    fn png_without_metadata_reencodes_pixels_and_leaves_the_source_file() -> anyhow::Result<()> {
        let root =
            std::env::temp_dir().join(format!("screenshots-png-sanitize-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root)?;
        let path = root.join("source.png");
        let pixels = RgbaImage::from_pixel(2, 2, image::Rgba([12, 34, 56, 78]));
        pixels.save(&path)?;
        let original = fs::read(&path)?;
        let with_text = insert_png_text_chunk(&original, "Comment", "secret location")?;
        fs::write(&path, &with_text)?;
        assert!(
            png_chunk_types(&with_text)?
                .iter()
                .any(|chunk| chunk == b"tEXt")
        );
        let sanitized = png_without_metadata(&path)?;
        assert!(sanitized.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10]));
        assert!(
            !png_chunk_types(&sanitized)?
                .iter()
                .any(|chunk| { matches!(chunk, b"tEXt" | b"zTXt" | b"iTXt" | b"eXIf" | b"tIME") })
        );
        assert_eq!(image::load_from_memory(&sanitized)?.into_rgba8(), pixels);
        assert_eq!(fs::read(&path)?, with_text);
        let missing = root.join("missing.png");
        assert!(png_without_metadata(&missing).is_err());
        assert!(!missing.exists());
        fs::write(&path, b"not a png")?;
        assert!(png_without_metadata(&path).is_err());
        assert_eq!(fs::read(&path)?, b"not a png");
        fs::remove_file(path)?;
        fs::remove_dir(root)?;
        Ok(())
    }

    fn png_chunk_types(bytes: &[u8]) -> anyhow::Result<Vec<[u8; 4]>> {
        anyhow::ensure!(bytes.len() >= 8 && bytes[..8] == [137, 80, 78, 71, 13, 10, 26, 10]);
        let mut offset = 8;
        let mut types = Vec::new();
        while offset + 12 <= bytes.len() {
            let length =
                usize::try_from(u32::from_be_bytes(bytes[offset..offset + 4].try_into()?))?;
            let ty = bytes[offset + 4..offset + 8].try_into()?;
            types.push(ty);
            offset = offset
                .checked_add(12)
                .and_then(|end| end.checked_add(length))
                .ok_or_else(|| anyhow::anyhow!("PNG chunk overflow"))?;
            if ty == *b"IEND" {
                break;
            }
        }
        Ok(types)
    }

    fn insert_png_text_chunk(png: &[u8], keyword: &str, text: &str) -> anyhow::Result<Vec<u8>> {
        let iend = png
            .windows(8)
            .position(|window| &window[4..] == b"IEND")
            .ok_or_else(|| anyhow::anyhow!("PNG IEND missing"))?;
        let mut payload = Vec::from(keyword.as_bytes());
        payload.push(0);
        payload.extend_from_slice(text.as_bytes());
        let mut chunk = Vec::new();
        chunk.extend_from_slice(&(u32::try_from(payload.len())?).to_be_bytes());
        chunk.extend_from_slice(b"tEXt");
        chunk.extend_from_slice(&payload);
        chunk.extend_from_slice(&png_crc(&chunk[4..]).to_be_bytes());
        let mut out = png[..iend].to_vec();
        out.extend_from_slice(&chunk);
        out.extend_from_slice(&png[iend..]);
        Ok(out)
    }

    fn png_crc(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn jpeg_quality_controls_memory_and_saved_png_exports() -> anyhow::Result<()> {
        let root =
            std::env::temp_dir().join(format!("screenshots-jpeg-quality-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root)?;
        let shot = CapturedImage {
            width: 64,
            height: 64,
            pixels: Arc::new(RgbaImage::from_fn(64, 64, |x, y| {
                image::Rgba([
                    ((x * 47 + y * 13) % 256) as u8,
                    ((y * 71 + x * 19) % 256) as u8,
                    ((x + y) * 31 % 256) as u8,
                    255,
                ])
            })),
            png_path: root.join("source.png"),
            display_id: "fixture".into(),
            window_id: None,
            elapsed_ms: 0,
            source_format: PixelFormat::Rgba8UnormSrgb,
            display_geometry: None,
        };
        shot.save_png()?;
        let mut errors = Vec::new();
        for quality in [10, 100] {
            let memory = root.join(format!("memory-{quality}.jpg"));
            let saved = root.join(format!("saved-{quality}.jpg"));
            shot.save_image_as(&memory, quality, ExportSize::Original)?;
            export_saved_png_as(&shot.png_path, &saved, quality, ExportSize::Original)?;
            assert_eq!(fs::read(&memory)?, fs::read(&saved)?);
            let decoded = image::open(&memory)?.into_rgba8();
            assert_eq!(decoded.dimensions(), (64, 64));
            errors.push(
                decoded
                    .as_raw()
                    .iter()
                    .zip(shot.pixels.as_raw())
                    .map(|(a, b)| u64::from(a.abs_diff(*b)))
                    .sum::<u64>(),
            );
            fs::remove_file(memory)?;
            fs::remove_file(saved)?;
        }
        assert!(errors[1] < errors[0], "{errors:?}");
        for quality in [0, 101, 255] {
            let destination = root.join("invalid.jpg");
            assert!(
                shot.save_image_as(&destination, quality, ExportSize::Original)
                    .is_err()
            );
            assert!(
                export_saved_png_as(&shot.png_path, &destination, quality, ExportSize::Original)
                    .is_err()
            );
            assert!(!destination.exists());
        }
        assert_eq!(fs::read_dir(&root)?.count(), 1);
        fs::remove_file(shot.png_path)?;
        fs::remove_dir(root)?;
        Ok(())
    }

    #[test]
    fn image_export_formats_preserve_sources_alpha_and_existing_files() -> anyhow::Result<()> {
        use image::GenericImageView as _;
        let root = std::env::temp_dir().join(format!(
            "screenshots-format-export-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root)?;
        let mut shot = CapturedImage {
            display_id: "fixture".into(),
            window_id: None,
            width: 3,
            height: 2,
            pixels: Arc::new(RgbaImage::from_pixel(3, 2, image::Rgba([12, 34, 56, 255]))),
            png_path: root.join("source.png"),
            elapsed_ms: 0,
            source_format: PixelFormat::Rgba8UnormSrgb,
            display_geometry: None,
        };
        shot.save_png()?;
        let original = fs::read(&shot.png_path)?;
        for extension in ["png", "JPG", "jpeg", "webp"] {
            let destination = root.join(format!("copy.{extension}"));
            let format = shot.save_image_as(&destination, 90, ExportSize::Original)?;
            let bytes = fs::read(&destination)?;
            assert_eq!(
                image::guess_format(&bytes)?,
                match format {
                    ImageExportFormat::Png => image::ImageFormat::Png,
                    ImageExportFormat::Jpeg => image::ImageFormat::Jpeg,
                    ImageExportFormat::WebP => image::ImageFormat::WebP,
                }
            );
            let decoded = image::load_from_memory(&bytes)?.into_rgba8();
            assert_eq!(decoded.dimensions(), (3, 2));
            if format != ImageExportFormat::Jpeg {
                assert_eq!(decoded, *shot.pixels);
            }
            assert!(
                shot.save_image_as(&destination, 90, ExportSize::Original)
                    .is_err()
            );
            assert_eq!(fs::read(&destination)?, bytes);
            let from_png = root.join(format!("from-png.{extension}"));
            assert_eq!(
                export_saved_png_as(&shot.png_path, &from_png, 90, ExportSize::Original)?,
                format
            );
            assert_eq!(image::open(&from_png)?.dimensions(), (3, 2));
            fs::remove_file(destination)?;
            fs::remove_file(from_png)?;
        }
        for extension in ["gif", "bmp", ""] {
            let destination = root.join(format!("unsupported.{extension}"));
            assert!(
                shot.save_image_as(&destination, 90, ExportSize::Original)
                    .is_err()
            );
            assert!(!destination.exists());
        }
        shot.pixels = Arc::new(RgbaImage::from_fn(3, 2, |x, _| {
            image::Rgba([12, 34, 56, if x == 0 { 0 } else { 78 }])
        }));
        let transparent = root.join("transparent.webp");
        shot.save_image_as(&transparent, 90, ExportSize::Original)?;
        assert_eq!(image::open(&transparent)?.into_rgba8(), *shot.pixels);
        let rejected = root.join("transparent.jpg");
        assert!(
            shot.save_image_as(&rejected, 90, ExportSize::Original)
                .unwrap_err()
                .to_string()
                .contains("transparency")
        );
        assert!(!rejected.exists());
        let wide = CapturedImage {
            width: 65536,
            height: 1,
            pixels: Arc::new(RgbaImage::from_pixel(
                65536,
                1,
                image::Rgba([12, 34, 56, 255]),
            )),
            ..shot.clone()
        };
        let failed = root.join("encoder-failure.jpg");
        assert!(
            wide.save_image_as(&failed, 90, ExportSize::Original)
                .is_err()
        );
        assert!(!failed.exists());
        assert_eq!(fs::read(&shot.png_path)?, original);
        assert_eq!(fs::read_dir(&root)?.count(), 2);
        fs::remove_file(transparent)?;
        fs::remove_file(shot.png_path)?;
        fs::remove_dir(root)?;
        Ok(())
    }

    #[test]
    #[ignore = "short optimized 4K PNG/JPEG/WebP export comparison; synthetic images and temporary files only"]
    fn image_export_4k_cost() -> anyhow::Result<()> {
        let root =
            std::env::temp_dir().join(format!("screenshots-export-cost-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root)?;
        let shot = CapturedImage {
            width: 3840,
            height: 2160,
            pixels: Arc::new(RgbaImage::from_fn(3840, 2160, |x, y| {
                image::Rgba([(x % 251) as u8, (y % 251) as u8, ((x + y) % 251) as u8, 255])
            })),
            png_path: root.join("unused.png"),
            display_id: "fixture".into(),
            window_id: None,
            elapsed_ms: 0,
            source_format: PixelFormat::Rgba8UnormSrgb,
            display_geometry: None,
        };
        for (extension, quality, scale) in [
            ("png", 90, ExportSize::Original),
            ("png", 90, ExportSize::Half),
            ("png", 90, ExportSize::Quarter),
            ("jpg", 50, ExportSize::Original),
            ("jpg", 90, ExportSize::Original),
            ("jpg", 100, ExportSize::Original),
            ("webp", 90, ExportSize::Original),
            ("png", 90, ExportSize::Width(640)),
            ("png", 90, ExportSize::Height(360)),
            ("png", 90, ExportSize::Exact(640, 360)),
        ] {
            let path = root.join(format!("export.{extension}"));
            let mut timings = Vec::new();
            let mut length = 0;
            for _ in 0..3 {
                let started = Instant::now();
                shot.save_image_as(&path, quality, scale)?;
                timings.push(started.elapsed());
                length = fs::metadata(&path)?.len();
                fs::remove_file(&path)?;
            }
            timings.sort_unstable();
            eprintln!(
                "4K {extension} (JPEG quality {quality}, size {scale:?}) resize/encode + durable publication median={:?}; output={length} bytes; 3 synthetic warm samples; excludes dialog/presentation/process memory",
                timings[1]
            );
        }
        fs::remove_dir(root)?;
        Ok(())
    }

    #[test]
    fn exact_region_metadata_checks_display_geometry_and_bounds_without_pixels() {
        let display = DisplayInfo {
            id: "fixture".into(),
            name: "Fixture".into(),
            x: -1920,
            y: -100,
            width: 1920,
            height: 1080,
            scale_factor: 1.5,
            rotation: 0.,
            is_primary: false,
        };
        let area = crate::PixelRegion {
            x: 1919,
            y: 1079,
            width: 1,
            height: 1,
        };
        let region = RememberedRegion::from_display(&display, area).unwrap();
        assert_eq!(region.area(), area);
        assert_eq!(region.geometry.x, -1920);
        region.validate_geometry(region.geometry).unwrap();
        for geometry in [
            DisplayGeometry {
                x: 0,
                ..region.geometry
            },
            DisplayGeometry {
                scale_factor: 2.,
                ..region.geometry
            },
            DisplayGeometry {
                rotation: 90.,
                ..region.geometry
            },
            DisplayGeometry {
                width: 1080,
                ..region.geometry
            },
        ] {
            assert!(matches!(
                region.validate_geometry(geometry),
                Err(CaptureError::StaleRegion)
            ));
        }
        for area in [
            crate::PixelRegion { width: 0, ..area },
            crate::PixelRegion { height: 0, ..area },
            crate::PixelRegion { width: 2, ..area },
            crate::PixelRegion { height: 2, ..area },
            crate::PixelRegion {
                x: u32::MAX,
                ..area
            },
        ] {
            assert!(RememberedRegion::from_display(&display, area).is_err());
        }
        for display in [
            DisplayInfo {
                id: String::new(),
                ..display.clone()
            },
            DisplayInfo {
                scale_factor: f32::NAN,
                ..display.clone()
            },
            DisplayInfo {
                width: u32::MAX,
                ..display
            },
        ] {
            assert!(RememberedRegion::from_display(&display, area).is_err());
        }
    }
    use image::Rgba;

    #[test]
    fn remembered_region_is_metadata_only_and_rejects_geometry_changes() {
        let geometry = DisplayGeometry {
            x: -8,
            y: 12,
            width: 8,
            height: 6,
            scale_factor: 1.25,
            rotation: 90.0,
        };
        let source = CapturedImage {
            display_id: "fixture".into(),
            window_id: None,
            width: 8,
            height: 6,
            pixels: Arc::new(RgbaImage::from_pixel(8, 6, Rgba([12, 34, 56, 255]))),
            png_path: std::env::temp_dir().join("unused-repeat-source.png"),
            elapsed_ms: 0,
            source_format: PixelFormat::Rgba8UnormSrgb,
            display_geometry: Some(geometry),
        };
        let area = crate::PixelRegion {
            x: 1,
            y: 2,
            width: 3,
            height: 2,
        };
        let pixels = Arc::downgrade(&source.pixels);
        let remembered = RememberedRegion::from_snapshot(&source, area).unwrap();
        assert_eq!(Arc::strong_count(&source.pixels), 1);
        assert_eq!(remembered.area(), area);
        remembered.validate_geometry(geometry).unwrap();
        for changed in [
            DisplayGeometry { x: -7, ..geometry },
            DisplayGeometry { y: 0, ..geometry },
            DisplayGeometry {
                width: 7,
                ..geometry
            },
            DisplayGeometry {
                height: 7,
                ..geometry
            },
            DisplayGeometry {
                scale_factor: 1.5,
                ..geometry
            },
            DisplayGeometry {
                rotation: 0.0,
                ..geometry
            },
            DisplayGeometry {
                scale_factor: f32::NAN,
                ..geometry
            },
            DisplayGeometry {
                scale_factor: 0.0,
                ..geometry
            },
        ] {
            assert!(matches!(
                remembered.validate_geometry(changed),
                Err(CaptureError::StaleRegion)
            ));
        }
        for invalid in [
            crate::PixelRegion { width: 0, ..area },
            crate::PixelRegion {
                x: u32::MAX,
                ..area
            },
            crate::PixelRegion { x: 7, ..area },
            crate::PixelRegion { y: 5, ..area },
        ] {
            assert!(RememberedRegion::from_snapshot(&source, invalid).is_err());
        }
        let cropped = crate::crop_snapshot(&source, area).unwrap();
        assert!(cropped.display_geometry.is_none());
        assert!(RememberedRegion::from_snapshot(&cropped, area).is_err());
        let edited = crate::redact_snapshot(&source, &[area]).unwrap();
        assert!(edited.display_geometry.is_none());
        let mut unrelated = source.clone();
        unrelated.window_id = Some("window".into());
        assert!(RememberedRegion::from_snapshot(&unrelated, area).is_err());
        unrelated.window_id = None;
        unrelated.display_geometry = None;
        assert!(RememberedRegion::from_snapshot(&unrelated, area).is_err());
        drop(unrelated);
        drop(source);
        assert!(
            pixels.upgrade().is_none(),
            "Remembered metadata retained the desktop pixels"
        );
        remembered.validate_geometry(geometry).unwrap();
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires an explicit opaque external window fixture; captures only a 2x2 rectangle inside it into RAM"]
    fn repeats_only_the_explicit_fixture_region() -> anyhow::Result<()> {
        use anyhow::ensure;
        use windows_sys::Win32::{
            Foundation::RECT,
            UI::WindowsAndMessaging::{GetWindowRect, GetWindowThreadProcessId, IsWindowVisible},
        };
        #[link(name = "user32")]
        unsafe extern "system" {
            fn SetThreadDpiAwarenessContext(
                context: *mut std::ffi::c_void,
            ) -> *mut std::ffi::c_void;
        }
        let previous = unsafe { SetThreadDpiAwarenessContext((-4isize) as *mut _) };
        ensure!(!previous.is_null(), "Per-monitor DPI context unavailable");
        struct RestoreDpi(*mut std::ffi::c_void);
        impl Drop for RestoreDpi {
            fn drop(&mut self) {
                unsafe {
                    SetThreadDpiAwarenessContext(self.0);
                }
            }
        }
        let _restore = RestoreDpi(previous);
        let expected: u32 = std::env::var("SCREENSHOTS_ACTIVE_WINDOW_FIXTURE_PID")?.parse()?;
        let handle = std::env::var("SCREENSHOTS_ACTIVE_WINDOW_FIXTURE_HANDLE")?.parse::<isize>()?
            as windows_sys::Win32::Foundation::HWND;
        let mut owner = 0;
        ensure!(
            unsafe { GetWindowThreadProcessId(handle, &mut owner) } != 0
                && owner == expected
                && owner != std::process::id(),
            "Unexpected fixture process; no capture attempted"
        );
        ensure!(
            unsafe { IsWindowVisible(handle) } != 0,
            "Fixture is not visible"
        );
        let mut bounds = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        ensure!(
            unsafe { GetWindowRect(handle, &mut bounds) } != 0,
            "Fixture bounds unavailable"
        );
        ensure!(
            (100..1024).contains(&(bounds.right - bounds.left))
                && (100..1024).contains(&(bounds.bottom - bounds.top)),
            "Unexpected fixture size"
        );
        let window = Window::all()?
            .into_iter()
            .find(|window| window.id().ok() == Some(handle as usize as u32))
            .ok_or_else(|| anyhow::anyhow!("Fixture not listed"))?;
        let monitor = window.current_monitor()?;
        let geometry = monitor_geometry(&monitor)?;
        validate_image_size(geometry.width, geometry.height)?;
        let area = crate::PixelRegion {
            x: u32::try_from(
                i64::from(bounds.left) + i64::from(bounds.right - bounds.left) / 2
                    - i64::from(geometry.x),
            )?,
            y: u32::try_from(
                i64::from(bounds.top) + i64::from(bounds.bottom - bounds.top) / 2
                    - i64::from(geometry.y),
            )?,
            width: 2,
            height: 2,
        };
        // The source is synthetic: no full-desktop pixels are captured to remember geometry.
        let source = CapturedImage {
            display_id: monitor_id(&monitor)?,
            window_id: None,
            width: geometry.width,
            height: geometry.height,
            pixels: Arc::new(RgbaImage::new(geometry.width, geometry.height)),
            png_path: std::env::temp_dir().join("unused-repeat-fixture.png"),
            elapsed_ms: 0,
            source_format: PixelFormat::Rgba8UnormSrgb,
            display_geometry: Some(geometry),
        };
        let remembered = RememberedRegion::from_snapshot(&source, area)?;
        let id = source.display_id.clone();
        drop(source);
        crate::wait_for_desktop_compositor()?;
        let backend = XcapBackend::new(std::env::temp_dir().join("unused-repeat-output"));
        let baseline_started = Instant::now();
        let baseline = backend.capture_batch(vec![CaptureRequest::Region {
            display_id: id,
            x: i32::try_from(area.x)?,
            y: i32::try_from(area.y)?,
            width: 2,
            height: 2,
        }])?;
        let baseline_elapsed = baseline_started.elapsed();
        let repeat_started = Instant::now();
        let repeated =
            backend.capture_batch(vec![CaptureRequest::RepeatRegion(remembered.clone())])?;
        let repeat_elapsed = repeat_started.elapsed();
        assert_eq!(baseline[0].pixels, repeated[0].pixels);
        let exact_region = RememberedRegion::from_display(
            &DisplayInfo {
                id: repeated[0].display_id.clone(),
                name: "Explicit fixture".into(),
                x: geometry.x,
                y: geometry.y,
                width: geometry.width,
                height: geometry.height,
                scale_factor: geometry.scale_factor,
                rotation: geometry.rotation,
                is_primary: false,
            },
            area,
        )?;
        let exact = backend.capture_batch(vec![CaptureRequest::ExactRegion(exact_region)])?;
        assert_eq!(baseline[0].pixels, exact[0].pixels);
        assert!(!exact[0].png_path.exists());
        assert_eq!(repeated[0].pixels.dimensions(), (2, 2));
        assert!(repeated[0].display_geometry.is_none());
        assert!(!baseline[0].png_path.exists() && !repeated[0].png_path.exists());
        let mut changed = remembered;
        changed.geometry.scale_factor += 0.25;
        assert!(matches!(
            backend.capture_batch(vec![CaptureRequest::RepeatRegion(changed)]),
            Err(CaptureError::StaleRegion)
        ));
        eprintln!(
            "Native 2x2 region baseline: {baseline_elapsed:?}; repeat: {repeat_elapsed:?}. Single observations, not a latency acceptance result."
        );
        Ok(())
    }

    #[test]
    fn capture_admission_bounds_total_without_allocating_pixels() {
        let mut budget = CaptureBudget::default();
        // Two 8192x4096 images fit exactly; another must fail before capture.
        budget.admit(8192, 4096).unwrap();
        budget.admit(8192, 4096).unwrap();
        assert_eq!(budget.0, MAX_CAPTURE_BYTES);
        assert!(matches!(budget.admit(1, 1), Err(CaptureError::MemoryLimit)));
        assert_eq!(budget.0, MAX_CAPTURE_BYTES);
        assert!(matches!(
            validate_image_size(u32::MAX, u32::MAX),
            Err(CaptureError::MemoryLimit)
        ));
        assert!(matches!(
            validate_image_size(0, 100),
            Err(CaptureError::EmptyFrame)
        ));
        validate_image_size(7680, 4320).unwrap();

        let mut partial = CaptureBudget::default();
        partial.admit(3840, 2160).unwrap();

        let mut unknown_native_extent = CaptureBudget::default();
        unknown_native_extent
            .admit_bytes(MAX_CAPTURE_BYTES)
            .unwrap();
        assert!(unknown_native_extent.admit(1, 1).is_err());
        assert_eq!(unknown_native_extent.0, MAX_CAPTURE_BYTES);
        let mut earlier_frame = CaptureBudget::default();
        earlier_frame.admit(1, 1).unwrap();
        assert!(earlier_frame.admit_bytes(MAX_CAPTURE_BYTES).is_err());
        assert_eq!(earlier_frame.0, 4);
        assert!(earlier_frame.admit_bytes(u64::MAX).is_err());
        assert_eq!(earlier_frame.0, 4);
        let before = partial.0;
        assert!(partial.admit(8192, 8192).is_err());
        assert_eq!(partial.0, before);
        partial.admit(3840, 2160).unwrap();
    }

    #[test]
    fn png_publication_is_complete_and_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("screenshots-save-{}", uuid::Uuid::new_v4()));
        let shot = CapturedImage {
            display_id: "test".into(),
            window_id: None,
            width: 2,
            height: 2,
            pixels: Arc::new(RgbaImage::from_pixel(2, 2, Rgba([12, 34, 56, 255]))),
            png_path: dir.join("capture.png"),
            elapsed_ms: 0,
            source_format: PixelFormat::Rgba8UnormSrgb,
            display_geometry: None,
        };
        shot.save_png().expect("publish PNG");
        assert_eq!(
            image::open(&shot.png_path).unwrap().into_rgba8(),
            *shot.pixels
        );
        let original = fs::read(&shot.png_path).unwrap();
        let different = CapturedImage {
            pixels: Arc::new(RgbaImage::from_pixel(2, 2, Rgba([255, 0, 0, 255]))),
            ..shot.clone()
        };
        assert!(different.save_png().is_err());
        assert_eq!(fs::read(&shot.png_path).unwrap(), original);
        assert_eq!(
            fs::read_dir(&dir).unwrap().count(),
            1,
            "temporary files leaked"
        );
        let exported = dir.join("export.png");
        export_saved_png(&shot.png_path, &exported).unwrap();
        assert_eq!(fs::read(&exported).unwrap(), original);
        assert!(export_saved_png(&shot.png_path, &exported).is_err());
        assert!(export_saved_png(&shot.png_path, &shot.png_path).is_err());
        assert_eq!(fs::read(&shot.png_path).unwrap(), original);
        let recovered = dir.join("recovered.png");
        let canceled = std::cell::Cell::new(false);
        let canceled_path = dir.join("canceled.png");
        let result = write_output_checked(
            &canceled_path,
            |file| {
                file.write_all(&original)?;
                canceled.set(true);
                Ok(())
            },
            || {
                if canceled.get() {
                    Err(CaptureError::Canceled)
                } else {
                    Ok(())
                }
            },
        );
        assert!(matches!(result, Err(CaptureError::Canceled)));
        assert!(!canceled_path.exists());
        different.save_png_as(&recovered).unwrap();
        assert_eq!(
            image::open(&recovered).unwrap().into_rgba8(),
            *different.pixels
        );
        assert_eq!(
            fs::read_dir(&dir).unwrap().count(),
            3,
            "export temporary files leaked"
        );
        fs::remove_file(exported).unwrap();
        fs::remove_file(recovered).unwrap();
        fs::remove_file(&shot.png_path).unwrap();
        fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn invalid_window_id_does_not_require_a_desktop() {
        let backend = XcapBackend::new(std::env::temp_dir());
        let result = backend.capture_window("invalid-window");
        if backend.capabilities().capture_window {
            assert!(matches!(result, Err(CaptureError::WindowNotFound(_))));
        } else {
            assert!(matches!(result, Err(CaptureError::Unsupported(_))));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_window_capabilities_require_a_nonempty_display() {
        use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
        for display in [None, Some(""), Some(" \t\n")] {
            assert!(!linux_x11_windows_available(display.map(OsStr::new)));
        }
        for display in [":0", ":1.0", "localhost:10.0"] {
            assert!(linux_x11_windows_available(Some(OsStr::new(display))));
        }
        assert!(!linux_x11_windows_available(Some(OsStr::from_bytes(&[
            0xff
        ]))));
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires an X11/XWayland desktop, Python 3 and libX11; creates/captures only its own synthetic external window in RAM"]
    fn captures_only_its_synthetic_x11_window() -> anyhow::Result<()> {
        use std::{
            io::{BufRead, BufReader},
            process::{Child, Command, Stdio},
            time::Duration,
        };
        struct Fixture(Child);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        // Only this client's window properties are written; root/other clients are untouched.
        let script = r#"
import ctypes as c, ctypes.util, os, sys
x = c.CDLL(ctypes.util.find_library('X11'))
def bind(name, result, *args):
    fn = getattr(x, name); fn.restype = result; fn.argtypes = args; return fn
ptr, ul, ui, integer = c.c_void_p, c.c_ulong, c.c_uint, c.c_int
open_display = bind('XOpenDisplay', ptr, c.c_char_p)
root_window = bind('XDefaultRootWindow', ul, ptr)
create_window = bind('XCreateSimpleWindow', ul, ptr, ul, integer, integer, ui, ui, ui, ul, ul)
atom = bind('XInternAtom', ul, ptr, c.c_char_p, integer)
change = bind('XChangeProperty', integer, ptr, ul, ul, ul, integer, integer, ptr, integer)
store_name = bind('XStoreName', integer, ptr, ul, c.c_char_p)
map_window = bind('XMapWindow', integer, ptr, ul)
clear_window = bind('XClearWindow', integer, ptr, ul)
sync = bind('XSync', integer, ptr, integer)
destroy = bind('XDestroyWindow', integer, ptr, ul)
close = bind('XCloseDisplay', integer, ptr)
d = open_display(None)
if not d: raise RuntimeError('Synthetic fixture cannot connect to DISPLAY')
w = create_window(d, root_window(d), 40, 40, 96, 64, 0, 0, 0x123456)
def prop(name, kind, values):
    data = (ul * len(values))(*values)
    change(d, w, atom(d, name.encode(), 0), atom(d, kind.encode(), 0), 32, 0, data, len(values))
store_name(d, w, b'Screenshots synthetic X11 fixture')
prop('_NET_WM_PID', 'CARDINAL', [os.getpid()])
prop('_NET_WM_USER_TIME', 'CARDINAL', [0])
prop('_MOTIF_WM_HINTS', '_MOTIF_WM_HINTS', [2, 0, 0, 0, 0])
prop('_NET_WM_STATE', 'ATOM', [])
map_window(d, w); clear_window(d, w); sync(d, 0)
print(w, flush=True)
for line in sys.stdin:
    command = line.strip().split()
    if command[0] == 'pid': prop('_NET_WM_PID', 'CARDINAL', [int(command[1])])
    elif command[0] == 'hidden': prop('_NET_WM_STATE', 'ATOM', [atom(d, b'_NET_WM_STATE_HIDDEN', 0)])
    elif command[0] == 'normal': prop('_NET_WM_STATE', 'ATOM', [])
    elif command[0] == 'close': destroy(d, w); w = 0
    sync(d, 0); print('ready', flush=True)
    if not w: break
if w: destroy(d, w)
close(d)
"#;
        let mut fixture = Fixture(
            Command::new("python3")
                .args(["-u", "-c", script])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()?,
        );
        let mut input = fixture.0.stdin.take().unwrap();
        let mut output = BufReader::new(fixture.0.stdout.take().unwrap());
        let mut id = String::new();
        output.read_line(&mut id)?;
        let id = id.trim().to_owned();
        assert!(
            id.parse::<u32>().is_ok(),
            "Fixture did not return a window ID"
        );
        let directory =
            std::env::temp_dir().join(format!("screenshots-x11-fixture-{}", uuid::Uuid::new_v4()));
        let backend = XcapBackend::new(&directory);
        assert!(backend.capabilities().list_windows && backend.capabilities().capture_window);
        let started = Instant::now();
        loop {
            if backend.list_windows()?.iter().any(|window| window.id == id) {
                break;
            }
            anyhow::ensure!(
                started.elapsed() < Duration::from_secs(3),
                "Fixture was not listed"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let shot = backend.capture_window(&id)?;
        assert_eq!(shot.window_id.as_deref(), Some(id.as_str()));
        assert!(shot.display_id.is_empty() && shot.display_geometry.is_none());
        assert_eq!((shot.width, shot.height), (96, 64));
        assert!(
            shot.pixels
                .pixels()
                .all(|pixel| pixel.0 == [0x12, 0x34, 0x56, 255])
        );
        assert!(!directory.exists() && !shot.png_path.exists());
        drop(shot);
        let mut send = |command: &str| -> anyhow::Result<()> {
            writeln!(input, "{command}")?;
            input.flush()?;
            let mut response = String::new();
            output.read_line(&mut response)?;
            anyhow::ensure!(response.trim() == "ready", "Fixture command failed");
            Ok(())
        };
        send(&format!("pid {}", std::process::id()))?;
        assert!(matches!(
            backend.capture_window(&id),
            Err(CaptureError::Unsupported(
                "capturing this application's own window"
            ))
        ));
        send(&format!("pid {}", fixture.0.id()))?;
        send("hidden")?;
        assert!(matches!(
            backend.capture_window(&id),
            Err(CaptureError::Unsupported("capturing a minimized window"))
        ));
        send("normal")?;
        send("close")?;
        assert!(backend.capture_window(&id).is_err());
        assert!(!directory.exists());
        Ok(())
    }

    #[test]
    #[ignore = "requires SCREENSHOTS_TEST_WINDOW_ID and an interactive desktop"]
    fn captures_selected_window_to_png() {
        let id = std::env::var("SCREENSHOTS_TEST_WINDOW_ID")
            .expect("set SCREENSHOTS_TEST_WINDOW_ID to a listed window ID");
        let backend = XcapBackend::new(std::env::temp_dir().join("screenshots-window-spike"));
        let shot = backend.capture_window(&id).expect("capture window");
        assert_eq!(shot.window_id.as_deref(), Some(id.as_str()));
        shot.save_png().expect("save PNG");
        let decoded = image::open(&shot.png_path).expect("decode captured PNG");
        assert_eq!(
            (decoded.width(), decoded.height()),
            (shot.width, shot.height)
        );
        assert!(shot.width > 0 && shot.height > 0);
    }

    #[test]
    fn rejects_empty_but_accepts_black_frames() {
        let empty = RgbaImage::new(0, 0);
        assert!(frame_is_unusable(&empty));

        let black = RgbaImage::from_pixel(4, 4, Rgba([0, 0, 0, 255]));
        assert!(!frame_is_unusable(&black));

        let mut pixel = RgbaImage::from_pixel(4, 4, Rgba([0, 0, 0, 255]));
        pixel.put_pixel(1, 1, Rgba([10, 0, 0, 255]));
        assert!(!frame_is_unusable(&pixel));
    }

    #[test]
    #[ignore = "requires an interactive desktop and writes a real screenshot"]
    fn captures_primary_monitor_to_png() {
        let dir = std::env::temp_dir().join("screenshots-xcap-spike");
        let backend = XcapBackend::new(&dir);
        let displays = backend.list_displays().expect("list displays");
        assert!(!displays.is_empty(), "XCap returned no monitors");
        let primary = displays
            .iter()
            .find(|info| info.is_primary)
            .unwrap_or(&displays[0]);
        let shot = backend
            .capture_display(&primary.id)
            .expect("capture primary");
        assert!(shot.width > 0 && shot.height > 0);
        assert_eq!(shot.source_format, PixelFormat::Rgba8UnormSrgb);
        shot.save_png().expect("save PNG");
        let bytes = std::fs::read(&shot.png_path).expect("read png");
        assert_eq!(
            &bytes[..8],
            &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
        );
    }
}

impl CapturedImage {
    pub fn save_image_as(
        &self,
        destination: &Path,
        jpeg_quality: u8,
        size: ExportSize,
    ) -> Result<ImageExportFormat, CaptureError> {
        self.save_image_as_with_cancel(destination, jpeg_quality, size, || false)
    }

    pub fn save_image_as_with_cancel(
        &self,
        destination: &Path,
        jpeg_quality: u8,
        size: ExportSize,
        cancelled: impl Fn() -> bool,
    ) -> Result<ImageExportFormat, CaptureError> {
        let cancelled = crate::image_cancel::latch(cancelled);
        let mut work = crate::image_cancel::ImageWork::new(&cancelled)?;
        validate_image_size(self.pixels.width(), self.pixels.height())?;
        let format = ImageExportFormat::from_path(destination)?;
        format.validate_quality(jpeg_quality)?;
        size.validate()?;
        let output_size = size.dimensions(self.pixels.width(), self.pixels.height())?;
        format.validate_output_size(output_size.0, output_size.1)?;
        // Check original transparency before reducing, preserving JPEG admission.
        if size != ExportSize::Original && format == ImageExportFormat::Jpeg {
            for pixel in self.pixels.pixels() {
                work.pixel()?;
                if pixel[3] != 255 {
                    return Err(CaptureError::Backend(
                        "JPEG cannot preserve transparency. Choose PNG or WebP.".into(),
                    ));
                }
            }
        }
        if output_size == self.pixels.dimensions() {
            write_image_export_with_cancel(
                &self.pixels,
                destination,
                format,
                jpeg_quality,
                &cancelled,
            )?;
        } else {
            let scaled = crate::downscale_export_with_cancel(&self.pixels, size, &cancelled)?;
            write_image_export_with_cancel(&scaled, destination, format, jpeg_quality, &cancelled)?;
        }
        Ok(format)
    }

    pub fn save_png(&self) -> Result<(), CaptureError> {
        self.save_png_as(&self.png_path)
    }

    pub fn save_png_as(&self, destination: &Path) -> Result<(), CaptureError> {
        write_image_export(
            &self.pixels,
            destination,
            ImageExportFormat::Png,
            crate::DEFAULT_JPEG_QUALITY,
        )
    }

    /// Encode the current pixels as PNG without writing a library file.
    pub fn png_bytes(&self) -> Result<Vec<u8>, CaptureError> {
        encode_png_bytes(&self.pixels)
    }

    pub fn png_bytes_with_cancel(
        &self,
        cancelled: impl Fn() -> bool,
    ) -> Result<Vec<u8>, CaptureError> {
        encode_png_bytes_with_cancel(&self.pixels, &cancelled)
    }
}

/// Re-encode a saved PNG from pixels so ancillary chunks are not sent.
/// The source file is not modified.
pub fn png_without_metadata(path: &Path) -> Result<Vec<u8>, CaptureError> {
    let pixels =
        crate::load_saved_png(path).map_err(|error| CaptureError::Backend(error.to_string()))?;
    encode_png_bytes(&pixels)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageExportFormat {
    #[default]
    Png,
    Jpeg,
    #[serde(rename = "webp")]
    WebP,
}

impl ImageExportFormat {
    pub const ALL: [Self; 3] = [Self::Png, Self::Jpeg, Self::WebP];
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::WebP => "webp",
        }
    }
    /// Shared geometric admission for the selected output encoder, without pixels or I/O.
    pub(crate) fn validate_output_size(self, width: u32, height: u32) -> Result<(), CaptureError> {
        validate_image_size(width, height)?;
        // image0.25 JPEG stores u16 axes; locked image-webp0.2 writes 14-bit
        // lossless dimensions minus one. Native encoders retain their checks.
        let (maximum, message) = match self {
            Self::Png => return Ok(()),
            Self::Jpeg => (u32::from(u16::MAX), "JPEG needs dimensions 1–65535"),
            Self::WebP => (1 << 14, "Lossless WebP needs dimensions 1–16384"),
        };
        if width > maximum || height > maximum {
            return Err(CaptureError::Backend(message.into()));
        }
        Ok(())
    }
    fn validate_quality(self, quality: u8) -> Result<(), CaptureError> {
        if self == Self::Jpeg && !(1..=100).contains(&quality) {
            return Err(CaptureError::Backend(
                "JPEG quality must be between 1 and 100".into(),
            ));
        }
        Ok(())
    }
    pub fn from_path(path: &Path) -> Result<Self, CaptureError> {
        match path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("png") => Ok(Self::Png),
            Some("jpg" | "jpeg") => Ok(Self::Jpeg),
            Some("webp") => Ok(Self::WebP),
            _ => Err(CaptureError::Backend(
                "Use a .png, .jpg, .jpeg or .webp file name".into(),
            )),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::WebP => "WebP",
        }
    }
}

pub(crate) fn encode_png_bytes(pixels: &RgbaImage) -> Result<Vec<u8>, CaptureError> {
    encode_png_bytes_with_cancel(pixels, &|| false)
}

fn encode_png_bytes_with_cancel(
    pixels: &RgbaImage,
    cancelled: &impl Fn() -> bool,
) -> Result<Vec<u8>, CaptureError> {
    let cancelled = crate::image_cancel::latch(cancelled);
    use image::ImageEncoder as _;
    let work = crate::image_cancel::ImageWork::new(&cancelled)?;
    validate_image_size(pixels.width(), pixels.height())?;
    let mut bytes = Vec::new();
    {
        use image::codecs::png::{CompressionType, FilterType, PngEncoder};
        let mut writer = crate::image_cancel::CheckedImageWriter::new(&mut bytes, &cancelled)?;
        let result =
            PngEncoder::new_with_quality(&mut writer, CompressionType::Fast, FilterType::Adaptive)
                .write_image(
                    pixels.as_raw(),
                    pixels.width(),
                    pixels.height(),
                    image::ExtendedColorType::Rgba8,
                )
                .map_err(|error| CaptureError::Backend(error.to_string()));
        work.check()?;
        result?;
        let flushed = writer.flush();
        work.check()?;
        flushed?;
    }
    work.check()?;
    if bytes.len() < 8
        || bytes.len() as u64 > MAX_CAPTURE_BYTES + 16 * 1024 * 1024
        || bytes[..8] != [137, 80, 78, 71, 13, 10, 26, 10]
    {
        return Err(CaptureError::Backend(
            "PNG encoding produced an invalid buffer".into(),
        ));
    }
    Ok(bytes)
}

/// Publish an already encoded PNG without replacing files or encoding a second time.
/// The caller supplies a trusted encoder result; this checks byte admission, not PNG validity.
pub fn save_png_bytes(destination: &Path, png: &[u8]) -> Result<(), CaptureError> {
    if png.len() < 8
        || png.len() as u64 > MAX_CAPTURE_BYTES + 16 * 1024 * 1024
        || png[..8] != [137, 80, 78, 71, 13, 10, 26, 10]
    {
        return Err(CaptureError::Backend(
            "Invalid or oversized encoded PNG".into(),
        ));
    }
    write_output(destination, |file| {
        file.write_all(png)?;
        Ok(())
    })
}

fn write_image_export(
    pixels: &RgbaImage,
    destination: &Path,
    format: ImageExportFormat,
    jpeg_quality: u8,
) -> Result<(), CaptureError> {
    write_image_export_with_cancel(pixels, destination, format, jpeg_quality, &|| false)
}

fn write_image_export_with_cancel(
    pixels: &RgbaImage,
    destination: &Path,
    format: ImageExportFormat,
    jpeg_quality: u8,
    cancelled: &impl Fn() -> bool,
) -> Result<(), CaptureError> {
    let mut work = crate::image_cancel::ImageWork::new(cancelled)?;
    use image::ImageEncoder as _;
    format.validate_output_size(pixels.width(), pixels.height())?;
    format.validate_quality(jpeg_quality)?;
    if format == ImageExportFormat::Jpeg {
        for pixel in pixels.pixels() {
            work.pixel()?;
            if pixel[3] != 255 {
                return Err(CaptureError::Backend(
                    "JPEG cannot preserve transparency. Choose PNG or WebP.".into(),
                ));
            }
        }
    }
    let result = write_output_checked(
        destination,
        |file| {
            let mut writer = crate::image_cancel::CheckedImageWriter::new(file, cancelled)?;
            match format {
                ImageExportFormat::Jpeg => {
                    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut writer, jpeg_quality)
                        .encode_image(pixels)?
                }
                ImageExportFormat::WebP => {
                    image::codecs::webp::WebPEncoder::new_lossless(&mut writer).write_image(
                        pixels.as_raw(),
                        pixels.width(),
                        pixels.height(),
                        image::ExtendedColorType::Rgba8,
                    )?
                }
                ImageExportFormat::Png => {
                    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
                    PngEncoder::new_with_quality(
                        &mut writer,
                        CompressionType::Fast,
                        FilterType::Adaptive,
                    )
                    .write_image(
                        pixels.as_raw(),
                        pixels.width(),
                        pixels.height(),
                        image::ExtendedColorType::Rgba8,
                    )?;
                }
            }
            writer.flush()?;
            Ok(())
        },
        || work.check(),
    );
    result.map_err(|error| {
        if cancelled() {
            CaptureError::Canceled
        } else {
            error
        }
    })
}

fn write_scaled_export(
    pixels: &RgbaImage,
    destination: &Path,
    format: ImageExportFormat,
    jpeg_quality: u8,
    size: ExportSize,
) -> Result<(), CaptureError> {
    format.validate_quality(jpeg_quality)?;
    let output_size = size.dimensions(pixels.width(), pixels.height())?;
    format.validate_output_size(output_size.0, output_size.1)?;
    if format == ImageExportFormat::Jpeg && pixels.pixels().any(|pixel| pixel.0[3] != 255) {
        return Err(CaptureError::Backend(
            "JPEG cannot preserve transparency. Choose PNG or WebP.".into(),
        ));
    }
    if output_size == pixels.dimensions() {
        return write_image_export(pixels, destination, format, jpeg_quality);
    }
    let scaled = crate::resample::downscale_export(pixels, size)?;
    write_image_export(&scaled, destination, format, jpeg_quality)
}

/// Explicit JPEG/WebP export of an existing PNG; PNG keeps the streaming copy path.
pub fn export_saved_png_as(
    source: &Path,
    destination: &Path,
    jpeg_quality: u8,
    size: ExportSize,
) -> Result<ImageExportFormat, CaptureError> {
    let format = ImageExportFormat::from_path(destination)?;
    format.validate_quality(jpeg_quality)?;
    size.validate()?;
    if format == ImageExportFormat::Png && size == ExportSize::Original {
        export_saved_png(source, destination)?;
    } else {
        let pixels = crate::load_saved_png(source).map_err(|error| {
            CaptureError::Backend(format!("Could not load source PNG: {error:#}"))
        })?;
        if size != ExportSize::Original {
            write_scaled_export(&pixels, destination, format, jpeg_quality, size)?;
        } else {
            write_image_export(&pixels, destination, format, jpeg_quality)?;
        }
    }
    Ok(format)
}

/// Export an already saved capture without decoding, allocating another frame or encoding again.
pub fn export_saved_png(source: &Path, destination: &Path) -> Result<(), CaptureError> {
    export_saved_png_cancellable(source, destination, || false)
}

pub(crate) fn export_saved_png_cancellable(
    source: &Path,
    destination: &Path,
    cancelled: impl Fn() -> bool,
) -> Result<(), CaptureError> {
    use std::io::Read;
    let started = Instant::now();
    let check = || {
        if cancelled() {
            return Err(CaptureError::Canceled);
        }
        if started.elapsed() > std::time::Duration::from_secs(60) {
            return Err(CaptureError::Backend(
                "PNG export exceeded its 60 second deadline".into(),
            ));
        }
        Ok(())
    };
    check()?;
    if destination.try_exists()? {
        return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists).into());
    }
    check()?;
    if !source.metadata()?.is_file() {
        return Err(CaptureError::Backend(
            "PNG export source must be a regular file".into(),
        ));
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let mut source = options.open(source)?;
    if !source.metadata()?.is_file() {
        return Err(CaptureError::Backend(
            "PNG export source must be a regular file".into(),
        ));
    }
    let mut signature = [0; 8];
    source.read_exact(&mut signature)?;
    if signature != [137, 80, 78, 71, 13, 10, 26, 10] {
        return Err(CaptureError::Backend(
            "The source file is not a PNG image".into(),
        ));
    }
    write_output_checked(
        destination,
        |file| {
            file.write_all(&signature)?;
            // Bounded streaming also handles a source modified by another process.
            let limit = MAX_CAPTURE_BYTES + 16 * 1024 * 1024;
            let mut source = source.take(limit + 1);
            let mut buffer = [0u8; 64 * 1024];
            let mut copied = 0;
            loop {
                check()?;
                let read = source.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                copied += read as u64;
                if copied > limit {
                    return Err(CaptureError::Backend(
                        "The PNG source file exceeds the export limit".into(),
                    ));
                }
                file.write_all(&buffer[..read])?;
            }
            Ok(())
        },
        check,
    )
}

pub(crate) fn write_output(
    destination: &Path,
    write: impl FnOnce(&mut fs::File) -> Result<(), CaptureError>,
) -> Result<(), CaptureError> {
    write_output_checked(destination, write, || Ok(()))
}

pub(crate) fn write_output_checked(
    destination: &Path,
    write: impl FnOnce(&mut fs::File) -> Result<(), CaptureError>,
    check: impl Fn() -> Result<(), CaptureError>,
) -> Result<(), CaptureError> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".screenshots-{}.part", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        write(&mut file)?;
        file.sync_all()?;
        drop(file);
        check()?;
        publish_output(&temporary, destination)?;
        Ok(())
    })();
    if let Err(error) = fs::remove_file(&temporary) {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(%error, "temporary capture file cleanup failed");
        }
    }
    result
}
#[cfg(target_os = "windows")]
pub(crate) fn publish_output(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
    let source: Vec<_> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<_> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // No REPLACE_EXISTING: an existing output is an error, including races.
    if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn publish_output(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    // No partial-file copy fallback on filesystems that cannot publish without replacement.
    fs::hard_link(temporary, destination)
}
