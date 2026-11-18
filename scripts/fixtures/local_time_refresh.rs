// Owned single-thread probe. Never link into the application or normal tests.
fn main() {
    let instant = jiff::Timestamp::new(1_700_000_000, 123_000_000).unwrap();
    for (zone, seconds) in [("AAA0", 0), ("BBB-2", 7200), ("CCC3", -10800), ("AAA0", 0)] {
        // This dedicated process has never created another thread. No parent/system TZ is changed.
        unsafe {
            std::env::set_var("TZ", zone);
        }
        let local = local_time::local_datetime(instant);
        assert_eq!(local.timestamp(), instant);
        assert_eq!(local.offset().seconds(), seconds);
        assert_eq!(local.subsec_nanosecond(), 123_000_000);
    }
    println!("PASS: same-process demanded timezone refresh; instant/fraction unchanged");
}
