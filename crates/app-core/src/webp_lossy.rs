//! Explicit lossy WebP export; the existing WebP exporter remains lossless.

use crate::CaptureError;
use image::RgbaImage;
use libwebp_sys as webp;
use std::{
    io::{BufWriter, Write},
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    time::{Duration, Instant},
};

const MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024;
const INPUT_LIMIT_MESSAGE: &str =
    "Lossy WebP needs dimensions 1–16383 and at most 64 MiB of RGBA pixels";

pub(crate) fn validate_output_size(width: u32, height: u32) -> Result<(), CaptureError> {
    // Axis admission precedes the multiplication, so untrusted u32 dimensions
    // cannot overflow the byte calculation. This policy is also used pre-render.
    if !(1..=webp::WEBP_MAX_DIMENSION).contains(&width)
        || !(1..=webp::WEBP_MAX_DIMENSION).contains(&height)
        || u64::from(width) * u64::from(height) * 4 > MAX_INPUT_BYTES
    {
        return Err(failure(INPUT_LIMIT_MESSAGE));
    }
    Ok(())
}
const MAX_OUTPUT_BYTES: u64 = 32 * 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(30);

#[derive(Clone, Copy)]
enum Failure {
    Canceled,
    Deadline,
    OutputLimit,
    Write,
    Callback,
}

impl Failure {
    fn error(self) -> CaptureError {
        match self {
            Self::Canceled => CaptureError::Canceled,
            Self::Deadline => failure("Lossy WebP export exceeded its 30 second deadline"),
            Self::OutputLimit => failure("Lossy WebP output exceeds 32 MiB"),
            Self::Write => failure("Could not write the lossy WebP output"),
            Self::Callback => failure("Lossy WebP cancellation callback failed"),
        }
    }
}

fn failure(message: &str) -> CaptureError {
    CaptureError::Backend(message.into())
}

struct StopCheck<'a, F> {
    cancelled: &'a F,
    deadline: Instant,
}

impl<F: Fn() -> bool> StopCheck<'_, F> {
    fn check(&self) -> Result<(), Failure> {
        match catch_unwind(AssertUnwindSafe(self.cancelled)) {
            Ok(true) => Err(Failure::Canceled),
            Ok(false) if Instant::now() >= self.deadline => Err(Failure::Deadline),
            Ok(false) => Ok(()),
            Err(_) => Err(Failure::Callback),
        }
    }
}

struct Picture(webp::WebPPicture);

impl Drop for Picture {
    fn drop(&mut self) {
        // Only successfully initialized pictures enter this guard.
        unsafe { webp::WebPPictureFree(&mut self.0) };
    }
}

struct Output<'a, F> {
    writer: BufWriter<&'a mut dyn Write>,
    stop: &'a StopCheck<'a, F>,
    bytes: u64,
    limit: u64,
    failure: Option<Failure>,
}

impl<F: Fn() -> bool> Output<'_, F> {
    fn check(&mut self) -> Result<(), Failure> {
        if let Some(failure) = self.failure {
            return Err(failure);
        }
        self.stop.check()
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), Failure> {
        self.check()?;
        let total = self
            .bytes
            .checked_add(bytes.len() as u64)
            .ok_or(Failure::OutputLimit)?;
        if total > self.limit {
            return Err(Failure::OutputLimit);
        }
        self.writer.write_all(bytes).map_err(|_| Failure::Write)?;
        self.bytes = total;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), Failure> {
        self.check()?;
        self.writer.flush().map_err(|_| Failure::Write)
    }
}

unsafe extern "C" fn write<F: Fn() -> bool>(
    data: *const u8,
    length: usize,
    picture: *const webp::WebPPicture,
) -> std::ffi::c_int {
    // WebPEncode is synchronous and thread_level is zero. The context and file
    // remain alive, exclusively accessed by these callbacks until it returns.
    let context = unsafe { (*picture).custom_ptr.cast::<Output<'_, F>>() };
    let result = catch_unwind(AssertUnwindSafe(|| {
        let output = unsafe { &mut *context };
        if length as u64 > output.limit.saturating_sub(output.bytes) {
            return Err(Failure::OutputLimit);
        }
        let bytes = if length == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(data, length) }
        };
        output.write(bytes)
    }));
    match result {
        Ok(Ok(())) => 1,
        Ok(Err(error)) => {
            unsafe { (*context).failure = Some(error) };
            0
        }
        Err(_) => {
            unsafe { (*context).failure = Some(Failure::Callback) };
            0
        }
    }
}

unsafe extern "C" fn progress<F: Fn() -> bool>(
    _: std::ffi::c_int,
    picture: *const webp::WebPPicture,
) -> std::ffi::c_int {
    let context = unsafe { (*picture).user_data.cast::<Output<'_, F>>() };
    let result = catch_unwind(AssertUnwindSafe(|| unsafe { (&mut *context).check() }));
    match result {
        Ok(Ok(())) => 1,
        Ok(Err(error)) => {
            unsafe { (*context).failure = Some(error) };
            0
        }
        Err(_) => {
            unsafe { (*context).failure = Some(Failure::Callback) };
            0
        }
    }
}

/// Save an explicitly requested lossy WebP without replacing an existing file.
/// Call from a worker. Cancellation and the 30-second deadline are cooperative;
/// libwebp allocation/import and filesystem calls cannot be interrupted mid-call.
pub fn save_lossy_webp(
    pixels: &RgbaImage,
    destination: &Path,
    quality: u8,
    cancelled: impl Fn() -> bool,
) -> Result<(), CaptureError> {
    save_with_limit(pixels, destination, quality, cancelled, MAX_OUTPUT_BYTES)
}

fn save_with_limit<F: Fn() -> bool>(
    pixels: &RgbaImage,
    destination: &Path,
    quality: u8,
    cancelled: F,
    limit: u64,
) -> Result<(), CaptureError> {
    validate_output_size(pixels.width(), pixels.height())?;
    if pixels.as_raw().len() as u64 > MAX_INPUT_BYTES {
        return Err(failure(INPUT_LIMIT_MESSAGE));
    }
    if !(1..=100).contains(&quality) {
        return Err(failure("Lossy WebP quality must be between 1 and 100"));
    }
    if destination.try_exists()? {
        return Err(failure("Lossy WebP output already exists"));
    }
    let stop = StopCheck {
        cancelled: &cancelled,
        deadline: Instant::now() + DEADLINE,
    };
    crate::capture::write_output_checked(
        destination,
        |file| encode_with_limit(pixels, file, quality, &cancelled, limit),
        || stop.check().map_err(Failure::error),
    )
}

/// Encode once into the caller's owned journal stage; no independent publication.
pub(crate) fn encode_lossy_webp_to(
    pixels: &RgbaImage,
    writer: &mut dyn Write,
    quality: u8,
    cancelled: &impl Fn() -> bool,
) -> Result<(), CaptureError> {
    encode_with_limit(pixels, writer, quality, cancelled, MAX_OUTPUT_BYTES)
}

fn encode_with_limit<F: Fn() -> bool>(
    pixels: &RgbaImage,
    writer: &mut dyn Write,
    quality: u8,
    cancelled: F,
    limit: u64,
) -> Result<(), CaptureError> {
    let (width, height) = pixels.dimensions();
    validate_output_size(width, height)?;
    if pixels.as_raw().len() as u64 > MAX_INPUT_BYTES {
        return Err(failure(INPUT_LIMIT_MESSAGE));
    }
    if !(1..=100).contains(&quality) {
        return Err(failure("Lossy WebP quality must be between 1 and 100"));
    }
    let stop = StopCheck {
        cancelled: &cancelled,
        deadline: Instant::now() + DEADLINE,
    };
    stop.check().map_err(Failure::error)?;
    let mut config =
        webp::WebPConfig::new().map_err(|_| failure("Could not initialize lossy WebP encoding"))?;
    config.lossless = 0;
    config.quality = f32::from(quality);
    config.method = 4;
    config.low_memory = 1;
    config.thread_level = 0;
    config.alpha_quality = 100;
    config.exact = 1;
    if unsafe { webp::WebPValidateConfig(&config) } == 0 {
        return Err(failure("Invalid lossy WebP encoding configuration"));
    }
    let mut picture = Picture(
        webp::WebPPicture::new().map_err(|_| failure("Could not initialize lossy WebP pixels"))?,
    );
    picture.0.width = width as i32;
    picture.0.height = height as i32;
    picture.0.use_argb = 0;
    if unsafe { webp::WebPPictureImportRGBA(&mut picture.0, pixels.as_ptr(), (width * 4) as i32) }
        == 0
    {
        return Err(failure("Could not prepare lossy WebP pixels"));
    }
    stop.check().map_err(Failure::error)?;
    let mut output = Output {
        writer: BufWriter::with_capacity(64 * 1024, writer),
        stop: &stop,
        bytes: 0,
        limit,
        failure: None,
    };
    output.check().map_err(Failure::error)?;
    let context = (&mut output as *mut Output<'_, F>).cast::<std::ffi::c_void>();
    picture.0.custom_ptr = context;
    picture.0.user_data = context;
    picture.0.writer = Some(write::<F>);
    picture.0.progress_hook = Some(progress::<F>);
    let encoded = unsafe { webp::WebPEncode(&config, &mut picture.0) };
    picture.0.custom_ptr = std::ptr::null_mut();
    picture.0.user_data = std::ptr::null_mut();
    picture.0.writer = None;
    picture.0.progress_hook = None;
    if let Some(failure) = output.failure {
        return Err(failure.error());
    }
    if encoded == 0 || output.bytes == 0 {
        return Err(failure("Lossy WebP encoding failed"));
    }
    output.flush().map_err(Failure::error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, path::PathBuf};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir()
                .join(format!("screenshots-lossy-webp-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn output(&self) -> PathBuf {
            self.0.join("output.webp")
        }
        fn empty(&self) {
            assert_eq!(std::fs::read_dir(&self.0).unwrap().count(), 0);
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn pixels() -> RgbaImage {
        RgbaImage::from_fn(16, 12, |x, y| {
            image::Rgba([
                (x * 17) as u8,
                (y * 23) as u8,
                ((x + y) * 13) as u8,
                [0, 64, 128, 255][(x % 4) as usize],
            ])
        })
    }

    #[test]
    fn lossy_pixels_keep_alpha_and_publish_a_still_vp8_image() {
        let f = Fixture::new();
        let pixels = pixels();
        let original = pixels.clone();
        save_lossy_webp(&pixels, &f.output(), 20, || false).unwrap();
        let bytes = std::fs::read(f.output()).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WEBP");
        let mut offset = 12;
        let mut chunks = Vec::new();
        while offset + 8 <= bytes.len() {
            chunks.push(&bytes[offset..offset + 4]);
            let length =
                u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
            offset += 8 + length + length % 2;
        }
        assert_eq!(offset, bytes.len());
        assert!(chunks.contains(&b"VP8 ".as_slice()));
        assert!(!chunks.contains(&b"VP8L".as_slice()));
        assert!(!chunks.contains(&b"ANIM".as_slice()));
        let decoded = image::load_from_memory(&bytes).unwrap().into_rgba8();
        assert_eq!(decoded.dimensions(), pixels.dimensions());
        assert!(
            decoded
                .pixels()
                .zip(pixels.pixels())
                .all(|(a, b)| a[3] == b[3])
        );
        assert_ne!(decoded, pixels);
        assert_eq!(pixels, original);
        assert_eq!(std::fs::read_dir(&f.0).unwrap().count(), 1);
    }

    #[test]
    fn invalid_input_and_precancellation_do_not_create_output() {
        let f = Fixture::new();
        for pixels in [
            RgbaImage::new(0, 1),
            RgbaImage::new(16384, 1),
            RgbaImage::new(4097, 4096),
        ] {
            assert!(save_lossy_webp(&pixels, &f.output(), 75, || false).is_err());
            f.empty();
        }
        for quality in [0, 101, 255] {
            assert!(save_lossy_webp(&pixels(), &f.output(), quality, || false).is_err());
        }
        assert!(matches!(
            save_lossy_webp(&pixels(), &f.output(), 75, || true),
            Err(CaptureError::Canceled)
        ));
        f.empty();
    }

    #[test]
    fn byte_limit_and_callback_failures_remove_partial_files() {
        let f = Fixture::new();
        let result = save_with_limit(&pixels(), &f.output(), 75, || false, 16);
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("output exceeds 32 MiB")
        );
        f.empty();
        for panic in [false, true] {
            let checks = Cell::new(0);
            let result = save_lossy_webp(&pixels(), &f.output(), 75, || {
                checks.set(checks.get() + 1);
                if checks.get() >= 4 {
                    assert!(!panic, "synthetic callback panic");
                    return true;
                }
                false
            });
            assert!(checks.get() >= 4);
            if panic {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("cancellation callback failed")
                );
            } else {
                assert!(matches!(result, Err(CaptureError::Canceled)));
            }
            f.empty();
        }
    }

    #[test]
    fn an_existing_output_is_never_replaced() {
        let f = Fixture::new();
        std::fs::write(f.output(), b"existing").unwrap();
        assert!(save_lossy_webp(&pixels(), &f.output(), 75, || false).is_err());
        assert_eq!(std::fs::read(f.output()).unwrap(), b"existing");
        assert_eq!(std::fs::read_dir(&f.0).unwrap().count(), 1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn write_and_flush_failures_are_reported_without_unwinding_through_c() {
        let cancelled = || false;
        let stop = StopCheck {
            cancelled: &cancelled,
            deadline: Instant::now() + DEADLINE,
        };
        for capacity in [1, 64 * 1024] {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .open("/dev/full")
                .unwrap();
            let mut output = Output {
                writer: BufWriter::with_capacity(capacity, &mut file),
                stop: &stop,
                bytes: 0,
                limit: MAX_OUTPUT_BYTES,
                failure: None,
            };
            let mut picture = Picture(webp::WebPPicture::new().unwrap());
            picture.0.custom_ptr = (&mut output as *mut Output<'_, _>).cast();
            // With a one-byte buffer the callback writes directly; otherwise
            // the same filesystem failure appears only when flushing.
            unsafe fn invoke<F: Fn() -> bool>(_: &F, picture: &webp::WebPPicture) -> i32 {
                unsafe { write::<F>(b"data".as_ptr(), 4, picture) }
            }
            let result = unsafe { invoke(&cancelled, &picture.0) };
            if capacity == 1 {
                assert_eq!(result, 0);
                assert!(matches!(output.failure, Some(Failure::Write)));
            } else {
                assert_eq!(result, 1);
                assert!(matches!(output.flush(), Err(Failure::Write)));
            }
            picture.0.custom_ptr = std::ptr::null_mut();
        }
    }

    #[test]
    #[ignore = "five-sample optimized 4K PNG/WebP/lossy WebP export observation; synthetic pixels only"]
    fn compare_4k_export_wall_time() {
        let f = Fixture::new();
        let pixels = RgbaImage::from_fn(3840, 2160, |x, y| {
            let noise = x.wrapping_mul(1_664_525) ^ y.wrapping_mul(1_013_904_223);
            image::Rgba([
                ((x / 8 + y / 4) & 255) as u8,
                ((noise >> 20) & 255) as u8,
                ((x / 3 + y / 5) & 255) as u8,
                255,
            ])
        });
        let image = crate::CapturedImage {
            desktop_layout: None,
            display_id: String::new(),
            window_id: None,
            width: pixels.width(),
            height: pixels.height(),
            png_path: Default::default(),
            pixels: std::sync::Arc::new(pixels),
            elapsed_ms: 0,
            source_format: crate::PixelFormat::Rgba8UnormSrgb,
            display_geometry: None,
        };
        for (label, extension) in [
            ("PNG", "png"),
            ("lossless WebP", "webp"),
            ("lossy WebP quality 75", "webp"),
        ] {
            let mut samples = Vec::new();
            let mut output_bytes = 0;
            for sample in 0..6 {
                let path = f.0.join(format!("sample-{sample}.{extension}"));
                let started = Instant::now();
                if label == "lossy WebP quality 75" {
                    save_lossy_webp(&image.pixels, &path, 75, || false).unwrap();
                } else {
                    image
                        .save_image_as(&path, 90, crate::ExportSize::Original)
                        .unwrap();
                }
                let elapsed = started.elapsed();
                if sample != 0 {
                    samples.push(elapsed);
                }
                output_bytes = std::fs::metadata(&path).unwrap().len();
                std::fs::remove_file(path).unwrap();
            }
            samples.sort();
            eprintln!(
                "4K {label}: median {:?}, {output_bytes} encoded bytes; 5 warm samples, {} shared input bytes, lossy writer 65536 bytes; C scratch/RSS unmeasured",
                samples[2],
                image.pixels.as_raw().len()
            );
        }
    }
}
