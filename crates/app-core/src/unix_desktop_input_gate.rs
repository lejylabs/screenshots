//! Capture input is retired at admission and every terminal boundary; control commands remain usable.
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
pub(super) const TRANSACTION: u8 = 1;
pub(super) const QUIT: u8 = 2;
pub(super) struct CaptureInputGate {
    epoch: AtomicU64,
    quit_boundary: AtomicU64,
    blockers: AtomicU8,
}
impl CaptureInputGate {
    pub(super) fn new() -> Self {
        Self {
            epoch: AtomicU64::new(1),
            quit_boundary: AtomicU64::new(0),
            blockers: AtomicU8::new(0),
        }
    }
    pub(super) fn new_incarnation(&self) {
        self.blockers.store(0, Ordering::Release);
        let epoch = self.epoch.fetch_add(1, Ordering::AcqRel) + 1;
        self.quit_boundary.store(epoch, Ordering::Release);
    }
    pub(super) fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }
    pub(super) fn set(&self, reason: u8, blocked: bool) {
        let previous = self.blockers.load(Ordering::Acquire);
        if (previous & reason != 0) == blocked {
            return;
        }
        let epoch = if blocked {
            self.blockers.fetch_or(reason, Ordering::AcqRel);
            self.epoch.fetch_add(1, Ordering::AcqRel) + 1
        } else {
            let epoch = self.epoch.fetch_add(1, Ordering::AcqRel) + 1;
            self.blockers.fetch_and(!reason, Ordering::AcqRel);
            epoch
        };
        if reason == QUIT {
            self.quit_boundary.store(epoch, Ordering::Release);
        }
    }
    pub(super) fn accepts_quit(&self, epoch: u64) -> bool {
        epoch >= self.quit_boundary.load(Ordering::Acquire)
    }
    pub(super) fn capture_stamp(&self) -> Option<u64> {
        let epoch = self.epoch();
        (self.blockers.load(Ordering::Acquire) == 0 && self.epoch() == epoch).then_some(epoch)
    }
    pub(super) fn accepts(&self, epoch: u64, control: bool) -> bool {
        control || (self.blockers.load(Ordering::Acquire) == 0 && self.epoch() == epoch)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_prepare_retires_pre_admission_hotkeys_and_menus() {
        let gate = CaptureInputGate::new();
        gate.new_incarnation();
        let queued = gate.capture_stamp().unwrap();
        gate.set(TRANSACTION, true);
        let during = gate.epoch();
        assert!(gate.capture_stamp().is_none());
        let queued_events = [
            ("hotkey", queued, false),
            ("capture menu", during, false),
            ("Show", queued, true),
            ("Quit", during, true),
        ];
        gate.set(TRANSACTION, false);
        let surviving: Vec<_> = queued_events
            .into_iter()
            .filter(|(_, epoch, control)| gate.accepts(*epoch, *control))
            .map(|(kind, _, _)| kind)
            .collect();
        assert_eq!(surviving, ["Show", "Quit"]);
        assert!(!gate.accepts(queued, false));
        assert!(!gate.accepts(during, false));
        assert!(gate.accepts(gate.epoch(), false));
    }
    #[test]
    fn commit_and_rollback_retire_paused_menu_captures_but_keep_show_and_quit() {
        for _commit in [true, false] {
            let gate = CaptureInputGate::new();
            gate.set(TRANSACTION, true);
            let menu = gate.epoch();
            assert!(gate.accepts(menu, true));
            assert!(!gate.accepts(menu, false));
            gate.set(TRANSACTION, false);
            assert!(!gate.accepts(menu, false));
            assert!(gate.accepts(menu, true));
            assert!(gate.accepts_quit(menu));
        }
    }
    #[test]
    fn aborted_quit_does_not_replay_an_older_quit_but_config_changes_keep_control_intent() {
        let gate = CaptureInputGate::new();
        let before = gate.epoch();
        gate.set(TRANSACTION, true);
        gate.set(TRANSACTION, false);
        assert!(gate.accepts_quit(before));
        gate.set(QUIT, true);
        let during = gate.epoch();
        gate.set(QUIT, false);
        assert!(!gate.accepts_quit(before));
        assert!(!gate.accepts_quit(during));
        assert!(gate.accepts_quit(gate.epoch()));
        assert!(gate.accepts(before, true));
    }
    #[test]
    fn quit_and_transaction_pause_nest_without_reopening_capture_input() {
        let gate = CaptureInputGate::new();
        gate.set(TRANSACTION, true);
        gate.set(QUIT, true);
        let before_failure = gate.epoch();
        gate.set(TRANSACTION, false);
        assert!(gate.capture_stamp().is_none());
        gate.set(QUIT, false);
        assert!(!gate.accepts(before_failure, false));
        assert!(gate.accepts(gate.epoch(), false));
    }
}
