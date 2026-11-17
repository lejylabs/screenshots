use jiff::Timestamp;
use parking_lot::Mutex;
use std::time::Duration;

pub use web_time::Instant;

pub trait Clock {
    fn utc_now(&self) -> Timestamp;
    fn now(&self) -> Instant;
}

pub struct TestClock(Mutex<TestClockState>);

struct TestClockState {
    now: Instant,
    utc_now: Timestamp,
}

impl TestClock {
    pub fn new() -> Self {
        const START_TIME: &str = "2025-07-01T23:59:58Z";
        let utc_now = START_TIME.parse::<Timestamp>().unwrap();
        Self(Mutex::new(TestClockState {
            now: Instant::now(),
            utc_now,
        }))
    }

    pub fn set_utc_now(&self, now: Timestamp) {
        let mut state = self.0.lock();
        state.utc_now = now;
    }

    pub fn advance(&self, duration: Duration) {
        let mut state = self.0.lock();
        state.now += duration;
        state.utc_now += duration;
    }
}

impl Clock for TestClock {
    fn utc_now(&self) -> Timestamp {
        self.0.lock().utc_now
    }

    fn now(&self) -> Instant {
        self.0.lock().now
    }
}
