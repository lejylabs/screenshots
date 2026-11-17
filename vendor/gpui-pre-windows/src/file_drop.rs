//! Shared admission for OLE and legacy clipboard file metadata.
use std::cell::Cell;
#[path = "drop_registration.rs"]
mod drop_registration;
pub(crate) use drop_registration::DropRegistration;

pub(super) const MAX_DROP_BYTES: usize = 64 * 1024;
pub(super) const MAX_DROP_FILES: usize = 32;
pub(super) const MAX_PATH_BYTES: usize = 4096;
// Windows OsString stores WTF-8 internally; check capacity, not just native length.
pub(super) const MAX_PATH_CAPACITY_BYTES: usize = MAX_PATH_BYTES * 2;
pub(super) const MAX_PATHS_CAPACITY_BYTES: usize = MAX_DROP_BYTES * 3;

/// Validate the entire native list before any Shell enumeration or path allocation.
pub(super) fn validate_drop_files(bytes: &[u8]) -> Option<usize> {
    if !(22..=MAX_DROP_BYTES).contains(&bytes.len()) {
        return None;
    }
    let offset = u32::from_le_bytes(bytes[..4].try_into().ok()?) as usize;
    let wide = u32::from_le_bytes(bytes[16..20].try_into().ok()?) != 0;
    let width = if wide { 2 } else { 1 };
    if offset < 20
        || offset > bytes.len().checked_sub(width * 2)?
        || (wide && !offset.is_multiple_of(2))
    {
        return None;
    }
    let mut cursor = offset;
    let mut path_bytes = 0usize;
    let mut total_bytes = 0usize;
    let mut count = 0usize;
    while let Some(unit) = bytes.get(cursor..cursor.checked_add(width)?) {
        cursor = cursor.checked_add(width)?;
        if unit.iter().all(|&byte| byte == 0) {
            if path_bytes == 0 {
                return (count != 0).then_some(count);
            }
            count = count.checked_add(1)?;
            if count > MAX_DROP_FILES {
                return None;
            }
            path_bytes = 0;
        } else {
            path_bytes = path_bytes.checked_add(width)?;
            total_bytes = total_bytes.checked_add(width)?;
            if path_bytes > MAX_PATH_BYTES || total_bytes > MAX_DROP_BYTES {
                return None;
            }
        }
    }
    None
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum Phase {
    #[default]
    Idle,
    Pending,
    Accepted,
}

/// No borrow is held across source/helper COM calls, which can pump native events.
pub(crate) struct FileDropState {
    generation: Cell<Option<u64>>,
    phase: Cell<Phase>,
    helper_open: Cell<bool>,
    closed: Cell<bool>,
}

impl Default for FileDropState {
    fn default() -> Self {
        Self {
            generation: Cell::new(Some(0)),
            phase: Cell::new(Phase::Idle),
            helper_open: Cell::new(false),
            closed: Cell::new(false),
        }
    }
}

impl FileDropState {
    pub(crate) fn begin(&self) -> Option<u64> {
        if self.closed.get() {
            return None;
        }
        self.phase.set(Phase::Idle);
        let generation = self
            .generation
            .get()
            .and_then(|value| value.checked_add(1))
            .filter(|&value| value < u64::MAX);
        self.generation.set(generation);
        let generation = generation?;
        self.phase.set(Phase::Pending);
        Some(generation)
    }

    pub(crate) fn begin_enter(
        &self,
        exit: impl FnOnce(),
        helper_leave: impl FnOnce(),
    ) -> Option<u64> {
        let previous_accepted = self.accepted().is_some();
        let generation = self.begin();
        if previous_accepted {
            exit();
        }
        let Some(generation) = generation else {
            if self.helper_open.replace(false) {
                helper_leave();
            }
            return None;
        };
        if !self.is_current(generation) {
            return None;
        }
        if self.helper_open.replace(false) {
            helper_leave();
        }
        self.is_current(generation).then_some(generation)
    }

    pub(crate) fn helper_enter(&self, generation: u64) -> bool {
        if !self.is_current(generation) {
            return false;
        }
        self.helper_open.set(true);
        true
    }

    pub(crate) fn helper_is_open(&self) -> bool {
        self.helper_open.get()
    }

    pub(crate) fn finish_leave(&self, exit: impl FnOnce(), helper_leave: impl FnOnce()) {
        if self.closed.get() {
            return;
        }
        let (generation, accepted) = self.retire();
        if accepted {
            exit();
        }
        // Exhaustion is permanent, so it may still release its last helper lease.
        if (self.is_retired(generation) || self.generation.get().is_none())
            && self.helper_open.replace(false)
        {
            helper_leave();
        }
    }

    pub(crate) fn finish_drop(
        &self,
        helper_drop: impl FnOnce(bool),
        complete: impl FnOnce(bool) -> bool,
    ) -> bool {
        if self.closed.get() {
            return false;
        }
        // Keep Accepted visible until the helper returns: nested Enter must clear
        // its GPUI payload before installing another generation's paths.
        let generation = self.generation.get();
        let accepted = self.accepted().is_some();
        if self.helper_open.replace(false) {
            helper_drop(accepted);
        }
        if generation.is_none() || self.generation.get() != generation {
            return false;
        }
        let (retired, accepted) = self.retire();
        complete(accepted) && self.is_retired(retired)
    }

    /// Claims the last helper Leave for the window's existing teardown task.
    pub(crate) fn close(&self) -> Option<bool> {
        if self.closed.replace(true) {
            return None;
        }
        self.retire();
        Some(self.helper_open.replace(false))
    }

    pub(crate) fn is_current(&self, generation: u64) -> bool {
        !self.closed.get()
            && self.generation.get() == Some(generation)
            && self.phase.get() != Phase::Idle
    }

    pub(crate) fn accept(&self, generation: u64) -> bool {
        if !self.is_current(generation) {
            return false;
        }
        self.phase.set(Phase::Accepted);
        true
    }

    pub(crate) fn accepted(&self) -> Option<u64> {
        (!self.closed.get() && self.phase.get() == Phase::Accepted)
            .then(|| self.generation.get())
            .flatten()
    }

    pub(crate) fn retire(&self) -> (Option<u64>, bool) {
        let accepted = self.phase.replace(Phase::Idle) == Phase::Accepted;
        let generation = self.generation.get().and_then(|value| value.checked_add(1));
        self.generation.set(generation);
        (generation, accepted)
    }

    pub(crate) fn is_retired(&self, generation: Option<u64>) -> bool {
        generation.is_some()
            && self.generation.get() == generation
            && self.phase.get() == Phase::Idle
    }
}

#[cfg(test)]
#[path = "file_drop_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "file_drop_helper_lifetime_tests.rs"]
mod helper_lifetime_tests;
