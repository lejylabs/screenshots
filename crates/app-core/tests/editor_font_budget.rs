//! Isolated allocation regression for the actual vendored preflight API.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};
#[path = "support/editor_kern_fonts.rs"]
mod fixtures;
thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}
fn count_allocation() {
    if COUNTING.try_with(Cell::get).unwrap_or(false) {
        let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
    }
}
struct Counter;
unsafe impl GlobalAlloc for Counter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count_allocation();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count_allocation();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count_allocation();
        unsafe { System.realloc(ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: Counter = Counter;
fn measured(bytes: &[u8]) -> (Option<usize>, usize) {
    ALLOCATIONS.set(0);
    COUNTING.set(true);
    let result = harfrust::kern_preprocessing_size(bytes, 0);
    COUNTING.set(false);
    (result, ALLOCATIONS.get())
}
#[test]
fn native_parser_preflight_allocates_nothing_for_normal_attack_or_malformed_kern() {
    let body = fixtures::kern_words([1, 6, 0, 0, 4, 115, (-120i16) as u16]);
    let normal = fixtures::font_with_legacy_kern(&fixtures::apple_kern(0, &body, 1));
    let attack = fixtures::font_with_legacy_kern(&fixtures::apple_kern(0, &body, 80_000));
    let mut malformed = fixtures::apple_kern(0, &body, 1);
    malformed[8..12].fill(0);
    let malformed = fixtures::font_with_legacy_kern(&malformed);
    let original = include_bytes!("../../../vendor/cosmic-text/fonts/FiraMono-Medium.ttf");
    assert_eq!(measured(original), (Some(0), 0));
    let (bound, allocations) = measured(&normal);
    assert!(bound.is_some_and(|bytes| bytes > 512 && bytes < 64 * 1024));
    assert_eq!(allocations, 0);
    // Exercise allocation freedom on every admitted legacy format as well.
    let mut state = fixtures::kern_words([4, 8, 128, 136, 0, 116]);
    state.extend(std::iter::repeat_n(1, 116));
    state.extend_from_slice(&[0; 8]);
    state.extend(fixtures::kern_words([128, 0]));
    let classes = fixtures::kern_words([2, 16, 22, 28, 4, 1, 28, 115, 1, 0, (-120i16) as u16]);
    let mut compact = fixtures::kern_words([116]);
    compact.extend_from_slice(&[1, 1, 1, 0]);
    compact.extend(fixtures::kern_words([(-120i16) as u16]));
    compact.extend(std::iter::repeat_n(0, 233));
    for (format, body) in [(1, state), (2, classes), (3, compact)] {
        let font = fixtures::font_with_legacy_kern(&fixtures::apple_kern(format, &body, 1));
        let (bound, allocations) = measured(&font);
        assert!(bound.is_some_and(|bytes| bytes < 64 * 1024));
        assert_eq!(allocations, 0);
    }
    assert_eq!(measured(&attack), (None, 0));
    assert_eq!(measured(&malformed), (None, 0));
}
