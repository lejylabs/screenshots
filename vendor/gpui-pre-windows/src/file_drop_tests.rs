use super::*;

fn offer(paths: &[&[u16]]) -> Vec<u8> {
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
fn validates_unicode_and_keeps_native_surrogates() {
    assert_eq!(
        validate_drop_files(&offer(&[&[0x0043, 0x003a, 0x005c, 0xd800, 0x6587]])),
        Some(1)
    );
    let paths = [&[b'a' as u16][..]; MAX_DROP_FILES];
    assert_eq!(validate_drop_files(&offer(&paths)), Some(MAX_DROP_FILES));
}

#[test]
fn rejects_whole_offer_at_count_path_and_native_extent_limits() {
    let too_many = [&[b'a' as u16][..]; MAX_DROP_FILES + 1];
    assert_eq!(validate_drop_files(&offer(&too_many)), None);
    let maximum = vec![b'x' as u16; MAX_PATH_BYTES / 2];
    assert_eq!(validate_drop_files(&offer(&[&maximum])), Some(1));
    let oversized = vec![b'x' as u16; MAX_PATH_BYTES / 2 + 1];
    assert_eq!(
        validate_drop_files(&offer(&[&[b'a' as u16], &oversized])),
        None
    );
    // The first path is valid, but there must be no partial acceptance.
    let full = [&maximum[..]; 16];
    assert_eq!(validate_drop_files(&offer(&full)), None);
    let mut exact = offer(&[&[b'a' as u16]]);
    exact.resize(MAX_DROP_BYTES, 0);
    assert_eq!(validate_drop_files(&exact), Some(1));
    exact.push(0);
    assert_eq!(validate_drop_files(&exact), None);
}

#[test]
fn rejects_short_headers_bad_offsets_and_missing_double_terminators() {
    assert_eq!(validate_drop_files(&[]), None);
    assert_eq!(validate_drop_files(&[0; 21]), None);
    assert_eq!(validate_drop_files(&offer(&[])), None);
    for offset in [0u32, 19, 21, u32::MAX] {
        let mut bytes = offer(&[&[b'a' as u16]]);
        bytes[..4].copy_from_slice(&offset.to_le_bytes());
        assert_eq!(validate_drop_files(&bytes), None);
    }
    let mut bytes = offer(&[&[b'a' as u16]]);
    bytes.truncate(bytes.len() - 1);
    assert_eq!(validate_drop_files(&bytes), None);
    bytes.pop();
    assert_eq!(validate_drop_files(&bytes), None);
    let mut unterminated_path = offer(&[&[b'a' as u16]]);
    unterminated_path.truncate(22);
    assert_eq!(validate_drop_files(&unterminated_path), None);
}

#[test]
fn validates_ansi_and_nonzero_wide_flags_without_lossy_conversion() {
    let mut ansi = vec![0; 20];
    ansi[..4].copy_from_slice(&20u32.to_le_bytes());
    ansi.extend_from_slice(b"C:\\private.png\0D:\\next.png\0\0");
    assert_eq!(validate_drop_files(&ansi), Some(2));
    ansi.pop();
    assert_eq!(validate_drop_files(&ansi), None);
    let mut wide = offer(&[&[b'a' as u16]]);
    wide[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(validate_drop_files(&wide), Some(1));
}

#[test]
fn state_refuses_unaccepted_replaced_and_retired_generations() {
    let state = FileDropState::default();
    assert_eq!(state.accepted(), None);
    let first = state.begin().unwrap();
    assert_eq!(state.accepted(), None);
    assert!(state.accept(first));
    assert_eq!(state.accepted(), Some(first));
    let next = state.begin().unwrap();
    assert!(!state.is_current(first));
    assert!(!state.accept(first));
    assert_eq!(state.accepted(), None);
    assert!(state.accept(next));
    let (retired, accepted) = state.retire();
    assert!(accepted);
    assert!(!state.is_current(next));
    assert!(!state.accept(next));
    assert!(state.is_retired(retired));
    // A reentrant Leave must invalidate an outer Drop waiting in a COM helper.
    state.retire();
    assert!(!state.is_retired(retired));
    let third = state.begin().unwrap();
    assert!(!state.is_retired(retired));
    assert!(state.accept(third));
}

#[test]
fn generation_exhaustion_fails_closed_without_aba_reuse() {
    let state = FileDropState::default();
    state.generation.set(Some(u64::MAX - 2));
    let last = state.begin().unwrap();
    assert!(state.accept(last));
    let (retired, accepted) = state.retire();
    assert!(accepted);
    assert!(state.is_retired(retired));
    assert!(!state.accept(last));
    assert_eq!(state.begin(), None);
    assert_eq!(state.accepted(), None);
}

#[test]
fn exhausted_generation_retires_its_last_helper_once() {
    let state = FileDropState::default();
    state.generation.set(Some(u64::MAX));
    state.helper_open.set(true);
    let terminals = Cell::new(0);
    state.finish_leave(
        || panic!("no accepted GPUI payload"),
        || terminals.set(terminals.get() + 1),
    );
    assert_eq!(terminals.get(), 1);
    assert!(!state.helper_is_open());
    state.finish_leave(
        || panic!("no accepted GPUI payload"),
        || panic!("duplicate terminal"),
    );
    assert_eq!(state.begin(), None);
}
