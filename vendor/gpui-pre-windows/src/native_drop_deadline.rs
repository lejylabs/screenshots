//! The controller's absolute deadline; progress and nested native work cannot renew it.
use std::time::{Duration, Instant};
const LIMIT: Duration = Duration::from_secs(5);
pub(super) struct Deadline {
    until: Option<Instant>,
    armed: bool,
    interval: u64,
    command: Option<u64>,
}
impl Deadline {
    pub(super) fn startup(now: Instant) -> Self {
        Self {
            until: Some(now + LIMIT),
            armed: false,
            interval: 0,
            command: None,
        }
    }
    pub(super) fn unexpired(&self, now: Instant) -> bool {
        self.until.is_none_or(|at| now < at)
    }
    pub(super) fn may_publish(&self, now: Instant) -> bool {
        self.armed && self.unexpired(now)
    }
    pub(super) fn expires_at(&self) -> Option<Instant> {
        self.until
    }
    pub(super) fn arm(&mut self, sequence: u64, now: Instant) -> bool {
        if !self.unexpired(now) || self.armed || self.interval.checked_add(1) != Some(sequence) {
            return false;
        }
        self.interval = sequence;
        self.armed = true;
        self.until.get_or_insert(now + LIMIT);
        true
    }
    pub(super) fn done(&mut self, sequence: u64, processed: u64, now: Instant) -> bool {
        if !self.unexpired(now) || !self.armed || self.interval != sequence {
            return false;
        }
        self.armed = false;
        if self.command == Some(processed) {
            self.command = None;
        }
        if self.command.is_none() {
            self.until = None;
        }
        true
    }
    pub(super) fn issue(&mut self, command: u64, now: Instant) -> bool {
        if !self.unexpired(now) || self.command.is_some() || command == 0 {
            return false;
        }
        self.command = Some(command);
        self.until.get_or_insert(now + LIMIT);
        true
    }
    pub(super) fn armed(&self) -> bool {
        self.armed
    }
    pub(super) fn pending(&self) -> bool {
        self.command.is_some()
    }
    pub(super) fn milliseconds(&self, now: Instant) -> u32 {
        self.until
            .map(|at| {
                at.saturating_duration_since(now)
                    .as_millis()
                    .min(u32::MAX as u128) as u32
            })
            .unwrap_or(u32::MAX)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_guard_and_progress_never_extend_deadline() {
        let start = Instant::now();
        let mut guard = Deadline::startup(start);
        assert!(guard.arm(1, start + Duration::from_secs(1)));
        assert!(!guard.arm(2, start + Duration::from_secs(4)));
        assert_eq!(guard.milliseconds(start + Duration::from_secs(4)), 1000);
        assert!(guard.done(1, 0, start + Duration::from_secs(4)));
        assert_eq!(guard.milliseconds(start), u32::MAX);
    }
    #[test]
    fn pump_completion_cannot_renew_an_unprocessed_command() {
        let start = Instant::now();
        let mut guard = Deadline::startup(start);
        assert!(guard.arm(1, start));
        assert!(guard.done(1, 0, start));
        assert!(guard.issue(1, start));
        assert!(guard.arm(2, start + Duration::from_secs(1)));
        assert!(guard.done(2, 0, start + Duration::from_secs(2)));
        assert!(guard.pending());
        assert!(guard.arm(3, start + Duration::from_secs(4)));
        assert_eq!(guard.milliseconds(start + Duration::from_secs(5)), 0);
        assert!(!guard.done(3, 1, start + Duration::from_secs(5)));
        assert!(guard.pending());
        assert!(guard.armed());
        assert_eq!(guard.expires_at(), Some(start + LIMIT));
    }
    #[test]
    fn overdue_packet_cannot_grant_permit_or_clear_completion() {
        let start = Instant::now();
        let expired = start + LIMIT;
        let mut startup = Deadline::startup(start);
        assert!(!startup.arm(1, expired));
        assert!(!startup.issue(1, expired));
        assert!(!startup.may_publish(expired));
        assert_eq!(startup.expires_at(), Some(expired));
        let mut active = Deadline::startup(start);
        assert!(active.arm(1, start));
        assert!(!active.may_publish(expired));
        assert!(!active.done(1, 0, expired));
        assert!(active.armed());
        assert_eq!(active.expires_at(), Some(expired));
        assert!(!active.arm(2, expired + LIMIT));
        assert!(!active.done(1, 0, expired + LIMIT));
        assert_eq!(active.expires_at(), Some(expired));
    }
}
