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
    Window(Window, String, String, Option<u32>),
    Region(Monitor, String, u32, u32, u32, u32, Option<DisplayGeometry>),
}

impl PreparedCapture {
    fn dimensions(&self) -> Result<(u32, u32), CaptureError> {
        match self {
            Self::Display(monitor, _) => Ok((
                monitor.width().map_err(backend_error)?,
                monitor.height().map_err(backend_error)?,
            )),
            Self::Window(window, _, _, _) => Ok((
                window.width().map_err(backend_error)?,
                window.height().map_err(backend_error)?,
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

#[derive(Debug, Clone, Copy, PartialEq)]
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
            && self.rotation.is_finite()
    }
}

/// Session metadata only: never retains the source frame or a texture.
#[derive(Debug, Clone)]
pub struct RememberedRegion {
    display_id: String,
    geometry: DisplayGeometry,
    area: crate::PixelRegion,
}

impl RememberedRegion {
    pub fn from_display(display: &DisplayInfo, area: crate::PixelRegion) -> Result<Self, CaptureError> {
        if display.id.is_empty() || display.id.len() > 64 { return Err(CaptureError::StaleRegion); }
        validate_image_size(display.width, display.height)?;
        let geometry = DisplayGeometry {
            x: display.x, y: display.y, width: display.width, height: display.height,
            scale_factor: display.scale_factor, rotation: display.rotation,
        };
        let region = Self { display_id: display.id.clone(), geometry, area };
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
    /// Present only for an unmodified full-display snapshot with stable native geometry.
    pub display_geometry: Option<DisplayGeometry>,
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
                let mut display_geometry = None;
                let (display_id, window_id, image, label) = match prepared {
                    PreparedCapture::Display(monitor, id) => {
                        let before = if cfg!(target_os = "windows") {
                            monitor_geometry(&monitor)
                                .ok()
                                .filter(|geometry| geometry.valid())
                        } else {
                            None
                        };
                        let image = monitor.capture_image().map_err(backend_error)?;
                        display_geometry = before.filter(|geometry| {
                            (geometry.width, geometry.height) == image.dimensions()
                                && monitor_geometry(&monitor).ok() == Some(*geometry)
                        });
                        (id, None, image, "display".to_owned())
                    }
                    PreparedCapture::Window(window, display, id, expected_process) => {
                        if expected_process
                            .is_some_and(|expected| window.pid().ok() != Some(expected))
                        {
                            return Err(CaptureError::WindowNotFound(id));
                        }
                        let image = window.capture_image().map_err(backend_error)?;
                        (display, Some(id.clone()), image, format!("window-{id}"))
                    }
                    PreparedCapture::Region(monitor, id, x, y, width, height, expected) => {
                        if let Some(expected) = expected {
                            if monitor_geometry(&monitor)? != expected {
                                return Err(CaptureError::StaleRegion);
                            }
                        }
                        let image = monitor
                            .capture_region(x, y, width, height)
                            .map_err(backend_error)?;
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
                let mut shot =
                    self.frame(&display_id, image, started.elapsed().as_millis(), &label)?;
                shot.window_id = window_id;
                shot.display_geometry = display_geometry;
                Ok(shot)
            })
            .collect()
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
                    monitor, display_id, x, y, width, height, None,
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
        if window.pid().map_err(backend_error)? == std::process::id() {
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
        let display = monitor_id(&window.current_monitor().map_err(backend_error)?)?;
        Ok(PreparedCapture::Window(
            window,
            display,
            id,
            expected_process,
        ))
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
            display_geometry: None,
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

fn monitor_geometry(monitor: &Monitor) -> Result<DisplayGeometry, CaptureError> {
    Ok(DisplayGeometry {
        x: monitor.x().map_err(backend_error)?,
        y: monitor.y().map_err(backend_error)?,
        width: monitor.width().map_err(backend_error)?,
        height: monitor.height().map_err(backend_error)?,
        scale_factor: monitor.scale_factor().map_err(backend_error)?,
        rotation: monitor.rotation().map_err(backend_error)?,
    })
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

    #[test]
    fn scaled_exports_use_final_pixels_preserve_sources_and_refuse_overwrite() -> anyhow::Result<()> {
        let root = std::env::temp_dir().join(format!("screenshots-scaled-export-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root)?;
        let shot = CapturedImage { width: 4, height: 4,
            pixels: Arc::new(RgbaImage::from_pixel(4, 4, image::Rgba([12, 34, 56, 255]))),
            png_path: root.join("source.png"), display_id: "fixture".into(), window_id: None,
            elapsed_ms: 0, source_format: PixelFormat::Rgba8UnormSrgb, display_geometry: None };
        shot.save_png()?;
        let before = fs::read(&shot.png_path)?;
        for extension in ["png", "jpg", "webp"] {
            let target = root.join(format!("half.{extension}"));
            let saved = root.join(format!("quarter.{extension}"));
            shot.save_image_as(&target, 73, 50)?;
            export_saved_png_as(&shot.png_path, &saved, 73, 25)?;
            let bytes = fs::read(&target)?;
            assert_eq!(image::open(&target)?.into_rgba8().dimensions(), (2, 2));
            assert_eq!(image::open(&saved)?.into_rgba8().dimensions(), (1, 1));
            assert!(shot.save_image_as(&target, 73, 50).is_err());
            assert_eq!(fs::read(&target)?, bytes);
            fs::remove_file(target)?; fs::remove_file(saved)?;
        }
        let mut pixels = shot.pixels.as_ref().clone();
        pixels.put_pixel(0, 0, image::Rgba([12, 34, 56, 254]));
        let transparent = CapturedImage { pixels: Arc::new(pixels), ..shot.clone() };
        let rejected = root.join("transparent.jpg");
        assert!(transparent.save_image_as(&rejected, 73, 25).is_err());
        assert!(!rejected.exists());
        assert!(shot.save_image_as(&rejected, 73, 75).is_err());
        assert_eq!(fs::read(&shot.png_path)?, before);
        assert_eq!(fs::read_dir(&root)?.count(), 1);
        fs::remove_file(shot.png_path)?; fs::remove_dir(root)?;
        Ok(())
    }

    #[test]
    fn jpeg_quality_controls_memory_and_saved_png_exports() -> anyhow::Result<()> {
        let root = std::env::temp_dir().join(format!("screenshots-jpeg-quality-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root)?;
        let shot = CapturedImage { width: 64, height: 64,
            pixels: Arc::new(RgbaImage::from_fn(64, 64, |x, y| image::Rgba([
                ((x * 47 + y * 13) % 256) as u8, ((y * 71 + x * 19) % 256) as u8, ((x + y) * 31 % 256) as u8, 255]))),
            png_path: root.join("source.png"), display_id: "fixture".into(), window_id: None,
            elapsed_ms: 0, source_format: PixelFormat::Rgba8UnormSrgb, display_geometry: None };
        shot.save_png()?;
        let mut errors = Vec::new();
        for quality in [10, 100] {
            let memory = root.join(format!("memory-{quality}.jpg"));
            let saved = root.join(format!("saved-{quality}.jpg"));
            shot.save_image_as(&memory, quality, 100)?;
            export_saved_png_as(&shot.png_path, &saved, quality, 100)?;
            assert_eq!(fs::read(&memory)?, fs::read(&saved)?);
            let decoded = image::open(&memory)?.into_rgba8();
            assert_eq!(decoded.dimensions(), (64, 64));
            errors.push(decoded.as_raw().iter().zip(shot.pixels.as_raw()).map(|(a, b)| u64::from(a.abs_diff(*b))).sum::<u64>());
            fs::remove_file(memory)?; fs::remove_file(saved)?;
        }
        assert!(errors[1] < errors[0], "{errors:?}");
        for quality in [0, 101, 255] {
            let destination = root.join("invalid.jpg");
            assert!(shot.save_image_as(&destination, quality, 100).is_err());
            assert!(export_saved_png_as(&shot.png_path, &destination, quality, 100).is_err());
            assert!(!destination.exists());
        }
        assert_eq!(fs::read_dir(&root)?.count(), 1);
        fs::remove_file(shot.png_path)?; fs::remove_dir(root)?;
        Ok(())
    }

    #[test]
    fn image_export_formats_preserve_sources_alpha_and_existing_files() -> anyhow::Result<()> {
        use image::GenericImageView as _;
        let root = std::env::temp_dir().join(format!("screenshots-format-export-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root)?;
        let mut shot = CapturedImage { display_id: "fixture".into(), window_id: None, width: 3, height: 2,
            pixels: Arc::new(RgbaImage::from_pixel(3, 2, image::Rgba([12, 34, 56, 255]))),
            png_path: root.join("source.png"), elapsed_ms: 0, source_format: PixelFormat::Rgba8UnormSrgb, display_geometry: None };
        shot.save_png()?;
        let original = fs::read(&shot.png_path)?;
        for extension in ["png", "JPG", "jpeg", "webp"] {
            let destination = root.join(format!("copy.{extension}"));
            let format = shot.save_image_as(&destination, 90, 100)?;
            let bytes = fs::read(&destination)?;
            assert_eq!(image::guess_format(&bytes)?, match format {
                ImageExportFormat::Png => image::ImageFormat::Png,
                ImageExportFormat::Jpeg => image::ImageFormat::Jpeg,
                ImageExportFormat::WebP => image::ImageFormat::WebP,
            });
            let decoded = image::load_from_memory(&bytes)?.into_rgba8();
            assert_eq!(decoded.dimensions(), (3, 2));
            if format != ImageExportFormat::Jpeg { assert_eq!(decoded, *shot.pixels); }
            assert!(shot.save_image_as(&destination, 90, 100).is_err());
            assert_eq!(fs::read(&destination)?, bytes);
            let from_png = root.join(format!("from-png.{extension}"));
            assert_eq!(export_saved_png_as(&shot.png_path, &from_png, 90, 100)?, format);
            assert_eq!(image::open(&from_png)?.dimensions(), (3, 2));
            fs::remove_file(destination)?;
            fs::remove_file(from_png)?;
        }
        for extension in ["gif", "bmp", ""] {
            let destination = root.join(format!("unsupported.{extension}"));
            assert!(shot.save_image_as(&destination, 90, 100).is_err());
            assert!(!destination.exists());
        }
        shot.pixels = Arc::new(RgbaImage::from_fn(3, 2, |x, _| image::Rgba([12, 34, 56, if x == 0 { 0 } else { 78 }])));
        let transparent = root.join("transparent.webp");
        shot.save_image_as(&transparent, 90, 100)?;
        assert_eq!(image::open(&transparent)?.into_rgba8(), *shot.pixels);
        let rejected = root.join("transparent.jpg");
        assert!(shot.save_image_as(&rejected, 90, 100).unwrap_err().to_string().contains("transparency"));
        assert!(!rejected.exists());
        let wide = CapturedImage { width: 65536, height: 1,
            pixels: Arc::new(RgbaImage::from_pixel(65536, 1, image::Rgba([12, 34, 56, 255]))), ..shot.clone() };
        let failed = root.join("encoder-failure.jpg");
        assert!(wide.save_image_as(&failed, 90, 100).is_err());
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
        let root = std::env::temp_dir().join(format!("screenshots-export-cost-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root)?;
        let shot = CapturedImage { width: 3840, height: 2160,
            pixels: Arc::new(RgbaImage::from_fn(3840, 2160, |x, y| image::Rgba([
                (x % 251) as u8, (y % 251) as u8, ((x + y) % 251) as u8, 255]))),
            png_path: root.join("unused.png"), display_id: "fixture".into(), window_id: None,
            elapsed_ms: 0, source_format: PixelFormat::Rgba8UnormSrgb, display_geometry: None };
        for (extension, quality, scale) in [("png", 90, 100), ("png", 90, 50), ("png", 90, 25),
            ("jpg", 50, 100), ("jpg", 90, 100), ("jpg", 100, 100), ("webp", 90, 100)] {
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
            eprintln!("4K {extension} (JPEG quality {quality}, scale {scale}%) resize/encode + durable publication median={:?}; output={length} bytes; 3 synthetic warm samples; excludes dialog/presentation/process memory", timings[1]);
        }
        fs::remove_dir(root)?;
        Ok(())
    }

    #[test]
    fn exact_region_metadata_checks_display_geometry_and_bounds_without_pixels() {
        let display = DisplayInfo { id: "fixture".into(), name: "Fixture".into(), x: -1920, y: -100,
            width: 1920, height: 1080, scale_factor: 1.5, rotation: 0., is_primary: false };
        let area = crate::PixelRegion { x: 1919, y: 1079, width: 1, height: 1 };
        let region = RememberedRegion::from_display(&display, area).unwrap();
        assert_eq!(region.area(), area);
        assert_eq!(region.geometry.x, -1920);
        region.validate_geometry(region.geometry).unwrap();
        for geometry in [DisplayGeometry { x: 0, ..region.geometry },
            DisplayGeometry { scale_factor: 2., ..region.geometry },
            DisplayGeometry { rotation: 90., ..region.geometry },
            DisplayGeometry { width: 1080, ..region.geometry }] {
            assert!(matches!(region.validate_geometry(geometry), Err(CaptureError::StaleRegion)));
        }
        for area in [crate::PixelRegion { width: 0, ..area }, crate::PixelRegion { height: 0, ..area },
            crate::PixelRegion { width: 2, ..area }, crate::PixelRegion { height: 2, ..area },
            crate::PixelRegion { x: u32::MAX, ..area }] {
            assert!(RememberedRegion::from_display(&display, area).is_err());
        }
        for display in [DisplayInfo { id: String::new(), ..display.clone() },
            DisplayInfo { scale_factor: f32::NAN, ..display.clone() },
            DisplayInfo { width: u32::MAX, ..display }] {
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
        let exact_region = RememberedRegion::from_display(&DisplayInfo {
            id: repeated[0].display_id.clone(), name: "Explicit fixture".into(),
            x: geometry.x, y: geometry.y, width: geometry.width, height: geometry.height,
            scale_factor: geometry.scale_factor, rotation: geometry.rotation, is_primary: false,
        }, area)?;
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
    pub fn save_image_as(&self, destination: &Path, jpeg_quality: u8, scale_percent: u8) -> Result<ImageExportFormat, CaptureError> {
        validate_image_size(self.pixels.width(), self.pixels.height())?;
        let format = ImageExportFormat::from_path(destination)?;
        crate::resample::validate_export_scale(scale_percent)?;
        if scale_percent != 100 { write_scaled_export(&self.pixels, destination, format, jpeg_quality, scale_percent)?; }
        else if format == ImageExportFormat::Png { self.save_png_as(destination)?; }
        else { write_image_export(&self.pixels, destination, format, jpeg_quality)?; }
        Ok(format)
    }

    pub fn save_png(&self) -> Result<(), CaptureError> {
        self.save_png_as(&self.png_path)
    }

    pub fn save_png_as(&self, destination: &Path) -> Result<(), CaptureError> {
        write_image_export(&self.pixels, destination, ImageExportFormat::Png, crate::DEFAULT_JPEG_QUALITY)
    }
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
        match self { Self::Png => "png", Self::Jpeg => "jpg", Self::WebP => "webp" }
    }
    fn validate_quality(self, quality: u8) -> Result<(), CaptureError> {
        if self == Self::Jpeg && !(1..=100).contains(&quality) {
            return Err(CaptureError::Backend("JPEG quality must be between 1 and 100".into()));
        }
        Ok(())
    }
    pub fn from_path(path: &Path) -> Result<Self, CaptureError> {
        match path.extension().and_then(|extension| extension.to_str()).map(str::to_ascii_lowercase).as_deref() {
            Some("png") => Ok(Self::Png),
            Some("jpg" | "jpeg") => Ok(Self::Jpeg),
            Some("webp") => Ok(Self::WebP),
            _ => Err(CaptureError::Backend("Use a .png, .jpg, .jpeg or .webp file name".into())),
        }
    }
    pub fn label(self) -> &'static str {
        match self { Self::Png => "PNG", Self::Jpeg => "JPEG", Self::WebP => "WebP" }
    }
}

fn write_image_export(pixels: &RgbaImage, destination: &Path, format: ImageExportFormat, jpeg_quality: u8) -> Result<(), CaptureError> {
    use image::ImageEncoder as _;
    validate_image_size(pixels.width(), pixels.height())?;
    format.validate_quality(jpeg_quality)?;
    if format == ImageExportFormat::Jpeg && pixels.pixels().any(|pixel| pixel.0[3] != 255) {
        return Err(CaptureError::Backend("JPEG cannot preserve transparency. Choose PNG or WebP.".into()));
    }
    write_output(destination, |file| {
        let mut writer = BufWriter::with_capacity(64 * 1024, file);
        match format {
            ImageExportFormat::Jpeg => image::codecs::jpeg::JpegEncoder::new_with_quality(&mut writer, jpeg_quality).encode_image(pixels)?,
            ImageExportFormat::WebP => image::codecs::webp::WebPEncoder::new_lossless(&mut writer)
                .write_image(pixels.as_raw(), pixels.width(), pixels.height(), image::ExtendedColorType::Rgba8)?,
            ImageExportFormat::Png => {
                use image::codecs::png::{CompressionType, FilterType, PngEncoder};
                PngEncoder::new_with_quality(&mut writer, CompressionType::Fast, FilterType::Adaptive)
                    .write_image(pixels.as_raw(), pixels.width(), pixels.height(), image::ExtendedColorType::Rgba8)?;
            }
        }
        writer.flush()?;
        Ok(())
    })
}

fn write_scaled_export(pixels: &RgbaImage, destination: &Path, format: ImageExportFormat, jpeg_quality: u8, scale_percent: u8) -> Result<(), CaptureError> {
    format.validate_quality(jpeg_quality)?;
    if format == ImageExportFormat::Jpeg && pixels.pixels().any(|pixel| pixel.0[3] != 255) {
        return Err(CaptureError::Backend("JPEG cannot preserve transparency. Choose PNG or WebP.".into()));
    }
    let scaled = crate::resample::downscale_export(pixels, scale_percent)?;
    write_image_export(&scaled, destination, format, jpeg_quality)
}

/// Explicit JPEG/WebP export of an existing PNG; PNG keeps the streaming copy path.
pub fn export_saved_png_as(source: &Path, destination: &Path, jpeg_quality: u8, scale_percent: u8) -> Result<ImageExportFormat, CaptureError> {
    let format = ImageExportFormat::from_path(destination)?;
    format.validate_quality(jpeg_quality)?;
    crate::resample::validate_export_scale(scale_percent)?;
    if format == ImageExportFormat::Png && scale_percent == 100 { export_saved_png(source, destination)?; }
    else {
        let pixels = crate::load_saved_png(source).map_err(|error| CaptureError::Backend(format!("Could not load source PNG: {error:#}")))?;
        if scale_percent != 100 { write_scaled_export(&pixels, destination, format, jpeg_quality, scale_percent)?; }
        else { write_image_export(&pixels, destination, format, jpeg_quality)?; }
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
    let mut source = fs::File::open(source)?;
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

fn write_output(
    destination: &Path,
    write: impl FnOnce(&mut fs::File) -> Result<(), CaptureError>,
) -> Result<(), CaptureError> {
    write_output_checked(destination, write, || Ok(()))
}

fn write_output_checked(
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
fn publish_output(temporary: &Path, destination: &Path) -> std::io::Result<()> {
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
fn publish_output(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    // No partial-file copy fallback on filesystems that cannot publish without replacement.
    fs::hard_link(temporary, destination)
}
