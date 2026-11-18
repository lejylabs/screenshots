//! Opt-in isolated allocator probe. Counts requested live Rust heap bytes, not RSS.
//! Run this target alone with --test-threads=1; no clipboard/capture/native I/O.

use image::ImageEncoder;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    io::{BufReader, Write},
    sync::atomic::{AtomicUsize, Ordering::SeqCst},
};

struct CountedSystem;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn acquired(bytes: usize) {
    let live = LIVE.fetch_add(bytes, SeqCst) + bytes;
    PEAK.fetch_max(live, SeqCst);
}

// The System allocator retains its allocation/deallocation layout contracts.
// Instrumentation never allocates and records only successful allocations.
unsafe impl GlobalAlloc for CountedSystem {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            acquired(layout.size());
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            acquired(layout.size());
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe {
            System.dealloc(pointer, layout);
        }
        LIVE.fetch_sub(layout.size(), SeqCst);
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let pointer = unsafe { System.realloc(pointer, layout, new_size) };
        if !pointer.is_null() {
            if new_size >= layout.size() {
                acquired(new_size - layout.size());
            } else {
                LIVE.fetch_sub(layout.size() - new_size, SeqCst);
            }
        }
        pointer
    }
}

#[global_allocator]
static ALLOCATOR: CountedSystem = CountedSystem;

fn measure<T>(work: impl FnOnce() -> T) -> (T, usize) {
    let baseline = LIVE.load(SeqCst);
    PEAK.store(baseline, SeqCst);
    let result = work();
    (result, PEAK.load(SeqCst).saturating_sub(baseline))
}

fn former(path: &std::path::Path) -> image::RgbaImage {
    let mut reader = image::ImageReader::with_format(
        BufReader::with_capacity(64 * 1024, std::fs::File::open(path).unwrap()),
        image::ImageFormat::Png,
    );
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().unwrap().into_rgba8()
}

#[test]
#[ignore = "optimized isolated synthetic allocator proof; no native activity"]
fn saved_png_metadata_live_heap_before_after() {
    let directory =
        std::env::temp_dir().join(format!("screenshots-png-heap-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(directory.clone());
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&[11, 22, 33, 0], 1, 1, image::ExtendedColorType::Rgba8)
        .unwrap();

    let raw_path = directory.join("large-raw-text.png");
    {
        let mut output = std::fs::File::create(&raw_path).unwrap();
        output.write_all(&png[..33]).unwrap();
        let payload_bytes = 64usize * 1024 * 1024;
        output
            .write_all(&(payload_bytes as u32).to_be_bytes())
            .unwrap();
        output.write_all(b"tEXt").unwrap();
        let mut crc = crc32fast::Hasher::new();
        crc.update(b"tEXt");
        crc.update(b"k\0");
        output.write_all(b"k\0").unwrap();
        let block = [b'x'; 64 * 1024];
        let mut remaining = payload_bytes - 2;
        while remaining > 0 {
            let count = remaining.min(block.len());
            output.write_all(&block[..count]).unwrap();
            crc.update(&block[..count]);
            remaining -= count;
        }
        output.write_all(&crc.finalize().to_be_bytes()).unwrap();
        output.write_all(&png[33..]).unwrap();
    }
    let encoded = raw_path.metadata().unwrap().len();
    assert!((32 * 1024 * 1024..272 * 1024 * 1024).contains(&encoded));
    let (old, old_peak) = measure(|| former(&raw_path));
    assert_eq!(old.as_raw(), &[11, 22, 33, 0]);
    drop(old);
    let (new, new_peak) = measure(|| app_core::load_saved_png(&raw_path));
    assert!(new.unwrap_err().to_string().contains("metadata exceeds"));
    assert!(
        old_peak >= 120 * 1024 * 1024,
        "old raw-text peak={old_peak}"
    );
    assert!(new_peak < 32 * 1024 * 1024, "new raw-text peak={new_peak}");
    eprintln!(
        "synthetic raw text: encoded_bytes={encoded}; former_live_heap_peak={old_peak}; current_live_heap_peak={new_peak}; current refuses metadata before payload"
    );

    let icc_path = directory.join("compressed-icc.png");
    {
        let mut compressor =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        for _ in 0..768 {
            compressor.write_all(&[0; 32 * 1024]).unwrap();
        }
        let compressed = compressor.finish().unwrap();
        let mut payload = b"profile\0\0".to_vec();
        payload.extend_from_slice(&compressed);
        assert!(payload.len() < 1024 * 1024);
        let mut output = std::fs::File::create(&icc_path).unwrap();
        output.write_all(&png[..33]).unwrap();
        output
            .write_all(&(payload.len() as u32).to_be_bytes())
            .unwrap();
        output.write_all(b"iCCP").unwrap();
        output.write_all(&payload).unwrap();
        let mut crc = crc32fast::Hasher::new();
        crc.update(b"iCCP");
        crc.update(&payload);
        output.write_all(&crc.finalize().to_be_bytes()).unwrap();
        output.write_all(&png[33..]).unwrap();
    }
    let (old, old_peak) = measure(|| former(&icc_path));
    let (new, new_peak) = measure(|| app_core::load_saved_png(&icc_path).unwrap());
    assert_eq!(new, old);
    assert!(new_peak < 32 * 1024 * 1024, "new ICC peak={new_peak}");
    assert!(new_peak < old_peak, "ICC old={old_peak} new={new_peak}");
    eprintln!(
        "synthetic 24 MiB expanded ICC: former_live_heap_peak={old_peak}; current_live_heap_peak={new_peak}; exact default still pixels match"
    );
}
