//! Phase 0 capture types and an XCap still-image backend.
//!
//! Capture stays synchronous and `Send`. Call it from a GPUI background
//! worker, not from the foreground executor.

use std::{
    fs,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use image::RgbaImage;
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
            .and_then(|bytes| self.0.checked_add(bytes))
            .filter(|bytes| *bytes <= MAX_CAPTURE_BYTES)
            .ok_or(CaptureError::MemoryLimit)?;
        self.0 = total;
        Ok(())
    }
}

pub enum CaptureRequest {
    Display(String),
    Window(String),
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
    Window(Window, String, String),
    Region(Monitor, String, u32, u32, u32, u32),
}

impl PreparedCapture {
    fn dimensions(&self) -> Result<(u32, u32), CaptureError> {
        match self {
            Self::Display(monitor, _) => Ok((
                monitor.width().map_err(backend_error)?,
                monitor.height().map_err(backend_error)?,
            )),
            Self::Window(window, _, _) => Ok((
                window.width().map_err(backend_error)?,
                window.height().map_err(backend_error)?,
            )),
            Self::Region(_, _, _, _, width, height) => Ok((*width, *height)),
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

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

#[derive(Debug, Error)]
pub enum CaptureError {
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
}

impl XcapBackend {
    pub fn new(output_dir: impl Into<PathBuf>) -> Self {
        Self {
            output_dir: output_dir.into(),
        }
    }

    /// One admission group, with no partial outputs if any request fails.
    pub fn capture_batch(
        &self,
        requests: Vec<CaptureRequest>,
    ) -> Result<Vec<CapturedImage>, CaptureError> {
        let mut budget = CaptureBudget::default();
        let prepared = requests
            .into_iter()
            .map(|request| {
                let prepared = self.prepare(request)?;
                let (width, height) = prepared.dimensions()?;
                budget.admit(width, height)?;
                Ok(prepared)
            })
            .collect::<Result<Vec<_>, CaptureError>>()?;
        let mut actual = CaptureBudget::default();
        prepared
            .into_iter()
            .map(|prepared| {
                let (width, height) = prepared.dimensions()?;
                CaptureBudget(actual.0).admit(width, height)?;
                let started = Instant::now();
                let (display_id, window_id, image, label) = match prepared {
                    PreparedCapture::Display(monitor, id) => (
                        id,
                        None,
                        monitor.capture_image().map_err(backend_error)?,
                        "display".to_owned(),
                    ),
                    PreparedCapture::Window(window, display, id) => {
                        let image = window.capture_image().map_err(backend_error)?;
                        (display, Some(id.clone()), image, format!("window-{id}"))
                    }
                    PreparedCapture::Region(monitor, id, x, y, width, height) => (
                        id,
                        None,
                        monitor
                            .capture_region(x, y, width, height)
                            .map_err(backend_error)?,
                        "region".to_owned(),
                    ),
                };
                // shortcut: XCap allocates before returning; isolate/cap the backend for a hard native allocation bound.
                actual.admit(image.width(), image.height())?;
                let mut shot =
                    self.frame(&display_id, image, started.elapsed().as_millis(), &label)?;
                shot.window_id = window_id;
                Ok(shot)
            })
            .collect()
    }

    fn prepare(&self, request: CaptureRequest) -> Result<PreparedCapture, CaptureError> {
        match request {
            CaptureRequest::Display(id) => {
                Ok(PreparedCapture::Display(self.find_monitor(&id)?, id))
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
                let display_width = monitor.width().map_err(backend_error)?;
                let display_height = monitor.height().map_err(backend_error)?;
                if width == 0
                    || height == 0
                    || x.checked_add(width).is_none_or(|end| end > display_width)
                    || y.checked_add(height).is_none_or(|end| end > display_height)
                {
                    return Err(CaptureError::InvalidRegion);
                }
                Ok(PreparedCapture::Region(
                    monitor, display_id, x, y, width, height,
                ))
            }
            CaptureRequest::Window(id) => {
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
                if window.pid().map_err(backend_error)? == std::process::id() {
                    return Err(CaptureError::Unsupported(
                        "capturing this application's own window",
                    ));
                }
                if window.is_minimized().map_err(backend_error)? {
                    return Err(CaptureError::Unsupported("capturing a minimized window"));
                }
                let display = monitor_id(&window.current_monitor().map_err(backend_error)?)?;
                Ok(PreparedCapture::Window(window, display, id))
            }
        }
    }

    fn find_monitor(&self, display_id: &str) -> Result<Monitor, CaptureError> {
        Monitor::all()
            .map_err(backend_error)?
            .into_iter()
            .find(|monitor| monitor_id(monitor).is_ok_and(|id| id == display_id))
            .ok_or_else(|| CaptureError::DisplayNotFound(display_id.to_string()))
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
        })
    }
}

impl CaptureBackend for XcapBackend {
    fn capabilities(&self) -> CaptureCapabilities {
        CaptureCapabilities {
            list_displays: true,
            list_windows: cfg!(not(target_os = "linux")),
            capture_display: true,
            capture_window: cfg!(not(target_os = "linux")),
            capture_region: true,
        }
    }

    fn list_displays(&self) -> Result<Vec<DisplayInfo>, CaptureError> {
        Monitor::all()
            .map_err(backend_error)?
            .iter()
            .map(display_info)
            .collect()
    }

    fn list_windows(&self) -> Result<Vec<WindowInfo>, CaptureError> {
        if !self.capabilities().list_windows {
            return Err(CaptureError::Unsupported("window listing"));
        }

        Window::all()
            .map_err(backend_error)?
            .iter()
            .map(window_info)
            .collect()
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

fn backend_error(error: impl std::fmt::Display) -> CaptureError {
    CaptureError::Backend(error.to_string())
}

fn monitor_id(monitor: &Monitor) -> Result<String, CaptureError> {
    Ok(monitor.id().map_err(backend_error)?.to_string())
}

fn display_info(monitor: &Monitor) -> Result<DisplayInfo, CaptureError> {
    Ok(DisplayInfo {
        id: monitor_id(monitor)?,
        name: monitor
            .friendly_name()
            .or_else(|_| monitor.name())
            .map_err(backend_error)?,
        x: monitor.x().map_err(backend_error)?,
        y: monitor.y().map_err(backend_error)?,
        width: monitor.width().map_err(backend_error)?,
        height: monitor.height().map_err(backend_error)?,
        scale_factor: monitor.scale_factor().map_err(backend_error)?,
        rotation: monitor.rotation().map_err(backend_error)?,
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
    use image::Rgba;

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
    pub fn save_png(&self) -> Result<(), CaptureError> {
        use image::{
            ImageEncoder,
            codecs::png::{CompressionType, FilterType, PngEncoder},
        };
        let parent = self
            .png_path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let temporary = parent.join(format!(".screenshots-{}.part", uuid::Uuid::new_v4()));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        let result = (|| {
            {
                let mut writer = BufWriter::with_capacity(64 * 1024, &mut file);
                PngEncoder::new_with_quality(
                    &mut writer,
                    CompressionType::Fast,
                    FilterType::Adaptive,
                )
                .write_image(
                    self.pixels.as_raw(),
                    self.pixels.width(),
                    self.pixels.height(),
                    image::ExtendedColorType::Rgba8,
                )?;
                writer.flush()?;
            }
            file.sync_all()?;
            drop(file);
            publish_png(&temporary, &self.png_path)?;
            Ok::<(), CaptureError>(())
        })();
        if let Err(error) = fs::remove_file(&temporary) {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(%error, "temporary capture file cleanup failed");
            }
        }
        result
    }
}

#[cfg(target_os = "windows")]
fn publish_png(temporary: &Path, destination: &Path) -> std::io::Result<()> {
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
fn publish_png(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    // No partial-file copy fallback on filesystems that cannot publish without replacement.
    fs::hard_link(temporary, destination)
}
