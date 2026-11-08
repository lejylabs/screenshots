//! Windows spike: construct and drop on the GPUI event-loop thread.
use anyhow::Result;
use global_hotkey::{
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
    hotkey::{Code, HotKey, Modifiers},
};

pub struct CaptureHotkey {
    manager: GlobalHotKeyManager,
    key: HotKey,
}

impl CaptureHotkey {
    pub fn register() -> Result<Self> {
        let manager = GlobalHotKeyManager::new()?;
        let key = HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyS);
        manager.register(key)?;
        Ok(Self { manager, key })
    }

    pub fn take_pressed(&self) -> bool {
        let mut pressed = false;
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            pressed |= event.id == self.key.id() && event.state == HotKeyState::Pressed;
        }
        pressed
    }
}

impl Drop for CaptureHotkey {
    fn drop(&mut self) {
        if let Err(error) = self.manager.unregister(self.key) {
            tracing::warn!(%error, "failed to unregister capture hotkey");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "registers a real system hotkey"]
    fn registration_is_released() -> Result<()> {
        drop(CaptureHotkey::register()?);
        drop(CaptureHotkey::register()?);
        Ok(())
    }
}
