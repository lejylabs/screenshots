//! Native global shortcuts: construct, change and drop on the native event-loop thread.
use anyhow::{Result, ensure};
use global_hotkey::{
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
    hotkey::{HotKey, Modifiers},
};

pub struct CaptureHotkey<M: ShortcutManager = GlobalHotKeyManager> {
    manager: M,
    keys: Vec<(HotKey, CaptureCommand)>,
    status: String,
    pending: Vec<(HotKey, CaptureCommand)>,
    prepared: Option<Vec<(HotKey, CaptureCommand)>>,
    cleanup_failed: bool,
    #[cfg(target_os = "macos")]
    _native_thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

pub use crate::capture_command::CaptureCommand;

fn shortcut_label(key: HotKey) -> String {
    let name = key.key.to_string();
    let name = name
        .strip_prefix("Key")
        .or_else(|| name.strip_prefix("Digit"))
        .unwrap_or(&name);
    let mut parts: Vec<&str> = [
        (Modifiers::CONTROL, "Ctrl"),
        (Modifiers::ALT, "Alt"),
        (Modifiers::SHIFT, "Shift"),
        (Modifiers::META, "Super"),
    ]
    .into_iter()
    .filter_map(|(modifier, label)| key.mods.contains(modifier).then_some(label))
    .collect();
    parts.push(name);
    parts.join("+")
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
impl CaptureHotkey<GlobalHotKeyManager> {
    pub fn unregistered() -> Result<Self> {
        #[cfg(target_os = "macos")]
        ensure!(
            objc2_foundation::MainThreadMarker::new().is_some(),
            "Carbon shortcuts require the Cocoa main thread"
        );
        Ok(Self {
            manager: GlobalHotKeyManager::new()?,
            keys: Vec::new(),
            status: "Waiting for settings; no global shortcuts active".into(),
            pending: Vec::new(),
            prepared: None,
            cleanup_failed: false,
            #[cfg(target_os = "macos")]
            _native_thread: std::marker::PhantomData,
        })
    }

    pub fn register() -> Result<Self> {
        let mut manager = Self::unregistered()?;
        manager.register_settings(&crate::CaptureHotkeys::default())?;
        Ok(manager)
    }
}

/// Registration and cleanup always run on the manager's native owner thread.
#[doc(hidden)]
pub trait ShortcutManager {
    fn register(&self, key: HotKey) -> Result<()>;
    fn unregister(&self, key: HotKey) -> Result<()>;
}

impl ShortcutManager for GlobalHotKeyManager {
    fn register(&self, key: HotKey) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            let _ = key;
            anyhow::bail!("Use the fd-driven X11 or native Wayland portal service")
        }
        #[cfg(not(target_os = "linux"))]
        {
            Ok(GlobalHotKeyManager::register(self, key)?)
        }
    }
    fn unregister(&self, key: HotKey) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            let _ = key;
            anyhow::bail!("Polling hotkey backend is disabled")
        }
        #[cfg(not(target_os = "linux"))]
        {
            Ok(GlobalHotKeyManager::unregister(self, key)?)
        }
    }
}

impl<M: ShortcutManager> CaptureHotkey<M> {
    pub(crate) fn with_manager(manager: M) -> Self {
        Self {
            manager,
            keys: Vec::new(),
            status: "Waiting for settings; no global shortcuts active".into(),
            pending: Vec::new(),
            prepared: None,
            cleanup_failed: false,
            #[cfg(target_os = "macos")]
            _native_thread: std::marker::PhantomData,
        }
    }
    pub(crate) fn manager(&self) -> &M {
        &self.manager
    }
    #[cfg(target_os = "macos")]
    pub(crate) fn registered_ids(&self) -> Vec<u32> {
        self.keys.iter().map(|(key, _)| key.id()).collect()
    }
    /// Startup admits each configured command independently; never substitutes defaults.
    pub fn register_settings(&mut self, settings: &crate::CaptureHotkeys) -> Result<()> {
        ensure!(
            self.keys.is_empty() && self.pending.is_empty() && self.prepared.is_none(),
            "Shortcuts already initialized"
        );
        let settings = settings.validated()?;
        let mut status = Vec::new();
        for (index, command) in [
            CaptureCommand::Primary,
            CaptureCommand::Region,
            CaptureCommand::EditRegion,
            CaptureCommand::ActiveWindow,
            CaptureCommand::RepeatRegion,
            CaptureCommand::ActiveDisplay,
        ]
        .into_iter()
        .enumerate()
        {
            let value = &settings.0[index];
            let label = crate::CaptureHotkeys::LABELS[index];
            if value.is_empty() {
                status.push(format!("{label}: disabled"));
                continue;
            }
            let key: HotKey = value.parse()?;
            match self.manager.register(key) {
                Ok(()) => {
                    self.keys.push((key, command));
                    status.push(format!("{value}: {label}"));
                }
                Err(error) => status.push(format!("{label}: {value} unavailable: {error}")),
            }
        }
        self.status = status.join(" | ");
        Ok(())
    }
    /// Reserve new keys while retaining the working bindings until settings are saved.
    pub fn prepare(&mut self, settings: &crate::CaptureHotkeys) -> Result<()> {
        ensure!(
            !self.cleanup_failed && self.prepared.is_none() && self.pending.is_empty(),
            "Shortcut change or cleanup is pending; restart to retry"
        );
        let settings = settings.validated()?;
        let mut desired = Vec::new();
        for (value, command) in settings.0.iter().zip([
            CaptureCommand::Primary,
            CaptureCommand::Region,
            CaptureCommand::EditRegion,
            CaptureCommand::ActiveWindow,
            CaptureCommand::RepeatRegion,
            CaptureCommand::ActiveDisplay,
        ]) {
            if !value.is_empty() {
                desired.push((value.parse::<HotKey>()?, command));
            }
        }
        for &(key, command) in &desired {
            if self.keys.iter().any(|(active, _)| *active == key) {
                continue;
            }
            if let Err(error) = self.manager.register(key) {
                let cleanup = self
                    .finish(false)
                    .err()
                    .map(|error| format!(" Could not release new reservations: {error}"))
                    .unwrap_or_default();
                anyhow::bail!(
                    "{}: {} unavailable: {error}.{cleanup}",
                    command.label(),
                    shortcut_label(key)
                );
            }
            self.pending.push((key, command));
        }
        self.prepared = Some(desired);
        Ok(())
    }

    pub fn finish(&mut self, commit: bool) -> Result<()> {
        let mut errors = Vec::new();
        if commit {
            let desired = self
                .prepared
                .take()
                .ok_or_else(|| anyhow::anyhow!("No prepared shortcuts"))?;
            self.keys.append(&mut self.pending);
            self.keys.retain(|(key, _)| {
                if desired.iter().any(|(wanted, _)| wanted == key) {
                    return true;
                }
                match self.manager.unregister(*key) {
                    Ok(()) => false,
                    Err(error) => {
                        errors.push(format!("Could not release {key}: {error}"));
                        true
                    }
                }
            });
            for (key, command) in &mut self.keys {
                if let Some((_, wanted)) = desired.iter().find(|(wanted, _)| wanted == key) {
                    *command = *wanted;
                }
            }
        } else {
            self.prepared = None;
            self.pending
                .retain(|(key, _)| match self.manager.unregister(*key) {
                    Ok(()) => false,
                    Err(error) => {
                        errors.push(format!("Could not release reserved {key}: {error}"));
                        true
                    }
                });
        }
        self.status = self
            .keys
            .iter()
            .map(|(key, command)| format!("{}: {}", shortcut_label(*key), command.label()))
            .collect::<Vec<_>>()
            .join(" | ");
        if self.keys.is_empty() {
            self.status = "All global shortcuts disabled".into();
        }
        if !errors.is_empty() {
            self.cleanup_failed = true;
            self.status.push_str(&format!(" | {}", errors.join("; ")));
        }
        ensure!(errors.is_empty(), "{}", errors.join("; "));
        Ok(())
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn is_registered(&self, shortcut: &str, command: CaptureCommand) -> bool {
        shortcut
            .parse::<HotKey>()
            .is_ok_and(|key| self.keys.contains(&(key, command)))
    }

    pub fn take_commands(&self) -> Vec<CaptureCommand> {
        GlobalHotKeyEvent::receiver()
            .try_iter()
            .filter_map(|event| command_for_event(&self.keys, event))
            .collect()
    }

    pub fn command(&self, event: GlobalHotKeyEvent) -> Option<CaptureCommand> {
        command_for_event(&self.keys, event)
    }
}

fn command_for_event(
    keys: &[(HotKey, CaptureCommand)],
    event: GlobalHotKeyEvent,
) -> Option<CaptureCommand> {
    if event.state != HotKeyState::Pressed {
        return None;
    }
    keys.iter()
        .find(|(key, _)| key.id() == event.id)
        .map(|(_, command)| *command)
}

impl<M: ShortcutManager> Drop for CaptureHotkey<M> {
    fn drop(&mut self) {
        for (key, _) in self.keys.iter().chain(&self.pending) {
            if let Err(error) = self.manager.unregister(*key) {
                tracing::warn!(%error, "failed to unregister capture hotkey");
            }
        }
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use global_hotkey::hotkey::Code;

    #[test]
    fn admitted_shortcuts_parse_with_the_native_backend() -> Result<()> {
        for key in ["Q", "0", "F1", "F24", "PrintScreen"] {
            for modifier in ["Ctrl", "Alt", "Super", "Ctrl+Alt+Shift+Super"] {
                let bindings = crate::CaptureHotkeys([
                    format!("{modifier}+{key}"),
                    "".into(),
                    "".into(),
                    "".into(),
                    "".into(),
                    String::new(),
                ])
                .validated()?;
                assert!(bindings.0[0].parse::<HotKey>().is_ok(), "{}", bindings.0[0]);
                assert_eq!(shortcut_label(bindings.0[0].parse()?), bindings.0[0]);
            }
        }
        Ok(())
    }

    #[test]
    #[ignore = "registers real Windows shortcuts; run on an interactive desktop"]
    fn changing_shortcuts_preserves_working_keys_until_commit() -> Result<()> {
        let manager = GlobalHotKeyManager::new()?;
        let mut hotkeys = CaptureHotkey {
            manager,
            keys: Vec::new(),
            status: String::new(),
            pending: Vec::new(),
            prepared: None,
            cleanup_failed: false,
        };
        let first = crate::CaptureHotkeys([
            "Ctrl+Alt+F20".into(),
            "".into(),
            "".into(),
            "".into(),
            "".into(),
            String::new(),
        ]);
        let second = crate::CaptureHotkeys([
            "Ctrl+Alt+F21".into(),
            "".into(),
            "".into(),
            "".into(),
            "".into(),
            String::new(),
        ]);
        hotkeys.prepare(&first)?;
        assert!(hotkeys.keys.is_empty());
        hotkeys.finish(true)?;
        let original = hotkeys.keys.clone();
        hotkeys.prepare(&second)?;
        assert_eq!(hotkeys.keys, original);
        let staged: HotKey = second.0[0].parse()?;
        assert_eq!(
            hotkeys.command(GlobalHotKeyEvent {
                id: staged.id(),
                state: HotKeyState::Pressed
            }),
            None
        );
        hotkeys.finish(false)?; // A failed settings save must keep the previous keys.
        assert_eq!(hotkeys.keys, original);
        assert!(hotkeys.pending.is_empty());

        let conflict: HotKey = "Ctrl+Alt+F22".parse()?;
        let blocker = GlobalHotKeyManager::new()?;
        blocker.register(conflict)?;
        let desired = crate::CaptureHotkeys([
            second.0[0].clone(),
            "Ctrl+Alt+F22".into(),
            "".into(),
            "".into(),
            "".into(),
            String::new(),
        ]);
        assert!(hotkeys.prepare(&desired).is_err());
        assert_eq!(hotkeys.keys, original);
        assert!(hotkeys.pending.is_empty());
        blocker.unregister(conflict)?;
        hotkeys.prepare(&second)?;
        hotkeys.finish(true)?;
        assert_ne!(hotkeys.keys, original);
        assert!(hotkeys.is_registered(&second.0[0], CaptureCommand::Primary));
        let swapped = crate::CaptureHotkeys([
            "".into(),
            second.0[0].clone(),
            "".into(),
            "".into(),
            "".into(),
            String::new(),
        ]);
        hotkeys.prepare(&swapped)?;
        assert!(hotkeys.is_registered(&second.0[0], CaptureCommand::Primary));
        hotkeys.finish(true)?;
        assert!(hotkeys.is_registered(&second.0[0], CaptureCommand::Region));
        let active = crate::CaptureHotkeys([
            "".into(),
            "".into(),
            "".into(),
            second.0[0].clone(),
            "".into(),
            String::new(),
        ]);
        hotkeys.prepare(&active)?;
        hotkeys.finish(true)?;
        assert!(hotkeys.is_registered(&second.0[0], CaptureCommand::ActiveWindow));
        let repeat = crate::CaptureHotkeys([
            "".into(),
            "".into(),
            "".into(),
            "".into(),
            second.0[0].clone(),
            String::new(),
        ]);
        hotkeys.prepare(&repeat)?;
        hotkeys.finish(true)?;
        assert!(hotkeys.is_registered(&second.0[0], CaptureCommand::RepeatRegion));
        let mut display = crate::CaptureHotkeys(std::array::from_fn(|_| String::new()));
        display.0[5] = second.0[0].clone();
        hotkeys.prepare(&display)?;
        assert!(hotkeys.is_registered(&second.0[0], CaptureCommand::RepeatRegion));
        hotkeys.finish(true)?;
        assert!(hotkeys.is_registered(&second.0[0], CaptureCommand::ActiveDisplay));
        hotkeys.prepare(&crate::CaptureHotkeys([
            "".into(),
            "".into(),
            "".into(),
            "".into(),
            "".into(),
            String::new(),
        ]))?;
        hotkeys.finish(true)?;
        assert!(hotkeys.keys.is_empty());
        drop(hotkeys);
        blocker.register(staged)?;
        blocker.unregister(staged)?;
        blocker.register(conflict)?;
        let mut startup = CaptureHotkey::unregistered()?;
        assert!(startup.keys.is_empty());
        startup.register_settings(&crate::CaptureHotkeys([
            "".into(),
            "Ctrl+Alt+F22".into(),
            "Ctrl+Alt+F23".into(),
            "".into(),
            "Ctrl+Alt+F24".into(),
            "Ctrl+Alt+F19".into(),
        ]))?;
        assert_eq!(startup.keys.len(), 3);
        assert!(startup.is_registered("Ctrl+Alt+F23", CaptureCommand::EditRegion));
        assert!(startup.is_registered("Ctrl+Alt+F24", CaptureCommand::RepeatRegion));
        assert!(startup.is_registered("Ctrl+Alt+F19", CaptureCommand::ActiveDisplay));
        assert!(!startup.is_registered("Ctrl+Shift+S", CaptureCommand::Primary));
        assert!(startup.status().contains("unavailable"));
        drop(startup);
        blocker.unregister(conflict)?;
        Ok(())
    }

    #[test]
    fn routes_only_matching_key_presses() {
        let primary = HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyS);
        let region = HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyR);
        let edit = HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyE);
        let active = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyW);
        let repeat = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyR);
        let display = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyD);
        let keys = [
            (primary, CaptureCommand::Primary),
            (region, CaptureCommand::Region),
            (edit, CaptureCommand::EditRegion),
            (active, CaptureCommand::ActiveWindow),
            (repeat, CaptureCommand::RepeatRegion),
            (display, CaptureCommand::ActiveDisplay),
        ];
        for (key, command) in keys {
            assert_eq!(
                command_for_event(
                    &keys,
                    GlobalHotKeyEvent {
                        id: key.id(),
                        state: HotKeyState::Pressed
                    }
                ),
                Some(command)
            );
            assert_eq!(
                command_for_event(
                    &keys,
                    GlobalHotKeyEvent {
                        id: key.id(),
                        state: HotKeyState::Released
                    }
                ),
                None
            );
        }
        assert_eq!(
            command_for_event(
                &keys,
                GlobalHotKeyEvent {
                    id: u32::MAX,
                    state: HotKeyState::Pressed
                }
            ),
            None
        );
    }

    #[test]
    #[ignore = "registers a real system hotkey"]
    fn registration_is_released() -> Result<()> {
        for _ in 0..2 {
            let hotkeys = CaptureHotkey::register()?;
            assert_eq!(hotkeys.keys.len(), 3, "{}", hotkeys.status());
            drop(hotkeys);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "hotkey_transaction_tests.rs"]
mod transaction_tests;
