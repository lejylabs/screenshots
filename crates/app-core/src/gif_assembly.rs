//! Explicit offline assembly of a finite, play-once GIF.

use crate::CaptureError;
use std::{
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};

const MAX_FRAME_BYTES: u64 = 1280 * 720 * 4;
const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
const MAX_OUTPUT_BYTES: u64 = 64 * 1024 * 1024;

fn error(message: &str) -> CaptureError {
    CaptureError::Backend(message.into())
}
fn gif_error(value: gif::EncodingError) -> CaptureError {
    match value {
        gif::EncodingError::Io(value) => CaptureError::Io(value),
        _ => error("GIF encoding failed"),
    }
}

struct LimitedWriter<W> {
    inner: W,
    written: u64,
    limit: u64,
}
impl<W: Write> Write for LimitedWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() as u64 > self.limit.saturating_sub(self.written) {
            return Err(std::io::Error::other("GIF output exceeds its 64 MiB limit"));
        }
        let count = self.inner.write(bytes)?;
        self.written += count as u64;
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn validate(paths: &[PathBuf], destination: &Path, delay_ms: u32) -> Result<(), CaptureError> {
    if !(2..=16).contains(&paths.len()) {
        return Err(error("GIF requires 2–16 input images"));
    }
    if !(10..=60000).contains(&delay_ms) || !delay_ms.is_multiple_of(10) {
        return Err(error(
            "GIF delay must be 10–60000 milliseconds in multiples of 10",
        ));
    }
    if !destination
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("gif"))
    {
        return Err(error("GIF output requires a .gif extension"));
    }
    match destination.symlink_metadata() {
        Ok(_) => return Err(error("GIF output already exists")),
        Err(value) if value.kind() == std::io::ErrorKind::NotFound => {}
        Err(value) => return Err(value.into()),
    }
    for path in paths {
        let metadata = path.metadata()?;
        if !metadata.is_file() || metadata.len() > 32 * 1024 * 1024 {
            return Err(error("GIF inputs must be regular files of at most 32 MiB"));
        }
    }
    Ok(())
}

fn encode<W: Write>(
    paths: &[PathBuf],
    writer: W,
    delay_ms: u32,
    limit: u64,
) -> Result<(), CaptureError> {
    let counter = LimitedWriter {
        inner: writer,
        written: 0,
        limit,
    };
    let mut writer = Some(BufWriter::with_capacity(64 * 1024, counter));
    let mut encoder = None;
    let mut dimensions = None;
    let mut used = 0u64;
    for path in paths {
        let remaining = MAX_TOTAL_BYTES - used;
        let mut pixels = crate::load_editor_image_with_limit(path, remaining.min(MAX_FRAME_BYTES))
            .map_err(|_| error("Could not decode GIF input within its image budget"))?;
        let (width, height) = pixels.dimensions();
        if width == 0 || height == 0 || width > 1280 || height > 720 {
            return Err(error("GIF frame dimensions must be 1–1280 by 1–720"));
        }
        let bytes = u64::from(width) * u64::from(height) * 4;
        if pixels.as_raw().len() as u64 != bytes
            || used
                .checked_add(bytes)
                .is_none_or(|total| total > MAX_TOTAL_BYTES)
        {
            return Err(error("GIF cumulative RGBA input exceeds 64 MiB"));
        }
        used += bytes;
        if dimensions.is_some_and(|size| size != (width, height)) {
            return Err(error("GIF input dimensions must match"));
        }
        dimensions = Some((width, height));
        for pixel in pixels.pixels_mut() {
            if !matches!(pixel[3], 0 | 255) {
                return Err(error(
                    "GIF inputs require opaque or fully transparent pixels",
                ));
            }
            if pixel[3] == 0 {
                pixel.0 = [0; 4];
            }
        }
        if encoder.is_none() {
            encoder = Some(
                gif::Encoder::new(writer.take().unwrap(), width as u16, height as u16, &[])
                    .map_err(gif_error)?,
            );
        }
        let mut frame =
            gif::Frame::from_rgba_speed(width as u16, height as u16, pixels.as_mut(), 10);
        if frame.transparent.is_none() {
            // Background disposal needs an explicit transparent slot even on opaque frames.
            let mut used = [false; 256];
            for index in frame.buffer.iter() {
                used[*index as usize] = true;
            }
            let index = used
                .iter()
                .position(|used| !*used)
                .ok_or_else(|| error("GIF palette has no spare transparency index"))?;
            let palette = frame
                .palette
                .as_mut()
                .ok_or_else(|| error("GIF local palette is missing"))?;
            palette.resize(palette.len().max((index + 1) * 3), 0);
            frame.transparent = Some(index as u8);
        }
        if frame.buffer.len() != pixels.width() as usize * pixels.height() as usize
            || frame
                .buffer
                .iter()
                .zip(pixels.pixels())
                .any(|(index, pixel)| (Some(*index) == frame.transparent) != (pixel[3] == 0))
        {
            return Err(error("GIF palette cannot preserve binary transparency"));
        }
        drop(pixels);
        frame.delay = (delay_ms / 10) as u16;
        frame.dispose = gif::DisposalMethod::Background;
        encoder
            .as_mut()
            .unwrap()
            .write_frame(&frame)
            .map_err(gif_error)?;
    }
    // Explicit finishing propagates trailer errors instead of relying on Drop.
    let mut writer = encoder
        .ok_or_else(|| error("GIF requires input frames"))?
        .into_inner()
        .map_err(gif_error)?;
    writer.flush()?;
    Ok(())
}

pub fn assemble_gif(
    paths: &[PathBuf],
    destination: &Path,
    delay_ms: u32,
) -> Result<(), CaptureError> {
    validate(paths, destination, delay_ms)?;
    crate::capture::write_output(destination, |file| {
        encode(paths, file, delay_ms, MAX_OUTPUT_BYTES)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageEncoder, Rgba, RgbaImage};
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("screenshots-gif-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
        fn image(&self, name: &str, pixels: &RgbaImage) -> PathBuf {
            let path = self.path(name);
            let file = std::fs::File::create(&path).unwrap();
            image::codecs::png::PngEncoder::new(file)
                .write_image(
                    pixels.as_raw(),
                    pixels.width(),
                    pixels.height(),
                    image::ExtendedColorType::Rgba8,
                )
                .unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn exact_palettes_delay_disposal_and_binary_alpha_are_written_once() {
        let fixture = Fixture::new();
        let first = RgbaImage::from_pixel(3, 2, Rgba([255, 0, 0, 255]));
        let second = RgbaImage::from_fn(3, 2, |x, _| {
            if x == 0 {
                Rgba([99, 88, 77, 0])
            } else {
                Rgba([0, 255, 0, 255])
            }
        });
        let paths = vec![
            fixture.image("a.png", &first),
            fixture.image("b.png", &second),
        ];
        let originals = paths
            .iter()
            .map(|p| std::fs::read(p).unwrap())
            .collect::<Vec<_>>();
        assemble_gif(&paths, &fixture.path("out.gif"), 120).unwrap();
        let bytes = std::fs::read(fixture.path("out.gif")).unwrap();
        assert!(!bytes.windows(11).any(|v| v == b"NETSCAPE2.0"));
        assert_eq!(bytes.last(), Some(&0x3b));
        let mut options = gif::DecodeOptions::new();
        options.set_color_output(gif::ColorOutput::RGBA);
        let mut decoder = options.read_info(std::io::Cursor::new(&bytes)).unwrap();
        for expected in [&first, &second] {
            let frame = decoder.read_next_frame().unwrap().unwrap();
            assert_eq!(frame.delay, 12);
            assert_eq!(frame.dispose, gif::DisposalMethod::Background);
            assert_eq!((frame.width, frame.height), (3, 2));
            for (actual, expected) in frame.buffer.chunks_exact(4).zip(expected.pixels()) {
                assert_eq!(actual[3], expected[3]);
                if expected[3] == 255 {
                    assert_eq!(actual, &expected.0);
                }
            }
        }
        assert!(decoder.read_next_frame().unwrap().is_none());
        for (path, bytes) in paths.iter().zip(originals) {
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
    }

    #[test]
    fn invalid_inputs_never_publish_and_output_caps_and_trailer_errors_propagate() {
        let fixture = Fixture::new();
        let a = fixture.image("a.png", &RgbaImage::from_pixel(2, 2, Rgba([1, 2, 3, 255])));
        let b = fixture.image("b.png", &RgbaImage::from_pixel(2, 2, Rgba([1, 2, 3, 128])));
        let paths = vec![a.clone(), b.clone()];
        assert!(assemble_gif(&paths, &fixture.path("bad.gif"), 10).is_err());
        assert!(!fixture.path("bad.gif").exists());
        for delay in [0, 1, 11, 60001] {
            assert!(
                assemble_gif(&[a.clone(), a.clone()], &fixture.path("bad.gif"), delay).is_err()
            );
        }
        assert!(assemble_gif(&[a.clone()], &fixture.path("bad.gif"), 10).is_err());
        assert!(assemble_gif(&vec![a.clone(); 17], &fixture.path("bad.gif"), 10).is_err());
        std::fs::write(&b, b"invalid").unwrap();
        assert!(assemble_gif(&paths, &fixture.path("bad.gif"), 10).is_err());
        std::fs::write(fixture.path("existing.gif"), b"keep").unwrap();
        assert!(assemble_gif(&[a.clone(), a.clone()], &fixture.path("existing.gif"), 10).is_err());
        assert_eq!(
            std::fs::read(fixture.path("existing.gif")).unwrap(),
            b"keep"
        );
        assert!(encode(&[a.clone(), a], Vec::new(), 10, 1).is_err());
        assert!(!fixture.0.read_dir().unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".part")
        }));
        struct TrailerFails;
        impl Write for TrailerFails {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if bytes == [0x3b] {
                    Err(std::io::Error::other("trailer failure"))
                } else {
                    Ok(bytes.len())
                }
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let encoder = gif::Encoder::new(TrailerFails, 1, 1, &[]).unwrap();
        assert!(encoder.into_inner().is_err());
    }

    #[test]
    fn mismatched_or_oversized_frames_fail_and_large_palettes_are_quantized() {
        let fixture = Fixture::new();
        let small = fixture.image(
            "small.png",
            &RgbaImage::from_pixel(2, 2, Rgba([1, 2, 3, 255])),
        );
        let mismatch = fixture.image(
            "mismatch.png",
            &RgbaImage::from_pixel(3, 2, Rgba([1, 2, 3, 255])),
        );
        assert!(
            assemble_gif(
                &[small.clone(), mismatch],
                &fixture.path("mismatch.gif"),
                10
            )
            .is_err()
        );
        let wide = fixture.image(
            "wide.png",
            &RgbaImage::from_pixel(1281, 1, Rgba([1, 2, 3, 255])),
        );
        assert!(assemble_gif(&[wide.clone(), wide], &fixture.path("wide.gif"), 10).is_err());
        let pattern = RgbaImage::from_fn(32, 32, |x, y| {
            Rgba([(x * 7) as u8, (y * 7) as u8, ((x + y) * 3) as u8, 255])
        });
        let image = fixture.image("colors.png", &pattern);
        if let Err(error) =
            assemble_gif(&[image.clone(), image], &fixture.path("colors.gif"), 60000)
        {
            assert!(error.to_string().contains("spare transparency index"));
            assert!(!fixture.path("colors.gif").exists());
            return;
        }
        let mut options = gif::DecodeOptions::new();
        options.set_color_output(gif::ColorOutput::RGBA);
        let mut decoder = options
            .read_info(std::fs::File::open(fixture.path("colors.gif")).unwrap())
            .unwrap();
        let frame = decoder.read_next_frame().unwrap().unwrap();
        assert_eq!(frame.delay, 6000);
        assert_ne!(frame.buffer.as_ref(), pattern.as_raw());
        assert!(frame.buffer.chunks_exact(4).all(|pixel| pixel[3] == 255));
        assert!(!fixture.path("mismatch.gif").exists());
        assert!(!fixture.path("wide.gif").exists());
        assert_eq!(std::fs::read(small).unwrap().len() > 0, true);
    }

    #[test]
    fn quantized_transparency_is_exact_or_fails_without_publication() {
        let fixture = Fixture::new();
        let pixels = RgbaImage::from_fn(32, 32, |x, y| {
            if x == 0 && y == 0 {
                Rgba([0, 0, 0, 0])
            } else {
                Rgba([(x * 7) as u8, (y * 7) as u8, ((x + y) * 3) as u8, 255])
            }
        });
        let path = fixture.image("many.png", &pixels);
        let output = fixture.path("many.gif");
        match assemble_gif(&[path.clone(), path], &output, 10) {
            Ok(()) => {
                let mut options = gif::DecodeOptions::new();
                options.set_color_output(gif::ColorOutput::RGBA);
                let mut decoder = options
                    .read_info(std::fs::File::open(&output).unwrap())
                    .unwrap();
                while let Some(frame) = decoder.read_next_frame().unwrap() {
                    assert!(
                        frame
                            .buffer
                            .chunks_exact(4)
                            .zip(pixels.pixels())
                            .all(|(actual, expected)| actual[3] == expected[3])
                    );
                }
            }
            Err(error) => {
                assert!(error.to_string().contains("binary transparency"));
                assert!(!output.exists());
            }
        }
        assert!(!fixture.0.read_dir().unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".part")
        }));
    }

    #[test]
    fn spare_palette_slot_and_buffered_flush_failure_are_enforced() {
        let fixture = Fixture::new();
        let mut accepted = None;
        for colors in [255, 256] {
            let pixels = RgbaImage::from_fn(colors, 1, |x, _| Rgba([x as u8, 17, 29, 255]));
            let input = fixture.image(&format!("{colors}.png"), &pixels);
            let output = fixture.path(&format!("{colors}.gif"));
            let result = assemble_gif(&[input.clone(), input.clone()], &output, 10);
            if colors == 255 {
                result.unwrap();
                assert!(output.is_file());
                let mut options = gif::DecodeOptions::new();
                options.set_color_output(gif::ColorOutput::RGBA);
                let mut decoder = options
                    .read_info(std::fs::File::open(output).unwrap())
                    .unwrap();
                for _ in 0..2 {
                    let frame = decoder.read_next_frame().unwrap().unwrap();
                    assert_eq!(frame.buffer.as_ref(), pixels.as_raw());
                }
                assert!(decoder.read_next_frame().unwrap().is_none());
                accepted = Some(input);
            } else {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("spare transparency index")
                );
                assert!(!output.exists());
            }
        }

        struct FailFlush<'a>(&'a mut std::fs::File);
        impl Write for FailFlush<'_> {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.write(bytes)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::other("synthetic final flush failure"))
            }
        }
        let input = accepted.unwrap();
        let output = fixture.path("flush.gif");
        let result = crate::capture::write_output(&output, |file| {
            encode(
                &[input.clone(), input],
                FailFlush(file),
                10,
                MAX_OUTPUT_BYTES,
            )
        });
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("synthetic final flush failure")
        );
        assert!(!output.exists());
        assert!(!fixture.0.read_dir().unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".part")
        }));
    }
}
