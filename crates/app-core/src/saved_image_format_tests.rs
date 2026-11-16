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
fn saved_png_reader_freezes_post_admission_growth_before_codec_reads_or_seeks() -> Result<()> {
    let f = Fixture::new();
    let path = f.path("growing.png");
    let pixels = RgbaImage::from_pixel(3, 2, Rgba([17, 31, 47, 128]));
    let bytes = encode(CaptureFormatPolicy::Png, &pixels);
    std::fs::write(&path, &bytes)?;
    let (mut file, admitted) = open(&path)?;
    let info = inspect_saved_image_file(&mut file, &|| false)?;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)?
        .write_all(&vec![0x6d; 128 * 1024])?;
    let cancel = || false;
    let mut reader = SavedPngReader::new(&file, info.encoded_bytes, &cancel, Instant::now())?;
    let decoded = crate::clipboard::decode_saved_png(
        BufReader::with_capacity(64 * 1024, &mut reader),
        crate::MAX_CAPTURE_BYTES,
    )?;
    assert_eq!(decoded, pixels);
    // An unrestricted 64 KiB BufReader consumes appended bytes on its first fill.
    assert!(reader.inner.stream_position()? <= info.encoded_bytes);
    assert_eq!(reader.seek(std::io::SeekFrom::End(0))?, info.encoded_bytes);
    assert_eq!(reader.read(&mut [0; 1])?, 0);
    assert!(
        reader
            .seek(std::io::SeekFrom::Start(info.encoded_bytes + 1))
            .is_err()
    );
    assert!(reader.seek(std::io::SeekFrom::End(1)).is_err());
    assert!(
        reader
            .seek(std::io::SeekFrom::End(-(info.encoded_bytes as i64) - 1))
            .is_err()
    );
    reader.seek(std::io::SeekFrom::End(-1))?;
    let mut last = [0];
    reader.read_exact(&mut last)?;
    assert_eq!(last[0], *bytes.last().unwrap());
    reader.seek(std::io::SeekFrom::Start(0))?;
    let mut admitted_bytes = Vec::new();
    reader.read_to_end(&mut admitted_bytes)?;
    assert_eq!(admitted_bytes, bytes);
    assert!(admitted.verify_file(&file).is_err());
    Ok(())
}

#[test]
fn saved_png_reader_checks_cancel_and_deadline_between_bounded_io() -> Result<()> {
    let f = Fixture::new();
    let path = f.path("bounded-reader-input");
    std::fs::write(&path, vec![0x3b; 128 * 1024])?;
    let file = File::open(&path)?;
    let canceled = std::cell::Cell::new(false);
    let stop = || canceled.get();
    let mut reader = SavedPngReader::new(&file, 128 * 1024, &stop, Instant::now())?;
    let mut bytes = vec![0; 128 * 1024];
    assert_eq!(reader.read(&mut bytes)?, 64 * 1024);
    canceled.set(true);
    assert!(reader.read(&mut bytes).is_err());
    assert!(reader.seek(std::io::SeekFrom::Start(0)).is_err());
    assert_eq!(reader.inner.stream_position()?, 64 * 1024);
    drop(reader);
    canceled.set(false);
    assert!(
        SavedPngReader::new(
            &file,
            128 * 1024,
            &stop,
            Instant::now() - Duration::from_secs(61)
        )
        .is_err()
    );
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
fn generic_sanitize_normalizes_genuine_icc_and_orientation_against_littlecms_oracles() -> Result<()>
{
    let f = Fixture::new();
    let directory = f.directory("managed-clean");
    let exif = b"II\x2a\x00\x08\x00\x00\x00\x01\x00\x12\x01\x03\x00\x01\x00\x00\x00\x06\x00\x00\x00\x00\x00\x00\x00";
    let p3 = include_bytes!("../tests/fixtures/color/lcms-display-p3.icc");
    let mut png = include_bytes!("../tests/fixtures/color/lcms-p3-rgba16.png")[..33].to_vec();
    png.extend_from_slice(&(exif.len() as u32).to_be_bytes());
    png.extend_from_slice(b"eXIf");
    png.extend_from_slice(exif);
    let mut crc = crc32fast::Hasher::new();
    crc.update(b"eXIf");
    crc.update(exif);
    png.extend_from_slice(&crc.finalize().to_be_bytes());
    png.extend_from_slice(&include_bytes!("../tests/fixtures/color/lcms-p3-rgba16.png")[33..]);
    let png_path = f.path("genuine-profile-16.png");
    std::fs::write(&png_path, &png)?;
    let png_expected = image::imageops::rotate90(
        &RgbaImage::from_raw(
            3,
            4,
            include_bytes!("../tests/fixtures/color/lcms-p3-rgba16-to-srgb8.rgba").to_vec(),
        )
        .unwrap(),
    );

    fn riff_chunk(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut chunk = kind.to_vec();
        chunk.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        chunk.extend_from_slice(payload);
        if !payload.len().is_multiple_of(2) {
            chunk.push(0);
        }
        chunk
    }
    let rgba = RgbaImage::from_raw(
        4,
        1,
        vec![
            200, 60, 20, 0, 20, 180, 130, 37, 128, 128, 128, 128, 80, 140, 220, 255,
        ],
    )
    .unwrap();
    let encoded = encode(CaptureFormatPolicy::WebpLossless, &rgba);
    let mut body = b"WEBP".to_vec();
    body.extend(riff_chunk(b"VP8X", &[0x38, 0, 0, 0, 3, 0, 0, 0, 0, 0]));
    body.extend(riff_chunk(b"ICCP", p3));
    body.extend_from_slice(&encoded[12..]);
    body.extend(riff_chunk(b"EXIF", exif));
    let mut webp = b"RIFF".to_vec();
    webp.extend_from_slice(&(body.len() as u32).to_le_bytes());
    webp.extend(body);
    let webp_path = f.path("genuine-profile-8.webp");
    std::fs::write(&webp_path, &webp)?;
    let webp_expected = image::imageops::rotate90(
        &RgbaImage::from_raw(
            4,
            1,
            include_bytes!("../tests/fixtures/capture_formats/p3-rgba8-to-srgb8.rgba").to_vec(),
        )
        .unwrap(),
    );
    let report = export_saved_images(
        &[png_path.clone(), webp_path.clone()],
        &directory,
        LibraryExportPolicy::Preserve,
        true,
        || false,
    )?;
    assert_eq!(
        (report.outputs.len(), report.failures.len()),
        (2, 0),
        "{:?}",
        report.failures
    );
    for (output, expected) in report.outputs.iter().zip([png_expected, webp_expected]) {
        let bytes = std::fs::read(&output.path)?;
        assert!(!bytes.windows(4).any(|c| c == b"iCCP" || c == b"eXIf"));
        let actual = image::load_from_memory(&bytes)?.into_rgba8();
        assert_eq!(actual.dimensions(), expected.dimensions());
        for (index, (&actual, &expected)) in
            actual.as_raw().iter().zip(expected.as_raw()).enumerate()
        {
            if index % 4 == 3 {
                assert_eq!(actual, expected);
            } else {
                assert!(
                    actual.abs_diff(expected) <= 2,
                    "independent LittleCMS oracle at channel {index}: {actual} vs {expected}"
                );
            }
        }
        let upload_png = saved_image_png_without_metadata(&output.source, || false)?;
        assert_eq!(
            image::load_from_memory(&upload_png)?.into_rgba8(),
            image::load_from_memory(&bytes)?.into_rgba8()
        );
    }
    assert_eq!(std::fs::read(png_path)?, png);
    assert_eq!(std::fs::read(webp_path)?, webp);
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
