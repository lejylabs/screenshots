//! Private HGLOBAL fixtures only; never opens or changes a clipboard/window.
use super::*;
use std::os::windows::ffi::OsStrExt;

fn with_offer<T>(bytes: &[u8], f: impl FnOnce(HGLOBAL) -> T) -> T {
    let memory = unsafe { Owned::new(GlobalAlloc(GMEM_MOVEABLE, bytes.len()).unwrap()) };
    let pointer = unsafe { GlobalLock(*memory) };
    assert!(!pointer.is_null());
    unsafe {
        std::ptr::write_bytes(pointer.cast::<u8>(), 0xff, GlobalSize(*memory));
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.cast::<u8>(), bytes.len());
        GlobalUnlock(*memory).ok();
    }
    f(*memory)
}

fn wide_offer(paths: &[&[u16]]) -> Vec<u8> {
    let mut bytes = vec![0; 20];
    bytes[..4].copy_from_slice(&20u32.to_le_bytes());
    bytes[16..20].copy_from_slice(&1u32.to_le_bytes());
    for path in paths {
        for &unit in *path {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes.extend_from_slice(&[0, 0]);
    }
    bytes.extend_from_slice(&[0, 0]);
    bytes
}

#[test]
fn movable_hglobal_paths_preserve_exact_unicode_units_and_capacity_bounds() {
    let native = [
        b'C' as u16,
        b':' as u16,
        b'\\' as u16,
        0x6587,
        0xd800,
        b'x' as u16,
        0xdfff,
    ];
    with_offer(&wide_offer(&[&native]), |handle| {
        let paths = read_file_paths(handle).unwrap();
        assert_eq!(paths.paths().len(), 1);
        assert_eq!(
            paths.paths()[0]
                .as_os_str()
                .encode_wide()
                .collect::<Vec<_>>(),
            native
        );
        assert!(paths.0.capacity() <= MAX_DROP_FILES);
        assert!(paths.paths()[0].capacity() <= MAX_PATH_CAPACITY_BYTES);
        assert!(
            paths
                .paths()
                .iter()
                .map(|path| path.capacity())
                .sum::<usize>()
                <= MAX_PATHS_CAPACITY_BYTES
        );
        // Reading borrowed CF_HDROP memory must leave ownership with its caller.
        assert!(unsafe { GlobalSize(handle) } != 0);
    });
}

#[test]
fn ansi_hglobal_uses_shell_native_unicode_conversion() {
    let mut bytes = vec![0; 20];
    bytes[..4].copy_from_slice(&20u32.to_le_bytes());
    bytes.extend_from_slice(b"C:\\private.png\0\0");
    with_offer(&bytes, |handle| {
        assert_eq!(
            read_file_paths(handle).unwrap().paths(),
            &[PathBuf::from(r"C:\private.png")]
        );
    });
}

#[test]
fn rejects_private_malformed_and_oversized_handles_without_partial_paths() {
    let too_long = vec![b'a' as u16; MAX_PATH_BYTES / 2 + 1];
    let too_many = [&[b'a' as u16][..]; MAX_DROP_FILES + 1];
    for bytes in [
        wide_offer(&[&[b'a' as u16], &too_long]),
        wide_offer(&too_many),
        vec![0; MAX_DROP_BYTES + 1],
    ] {
        with_offer(&bytes, |handle| assert!(read_file_paths(handle).is_none()));
    }
    let mut missing_end = wide_offer(&[&[b'a' as u16]]);
    missing_end.truncate(missing_end.len() - 2);
    with_offer(&missing_end, |handle| {
        assert!(read_file_paths(handle).is_none())
    });
    assert!(read_file_paths(HGLOBAL::default()).is_none());
}

#[test]
fn native_count_path_and_converted_aggregate_boundaries_are_all_or_nothing() {
    let maximum = vec![b'x' as u16; MAX_PATH_BYTES / 2];
    with_offer(&wide_offer(&[&maximum]), |handle| {
        assert_eq!(read_file_paths(handle).unwrap().paths().len(), 1);
    });
    for length in [1024usize, 1025] {
        let mut ansi = vec![0; 20];
        ansi[..4].copy_from_slice(&20u32.to_le_bytes());
        for _ in 0..MAX_DROP_FILES {
            ansi.extend(std::iter::repeat_n(b'x', length));
            ansi.push(0);
        }
        ansi.push(0);
        with_offer(&ansi, |handle| {
            let paths = read_file_paths(handle);
            if length == 1024 {
                let paths = paths.unwrap();
                assert_eq!(paths.paths().len(), MAX_DROP_FILES);
                assert!(paths.0.capacity() <= MAX_DROP_FILES);
                assert!(
                    paths
                        .paths()
                        .iter()
                        .map(|path| path.capacity())
                        .sum::<usize>()
                        <= MAX_PATHS_CAPACITY_BYTES
                );
            } else {
                assert!(paths.is_none());
            }
        });
    }
}
