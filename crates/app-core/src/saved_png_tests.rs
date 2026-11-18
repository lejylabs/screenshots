use super::*;
use image::{ExtendedColorType, ImageEncoder, Rgba, RgbaImage};
use std::io::{BufRead, BufReader, Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("screenshots-saved-png-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn chunk(output: &mut Vec<u8>, kind: &[u8; 4], payload: &[u8]) {
    output.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    output.extend_from_slice(kind);
    output.extend_from_slice(payload);
    let mut crc = crc32fast::Hasher::new();
    crc.update(kind);
    crc.update(payload);
    output.extend_from_slice(&crc.finalize().to_be_bytes());
}

// Tiny independent zlib stream using uncompressed DEFLATE blocks.
fn zlib(bytes: &[u8]) -> Vec<u8> {
    let mut output = vec![0x78, 0x01];
    for (index, block) in bytes.chunks(65535).enumerate() {
        output.push(u8::from((index + 1) * 65535 >= bytes.len()));
        let length = block.len() as u16;
        output.extend_from_slice(&length.to_le_bytes());
        output.extend_from_slice(&(!length).to_le_bytes());
        output.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for byte in bytes {
        a = (a + u32::from(*byte)) % 65521;
        b = (b + a) % 65521;
    }
    output.extend_from_slice(&((b << 16) | a).to_be_bytes());
    output
}

fn header(width: u32, height: u32, depth: u8, color: u8, interlace: u8) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::from(width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[depth, color, 0, 0, interlace]);
    chunk(&mut bytes, b"IHDR", &ihdr);
    bytes
}

fn png(
    width: u32,
    height: u32,
    depth: u8,
    color: u8,
    rows: &[u8],
    extras: &[(&[u8; 4], &[u8])],
) -> Vec<u8> {
    let mut bytes = header(width, height, depth, color, 0);
    for (kind, payload) in extras {
        chunk(&mut bytes, kind, payload);
    }
    chunk(&mut bytes, b"IDAT", &zlib(rows));
    chunk(&mut bytes, b"IEND", &[]);
    bytes
}

fn old_loader(path: &Path) -> RgbaImage {
    let mut reader = image::ImageReader::with_format(
        BufReader::new(std::fs::File::open(path).unwrap()),
        image::ImageFormat::Png,
    );
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(crate::MAX_CAPTURE_BYTES);
    reader.limits(limits);
    reader.decode().unwrap().into_rgba8()
}

#[test]
fn saved_png_variants_match_former_loader_and_preserve_straight_alpha() {
    let fixture = Fixture::new();
    let rgba = [11, 22, 33, 0, 44, 55, 66, 128, 77, 88, 99, 255];
    let cases = [
        (
            "rgba",
            png(3, 1, 8, 6, &[&[0], rgba.as_slice()].concat(), &[]),
        ),
        ("gray", png(3, 1, 8, 0, &[0, 0, 128, 255], &[])),
        (
            "gray-alpha",
            png(3, 1, 8, 4, &[0, 10, 0, 20, 128, 30, 255], &[]),
        ),
        (
            "rgb",
            png(3, 1, 8, 2, &[0, 11, 22, 33, 44, 55, 66, 77, 88, 99], &[]),
        ),
        (
            "indexed",
            png(
                3,
                1,
                8,
                3,
                &[0, 0, 1, 2],
                &[
                    (b"PLTE", &[11, 22, 33, 44, 55, 66, 77, 88, 99]),
                    (b"tRNS", &[0, 128, 255]),
                ],
            ),
        ),
        (
            "gray16",
            png(3, 1, 16, 0, &[0, 0, 0, 0x80, 0x01, 0xff, 0xff], &[]),
        ),
        (
            "rgb16",
            png(1, 1, 16, 2, &[0, 0x12, 0x34, 0x80, 0x01, 0xff, 0xff], &[]),
        ),
        (
            "rgba16",
            png(
                1,
                1,
                16,
                6,
                &[0, 0x12, 0x34, 0x80, 0x01, 0xff, 0xff, 0x40, 0x01],
                &[],
            ),
        ),
    ];
    for (name, bytes) in cases {
        let path = fixture.path(&format!("{name}.misleading"));
        std::fs::write(&path, &bytes).unwrap();
        let pixels = load_saved_png(&path).unwrap();
        assert_eq!(pixels, old_loader(&path), "{name}");
        if name == "rgba" || name == "indexed" {
            assert_eq!(pixels.as_raw(), &rgba);
        }
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn saved_png_interlace_exif_and_apng_keep_default_still_semantics() {
    let fixture = Fixture::new();
    let expected = RgbaImage::from_fn(3, 3, |x, y| {
        Rgba([
            (x * 80) as u8,
            (y * 70) as u8,
            31,
            [0, 128, 255][((x + y) % 3) as usize],
        ])
    });
    let mut scanlines = Vec::new();
    for (start_x, start_y, step_x, step_y) in [
        (0, 0, 8, 8),
        (4, 0, 8, 8),
        (0, 4, 4, 8),
        (2, 0, 4, 4),
        (0, 2, 2, 4),
        (1, 0, 2, 2),
        (0, 1, 1, 2),
    ] {
        if start_x >= 3 || start_y >= 3 {
            continue;
        }
        for y in (start_y..3).step_by(step_y) {
            scanlines.push(0);
            for x in (start_x..3).step_by(step_x) {
                scanlines.extend_from_slice(&expected.get_pixel(x, y).0);
            }
        }
    }
    let mut interlaced = header(3, 3, 8, 6, 1);
    chunk(&mut interlaced, b"IDAT", &zlib(&scanlines));
    chunk(&mut interlaced, b"IEND", &[]);
    let path = fixture.path("interlaced.png");
    std::fs::write(&path, interlaced).unwrap();
    assert_eq!(load_saved_png(&path).unwrap(), expected);
    assert_eq!(load_saved_png(&path).unwrap(), old_loader(&path));

    let exif = b"II\x2a\x00\x08\x00\x00\x00\x01\x00\x12\x01\x03\x00\x01\x00\x00\x00\x06\x00\x00\x00\x00\x00\x00\x00";
    let oriented = png(
        2,
        1,
        8,
        6,
        &[0, 11, 22, 33, 0, 44, 55, 66, 128],
        &[(b"eXIf", exif)],
    );
    let path = fixture.path("oriented.png");
    std::fs::write(&path, oriented).unwrap();
    assert_eq!(load_saved_png(&path).unwrap().dimensions(), (2, 1));
    assert_eq!(load_saved_png(&path).unwrap(), old_loader(&path));
    assert_eq!(load_editor_image(&path).unwrap().dimensions(), (1, 2));

    // The IDAT default is red; the sole animated frame is blue and comes after it.
    let mut animation = header(1, 1, 8, 6, 0);
    chunk(&mut animation, b"acTL", &[0, 0, 0, 1, 0, 0, 0, 0]);
    chunk(&mut animation, b"IDAT", &zlib(&[0, 255, 0, 0, 255]));
    let mut frame = Vec::new();
    for number in [0u32, 1, 1, 0, 0] {
        frame.extend_from_slice(&number.to_be_bytes());
    }
    frame.extend_from_slice(&[0, 1, 0, 10, 0, 0]);
    chunk(&mut animation, b"fcTL", &frame);
    let mut data = 1u32.to_be_bytes().to_vec();
    data.extend_from_slice(&zlib(&[0, 0, 0, 255, 255]));
    chunk(&mut animation, b"fdAT", &data);
    chunk(&mut animation, b"IEND", &[]);
    let path = fixture.path("default.apng");
    std::fs::write(&path, animation).unwrap();
    assert_eq!(load_saved_png(&path).unwrap().as_raw(), &[255, 0, 0, 255]);
    assert_eq!(load_saved_png(&path).unwrap(), old_loader(&path));
}

#[test]
fn saved_png_native_and_rgba_budgets_are_checked_before_idat_decode() {
    for color in [0, 3] {
        let mut bytes = header(3, 1, 8, color, 0);
        if color == 3 {
            chunk(&mut bytes, b"PLTE", &[1, 2, 3]);
        }
        chunk(&mut bytes, b"IDAT", &[]);
        chunk(&mut bytes, b"IEND", &[]);
        let error = decode_saved_png(Cursor::new(bytes), 11)
            .unwrap_err()
            .to_string();
        assert!(error.contains("RGBA"), "{error}");
    }
    let mut bytes = header(2, 1, 16, 6, 0);
    chunk(&mut bytes, b"IDAT", &[]);
    chunk(&mut bytes, b"IEND", &[]);
    let error = decode_saved_png(Cursor::new(bytes), 8)
        .unwrap_err()
        .to_string();
    assert!(error.contains("native"), "{error}");
    let bytes = png(
        3,
        1,
        8,
        6,
        &[0, 1, 2, 3, 0, 4, 5, 6, 128, 7, 8, 9, 255],
        &[],
    );
    assert!(decode_saved_png(Cursor::new(&bytes), 12).is_ok());
    assert!(
        decode_saved_png(Cursor::new(&bytes), 11)
            .unwrap_err()
            .to_string()
            .contains("RGBA")
    );
    for budget in [0, crate::MAX_CAPTURE_BYTES + 1] {
        assert!(
            decode_saved_png(Cursor::new(b"not a PNG"), budget)
                .unwrap_err()
                .to_string()
                .contains("budget")
        );
    }
    let native = png(
        2,
        1,
        16,
        6,
        &[0, 0, 1, 0, 2, 0, 3, 0, 4, 0, 5, 0, 6, 0, 7, 0, 8],
        &[],
    );
    assert!(decode_saved_png(Cursor::new(&native), 16).is_ok());
    assert!(
        decode_saved_png(Cursor::new(&native), 15)
            .unwrap_err()
            .to_string()
            .contains("native")
    );
}

#[test]
fn saved_png_file_cap_and_extent_allow_large_saved_files_without_snapshotting() {
    let fixture = Fixture::new();
    assert_eq!(MAX_SAVED_PNG_FILE_BYTES, 272 * 1024 * 1024);
    let too_short = fixture.path("short.png");
    std::fs::write(&too_short, b"1234567").unwrap();
    assert!(open_saved_png(&too_short).is_err());
    let path = fixture.path("large.png");
    std::fs::write(&path, png(1, 1, 8, 6, &[0, 1, 2, 3, 128], &[])).unwrap();
    let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_len(MAX_SAVED_PNG_FILE_BYTES).unwrap();
    let reader = open_saved_png(&path).unwrap();
    assert_eq!(reader.length, MAX_SAVED_PNG_FILE_BYTES);
    assert_eq!(load_saved_png(&path).unwrap().as_raw(), &[1, 2, 3, 128]);
    file.set_len(MAX_SAVED_PNG_FILE_BYTES + 1).unwrap();
    assert!(
        open_saved_png_after_preflight(&path)
            .err()
            .unwrap()
            .to_string()
            .contains("272 MiB")
    );
    assert!(
        load_saved_png(&path)
            .unwrap_err()
            .to_string()
            .contains("272 MiB")
    );

    let path = fixture.path("extent.png");
    std::fs::write(&path, b"abcdefgh").unwrap();
    let mut reader = open_saved_png(&path).unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"ijkl")
        .unwrap();
    assert_eq!(reader.seek(SeekFrom::End(0)).unwrap(), 8);
    assert_eq!(reader.seek(SeekFrom::End(-3)).unwrap(), 5);
    let mut tail = Vec::new();
    reader.read_to_end(&mut tail).unwrap();
    assert_eq!(tail, b"fgh");
    assert!(reader.seek(SeekFrom::End(-9)).is_err());
    reader.seek(SeekFrom::Start(0)).unwrap();
    reader.seek(SeekFrom::Current(2)).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(4)
        .unwrap();
    let mut tail = Vec::new();
    reader.read_to_end(&mut tail).unwrap();
    assert_eq!(tail, b"cd");
    assert_eq!(reader.seek(SeekFrom::End(0)).unwrap(), 8);
}

#[test]
fn saved_png_open_handle_survives_path_replacement_and_failures_do_not_publish() {
    let fixture = Fixture::new();
    let path = fixture.path("source.png");
    let bytes = png(1, 1, 8, 6, &[0, 11, 22, 33, 0], &[]);
    std::fs::write(&path, &bytes).unwrap();
    let reader = open_saved_png(&path).unwrap();
    std::fs::rename(&path, fixture.path("opened.png")).unwrap();
    std::fs::write(&path, png(1, 1, 8, 6, &[0, 44, 55, 66, 255], &[])).unwrap();
    assert_eq!(
        decode_saved_png(BufReader::new(reader), crate::MAX_CAPTURE_BYTES)
            .unwrap()
            .as_raw(),
        &[11, 22, 33, 0]
    );
    assert_eq!(std::fs::read(fixture.path("opened.png")).unwrap(), bytes);
    let invalid = fixture.path("invalid.png");
    std::fs::write(&invalid, b"not PNG!").unwrap();
    let mut clipboard = ImageClipboard::default();
    assert!(clipboard.copy_saved_png(&invalid).is_err());
    assert!(clipboard.clipboard.is_none());
    let destination = fixture.path("result.jpg");
    assert!(
        crate::export_saved_png_as(&invalid, &destination, 90, crate::ExportSize::Original)
            .is_err()
    );
    assert!(!destination.exists());
    assert!(crate::png_without_metadata(&invalid).is_err());
    assert!(load_saved_png(&fixture.0).is_err());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&path, fixture.path("linked.png")).unwrap();
        assert_eq!(
            load_saved_png(&fixture.path("linked.png"))
                .unwrap()
                .as_raw(),
            &[44, 55, 66, 255]
        );
        std::os::unix::fs::symlink(fixture.path("missing"), fixture.path("broken.png")).unwrap();
        assert!(load_saved_png(&fixture.path("broken.png")).is_err());
        assert!(load_saved_png(Path::new("/dev/null")).is_err());
    }
}

#[cfg(unix)]
#[test]
fn saved_png_fifo_substitution_child() {
    let Some(path) = std::env::var_os("SCREENSHOTS_TEST_SAVED_PNG_FIFO") else {
        return;
    };
    let path = PathBuf::from(path);
    assert!(path.metadata().unwrap().is_file());
    std::fs::remove_file(&path).unwrap();
    use std::os::unix::ffi::OsStrExt;
    let native = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(native.as_ptr(), 0o600) }, 0);
    assert!(open_saved_png_after_preflight(&path).is_err());
    assert!(load_saved_png(&path).is_err());
}

#[cfg(unix)]
#[test]
fn saved_png_fifo_substitution_is_rejected_in_a_killable_process() {
    let fixture = Fixture::new();
    let path = fixture.path("substituted.png");
    std::fs::write(&path, b"12345678").unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "clipboard::saved_png_tests::saved_png_fifo_substitution_child",
            "--nocapture",
        ])
        .env("SCREENSHOTS_TEST_SAVED_PNG_FIFO", &path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("FIFO substitution open blocked");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    use std::os::unix::fs::FileTypeExt;
    assert!(
        path.symlink_metadata().unwrap().file_type().is_fifo(),
        "The exact child fixture did not execute"
    );
}

#[test]
fn saved_png_generated_output_larger_than_editor_cap_is_accepted() {
    let fixture = Fixture::new();
    let path = fixture.path("large-output.png");
    let pixels = RgbaImage::from_fn(3000, 3000, |x, y| {
        let mut value = (x + y * 3000).wrapping_add(0x9e3779b9);
        value ^= value >> 16;
        value = value.wrapping_mul(0x85ebca6b);
        value ^= value >> 13;
        value = value.wrapping_mul(0xc2b2ae35);
        value ^= value >> 16;
        Rgba(value.to_le_bytes())
    });
    let file = std::fs::File::create(&path).unwrap();
    image::codecs::png::PngEncoder::new_with_quality(
        file,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::NoFilter,
    )
    .write_image(
        pixels.as_raw(),
        pixels.width(),
        pixels.height(),
        ExtendedColorType::Rgba8,
    )
    .unwrap();
    let size = path.metadata().unwrap().len();
    assert!(
        size > 32 * 1024 * 1024 && size < MAX_SAVED_PNG_FILE_BYTES,
        "{size}"
    );
    assert_eq!(load_saved_png(&path).unwrap(), pixels);
}

#[test]
#[ignore = "optimized synthetic 4K saved-PNG old/new comparison; no capture or clipboard"]
fn saved_png_admission_cost() {
    let fixture = Fixture::new();
    for bits in [8, 16] {
        let path = fixture.path(&format!("rgba{bits}.png"));
        let mut seed = 0x504e_4741u32;
        let mut rgba = Vec::with_capacity(3840 * 2160 * 4 * bits / 8);
        for index in 0..3840 * 2160 {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            if bits == 8 {
                rgba.extend_from_slice(&[
                    seed as u8,
                    (seed >> 8) as u8,
                    (seed >> 16) as u8,
                    [0, 128, 255][index % 3],
                ]);
            } else {
                for channel in [
                    seed as u16,
                    (seed >> 8) as u16,
                    (seed >> 16) as u16,
                    [0, 32768, 65535][index % 3],
                ] {
                    rgba.extend_from_slice(&channel.to_ne_bytes());
                }
            }
        }
        image::codecs::png::PngEncoder::new_with_quality(
            std::fs::File::create(&path).unwrap(),
            image::codecs::png::CompressionType::Fast,
            image::codecs::png::FilterType::NoFilter,
        )
        .write_image(
            &rgba,
            3840,
            2160,
            if bits == 8 {
                ExtendedColorType::Rgba8
            } else {
                ExtendedColorType::Rgba16
            },
        )
        .unwrap();
        drop(rgba);
        let mut before = Vec::new();
        let mut after = Vec::new();
        for sample in 0..12 {
            let mut load = |new: bool| {
                let start = std::time::Instant::now();
                let pixels = std::hint::black_box(if new {
                    load_saved_png(&path).unwrap()
                } else {
                    old_loader(&path)
                });
                let elapsed = start.elapsed();
                assert_eq!(pixels.dimensions(), (3840, 2160));
                if sample >= 2 {
                    if new {
                        after.push(elapsed);
                    } else {
                        before.push(elapsed);
                    }
                }
                pixels
            };
            let first = load(sample % 2 == 1);
            let second = load(sample % 2 == 0);
            assert_eq!(first, second);
        }
        eprintln!("saved PNG RGBA{bits} raw old={before:?}; new={after:?}");
        before.sort_unstable();
        after.sort_unstable();
        let median = |samples: &[std::time::Duration]| (samples[4] + samples[5]) / 2;
        eprintln!(
            "Saved PNG RGBA{bits}; 2 warmups, 10 interleaved samples: old median={:?} p95={:?}; new median={:?} p95={:?}; encoded={} bytes; native={} bytes; final RGBA=33177600 bytes; new read buffer=65536 bytes. Development observation, not hardware acceptance.",
            median(&before),
            before[9],
            median(&after),
            after[9],
            path.metadata().unwrap().len(),
            3840 * 2160 * 4 * bits / 8
        );
    }
}

#[test]
fn saved_png_retained_metadata_is_admitted_before_payload_reads() {
    for kind in [b"tEXt", b"zTXt", b"iTXt", b"eXIf", b"iCCP"] {
        let mut bytes = header(1, 1, 8, 6, 0);
        bytes.extend_from_slice(&(64u32 * 1024 * 1024).to_be_bytes());
        bytes.extend_from_slice(kind);
        // There is deliberately no payload: metadata admission must win over EOF.
        let mut input = Cursor::new(bytes);
        let error = decode_saved_png(&mut input, 4).unwrap_err().to_string();
        assert!(error.contains("metadata exceeds"), "{kind:?}: {error}");
        assert_eq!(input.position(), 41);
    }
}

#[test]
fn saved_png_metadata_payload_and_record_bounds_are_cumulative() {
    let mut bytes = header(1, 1, 8, 6, 0);
    for _ in 0..MAX_SAVED_PNG_METADATA_CHUNKS {
        chunk(&mut bytes, b"tEXt", b"k\0");
    }
    chunk(&mut bytes, b"IDAT", &zlib(&[0, 7, 8, 9, 0]));
    chunk(&mut bytes, b"IEND", &[]);
    assert_eq!(
        decode_saved_png(Cursor::new(&bytes), 4).unwrap().as_raw(),
        &[7, 8, 9, 0]
    );
    bytes.splice(33..33, {
        let mut extra = Vec::new();
        chunk(&mut extra, b"tEXt", b"k\0");
        extra
    });
    assert!(
        decode_saved_png(Cursor::new(bytes), 4)
            .unwrap_err()
            .to_string()
            .contains("4096-chunk")
    );

    let mut payload = vec![0xff; MAX_SAVED_PNG_METADATA_BYTES as usize / 2];
    payload[..2].copy_from_slice(b"k\0");
    let admitted = png(
        1,
        1,
        8,
        6,
        &[0, 11, 22, 33, 128],
        &[(b"tEXt", &payload), (b"tEXt", &payload)],
    );
    assert_eq!(
        decode_saved_png(Cursor::new(&admitted), 4)
            .unwrap()
            .as_raw(),
        &[11, 22, 33, 128]
    );
    let rejected = png(
        1,
        1,
        8,
        6,
        &[0, 11, 22, 33, 128],
        &[(b"tEXt", &payload), (b"tEXt", &payload), (b"tEXt", b"k\0")],
    );
    assert!(
        decode_saved_png(Cursor::new(rejected), 4)
            .unwrap_err()
            .to_string()
            .contains("metadata exceeds")
    );
}

fn saved_png_compressed_metadata() -> Vec<u8> {
    let mut compressor = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    for _ in 0..768 {
        compressor.write_all(&[0; 32 * 1024]).unwrap();
    }
    compressor.finish().unwrap()
}

#[test]
fn saved_png_compressed_icc_and_text_do_not_expand_outside_small_image_scratch() {
    let compressed = saved_png_compressed_metadata();
    let mut payload = b"profile\0\0".to_vec();
    payload.extend_from_slice(&compressed);
    assert!(payload.len() < MAX_SAVED_PNG_METADATA_BYTES as usize);
    for kind in [b"iCCP", b"zTXt"] {
        let bytes = png(1, 1, 8, 6, &[0, 11, 22, 33, 0], &[(kind, &payload)]);
        // png ignores invalid/over-budget ICC payloads; it does not eagerly
        // decompress zTXt. Preserve that historical raster behavior.
        let original = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
            .unwrap()
            .into_rgba8();
        assert_eq!(decode_saved_png(Cursor::new(&bytes), 4).unwrap(), original);
    }
}

#[test]
fn saved_png_guard_preserves_codec_crc_critical_order_and_truncation_policy() {
    let fixture = Fixture::new();
    let valid = png(1, 1, 8, 6, &[0, 11, 22, 33, 128], &[(b"tEXt", b"k\0value")]);
    let mut bad_text_crc = valid.clone();
    bad_text_crc[33 + 8 + b"k\0value".len()] ^= 1;
    let mut bad_ihdr_crc = valid.clone();
    bad_ihdr_crc[29] ^= 1;
    let mut duplicate_ihdr = header(1, 1, 8, 6, 0);
    duplicate_ihdr.extend_from_slice(&valid[8..]);
    let unknown_critical = png(1, 1, 8, 6, &[0, 11, 22, 33, 128], &[(b"ABCD", b"secret")]);
    let bad_text = png(1, 1, 8, 6, &[0, 11, 22, 33, 128], &[(b"tEXt", b"no-null")]);
    for (index, bytes) in [
        valid.clone(),
        bad_text_crc,
        bad_ihdr_crc,
        duplicate_ihdr,
        unknown_critical,
        bad_text,
        valid[..40].to_vec(),
    ]
    .into_iter()
    .enumerate()
    {
        let path = fixture.path(&format!("codec-{index}.png"));
        std::fs::write(&path, &bytes).unwrap();
        let former = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
            .map(image::DynamicImage::into_rgba8);
        let current = load_saved_png(&path);
        assert_eq!(current.is_ok(), former.is_ok(), "case {index}");
        if let (Ok(current), Ok(former)) = (current, former) {
            assert_eq!(current, former, "case {index}");
        }
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn saved_png_subbyte_trns_and_gray_alpha16_keep_pinned_conversion() {
    let cases = [
        png(3, 1, 1, 0, &[0, 0b0100_0000], &[]),
        png(3, 1, 2, 0, &[0, 0b0001_1000], &[(b"tRNS", &[0, 1])]),
        png(3, 1, 4, 0, &[0, 0x08, 0xf0], &[]),
        png(
            3,
            1,
            1,
            3,
            &[0, 0b0100_0000],
            &[(b"PLTE", &[11, 22, 33, 44, 55, 66]), (b"tRNS", &[0, 128])],
        ),
        png(
            2,
            1,
            8,
            2,
            &[0, 11, 22, 33, 44, 55, 66],
            &[(b"tRNS", &[0, 11, 0, 22, 0, 33])],
        ),
        png(1, 1, 16, 0, &[0, 0x12, 0x34], &[(b"tRNS", &[0x12, 0x34])]),
        png(1, 1, 16, 4, &[0, 0x12, 0x34, 0x80, 0x01], &[]),
        png(
            1,
            1,
            16,
            2,
            &[0, 0x12, 0x34, 0x80, 0x01, 0xff, 0xff],
            &[(b"tRNS", &[0x12, 0x34, 0x80, 0x01, 0xff, 0xff])],
        ),
    ];
    for bytes in cases {
        let former = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
            .unwrap()
            .into_rgba8();
        assert_eq!(
            decode_saved_png(Cursor::new(&bytes), crate::MAX_CAPTURE_BYTES).unwrap(),
            former
        );
    }
}

#[test]
fn saved_png_parser_scratch_scales_with_rows_without_lowering_raster_admission() {
    let bytes = png(1, 1, 8, 6, &[0, 11, 22, 33, 0], &[(b"tEXt", b"k\0value")]);
    let (mut guarded, _) = SavedPngMetadataReader::new(Cursor::new(&bytes), 4).unwrap();
    let mut forwarded = Vec::new();
    guarded.read_to_end(&mut forwarded).unwrap();
    assert_eq!(forwarded, bytes);
    assert!(guarded.fill_buf().unwrap().is_empty());
    assert_eq!(guarded.read(&mut []).unwrap(), 0);
    for (width, height) in [
        (1, 1),
        (3840, 2160),
        (crate::MAX_CAPTURE_BYTES as u32 / 4, 1),
        (1, crate::MAX_CAPTURE_BYTES as u32 / 4),
    ] {
        let bytes = header(width, height, 8, 6, 0);
        let (_, scratch) =
            SavedPngMetadataReader::new(Cursor::new(bytes), crate::MAX_CAPTURE_BYTES).unwrap();
        assert_eq!(
            scratch,
            (u64::from(width) * 4 + 1 + SAVED_PNG_FIXED_SCRATCH_BYTES)
                .min(crate::MAX_CAPTURE_BYTES)
        );
    }
    let mut bytes = header(1, 1, 8, 6, 0);
    bytes.extend_from_slice(&((MAX_SAVED_PNG_METADATA_BYTES + 1) as u32).to_be_bytes());
    bytes.extend_from_slice(b"tEXt");
    let (mut guarded, _) = SavedPngMetadataReader::new(Cursor::new(bytes), 4).unwrap();
    let mut initial = [0; 33];
    guarded.read_exact(&mut initial).unwrap();
    assert!(guarded.fill_buf().is_err());
    assert!(guarded.fill_buf().is_err());
    assert!(guarded.seek(SeekFrom::Start(0)).is_err());
    assert_eq!(guarded.seek(SeekFrom::Current(0)).unwrap(), 33);
}

#[test]
#[ignore = "optimized synthetic wide/tall row scratch compatibility; no native activity"]
fn saved_png_wide_and_tall_rasters_larger_than_fixed_scratch_remain_accepted() {
    let fixture = Fixture::new();
    for (width, height) in [(3_000_000, 1), (1, 3_000_000)] {
        let source = RgbaImage::from_fn(width, height, |x, y| Rgba([(x + y) as u8, 2, 3, 128]));
        let path = fixture.path(&format!("{width}-{height}.png"));
        source.save(&path).unwrap();
        assert_eq!(load_saved_png(&path).unwrap(), source);
    }
}

#[test]
fn saved_png_successful_icc_inflation_can_refuse_an_otherwise_admitted_wide_row() {
    let mut compressor = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    for _ in 0..512 {
        compressor.write_all(&[0; 32 * 1024]).unwrap();
    }
    let mut profile = b"profile\0\0".to_vec();
    profile.extend_from_slice(&compressor.finish().unwrap());
    assert!(profile.len() < MAX_SAVED_PNG_METADATA_BYTES as usize);
    let width = 3_000_000;
    let raw = vec![0; width as usize * 4 + 1];
    let bytes = png(width, 1, 8, 6, &raw, &[(b"iCCP", &profile)]);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(crate::MAX_CAPTURE_BYTES);
    let former = image::codecs::png::PngDecoder::with_limits(Cursor::new(&bytes), limits).unwrap();
    let former = image::DynamicImage::from_decoder(former)
        .unwrap()
        .into_rgba8();
    assert_eq!(former.dimensions(), (width, 1));
    assert_eq!(former.get_pixel(width - 1, 0).0, [0; 4]);
    drop(former);
    let error = decode_saved_png(Cursor::new(&bytes), crate::MAX_CAPTURE_BYTES).unwrap_err();
    assert!(
        error.to_string().to_lowercase().contains("limit"),
        "{error:#}"
    );
}
