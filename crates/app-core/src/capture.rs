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
        let monitor = self.find_monitor(display_id)?;
        let started = Instant::now();
        let image = monitor.capture_image().map_err(backend_error)?;
        self.frame(display_id, image, started.elapsed().as_millis(), "display")
    }

    fn capture_region(
        &self,
        display_id: &str,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<CapturedImage, CaptureError> {
        let monitor = self.find_monitor(display_id)?;
        let x = u32::try_from(x).unwrap_or(0);
        let y = u32::try_from(y).unwrap_or(0);
        let started = Instant::now();
        let image = monitor
            .capture_region(x, y, width, height)
            .map_err(backend_error)?;
        self.frame(display_id, image, started.elapsed().as_millis(), "region")
    }

    fn capture_window(&self, window_id: &str) -> Result<CapturedImage, CaptureError> {
        if !self.capabilities().capture_window {
            return Err(CaptureError::Unsupported("window capture"));
        }
        let numeric_id = window_id
            .parse::<u32>()
            .map_err(|_| CaptureError::WindowNotFound(window_id.to_string()))?;
        let window = Window::all()
            .map_err(backend_error)?
            .into_iter()
            .find(|window| window.id().is_ok_and(|id| id == numeric_id))
            .ok_or_else(|| CaptureError::WindowNotFound(window_id.to_string()))?;
        if window.pid().map_err(backend_error)? == std::process::id() {
            return Err(CaptureError::Unsupported(
                "capturing this application's own window",
            ));
        }
        if window.is_minimized().map_err(backend_error)? {
            return Err(CaptureError::Unsupported("capturing a minimized window"));
        }
        let display_id = monitor_id(&window.current_monitor().map_err(backend_error)?)?;
        let started = Instant::now();
        let image = window.capture_image().map_err(backend_error)?;
        let id = window.id().map_err(backend_error)?.to_string();
        let mut shot = self.frame(
            &display_id,
            image,
            started.elapsed().as_millis(),
            &format!("window-{id}"),
        )?;
        shot.window_id = Some(id);
        Ok(shot)
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
