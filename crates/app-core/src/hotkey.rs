//! Windows spike: construct and drop on the GPUI event-loop thread.
use anyhow::Result;
use global_hotkey::{
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
    hotkey::{Code, HotKey, Modifiers},
};

pub struct CaptureHotkey {
    manager: GlobalHotKeyManager,
    keys: Vec<(HotKey, CaptureCommand)>,
    status: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureCommand {
    Primary,
    Region,
}

impl CaptureHotkey {
    pub fn register() -> Result<Self> {
        let manager = GlobalHotKeyManager::new()?;
        let mut keys = Vec::new();
        let mut status = Vec::new();
        for (code, command, label) in [
            (Code::KeyS, CaptureCommand::Primary, "Ctrl+Shift+S: primary"),
            (Code::KeyR, CaptureCommand::Region, "Ctrl+Shift+R: region"),
        ] {
            let key = HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), code);
            match manager.register(key) {
                Ok(()) => {
                    keys.push((key, command));
                    status.push(label.to_string());
                }
                Err(error) => {
                    tracing::warn!(%error, label, "global hotkey unavailable");
                    status.push(format!("{label} unavailable: {error}"));
                }
            }
        }
        Ok(Self {
            manager,
            keys,
            status: status.join(" | "),
        })
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn take_commands(&self) -> Vec<CaptureCommand> {
        GlobalHotKeyEvent::receiver()
            .try_iter()
            .filter_map(|event| command_for_event(&self.keys, event))
            .collect()
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

impl Drop for CaptureHotkey {
    fn drop(&mut self) {
        for (key, _) in &self.keys {
            if let Err(error) = self.manager.unregister(*key) {
                tracing::warn!(%error, "failed to unregister capture hotkey");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_only_matching_key_presses() {
        let primary = HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyS);
        let region = HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyR);
        let keys = [
            (primary, CaptureCommand::Primary),
            (region, CaptureCommand::Region),
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
            assert_eq!(hotkeys.keys.len(), 2, "{}", hotkeys.status());
            drop(hotkeys);
        }
        Ok(())
    }
}
