//! Tracks OLE's explicit AddRef/Release ownership without borrowing across calls.
use std::cell::Cell;

#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum Phase {
    #[default]
    Absent,
    Registered,
    Revoking,
}

#[derive(Default)]
pub(crate) struct DropRegistration {
    phase: Cell<Phase>,
}

impl DropRegistration {
    pub(crate) fn register<E>(&self, call: impl FnOnce() -> Result<(), E>) -> Result<(), E> {
        debug_assert!(self.phase.get() == Phase::Absent);
        self.phase.set(Phase::Registered);
        let result = call();
        if result.is_err() && self.phase.get() == Phase::Registered {
            self.phase.set(Phase::Absent);
        }
        result
    }

    /// False means an outer revocation still owns the call; destruction must wait.
    pub(crate) fn revoke<E>(&self, call: impl FnOnce() -> Result<(), E>) -> Result<bool, E> {
        match self.phase.get() {
            Phase::Absent => return Ok(true),
            Phase::Revoking => return Ok(false),
            Phase::Registered => {}
        }
        self.phase.set(Phase::Revoking);
        match call() {
            Ok(()) => {
                self.phase.set(Phase::Absent);
                Ok(true)
            }
            Err(error) => {
                self.phase.set(Phase::Registered);
                Err(error)
            }
        }
    }
}

#[cfg(test)]
#[path = "drop_registration_tests.rs"]
mod tests;
