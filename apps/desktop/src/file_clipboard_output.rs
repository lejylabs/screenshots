//! One streaming PNG representation; Windows requests raw DIB rows only after PNG transfer.
use super::*;
#[cfg(target_os = "linux")]
const MAGIC: &[u8; 8] = b"SCOUTP01";
#[cfg(any(target_os = "windows", target_os = "macos"))]
const MAGIC: &[u8; 8] = b"SCOUTP02";
#[derive(Debug)]
pub(super) struct PreparationFailed;
impl std::fmt::Display for PreparationFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Native image preparation failed before publication")
    }
}
impl std::error::Error for PreparationFailed {}
pub(super) const CHILD: &str = "__native-image-text-clipboard-worker";
pub(super) struct Request {
    kind: u8,
    width: u32,
    height: u32,
    pixels: Option<Arc<image::RgbaImage>>,
    bytes: Vec<u8>,
}
impl Request {
    pub(super) fn new(value: app_core::NativeClipboardOutput<'_>) -> Result<Self> {
        match value {
            app_core::NativeClipboardOutput::Image(image) => {
                validate(1, image.width(), image.height(), image.as_raw().len())?;
                validate_png_rows(image.width())?;
                Ok(Self {
                    kind: 1,
                    width: image.width(),
                    height: image.height(),
                    pixels: Some(image),
                    bytes: Vec::new(),
                })
            }
            app_core::NativeClipboardOutput::Text(text) => {
                validate(2, 0, 0, text.len())?;
                Ok(Self {
                    kind: 2,
                    width: 0,
                    height: 0,
                    pixels: None,
                    bytes: text.as_bytes().to_vec(),
                })
            }
            #[cfg(target_os = "windows")]
            app_core::NativeClipboardOutput::File(path) => {
                use std::os::windows::ffi::OsStrExt;
                app_core::validate_file_clipboard_path(path)?;
                let bytes = path
                    .as_os_str()
                    .encode_wide()
                    .flat_map(u16::to_le_bytes)
                    .collect();
                Ok(Self {
                    kind: 3,
                    width: 0,
                    height: 0,
                    pixels: None,
                    bytes,
                })
            }
        }
    }
    pub(super) fn exchange(
        self,
        wayland: bool,
        nonce: &[u8; 16],
        io: &mut transport::ActiveIo<'_>,
        publication_gate: &AtomicBool,
    ) -> Result<()> {
        let image = self.pixels.is_some();
        let exchanged = (|| -> Result<()> {
            let mut hello = [0; 24];
            io.read_exact(&mut hello)?;
            ensure!(
                &hello[..8] == MAGIC && hello[8..] == *nonce,
                "Clipboard output challenge mismatch"
            );
            io.write_all(MAGIC)?;
            io.write_all(nonce)?;
            io.write_all(&[self.kind])?;
            io.write_all(&self.width.to_be_bytes())?;
            io.write_all(&self.height.to_be_bytes())?;
            let length = self
                .pixels
                .as_ref()
                .map_or(self.bytes.len(), |image| image.as_raw().len());
            io.write_all(&(length as u64).to_be_bytes())?;
            io.write_all(&[u8::from(wayland)])?;
            if let Some(pixels) = &self.pixels {
                let check = io.check;
                {
                    let mut framed = Framed {
                        io,
                        total: 0,
                        limit: length + PNG_ALLOWANCE,
                    };
                    encode_png_stream(pixels, &mut framed, check)?;
                    framed.io.write_all(&0u32.to_be_bytes())?;
                }
                #[cfg(any(target_os = "windows", target_os = "macos"))]
                {
                    let mut prepared = [0];
                    io.read_exact(&mut prepared)?;
                    ensure!(prepared == [3], "Native PNG preparation failed");
                }
                #[cfg(target_os = "windows")]
                {
                    // Once this byte might reach the helper, native output can already change.
                    publication_gate.store(true, Ordering::Release);
                    io.write_all(&[1])?;
                    let mut published = [0];
                    io.read_exact(&mut published)?;
                    ensure!(
                        published == [4],
                        "Native PNG publication or DIB preparation failed"
                    );
                    io.write_all(pixels.as_raw())?;
                }
            } else {
                io.write_all(&self.bytes)?;
            }
            drop(self);
            if image {
                publication_gate.store(true, Ordering::Release);
            }
            io.write_all(&[1])?;
            let mut ack = [0];
            io.read_exact(&mut ack)?;
            ensure!(ack == [1], "Native clipboard output failed");
            Ok(())
        })();
        if exchanged.is_err() && image && !publication_gate.load(Ordering::Acquire) {
            Err(PreparationFailed.into())
        } else {
            exchanged
        }
    }
}
fn validate(kind: u8, width: u32, height: u32, length: usize) -> Result<()> {
    match kind {
        1 => {
            app_core::validate_image_size(width, height)?;
            ensure!(
                u64::from(width) * u64::from(height) * 4 == length as u64,
                "Invalid clipboard output raster extent"
            );
        }
        2 => ensure!(
            width == 0 && height == 0 && length <= app_core::MAX_CLIPBOARD_TEXT_BYTES,
            "Clipboard text exceeds its bound"
        ),
        #[cfg(target_os = "windows")]
        3 => ensure!(
            width == 0
                && height == 0
                && length > 0
                && length % 2 == 0
                && length <= app_core::CLIPBOARD_FILE_BYTES,
            "Invalid native clipboard file path"
        ),
        _ => anyhow::bail!("Invalid native clipboard output kind"),
    }
    Ok(())
}
const PNG_ALLOWANCE: usize = 16 * 1024 * 1024;
fn validate_png_rows(width: u32) -> Result<()> {
    // png 0.18.1's StreamWriter allocates prev/current/filtered RGBA scanlines.
    ensure!(
        u64::from(width) * 12 <= 8 * 1024 * 1024,
        "PNG clipboard output is unsupported when its three scanlines exceed 8 MiB"
    );
    Ok(())
}
struct Framed<'a, 'b> {
    io: &'a mut transport::ActiveIo<'b>,
    total: usize,
    limit: usize,
}
impl Write for Framed<'_, '_> {
    fn write(&mut self, value: &[u8]) -> std::io::Result<usize> {
        let total = self
            .total
            .checked_add(value.len())
            .filter(|n| *n <= self.limit)
            .ok_or_else(|| {
                std::io::Error::other("Clipboard PNG exceeds its encoded output limit")
            })?;
        for chunk in value.chunks(64 * 1024) {
            self.io.write_all(&(chunk.len() as u32).to_be_bytes())?;
            self.io.write_all(chunk)?;
        }
        self.total = total;
        Ok(value.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.io.flush()
    }
}
struct StickyWriter<'a, W: Write> {
    inner: &'a mut W,
    failed: bool,
}
impl<W: Write> Write for StickyWriter<'_, W> {
    fn write(&mut self, value: &[u8]) -> std::io::Result<usize> {
        let result = self.inner.write(value);
        if result.is_err() || matches!(&result, Ok(0) if !value.is_empty()) {
            self.failed = true;
        }
        result
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let result = self.inner.flush();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}
pub(super) fn encode_png_stream(
    image: &image::RgbaImage,
    writer: &mut impl Write,
    check: &app_core::NativeCaptureCheck<'_>,
) -> Result<()> {
    check()?;
    validate_png_rows(image.width())?;
    let mut sticky = StickyWriter {
        inner: writer,
        failed: false,
    };
    let encoded = encode_png_data(image, &mut sticky, check);
    // png 0.18.1 drops its final ChunkWriter and can suppress that writer's flush error.
    // All PNG owners have dropped before this check, so no failed final IDAT can reach the gate.
    ensure!(
        !sticky.failed,
        "Clipboard PNG output encountered a write or flush failure"
    );
    encoded
}

fn encode_png_data(
    image: &image::RgbaImage,
    writer: &mut impl Write,
    check: &app_core::NativeCaptureCheck<'_>,
) -> Result<()> {
    let mut encoder = png::Encoder::new(writer, image.width(), image.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    // Balanced avoids fdeflate's writer-error panic; the latch also covers PNG Drop flushes.
    encoder.set_compression(png::Compression::Balanced);
    encoder.set_filter(png::Filter::Adaptive);
    let mut writer = encoder.write_header()?;
    {
        let mut stream = writer.stream_writer_with_size(64 * 1024)?;
        for chunk in image.as_raw().chunks(64 * 1024) {
            check()?;
            stream.write_all(chunk)?;
        }
        check()?;
        stream.finish()?;
    }
    check()?;
    writer.finish()?;
    Ok(())
}

pub(super) fn serve() -> Result<()> {
    let (channel, nonce, parent) = transport::Transport::inherited()?;
    #[cfg(target_os = "macos")]
    let _lifeline = transport::Lifeline::new(parent)?;
    #[cfg(not(target_os = "macos"))]
    let _ = parent;
    #[cfg(unix)]
    let budget = budget::Budget::enter()?;
    let mut io = transport::ActiveIo {
        transport: channel,
        deadline: Instant::now() + DEADLINE,
        check: &|| Ok(()),
    };
    io.write_all(MAGIC)?;
    io.write_all(&nonce)?;
    let mut header = [0; 42];
    io.read_exact(&mut header)?;
    ensure!(
        &header[..8] == MAGIC && header[8..24] == nonce && header[41] <= 1,
        "Invalid private clipboard output request"
    );
    let kind = header[24];
    let width = u32::from_be_bytes(header[25..29].try_into()?);
    let height = u32::from_be_bytes(header[29..33].try_into()?);
    let length = usize::try_from(u64::from_be_bytes(header[33..41].try_into()?))?;
    validate(kind, width, height, length)?;
    if kind == 1 {
        validate_png_rows(width)?;
        #[cfg(unix)]
        budget.clipboard_output(length as u64)?;
        #[cfg(target_os = "windows")]
        {
            ensure!(header[41] == 0, "Wayland clipboard output requires Linux");
            let prepared = image_windows::prepare(&mut io, width, height, length)?;
            io.write_all(&[3])?;
            publication_gate(&mut io)?;
            let mut publishing = prepared.begin()?;
            io.write_all(&[4])?;
            publishing.read_rows(&mut io)?;
            publication_gate(&mut io)?;
            publishing.finish()?;
            io.write_all(&[1])?;
            return Ok(());
        }
        #[cfg(target_os = "macos")]
        {
            ensure!(header[41] == 0, "Wayland clipboard output requires Linux");
            let mut data = macos::ImageData::new(length + PNG_ALLOWANCE)?;
            ensure!(
                data.allocation_bytes() <= length + PNG_ALLOWANCE + 64 * 1024,
                "Native PNG allocation exceeds its admitted extent"
            );
            read_png_chunks(&mut io, length + PNG_ALLOWANCE, |chunk| data.append(chunk))?;
            validate_png_header(&data.header()?, width, height)?;
            let prepared = data.finish()?;
            io.write_all(&[3])?;
            publication_gate(&mut io)?;
            prepared.publish("com.apple.pasteboard.clipboard")?;
            io.write_all(&[1])?;
            return Ok(());
        }
        #[cfg(target_os = "linux")]
        {
            let mut segmented = linux::Bytes::new();
            read_png_chunks(&mut io, length + PNG_ALLOWANCE, |chunk| {
                segmented.append(chunk)
            })?;
            validate_png_header(segmented.range(0, 33), width, height)?;
            publication_gate(&mut io)?;
            linux::serve_output(segmented, "image/png", header[41] == 1, &mut io)?;
            return Ok(());
        }
    }
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length)?;
    bytes.resize(length, 0);
    io.read_exact(&mut bytes)?;
    publication_gate(&mut io)?;
    if kind == 2 {
        std::str::from_utf8(&bytes)?;
    }
    #[cfg(target_os = "windows")]
    {
        ensure!(header[41] == 0, "Wayland clipboard output requires Linux");
        let file = if kind == 3 {
            use std::os::windows::ffi::OsStringExt;
            let units = bytes
                .chunks_exact(2)
                .map(|p| u16::from_le_bytes([p[0], p[1]]))
                .collect::<Vec<_>>();
            let path = std::path::PathBuf::from(OsString::from_wide(&units));
            app_core::validate_file_clipboard_path(&path)?;
            ensure!(
                std::fs::metadata(&path)?.is_file(),
                "Clipboard source is not a regular file"
            );
            Some(path)
        } else {
            None
        };
        let mut clipboard = arboard::Clipboard::new()?;
        match kind {
            2 => clipboard.set_text(String::from_utf8(bytes)?)?,
            3 => clipboard
                .set()
                .file_list(&[file.as_ref().context("Native file path missing")?])?,
            _ => unreachable!(),
        }
        io.write_all(&[1])?;
    }
    #[cfg(target_os = "linux")]
    {
        let representation = linux::Bytes::from_slice(&bytes)?;
        drop(bytes);
        linux::serve_output(
            representation,
            "text/plain;charset=utf-8",
            header[41] == 1,
            &mut io,
        )?;
    }
    #[cfg(target_os = "macos")]
    {
        ensure!(header[41] == 0, "Wayland clipboard output requires Linux");
        macos::publish_bytes(
            &bytes,
            "public.utf8-plain-text",
            "com.apple.pasteboard.clipboard",
        )?;
        io.write_all(&[1])?;
    }
    Ok(())
}
fn publication_gate(io: &mut transport::ActiveIo<'_>) -> Result<()> {
    let mut gate = [0];
    io.read_exact(&mut gate)?;
    ensure!(
        gate == [1],
        "Clipboard output was not committed for publication"
    );
    (io.check)()?;
    ensure!(
        Instant::now() < io.deadline,
        "Clipboard publication deadline expired"
    );
    Ok(())
}
#[cfg(unix)]
fn read_png_chunks(
    reader: &mut impl Read,
    limit: usize,
    mut append: impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    let mut received = 0usize;
    let mut chunk = [0; 64 * 1024];
    loop {
        let mut size = [0; 4];
        reader.read_exact(&mut size)?;
        let size = u32::from_be_bytes(size) as usize;
        if size == 0 {
            return Ok(());
        }
        ensure!(
            size <= chunk.len() && received.checked_add(size).is_some_and(|n| n <= limit),
            "Invalid clipboard PNG stream extent"
        );
        reader.read_exact(&mut chunk[..size])?;
        received += size;
        append(&chunk[..size])?;
    }
}
#[cfg(unix)]
fn validate_png_header(header: &[u8], width: u32, height: u32) -> Result<()> {
    ensure!(
        header.len() == 33
            && &header[..8] == b"\x89PNG\r\n\x1a\n"
            && &header[12..16] == b"IHDR"
            && header[16..20] == width.to_be_bytes()
            && header[20..24] == height.to_be_bytes()
            && header[24..26] == [8, 6],
        "Invalid clipboard PNG stream header"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn output_wire_admission_precedes_payload_allocation() {
        assert!(validate(1, 0, 1, 0).is_err());
        assert!(validate(1, 2, 2, 15).is_err());
        assert!(validate(1, u32::MAX, u32::MAX, 4).is_err());
        assert!(validate(2, 0, 0, app_core::MAX_CLIPBOARD_TEXT_BYTES + 1).is_err());
        assert!(validate(3, 0, 0, 1).is_err());
        assert!(validate(4, 0, 0, 0).is_err());
        validate(1, 2, 2, 16).unwrap();
        validate(2, 0, 0, 0).unwrap();
    }
    #[test]
    fn final_idat_drop_error_is_sticky_even_when_iend_succeeds() {
        #[derive(Default)]
        struct RefuseFinalIdatOnce {
            idats: usize,
            refused: bool,
            iend_succeeded: bool,
        }
        impl Write for RefuseFinalIdatOnce {
            fn write(&mut self, value: &[u8]) -> std::io::Result<usize> {
                if value == b"IDAT" {
                    self.idats += 1;
                    if self.idats == 2 {
                        self.refused = true;
                        return Err(std::io::Error::other("private final IDAT rejection"));
                    }
                }
                if value == b"IEND" {
                    self.iend_succeeded = true;
                }
                Ok(value.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let image = image::RgbaImage::from_pixel(3, 2, image::Rgba([17, 23, 47, 83]));
        // First IDAT is an explicit flush; only the second carries the Drop-only zlib trailer.
        let mut baseline = RefuseFinalIdatOnce::default();
        assert!(
            encode_png_data(&image, &mut baseline, &|| Ok(())).is_ok(),
            "pinned codec baseline must complete both explicit finish calls"
        );
        assert!(baseline.refused && baseline.idats == 2 && baseline.iend_succeeded);
        let mut sink = RefuseFinalIdatOnce::default();
        let result = encode_png_stream(&image, &mut sink, &|| Ok(()));
        assert!(sink.refused && sink.idats == 2 && sink.iend_succeeded);
        assert!(
            result.is_err(),
            "the sticky latch alone must prohibit publication"
        );
    }
    #[cfg(unix)]
    #[test]
    fn streaming_png_preserves_alpha_and_propagates_write_finish_and_cancellation_errors()
    -> Result<()> {
        let image = image::RgbaImage::from_fn(3, 2, |x, y| image::Rgba([x as u8, y as u8, 47, 83]));
        let mut png = Vec::new();
        encode_png_stream(&image, &mut png, &|| Ok(()))?;
        assert_eq!(image::load_from_memory(&png)?.into_rgba8(), image);
        let metadata = png::Decoder::new(std::io::Cursor::new(&png)).read_info()?;
        assert_eq!(
            metadata.info().srgb,
            Some(png::SrgbRenderingIntent::Perceptual)
        );
        struct Deny;
        impl Write for Deny {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("private write rejection"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::other("private finish rejection"))
            }
        }
        assert!(encode_png_stream(&image, &mut Deny, &|| Ok(())).is_err());
        struct RejectIend;
        impl Write for RejectIend {
            fn write(&mut self, value: &[u8]) -> std::io::Result<usize> {
                if value == b"IEND" {
                    Err(std::io::Error::other("private IEND rejection"))
                } else {
                    Ok(value.len())
                }
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        assert!(encode_png_stream(&image, &mut RejectIend, &|| Ok(())).is_err());
        assert!(
            encode_png_stream(&image, &mut Vec::new(), &|| Err(
                app_core::CaptureError::NativeCanceled
            ))
            .is_err()
        );
        let image = Arc::new(image);
        let request = Request::new(app_core::NativeClipboardOutput::Image(image.clone()))?;
        assert!(request.bytes.is_empty());
        assert!(Arc::ptr_eq(request.pixels.as_ref().unwrap(), &image));
        drop(request);
        assert_eq!(Arc::strong_count(&image), 1);
        assert!(validate_png_rows(699051).is_err());
        validate_png_rows(699050)?;
        Ok(())
    }
}
