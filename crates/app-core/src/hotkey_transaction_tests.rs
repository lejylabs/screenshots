//! Pure registration bookkeeping. No native manager, desktop, file, capture or clipboard.
use super::*;
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
};
#[derive(Default)]
struct Registrar {
    held: RefCell<HashSet<HotKey>>,
    conflict: Cell<Option<HotKey>>,
    refuse_release: Cell<Option<HotKey>>,
}
impl ShortcutManager for Registrar {
    fn register(&self, key: HotKey) -> Result<()> {
        ensure!(self.conflict.get() != Some(key), "fixture conflict");
        ensure!(self.held.borrow_mut().insert(key), "duplicate registration");
        Ok(())
    }
    fn unregister(&self, key: HotKey) -> Result<()> {
        ensure!(
            self.refuse_release.get() != Some(key),
            "fixture cleanup failure"
        );
        ensure!(self.held.borrow_mut().remove(&key), "unknown key");
        Ok(())
    }
}
fn settings(keys: &[&str]) -> crate::CaptureHotkeys {
    crate::CaptureHotkeys(std::array::from_fn(|i| {
        keys.get(i).unwrap_or(&"").to_string()
    }))
}
#[test]
fn startup_uses_only_configured_bindings_and_reports_conflicts() -> Result<()> {
    let mut keys = CaptureHotkey::with_manager(Registrar::default());
    keys.manager.conflict.set(Some("Ctrl+Alt+F8".parse()?));
    keys.register_settings(&settings(&[
        "",
        "Ctrl+Alt+F8",
        "Ctrl+Alt+F9",
        "",
        "",
        "Ctrl+Alt+F10",
    ]))?;
    assert!(keys.is_registered("Ctrl+Alt+F9", CaptureCommand::EditRegion));
    assert!(keys.is_registered("Ctrl+Alt+F10", CaptureCommand::ActiveDisplay));
    assert!(keys.status().contains("unavailable"));
    assert!(!keys.is_registered("Ctrl+Shift+S", CaptureCommand::Primary));
    assert_eq!(keys.manager.held.borrow().len(), 2);
    Ok(())
}
#[test]
fn rollback_releases_only_new_reservations_and_never_routes_them() -> Result<()> {
    let mut keys = CaptureHotkey::with_manager(Registrar::default());
    keys.register_settings(&settings(&["Ctrl+Alt+F8"]))?;
    keys.prepare(&settings(&["Ctrl+Alt+F9"]))?;
    assert!(keys.is_registered("Ctrl+Alt+F8", CaptureCommand::Primary));
    let reserved: HotKey = "Ctrl+Alt+F9".parse()?;
    assert_eq!(
        keys.command(GlobalHotKeyEvent {
            id: reserved.id(),
            state: HotKeyState::Pressed
        }),
        None
    );
    keys.finish(false)?;
    assert_eq!(keys.manager.held.borrow().len(), 1);
    assert!(keys.is_registered("Ctrl+Alt+F8", CaptureCommand::Primary));
    Ok(())
}
#[test]
fn second_reservation_conflict_rolls_back_the_first_and_preserves_old_routes() -> Result<()> {
    let mut keys = CaptureHotkey::with_manager(Registrar::default());
    keys.register_settings(&settings(&["Ctrl+Alt+F8"]))?;
    keys.manager.conflict.set(Some("Ctrl+Alt+F10".parse()?));
    assert!(
        keys.prepare(&settings(&["Ctrl+Alt+F9", "Ctrl+Alt+F10"]))
            .is_err()
    );
    assert_eq!(keys.manager.held.borrow().len(), 1);
    assert!(keys.is_registered("Ctrl+Alt+F8", CaptureCommand::Primary));
    assert!(keys.pending.is_empty());
    Ok(())
}
#[test]
fn commit_reuses_native_reservations_when_commands_are_swapped() -> Result<()> {
    let mut keys = CaptureHotkey::with_manager(Registrar::default());
    keys.register_settings(&settings(&["Ctrl+Alt+F8", "Ctrl+Alt+F9"]))?;
    keys.prepare(&settings(&["Ctrl+Alt+F9", "Ctrl+Alt+F8"]))?;
    assert_eq!(keys.manager.held.borrow().len(), 2);
    assert!(keys.is_registered("Ctrl+Alt+F8", CaptureCommand::Primary));
    keys.finish(true)?;
    assert!(keys.is_registered("Ctrl+Alt+F8", CaptureCommand::Region));
    assert!(keys.is_registered("Ctrl+Alt+F9", CaptureCommand::Primary));
    keys.prepare(&settings(&[]))?;
    keys.finish(true)?;
    assert!(keys.manager.held.borrow().is_empty());
    Ok(())
}
#[test]
fn cleanup_failure_is_truthful_and_blocks_further_changes() -> Result<()> {
    let mut keys = CaptureHotkey::with_manager(Registrar::default());
    keys.register_settings(&settings(&["Ctrl+Alt+F8"]))?;
    keys.prepare(&settings(&["Ctrl+Alt+F9"]))?;
    keys.manager
        .refuse_release
        .set(Some("Ctrl+Alt+F9".parse()?));
    assert!(keys.finish(false).is_err());
    assert!(keys.status().contains("Could not release reserved"));
    assert!(keys.prepare(&settings(&["Ctrl+Alt+F10"])).is_err());
    assert!(keys.is_registered("Ctrl+Alt+F8", CaptureCommand::Primary));
    keys.manager.refuse_release.set(None);
    Ok(())
}
#[test]
fn malformed_configuration_reaches_no_native_registration() {
    let mut keys = CaptureHotkey::with_manager(Registrar::default());
    assert!(keys.register_settings(&settings(&["Q"])).is_err());
    assert!(keys.manager.held.borrow().is_empty());
}
