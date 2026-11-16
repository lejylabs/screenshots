//! Typed calls for an explicitly installed desktop process executor.
use crate::{
    CaptureBackend, CaptureError, CaptureMonitorIdentity, CaptureOptions, CaptureRegionTemplate,
    CaptureRequest, CapturedImage, DisplayInfo, PreparedRegionTemplate, WindowInfo, XcapBackend,
};
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
};

pub type NativeCaptureCheck<'a> = dyn Fn() -> Result<(), CaptureError> + 'a;
pub type NativeCaptureExecutor =
    fn(NativeCaptureCall, &NativeCaptureCheck<'_>) -> Result<NativeCaptureReply, CaptureError>;
static EXECUTOR: OnceLock<NativeCaptureExecutor> = OnceLock::new();

/// Stores one function pointer only; it starts no process, runtime or native call.
pub fn install_native_capture_executor(
    executor: NativeCaptureExecutor,
) -> Result<(), CaptureError> {
    EXECUTOR
        .set(executor)
        .map_err(|_| CaptureError::Backend("Native capture executor is already installed".into()))
}
pub(crate) fn executor() -> Option<NativeCaptureExecutor> {
    EXECUTOR.get().copied()
}

#[derive(Clone, Default)]
pub struct NativeCaptureCancellation(Arc<AtomicBool>);
impl NativeCaptureCancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn check(&self) -> Result<(), CaptureError> {
        if self.0.load(Ordering::Acquire) {
            Err(CaptureError::NativeCanceled)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeCaptureCall {
    MacScroll {
        request: crate::MacScrollRequest,
    },
    MacVideo {
        request: crate::MacVideoRequest,
    },
    ClipboardImage {
        backend: crate::NativeClipboardBackend,
    },
    ClipboardInspect {
        kind: crate::ClipboardInspectKind,
        backend: crate::NativeClipboardBackend,
    },
    Inventory,
    Displays,
    Windows,
    Identities {
        displays: Vec<DisplayInfo>,
    },
    Batch {
        requests: Vec<CaptureRequest>,
        options: CaptureOptions,
    },
    Desktop {
        options: CaptureOptions,
    },
    FocusedWindow {
        options: CaptureOptions,
    },
    FocusedDisplay {
        options: CaptureOptions,
    },
    Template {
        template: CaptureRegionTemplate,
        options: CaptureOptions,
    },
    ImageUrl {
        source: crate::ImageUrlSource,
    },
    Portal {
        source: crate::PortalCaptureSource,
        cursor: crate::PortalCaptureCursor,
    },
}
pub enum NativeCaptureReply {
    MacScroll {
        image: CapturedImage,
        summary: crate::MacScrollSummary,
    },
    MacVideo {
        summary: crate::MacVideoSummary,
    },
    Clipboard(crate::ClipboardSnapshot),
    Inventory(NativeSourceInventory),
    Displays(Vec<DisplayInfo>),
    Windows(Vec<WindowInfo>),
    Identities(Vec<CaptureMonitorIdentity>),
    Frames(Vec<CapturedImage>),
}
pub struct NativeSourceInventory {
    pub displays: Result<Vec<DisplayInfo>, CaptureError>,
    pub windows: Result<Vec<WindowInfo>, CaptureError>,
}
fn invalid() -> CaptureError {
    CaptureError::Backend("Invalid native capture protocol data".into())
}
fn id(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
}
impl NativeCaptureCall {
    pub fn frame_count(&self) -> Option<usize> {
        match self {
            Self::Batch { requests, .. } => Some(requests.len()),
            Self::FocusedWindow { .. }
            | Self::Desktop { .. }
            | Self::FocusedDisplay { .. }
            | Self::Template { .. }
            | Self::Portal { .. }
            | Self::ImageUrl { .. }
            | Self::ClipboardImage { .. }
            | Self::MacScroll { .. } => Some(1),
            _ => None,
        }
    }
    pub fn validate_frame_metadata(
        &self,
        index: usize,
        display: &str,
        window: Option<&str>,
        width: u32,
        height: u32,
        geometry: Option<crate::DisplayGeometry>,
    ) -> Result<(), CaptureError> {
        crate::validate_image_size(width, height)?;
        if (!id(display) && !(display.is_empty() && window.is_some()))
            || window.is_some_and(|value| !id(value))
        {
            return Err(invalid());
        }
        if let Some(geometry) = geometry {
            if window.is_some()
                || geometry.width != width
                || geometry.height != height
                || geometry.scale_factor <= 0.0
                || !geometry.scale_factor.is_finite()
                || !geometry.rotation.is_finite()
            {
                return Err(invalid());
            }
        }
        match self {
            Self::Desktop { .. } => {
                if index != 0 || display != "desktop" || window.is_some() || geometry.is_some() {
                    return Err(invalid());
                }
            }
            Self::MacScroll { request } => {
                if index != 0
                    || window != Some(request.window.to_string().as_str())
                    || width != request.region.width
                    || geometry.is_some()
                {
                    return Err(invalid());
                }
            }
            Self::Batch { requests, .. } => match requests.get(index).ok_or_else(invalid)? {
                CaptureRequest::Display(id) => {
                    if display != id || window.is_some() {
                        return Err(invalid());
                    }
                }
                CaptureRequest::Window(id)
                | CaptureRequest::ForegroundWindow { window_id: id, .. } => {
                    if window != Some(id.as_str()) {
                        return Err(invalid());
                    }
                }
                CaptureRequest::Region {
                    display_id,
                    width: w,
                    height: h,
                    ..
                } => {
                    if display != display_id
                        || window.is_some()
                        || width != *w
                        || height != *h
                        || geometry.is_some()
                    {
                        return Err(invalid());
                    }
                }
                CaptureRequest::ExactRegion(region) | CaptureRequest::RepeatRegion(region) => {
                    let (id, _, area) = region.native_parts();
                    if display != id
                        || window.is_some()
                        || (width, height) != (area.width, area.height)
                        || geometry.is_some()
                    {
                        return Err(invalid());
                    }
                }
            },
            Self::FocusedWindow { .. } => {
                if index != 0 || window.is_none() {
                    return Err(invalid());
                }
            }
            Self::FocusedDisplay { .. } => {
                if index != 0 || window.is_some() {
                    return Err(invalid());
                }
            }
            Self::Template { template, .. } => {
                if index != 0
                    || window.is_some()
                    || (width, height) != (template.width, template.height)
                    || geometry.is_some()
                {
                    return Err(invalid());
                }
            }
            Self::ClipboardImage { .. } => {
                if index != 0
                    || display != "clipboard:image"
                    || window.is_some()
                    || geometry.is_some()
                {
                    return Err(invalid());
                }
            }
            Self::ImageUrl { .. } => {
                if index != 0 || display != "image-url" || window.is_some() || geometry.is_some() {
                    return Err(invalid());
                }
            }
            Self::Portal { source, .. } => {
                if index != 0
                    || display != source.identifier()
                    || window.is_some()
                    || geometry.is_some()
                {
                    return Err(invalid());
                }
            }
            _ => return Err(invalid()),
        }
        Ok(())
    }
    pub fn is_enumeration(&self) -> bool {
        matches!(
            self,
            Self::Inventory
                | Self::Displays
                | Self::Windows
                | Self::Identities { .. }
                | Self::ClipboardInspect { .. }
        )
    }
    pub fn validate(&self) -> Result<(), CaptureError> {
        match self {
            Self::MacScroll { request } => request.validate().map_err(|_| invalid())?,
            Self::MacVideo { request } => request.validate().map_err(|_| invalid())?,
            Self::Inventory | Self::Displays | Self::Windows | Self::Portal { .. } => {}
            Self::ImageUrl { source } => source.validate()?,
            Self::ClipboardImage { backend } | Self::ClipboardInspect { backend, .. } => {
                if !cfg!(target_os = "linux")
                    && *backend == crate::NativeClipboardBackend::FocusedX11
                {
                    return Err(invalid());
                }
            }
            Self::Identities { displays } => {
                if displays.is_empty() || displays.len() > 32 {
                    return Err(invalid());
                }
                validate_displays(displays)?;
            }
            Self::Batch { requests, options } => {
                options.validate().map_err(|_| invalid())?;
                if requests.is_empty() || requests.len() > 32 || options.delay_ms != 0 {
                    return Err(invalid());
                }
                for request in requests {
                    let identifier = match request {
                        CaptureRequest::Display(id) | CaptureRequest::Window(id) => id,
                        CaptureRequest::ForegroundWindow {
                            window_id,
                            process_id,
                        } => {
                            if *process_id == 0 {
                                return Err(invalid());
                            }
                            window_id
                        }
                        CaptureRequest::RepeatRegion(region)
                        | CaptureRequest::ExactRegion(region) => {
                            let (id, geometry, area) = region.native_parts();
                            if !geometry.scale_factor.is_finite()
                                || geometry.scale_factor <= 0.0
                                || !geometry.rotation.is_finite()
                                || area
                                    .x
                                    .checked_add(area.width)
                                    .is_none_or(|end| end > geometry.width)
                                || area
                                    .y
                                    .checked_add(area.height)
                                    .is_none_or(|end| end > geometry.height)
                            {
                                return Err(invalid());
                            }
                            crate::validate_image_size(area.width, area.height)?;
                            if !crate::native_capture_job::id(id) {
                                return Err(invalid());
                            }
                            continue;
                        }
                        CaptureRequest::Region {
                            display_id,
                            x,
                            y,
                            width,
                            height,
                        } => {
                            if *x < 0
                                || *y < 0
                                || (*x as u32).checked_add(*width).is_none()
                                || (*y as u32).checked_add(*height).is_none()
                            {
                                return Err(invalid());
                            }
                            crate::validate_image_size(*width, *height)?;
                            display_id
                        }
                    };
                    if !id(identifier) {
                        return Err(invalid());
                    }
                }
            }
            Self::FocusedWindow { options }
            | Self::FocusedDisplay { options }
            | Self::Desktop { options } => {
                options.validate().map_err(|_| invalid())?;
                if options.delay_ms != 0 {
                    return Err(invalid());
                }
            }
            Self::Template { template, options } => {
                let mut template = template.clone();
                template.validate().map_err(|_| invalid())?;
                options.validate().map_err(|_| invalid())?;
                if options.delay_ms != 0 || template.profile.is_some() || template.hotkey.is_some()
                {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }

    /// Private worker entry: the executable must handle this before installing its executor.
    pub fn run_inline(self, originating_pid: u32) -> Result<NativeCaptureReply, CaptureError> {
        if executor().is_some() || originating_pid == 0 {
            return Err(invalid());
        }
        self.validate()?;
        if matches!(self, Self::MacVideo { .. }) {
            return Err(CaptureError::Unsupported(
                "Mac video requires the owned streaming controller and raw sink",
            ));
        }
        if let Self::MacScroll { request } = &self {
            #[cfg(target_os = "macos")]
            {
                let (image, summary) =
                    crate::capture_macos_scroll::run(request.clone(), originating_pid)?;
                let reply = NativeCaptureReply::MacScroll { image, summary };
                self.validate_reply(&reply)?;
                return Ok(reply);
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = request;
                return Err(CaptureError::Unsupported(
                    "Native macOS scrolling requires macOS",
                ));
            }
        }
        if let Self::ClipboardImage { backend } = &self {
            let reply = NativeCaptureReply::Frames(vec![crate::clipboard_input::image_inline(
                *backend,
                &|| Ok(()),
            )?]);
            self.validate_reply(&reply)?;
            return Ok(reply);
        }
        if let Self::ClipboardInspect { kind, backend } = &self {
            let reply = NativeCaptureReply::Clipboard(crate::clipboard_input::inspect_inline(
                *kind,
                *backend,
                &|| Ok(()),
            )?);
            self.validate_reply(&reply)?;
            return Ok(reply);
        }
        if let Self::ImageUrl { source } = &self {
            let frame = source
                .download_in_worker(&|| Ok(()))
                .map_err(|error| match error {
                    CaptureError::NativeCanceled | CaptureError::Canceled => error,
                    _ => CaptureError::ImageUrlFailed,
                })?;
            let reply = NativeCaptureReply::Frames(vec![frame]);
            self.validate_reply(&reply)?;
            return Ok(reply);
        }
        if matches!(self, Self::Portal { .. }) {
            return Err(CaptureError::Unsupported(
                "Portal source requires the desktop native adapter",
            ));
        }
        #[cfg(target_os = "macos")]
        if matches!(
            self,
            Self::Batch { .. }
                | Self::FocusedWindow { .. }
                | Self::FocusedDisplay { .. }
                | Self::Template { .. }
                | Self::Desktop { .. }
        ) {
            crate::capture_macos_ax::screen_permission().map_err(|error| {
                error
                    .downcast::<CaptureError>()
                    .unwrap_or_else(|_| CaptureError::MacScreenPermissionDenied)
            })?;
        }
        #[cfg(target_os = "linux")]
        {
            let display = std::env::var("DISPLAY").map_err(|_| {
                CaptureError::Unsupported(
                    "XCap source metadata requires explicit X11/XWayland DISPLAY",
                )
            })?;
            if display.trim().is_empty()
                || display.len() > 256
                || display.chars().any(char::is_control)
            {
                return Err(CaptureError::Unsupported(
                    "XCap source metadata requires explicit X11/XWayland DISPLAY",
                ));
            }
        }
        #[cfg(target_os = "windows")]
        let topology = crate::capture_native::topology_guard()
            .map_err(|error| CaptureError::Backend(error.to_string()))?;
        let backend =
            XcapBackend::new(std::path::PathBuf::new()).with_originating_process(originating_pid);
        let reply = match &self {
            Self::Inventory => NativeCaptureReply::Inventory(NativeSourceInventory {
                displays: backend.list_displays(),
                windows: backend.list_windows(),
            }),
            Self::Displays => NativeCaptureReply::Displays(backend.list_displays()?),
            Self::Windows => NativeCaptureReply::Windows(backend.list_windows()?),
            Self::Identities { displays } => NativeCaptureReply::Identities(
                crate::capture_monitor_identities(displays).map_err(|_| invalid())?,
            ),
            Self::Batch { requests, options } => NativeCaptureReply::Frames(
                backend
                    .with_options(*options)?
                    .capture_batch(requests.clone())?,
            ),
            Self::Desktop { options } => NativeCaptureReply::Frames(vec![
                crate::desktop_composite::capture_stitched_desktop_in_worker(
                    &backend.with_options(*options)?,
                    &|| Ok(()),
                )?,
            ]),
            Self::FocusedWindow { options } => NativeCaptureReply::Frames(vec![
                backend.with_options(*options)?.capture_focused_window()?,
            ]),
            Self::FocusedDisplay { options } => NativeCaptureReply::Frames(vec![
                backend.with_options(*options)?.capture_focused_display()?,
            ]),
            Self::Template { template, options } => {
                let prepared =
                    PreparedRegionTemplate::from_native_parts(template.clone(), *options);
                NativeCaptureReply::Frames(vec![
                    prepared
                        .capture_inline(std::path::PathBuf::new(), originating_pid, &|| Ok(()))
                        .map_err(|_| invalid())?,
                ])
            }
            Self::ClipboardImage { .. } | Self::ClipboardInspect { .. } => {
                unreachable!("Clipboard was handled before capture metadata")
            }
            Self::ImageUrl { .. } => {
                unreachable!("URL import was handled before native source enumeration")
            }
            Self::Portal { .. } => unreachable!("Portal is handled by the desktop native adapter"),
            Self::MacScroll { .. } => {
                unreachable!("Mac scrolling was handled before native metadata")
            }
            Self::MacVideo { .. } => unreachable!("Mac video requires the streaming controller"),
        };
        #[cfg(target_os = "windows")]
        topology
            .check()
            .map_err(|error| CaptureError::Backend(error.to_string()))?;
        self.validate_reply(&reply)?;
        Ok(reply)
    }

    pub fn validate_reply(&self, reply: &NativeCaptureReply) -> Result<(), CaptureError> {
        match (self, reply) {
            (Self::MacVideo { request }, NativeCaptureReply::MacVideo { summary }) => {
                summary.validate(*request).map_err(|_| invalid())
            }
            (Self::MacScroll { request }, NativeCaptureReply::MacScroll { image, summary }) => {
                summary
                    .validate(request, image.width, image.height)
                    .map_err(|_| invalid())?;
                validate_frames(std::slice::from_ref(image))?;
                self.validate_frame_metadata(
                    0,
                    &image.display_id,
                    image.window_id.as_deref(),
                    image.width,
                    image.height,
                    image.display_geometry,
                )
            }
            (Self::ClipboardInspect { kind, .. }, NativeCaptureReply::Clipboard(value)) => {
                if *kind != value.kind() {
                    return Err(invalid());
                }
                value.validate().map_err(|_| invalid())
            }
            (Self::Inventory, NativeCaptureReply::Inventory(inventory)) => {
                if let Ok(displays) = &inventory.displays {
                    validate_displays(displays)?;
                }
                if let Ok(windows) = &inventory.windows {
                    Self::Windows.validate_reply(&NativeCaptureReply::Windows(windows.clone()))?;
                }
                Ok(())
            }
            (Self::Displays, NativeCaptureReply::Displays(displays)) => validate_displays(displays),
            (Self::Windows, NativeCaptureReply::Windows(windows)) => {
                if windows.len() > 1024 {
                    return Err(invalid());
                }
                let mut ids = std::collections::HashSet::new();
                for window in windows {
                    if !id(&window.id)
                        || !ids.insert(&window.id)
                        || window.pid == 0
                        || window.title.len() > 4096
                        || window.app_name.len() > 512
                    {
                        return Err(invalid());
                    }
                }
                Ok(())
            }
            (Self::Identities { displays }, NativeCaptureReply::Identities(identities)) => {
                if identities.len() > displays.len() * 2 {
                    return Err(invalid());
                }
                let mut seen = std::collections::HashSet::new();
                for identity in identities {
                    identity.binding.validate().map_err(|_| invalid())?;
                    if !displays
                        .iter()
                        .any(|display| display.id == identity.display_id)
                    {
                        return Err(invalid());
                    }
                    let value = serde_json::to_string(&identity.binding).map_err(|_| invalid())?;
                    if value.len() > 2048 || !seen.insert((identity.display_id.clone(), value)) {
                        return Err(invalid());
                    }
                }
                if displays.iter().any(|display| {
                    !identities
                        .iter()
                        .any(|identity| identity.display_id == display.id)
                }) {
                    return Err(invalid());
                }
                Ok(())
            }
            (_, NativeCaptureReply::Frames(frames)) => {
                if matches!(self, Self::MacScroll { .. }) {
                    return Err(invalid());
                }
                validate_frames(frames)?;
                if self.frame_count() != Some(frames.len()) {
                    return Err(invalid());
                }
                for (index, frame) in frames.iter().enumerate() {
                    self.validate_frame_metadata(
                        index,
                        &frame.display_id,
                        frame.window_id.as_deref(),
                        frame.width,
                        frame.height,
                        frame.display_geometry,
                    )?;
                }
                Ok(())
            }
            _ => Err(invalid()),
        }
    }
}
fn validate_displays(displays: &[DisplayInfo]) -> Result<(), CaptureError> {
    if displays.len() > 32 {
        return Err(invalid());
    }
    let mut seen = std::collections::HashSet::new();
    for display in displays {
        if !id(&display.id)
            || !seen.insert(&display.id)
            || display.name.len() > 1024
            || display.width == 0
            || display.height == 0
            || !display.scale_factor.is_finite()
            || display.scale_factor <= 0.0
            || !display.rotation.is_finite()
        {
            return Err(invalid());
        }
    }
    Ok(())
}
pub fn validate_native_frames(frames: &[CapturedImage]) -> Result<(), CaptureError> {
    validate_frames(frames)
}
fn validate_frames(frames: &[CapturedImage]) -> Result<(), CaptureError> {
    if frames.is_empty() || frames.len() > 32 {
        return Err(invalid());
    }
    let mut bytes = 0u64;
    for frame in frames {
        crate::validate_image_size(frame.width, frame.height)?;
        let length = u64::from(frame.width) * u64::from(frame.height) * 4;
        bytes = bytes
            .checked_add(length)
            .filter(|total| *total <= crate::MAX_CAPTURE_BYTES)
            .ok_or(CaptureError::MemoryLimit)?;
        if frame.pixels.dimensions() != (frame.width, frame.height)
            || frame.pixels.as_raw().len() as u64 != length
            || (!id(&frame.display_id)
                && !(frame.display_id.is_empty() && frame.window_id.is_some()))
            || frame.window_id.as_ref().is_some_and(|value| !id(value))
        {
            return Err(invalid());
        }
        if let Some(geometry) = frame.display_geometry {
            if frame.window_id.is_some()
                || geometry.width != frame.width
                || geometry.height != frame.height
                || geometry.scale_factor <= 0.0
                || !geometry.scale_factor.is_finite()
                || !geometry.rotation.is_finite()
            {
                return Err(invalid());
            }
        }
    }
    Ok(())
}
