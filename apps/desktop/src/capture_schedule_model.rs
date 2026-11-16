//! Pure, bounded UTC recurrence decisions. No clock reads or side effects.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

pub(super) const MAX_UTC_MS: i64 = 253402300799999;
pub(super) const MAX_SCHEDULES: usize = 32;
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Missed {
    Skip,
    Once,
    Pause,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Spec {
    pub start_ms: i64,
    pub interval_ms: Option<u64>,
    pub count: Option<u64>,
    pub until_ms: Option<i64>,
    pub missed: Missed,
    pub grace_ms: u64,
    pub wake: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Decision {
    Complete,
    Wait(i64),
    Pause,
    Advance(u64),
    Run {
        ordinal: u64,
        due_ms: i64,
        next: u64,
    },
}
pub(super) fn valid_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
impl Spec {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (0..=MAX_UTC_MS).contains(&self.start_ms),
            "Schedule start is outside 1970–9999 UTC"
        );
        ensure!(self.grace_ms <= 60000, "Schedule grace must be 0–60000 ms");
        ensure!(
            self.count.is_none_or(|n| (1..=1000000).contains(&n)),
            "Schedule count must be 1–1000000"
        );
        ensure!(
            self.interval_ms
                .is_none_or(|n| (100..=31536000000).contains(&n)),
            "Schedule interval must be 100–31536000000 ms"
        );
        ensure!(
            self.interval_ms.is_some() || self.count == Some(1),
            "A single occurrence requires count 1"
        );
        ensure!(
            self.until_ms
                .is_none_or(|n| (self.start_ms..=MAX_UTC_MS).contains(&n)),
            "Schedule until must be at or after its start, within 1970–9999 UTC"
        );
        Ok(())
    }
    pub fn due(&self, ordinal: u64) -> Option<i64> {
        if self.count.is_some_and(|n| ordinal >= n) || (self.interval_ms.is_none() && ordinal != 0)
        {
            return None;
        }
        let due = i128::from(self.start_ms)
            + i128::from(ordinal) * i128::from(self.interval_ms.unwrap_or(0));
        if due > i128::from(self.until_ms.unwrap_or(MAX_UTC_MS)) {
            return None;
        }
        i64::try_from(due).ok()
    }
    /// Explicit resume discards elapsed slots without running or reauthorizing a Task.
    pub fn resume_after(&self, next: u64, now_ms: i64) -> u64 {
        let mut skip = self.clone();
        skip.missed = Missed::Skip;
        skip.grace_ms = 0;
        match skip.decide(next, now_ms.saturating_add(1)) {
            Decision::Advance(n) => n,
            _ => next,
        }
    }
    pub fn decide(&self, next: u64, now_ms: i64) -> Decision {
        let Some(due_ms) = self.due(next) else {
            return Decision::Complete;
        };
        if now_ms < due_ms {
            return Decision::Wait(due_ms);
        }
        if i128::from(now_ms) - i128::from(due_ms) <= i128::from(self.grace_ms) {
            return Decision::Run {
                ordinal: next,
                due_ms,
                next: next + 1,
            };
        }
        if self.missed == Missed::Pause {
            return Decision::Pause;
        }
        // Arithmetic skips any number of elapsed slots in O(1), including years asleep.
        let last = self
            .interval_ms
            .map_or(next, |interval| {
                let elapsed =
                    (i128::from(now_ms) - i128::from(self.start_ms)) / i128::from(interval);
                u64::try_from(elapsed)
                    .unwrap_or(u64::MAX)
                    .min(self.count.map_or(u64::MAX, |n| n - 1))
                    .min(((self.until_ms.unwrap_or(MAX_UTC_MS) - self.start_ms) as u64) / interval)
            })
            .max(next);
        match self.missed {
            Missed::Skip => Decision::Advance(last.saturating_add(1)),
            Missed::Once => {
                // No execution outside count/until boundaries; coalesce only an actual slot.
                let Some(due_ms) = self.due(last) else {
                    return Decision::Complete;
                };
                Decision::Run {
                    ordinal: last,
                    due_ms,
                    next: last + 1,
                }
            }
            Missed::Pause => unreachable!(),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn spec(missed: Missed) -> Spec {
        Spec {
            start_ms: 10000,
            interval_ms: Some(100),
            count: Some(10),
            until_ms: None,
            missed,
            grace_ms: 5,
            wake: false,
        }
    }
    #[test]
    fn anchored_missed_rules_have_no_backlog() {
        assert_eq!(spec(Missed::Skip).decide(0, 10450), Decision::Advance(5));
        assert_eq!(
            spec(Missed::Once).decide(0, 10450),
            Decision::Run {
                ordinal: 4,
                due_ms: 10400,
                next: 5
            }
        );
        assert_eq!(spec(Missed::Pause).decide(0, 10450), Decision::Pause);
        assert_eq!(
            spec(Missed::Once).decide(0, 10005),
            Decision::Run {
                ordinal: 0,
                due_ms: 10000,
                next: 1
            }
        );
        assert_eq!(spec(Missed::Once).decide(5, 9900), Decision::Wait(10500));
        assert_eq!(spec(Missed::Once).decide(10, 11000), Decision::Complete);
    }
    #[test]
    fn finite_until_and_huge_forward_change_stay_bounded() {
        let mut s = spec(Missed::Once);
        s.until_ms = Some(10250);
        assert_eq!(
            s.decide(0, MAX_UTC_MS),
            Decision::Run {
                ordinal: 2,
                due_ms: 10200,
                next: 3
            }
        );
        assert_eq!(s.decide(3, MAX_UTC_MS), Decision::Complete);
        s.count = None;
        s.until_ms = None;
        let Decision::Run { ordinal, next, .. } = s.decide(0, MAX_UTC_MS) else {
            panic!()
        };
        assert_eq!(next, ordinal + 1);
        assert_eq!(s.decide(next, MAX_UTC_MS), Decision::Complete);
    }
    #[test]
    fn explicit_resume_consumes_only_elapsed_slots_without_changing_anchor() {
        let s = spec(Missed::Pause);
        assert_eq!(s.resume_after(0, 10400), 5);
        assert_eq!(s.due(5), Some(10500));
        assert_eq!(s.resume_after(5, 10300), 5);
        assert_eq!(s.resume_after(0, 20000), 10);
    }
    #[test]
    fn strict_admission_and_single_occurrence() {
        let mut s = spec(Missed::Skip);
        s.interval_ms = None;
        s.count = Some(1);
        s.validate().unwrap();
        assert_eq!(s.decide(0, 10006), Decision::Advance(1));
        assert_eq!(s.due(1), None);
        s.count = None;
        assert!(s.validate().is_err());
        assert!(!valid_id("../escape"));
        assert!(!valid_id(""));
        assert!(valid_id("daily_1"));
    }
}
