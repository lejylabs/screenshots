//! Windows tray spike. Create, poll and drop on the UI event-loop thread.
use anyhow::Result;
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem},
};

#[derive(Debug, PartialEq, Eq)]
pub enum TrayCommand {
    Capture,
    Region,
    Show,
    Quit,
}

pub struct SpikeTray {
    _icon: TrayIcon,
    capture: MenuItem,
    region: MenuItem,
    show: MenuItem,
    quit: MenuItem,
}

impl SpikeTray {
    pub fn new() -> Result<Self> {
        let menu = Menu::new();
        let capture = MenuItem::new("Capture primary (Ctrl+Shift+S)", true, None);
        let region = MenuItem::new("Bereich auswählen (Ctrl+Shift+R)", true, None);
        let show = MenuItem::new("Show control window", true, None);
        let quit = MenuItem::new("Quit", true, None);
        menu.append_items(&[&region, &capture, &show, &quit])?;
        // A simple camera frame for the prototype; no external asset needed.
        let mut pixels = vec![0; 32 * 32 * 4];
        for y in 6..26 {
            for x in 3..29 {
                let lens = (x as i32 - 16).pow(2) + (y as i32 - 16).pow(2) < 36;
                let color = if lens {
                    [24, 24, 24, 255]
                } else {
                    [96, 165, 250, 255]
                };
                pixels[(y * 32 + x) * 4..(y * 32 + x + 1) * 4].copy_from_slice(&color);
            }
        }
        let icon = TrayIconBuilder::new()
            .with_tooltip("screenshots — phase 0")
            .with_icon(Icon::from_rgba(pixels, 32, 32)?)
            .with_menu(Box::new(menu))
            .build()?;
        Ok(Self {
            _icon: icon,
            capture,
            region,
            show,
            quit,
        })
    }

    pub fn take_commands(&self) -> Vec<TrayCommand> {
        MenuEvent::receiver()
            .try_iter()
            .filter_map(|event| {
                if event.id == *self.capture.id() {
                    Some(TrayCommand::Capture)
                } else if event.id == *self.region.id() {
                    Some(TrayCommand::Region)
                } else if event.id == *self.show.id() {
                    Some(TrayCommand::Show)
                } else if event.id == *self.quit.id() {
                    Some(TrayCommand::Quit)
                } else {
                    None
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "creates a real Windows tray icon"]
    fn tray_creation_and_cleanup() -> Result<()> {
        let tray = SpikeTray::new()?;
        assert!(tray.take_commands().is_empty());
        drop(tray);
        Ok(())
    }
}
