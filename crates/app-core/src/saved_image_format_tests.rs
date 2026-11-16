use super::*;
use crate::CaptureImageQuality;
use image::{ImageEncoder, Rgba, RgbaImage};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "screenshots-saved-formats-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn directory(&self, name: &str) -> PathBuf {
        let path = self.path(name);
        std::fs::create_dir(&path).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn encode(policy: CaptureFormatPolicy, pixels: &RgbaImage) -> Vec<u8> {
    let mut bytes = Vec::new();
    policy.encode_to(pixels, &mut bytes, &|| false).unwrap();
    bytes
}

fn policies() -> [CaptureFormatPolicy; 4] {
    [
        CaptureFormatPolicy::Png,
        CaptureFormatPolicy::Jpeg {
            quality: CaptureImageQuality::new(90).unwrap(),
        },
        CaptureFormatPolicy::WebpLossless,
        CaptureFormatPolicy::WebpLossy {
            quality: CaptureImageQuality::new(60).unwrap(),
        },
    ]
}

#[test]
fn png_animation_keeps_default_still_pixels_and_byte_preserving_copy() -> Result<()> {
    fn chunk(bytes: &mut Vec<u8>, kind: &[u8; 4], payload: &[u8]) {
        bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        bytes.extend_from_slice(kind);
        bytes.extend_from_slice(payload);
        let mut hash = crc32fast::Hasher::new();
        hash.update(kind);
        hash.update(payload);
        bytes.extend_from_slice(&hash.finalize().to_be_bytes());
    }
    let f = Fixture::new();
    let red = encode(
        CaptureFormatPolicy::Png,
        &RgbaImage::from_pixel(1, 1, Rgba([255, 0, 0, 255])),
    );
    let mut bytes = red[..33].to_vec();
    chunk(&mut bytes, b"acTL", &[0, 0, 0, 1, 0, 0, 0, 0]);
    bytes.extend_from_slice(&red[33..red.len() - 12]);
    let mut frame = Vec::new();
    for number in [0u32, 1, 1, 0, 0] {
        frame.extend_from_slice(&number.to_be_bytes());
    }
    frame.extend_from_slice(&[0, 1, 0, 10, 0, 0]);
    chunk(&mut bytes, b"fcTL", &frame);
    let mut compression =
        flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    compression.write_all(&[0, 0, 0, 255, 255])?;
    let mut data = 1u32.to_be_bytes().to_vec();
    data.extend_from_slice(&compression.finish()?);
    chunk(&mut bytes, b"fdAT", &data);
    chunk(&mut bytes, b"IEND", &[]);
    let path = f.path("default.apng");
    std::fs::write(&path, &bytes)?;
    assert_eq!(
        load_saved_image(&path, || false)?.as_raw(),
        &[255, 0, 0, 255]
    );
    let directory = f.directory("copy");
    let report = export_saved_images(
        &[path],
        &directory,
        LibraryExportPolicy::Preserve,
        false,
        || false,
    )?;
    assert_eq!(report.outputs.len(), 1);
    assert_eq!(std::fs::read(directory.join("default.png"))?, bytes);
    Ok(())
}

#[test]
fn bounded_magic_inspection_decode_and_thumbnails_ignore_misleading_extensions() -> Result<()> {
    let f = Fixture::new();
    let opaque = RgbaImage::from_fn(31, 13, |x, y| {
        Rgba([(x * 7) as u8, (y * 17) as u8, 71, 255])
    });
    for (index, policy) in policies().into_iter().enumerate() {
        let path = f.path(&format!("image-{index}.wrong"));
        let bytes = encode(policy, &opaque);
        std::fs::write(&path, &bytes)?;
        let info = inspect_saved_image(&path)?;
        assert_eq!(
            (info.width, info.height, info.encoded_bytes, info.format),
            (31, 13, bytes.len() as u64, policy.format())
        );
        assert_eq!(info.mime_type(), policy.mime_type());
        let pixels = load_saved_image(&path, || false)?;
        assert_eq!(pixels.dimensions(), opaque.dimensions());
        if !policy.is_lossy() {
            assert_eq!(pixels, opaque);
        }
        let thumbnail = load_image_thumbnail(&path, || false)?;
        assert_eq!(thumbnail, pixels);
        assert_eq!(std::fs::read(path)?, bytes);
    }
    Ok(())
}

#[test]
fn preserved_copies_keep_bytes_and_conversion_reports_real_kind_without_overwrite() -> Result<()> {
    let f = Fixture::new();
    let directory = f.directory("preserved");
    let opaque = RgbaImage::from_pixel(11, 7, Rgba([31, 91, 171, 255]));
    let mut sources = Vec::new();
    for (index, policy) in policies().into_iter().enumerate() {
        let path = f.path(&format!("image-{index}.{}", policy.extension()));
        std::fs::write(&path, encode(policy, &opaque))?;
        sources.push(path);
    }
    let report = export_saved_images(
        &sources,
        &directory,
        LibraryExportPolicy::Preserve,
        false,
        || false,
    )?;
    assert_eq!(
        (
            report.outputs.len(),
            report.failures.len(),
            report.not_exported
        ),
        (4, 0, 0)
    );
    for output in &report.outputs {
        assert!(!output.reencoded && !output.lossy);
        assert_eq!(std::fs::read(&output.source)?, std::fs::read(&output.path)?);
    }
    let conflicts = export_saved_images(
        &sources,
        &directory,
        LibraryExportPolicy::Preserve,
        false,
        || false,
    )?;
    assert_eq!((conflicts.outputs.len(), conflicts.failures.len()), (0, 4));
    let convert = f.directory("converted");
    let report = export_saved_images(
        &sources,
        &convert,
        LibraryExportPolicy::Convert {
            encoding: CaptureFormatPolicy::WebpLossless,
        },
        false,
        || false,
    )?;
    assert_eq!(report.outputs.len(), 4);
    for output in report.outputs {
        assert_eq!(output.format, ImageExportFormat::WebP);
        assert_eq!(output.path.extension().unwrap(), "webp");
        assert!(output.reencoded && !output.lossy);
        assert_eq!(
            image::load_from_memory(&std::fs::read(&output.path)?)?.into_rgba8(),
            load_saved_image(&output.source, || false)?
        );
    }
    assert_eq!(std::fs::read_dir(&convert)?.count(), 4);
    Ok(())
}

#[test]
fn sanitizing_preserve_creates_lossless_oriented_png_and_keeps_original_payloads() -> Result<()> {
    let f = Fixture::new();
    let clean = f.directory("clean");
    let source_pixels =
        RgbaImage::from_fn(3, 2, |x, y| Rgba([(x * 80) as u8, (y * 90) as u8, 50, 255]));
    let exif = b"II\x2a\x00\x08\x00\x00\x00\x01\x00\x12\x01\x03\x00\x01\x00\x00\x00\x06\x00\x00\x00\x00\x00\x00\x00";
    let mut sources = Vec::new();
    for format in ImageExportFormat::ALL {
        let path = f.path(&format!(
            "with-exif-{}.{}",
            format.extension(),
            format.extension()
        ));
        let mut bytes = Vec::new();
        match format {
            ImageExportFormat::Png => {
                let mut encoder = image::codecs::png::PngEncoder::new(&mut bytes);
                encoder.set_exif_metadata(exif.to_vec())?;
                encoder.write_image(
                    source_pixels.as_raw(),
                    3,
                    2,
                    image::ExtendedColorType::Rgba8,
                )?;
            }
            ImageExportFormat::Jpeg => {
                let mut encoder =
                    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 95);
                encoder.set_exif_metadata(exif.to_vec())?;
                encoder.encode_image(&source_pixels)?;
            }
            ImageExportFormat::WebP => {
                let mut encoder = image::codecs::webp::WebPEncoder::new_lossless(&mut bytes);
                encoder.set_exif_metadata(exif.to_vec())?;
                encoder.write_image(
                    source_pixels.as_raw(),
                    3,
                    2,
                    image::ExtendedColorType::Rgba8,
                )?;
            }
        }
        std::fs::write(&path, &bytes)?;
        let decoded = load_saved_image(&path, || false)?;
        assert_eq!(
            decoded.dimensions(),
            if format == ImageExportFormat::Png {
                (3, 2)
            } else {
                (2, 3)
            }
        );
        let info = inspect_saved_image(&path)?;
        assert_eq!((info.width, info.height), decoded.dimensions());
        sources.push((path, bytes));
    }
    let paths = sources.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>();
    let report = export_saved_images(&paths, &clean, LibraryExportPolicy::Preserve, true, || {
        false
    })?;
    assert_eq!(
        (report.outputs.len(), report.failures.len()),
        (3, 0),
        "{:?}",
        report.failures
    );
    for output in report.outputs {
        assert_eq!(output.format, ImageExportFormat::Png);
        assert!(output.reencoded && !output.lossy);
        assert_eq!(output.path.extension().unwrap(), "png");
        let bytes = std::fs::read(&output.path)?;
        assert_eq!(image::guess_format(&bytes)?, image::ImageFormat::Png);
        assert!(
            !bytes
                .windows(4)
                .any(|b| b == b"eXIf" || b == b"iCCP" || b == b"tEXt")
        );
        let expected = if output.source.extension().unwrap() == "png" {
            image::imageops::rotate90(&source_pixels)
        } else {
            load_saved_image(&output.source, || false)?
        };
        assert_eq!(image::load_from_memory(&bytes)?.into_rgba8(), expected);
    }
    for (source, bytes) in sources {
        assert_eq!(std::fs::read(source)?, bytes);
    }
    Ok(())
}

#[test]
fn invalid_inputs_byte_limits_changed_sources_and_cancel_keep_output_unpublished() -> Result<()> {
    let f = Fixture::new();
    let output = f.directory("output");
    let path = f.path("source.jpg");
    let pixels = RgbaImage::from_pixel(3, 2, Rgba([71, 83, 97, 255]));
    std::fs::write(&path, encode(policies()[1], &pixels))?;
    let (mut file, admitted) = open(&path)?;
    let info = inspect_saved_image_file(&mut file, &|| false)?;
    let mut bytes = Vec::new();
    assert!(copy_saved_image_to(&mut file, &admitted, info, &mut bytes, &|| true).is_err());
    assert!(bytes.is_empty());
    let mut changed = std::fs::OpenOptions::new().write(true).open(&path)?;
    changed.set_len(info.encoded_bytes + 1)?;
    assert!(copy_saved_image_to(&mut file, &admitted, info, &mut bytes, &|| false).is_err());
    assert!(bytes.is_empty());
    changed.set_len(crate::capture_format::MAX_SAVED_JPEG_WEBP_BYTES + 1)?;
    assert!(inspect_saved_image(&path).is_err());
    drop(changed);
    std::fs::write(&path, b"not an image")?;
    assert!(inspect_saved_image(&path).is_err());
    assert!(load_saved_image(&path, || false).is_err());
    let png = f.path("valid.png");
    std::fs::write(&png, encode(CaptureFormatPolicy::Png, &pixels))?;
    let report = export_saved_images(
        &[png.clone(), path.clone()],
        &output,
        LibraryExportPolicy::Preserve,
        false,
        || false,
    )?;
    assert_eq!((report.outputs.len(), report.failures.len()), (1, 1));
    let cancel = f.directory("cancel");
    let report = export_saved_images(
        &[png.clone(), path],
        &cancel,
        LibraryExportPolicy::Preserve,
        false,
        || true,
    )?;
    assert_eq!((report.outputs.len(), report.not_exported), (0, 2));
    assert_eq!(std::fs::read_dir(&cancel)?.count(), 0);
    let mut alpha = pixels.clone();
    alpha.put_pixel(0, 0, Rgba([71, 83, 97, 0]));
    std::fs::write(&png, encode(CaptureFormatPolicy::Png, &alpha))?;
    let report = export_saved_images(
        &[png],
        &cancel,
        LibraryExportPolicy::Convert {
            encoding: policies()[1],
        },
        false,
        || false,
    )?;
    assert_eq!((report.outputs.len(), report.failures.len()), (0, 1));
    assert_eq!(std::fs::read_dir(&cancel)?.count(), 0);
    assert_eq!(std::fs::read_dir(&output)?.count(), 1);
    Ok(())
}

#[cfg(unix)]
#[test]
fn generic_library_input_refuses_fifo_without_opening_clipboard_or_capture() -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let f = Fixture::new();
    let fifo = f.path("pipe.png");
    let name = std::ffi::CString::new(fifo.as_os_str().as_bytes())?;
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(inspect_saved_image(&fifo).is_err());
    assert!(load_saved_image(&fifo, || false).is_err());
    Ok(())
}
