//! StatusNotifierItem and DBusMenu over the worker's existing private session connection.
use super::*;
use dbus::{
    Message, Path,
    arg::{PropMap, RefArg, Variant},
    blocking::{Connection, stdintf::org_freedesktop_dbus::Properties},
    channel::{MatchingReceiver, Sender},
    message::MatchRule,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
const ITEM: &str = "/StatusNotifierItem";
const MENU: &str = "/Menu";
const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const SNI: &str = "org.kde.StatusNotifierItem";
const DBUSMENU: &str = "com.canonical.dbusmenu";
const ROWS: [&str; 8] = [
    "Capture and annotate",
    "Select region",
    "Capture primary",
    "Active window (configured delay)…",
    "Active display (configured delay)…",
    "Repeat last region (current session)",
    "Show control window",
    "Quit",
];
fn value(value: impl RefArg + 'static) -> Variant<Box<dyn RefArg>> {
    Variant(Box::new(value))
}
fn action(id: i32, repeat: bool) -> Option<TrayCommand> {
    Some(match id {
        1 => TrayCommand::EditRegion,
        2 => TrayCommand::Region,
        3 => TrayCommand::Capture,
        4 => TrayCommand::ActiveWindow,
        5 => TrayCommand::ActiveDisplay,
        6 if repeat => TrayCommand::RepeatRegion,
        7 => TrayCommand::Show,
        8 => TrayCommand::Quit,
        _ => return None,
    })
}
fn properties(id: i32, repeat: bool) -> PropMap {
    let mut map = PropMap::new();
    if id == 0 {
        map.insert("children-display".into(), value("submenu".to_owned()));
    } else if let Some(label) = usize::try_from(id - 1).ok().and_then(|i| ROWS.get(i)) {
        map.insert("label".into(), value((*label).to_owned()));
        map.insert("enabled".into(), value(id != 6 || repeat));
        map.insert("visible".into(), value(true));
    }
    map
}
fn selected_properties(id: i32, repeat: bool, names: &[String]) -> PropMap {
    let mut properties = properties(id, repeat);
    if !names.is_empty() {
        properties.retain(|name, _| names.contains(name));
    }
    properties
}
fn layout(
    id: i32,
    repeat: bool,
    depth: i32,
    names: &[String],
) -> (i32, PropMap, Vec<Variant<Box<dyn RefArg>>>) {
    (
        id,
        selected_properties(id, repeat, names),
        if id == 0 && depth != 0 {
            (1..=8)
                .map(|id| value(layout(id, repeat, depth.saturating_sub(1), names)))
                .collect()
        } else {
            Vec::new()
        },
    )
}
fn icon() -> Vec<(i32, i32, Vec<u8>)> {
    let mut pixels = vec![0u8; 32 * 32 * 4];
    for y in 6..26 {
        for x in 3..29 {
            let dark = (x as i32 - 16).pow(2) + (y as i32 - 16).pow(2) < 36;
            pixels[(y * 32 + x) * 4..(y * 32 + x + 1) * 4].copy_from_slice(if dark {
                &[255, 24, 24, 24]
            } else {
                &[255, 96, 165, 250]
            });
        }
    }
    vec![(32, 32, pixels)]
}
fn item_properties() -> PropMap {
    let mut p = PropMap::new();
    for (key, text) in [
        ("Category", "ApplicationStatus"),
        ("Id", "screenshots"),
        ("Title", "screenshots"),
        ("Status", "Active"),
        ("IconName", ""),
        ("AttentionIconName", ""),
        ("OverlayIconName", ""),
    ] {
        p.insert(key.into(), value(text.to_owned()));
    }
    p.insert("WindowId".into(), value(0u32));
    p.insert("IconPixmap".into(), value(icon()));
    p.insert(
        "AttentionIconPixmap".into(),
        value(Vec::<(i32, i32, Vec<u8>)>::new()),
    );
    p.insert(
        "OverlayIconPixmap".into(),
        value(Vec::<(i32, i32, Vec<u8>)>::new()),
    );
    p.insert("ItemIsMenu".into(), value(false));
    p.insert("Menu".into(), value(Path::from(MENU)));
    p.insert(
        "ToolTip".into(),
        value((
            String::new(),
            icon(),
            "screenshots".to_owned(),
            "Capture / Show / Quit".to_owned(),
        )),
    );
    p
}
fn menu_properties() -> PropMap {
    let mut p = PropMap::new();
    p.insert("Version".into(), value(3u32));
    p.insert("TextDirection".into(), value("ltr".to_owned()));
    p.insert("Status".into(), value("normal".to_owned()));
    p.insert("IconThemePath".into(), value(Vec::<String>::new()));
    p
}
fn reply(
    message: &Message,
    repeat: bool,
    sender: &mpsc::Sender<UnixDesktopEvent>,
) -> Result<Message> {
    let interface = message
        .interface()
        .map(|i| i.to_string())
        .unwrap_or_default();
    let member = message.member().map(|m| m.to_string()).unwrap_or_default();
    let path = message.path().map(|p| p.to_string()).unwrap_or_default();
    if interface == "org.freedesktop.DBus.Introspectable" && member == "Introspect" {
        anyhow::ensure!(path == ITEM || path == MENU, "Unknown native object");
        return Ok(message
            .method_return()
            .append1(include_str!("unix_desktop_services_introspection.xml")));
    }
    if interface == "org.freedesktop.DBus.Peer" && member == "Ping" {
        return Ok(message.method_return());
    }
    if interface == "org.freedesktop.DBus.Properties" {
        let (target,): (String,) = message.read1().map(|s| (s,))?;
        let mut p = match (path.as_str(), target.as_str()) {
            (ITEM, SNI) => item_properties(),
            (MENU, DBUSMENU) => menu_properties(),
            _ => anyhow::bail!("Unknown property interface"),
        };
        return match member.as_str() {
            "GetAll" => Ok(message.method_return().append1(p)),
            "Get" => {
                let (_, name): (String, String) = message.read2()?;
                Ok(message.method_return().append1(
                    p.remove(&name)
                        .ok_or_else(|| anyhow::anyhow!("Unknown property"))?,
                ))
            }
            _ => anyhow::bail!("Read-only properties"),
        };
    }
    if path == ITEM && interface == SNI {
        match member.as_str() {
            "Activate" | "SecondaryActivate" | "ContextMenu" => {
                let _: (i32, i32) = message.read2()?;
                deliver(
                    sender,
                    UnixDesktopEvent::Action(UnixDesktopAction::Tray(TrayCommand::Show)),
                );
            }
            "Scroll" => {
                let _: (i32, String) = message.read2()?;
            }
            _ => anyhow::bail!("Unknown StatusNotifier method"),
        }
        return Ok(message.method_return());
    }
    anyhow::ensure!(
        path == MENU && interface == DBUSMENU,
        "Unknown native object or interface"
    );
    match member.as_str() {
        "GetLayout" => {
            let (parent, depth, names): (i32, i32, Vec<String>) = message.read3()?;
            anyhow::ensure!(
                (0..=8).contains(&parent) && names.len() <= 32,
                "Invalid menu request"
            );
            Ok(message
                .method_return()
                .append2(1u32, layout(parent, repeat, depth, &names)))
        }
        "GetGroupProperties" => {
            let (ids, names): (Vec<i32>, Vec<String>) = message.read2()?;
            anyhow::ensure!(
                ids.len() <= 9 && names.len() <= 32,
                "Menu request exceeds its limit"
            );
            let ids = if ids.is_empty() {
                (0..=8).collect()
            } else {
                ids
            };
            Ok(message.method_return().append1(
                ids.into_iter()
                    .filter(|id| (0..=8).contains(id))
                    .map(|id| (id, selected_properties(id, repeat, &names)))
                    .collect::<Vec<_>>(),
            ))
        }
        "GetProperty" => {
            let (id, name): (i32, String) = message.read2()?;
            Ok(message.method_return().append1(
                properties(id, repeat)
                    .remove(&name)
                    .ok_or_else(|| anyhow::anyhow!("Unknown menu property"))?,
            ))
        }
        "AboutToShow" => {
            let id: i32 = message.read1()?;
            anyhow::ensure!((0..=8).contains(&id), "Unknown menu entry");
            Ok(message.method_return().append1(false))
        }
        "AboutToShowGroup" => {
            let ids: Vec<i32> = message.read1()?;
            anyhow::ensure!(ids.len() <= 9, "Menu request exceeds its limit");
            Ok(message.method_return().append2(
                Vec::<i32>::new(),
                ids.into_iter()
                    .filter(|id| !(0..=8).contains(id))
                    .collect::<Vec<_>>(),
            ))
        }
        "Event" => {
            let (id, event, _data, _timestamp): (i32, String, Variant<Box<dyn RefArg>>, u32) =
                message.read4()?;
            if event == "clicked" {
                if let Some(command) = action(id, repeat) {
                    deliver(
                        sender,
                        UnixDesktopEvent::Action(UnixDesktopAction::Tray(command)),
                    );
                }
            }
            Ok(message.method_return())
        }
        "EventGroup" => {
            let events: Vec<(i32, String, Variant<Box<dyn RefArg>>, u32)> = message.read1()?;
            anyhow::ensure!(events.len() <= 9, "Menu request exceeds its limit");
            let mut errors = Vec::new();
            for (id, event, _, _) in events {
                if let Some(command) = action(id, repeat) {
                    if event == "clicked" {
                        deliver(
                            sender,
                            UnixDesktopEvent::Action(UnixDesktopAction::Tray(command)),
                        );
                    }
                } else {
                    errors.push(id);
                }
            }
            Ok(message.method_return().append1(errors))
        }
        _ => anyhow::bail!("Unknown menu method"),
    }
}

pub(super) struct Tray {
    pub repeat: Arc<AtomicBool>,
    pub changed: Arc<AtomicBool>,
    available: Arc<AtomicBool>,
    owner: Arc<std::sync::Mutex<Option<String>>>,
}
impl Tray {
    pub(super) fn install(
        connection: &Connection,
        events: mpsc::Sender<UnixDesktopEvent>,
        repeat: Arc<AtomicBool>,
        available: Arc<AtomicBool>,
    ) -> Result<Self> {
        let state = repeat.clone();
        let sender = events.clone();
        let mut rule = MatchRule::new_method_call();
        // One fixed object tree; unknown paths receive an error instead of a hung caller.
        rule.path = None;
        connection.start_receive(
            rule,
            Box::new(move |message, connection| {
                let response = reply(&message, state.load(Ordering::Acquire), &sender)
                    .unwrap_or_else(|_| {
                        message.error(
                            &"org.freedesktop.DBus.Error.InvalidArgs".into(),
                            c"Unknown or invalid native menu request",
                        )
                    });
                let _ = connection.send(response);
                true
            }),
        );
        let owner: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
        let owner_changed = owner.clone();
        let changed = Arc::new(AtomicBool::new(true));
        let flag = changed.clone();
        let availability = available.clone();
        let notifications = events.clone();
        let owner_rule = super::daemon_owner_rule();
        connection.add_match_no_cb(&format!("{},arg0='{}'", owner_rule.match_str(), WATCHER))?;
        connection.start_receive(
            owner_rule,
            Box::new(move |message, _| {
                let Ok((name, _, _)) = message.read3::<String, String, String>() else {
                    return true;
                };
                if name == WATCHER {
                    *owner_changed.lock().unwrap() = None;
                    flag.store(true, Ordering::Release);
                    availability.store(false, Ordering::Release);
                    deliver(
                        &notifications,
                        UnixDesktopEvent::TrayStatus {
                            available: false,
                            status: "Tray host changed; Close quits until registration is restored"
                                .into(),
                        },
                    );
                }
                true
            }),
        );
        for signal in [
            "StatusNotifierHostRegistered",
            "StatusNotifierHostUnregistered",
        ] {
            let host_owner = owner.clone();
            let flag = changed.clone();
            let availability = available.clone();
            let notifications = events.clone();
            let removed = signal == "StatusNotifierHostUnregistered";
            connection.add_match(
                MatchRule::new_signal(WATCHER, signal).with_sender(WATCHER),
                move |(): (), _, message| {
                    let authenticated = host_owner
                        .lock()
                        .unwrap()
                        .as_deref()
                        .is_some_and(|expected| message.sender().as_deref() == Some(expected));
                    if !authenticated {
                        return true;
                    }
                    flag.store(true, Ordering::Release);
                    if removed {
                        availability.store(false, Ordering::Release);
                        deliver(
                            &notifications,
                            UnixDesktopEvent::TrayStatus {
                                available: false,
                                status: "Tray host unavailable; Close quits".into(),
                            },
                        );
                    }
                    true
                },
            )?;
        }
        Ok(Self {
            repeat,
            changed,
            available,
            owner,
        })
    }
    pub(super) fn refresh(
        &mut self,
        connection: &Connection,
        events: &mpsc::Sender<UnixDesktopEvent>,
    ) {
        if !self.changed.swap(false, Ordering::AcqRel) {
            return;
        }
        let owner: Result<(String,), dbus::Error> = connection
            .with_proxy(
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                Duration::from_secs(2),
            )
            .method_call("org.freedesktop.DBus", "GetNameOwner", (WATCHER,));
        let registration: Result<(), dbus::Error> = owner.and_then(|(owner,)| {
            if !owner.starts_with(':') || owner.len() > 255 {
                return Err(dbus::Error::new_failed("Invalid tray watcher owner"));
            }
            *self.owner.lock().unwrap() = Some(owner.clone());
            let watcher =
                connection.with_proxy(owner, "/StatusNotifierWatcher", Duration::from_secs(2));
            let registered: bool = watcher.get(WATCHER, "IsStatusNotifierHostRegistered")?;
            if !registered {
                return Err(dbus::Error::new_failed(
                    "No StatusNotifier host is available",
                ));
            }
            watcher.method_call(WATCHER, "RegisterStatusNotifierItem", (ITEM,))
        });
        self.available
            .store(registration.is_ok(), Ordering::Release);
        let status = registration
            .map(|()| "Tray ready".to_owned())
            .unwrap_or_else(|error| format!("Tray unavailable: {error}; Close quits"));
        deliver(
            events,
            UnixDesktopEvent::TrayStatus {
                available: self.available.load(Ordering::Acquire),
                status,
            },
        );
    }
    pub(super) fn available(&self) -> bool {
        self.available.load(Ordering::Acquire)
    }
    pub(super) fn repeat_changed(&self, connection: &Connection) {
        let message = Message::new_signal(MENU, DBUSMENU, "ItemsPropertiesUpdated")
            .unwrap()
            .append2(
                vec![(6i32, properties(6, self.repeat.load(Ordering::Acquire)))],
                Vec::<(i32, Vec<String>)>::new(),
            );
        let _ = connection.send(message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn menu_contains_existing_commands_and_honors_requested_depth_and_properties() {
        let all = layout(0, false, -1, &[]);
        assert_eq!(all.2.len(), 8);
        assert!(layout(0, false, 0, &[]).2.is_empty());
        let leaf = layout(2, false, -1, &["label".into()]);
        assert!(leaf.2.is_empty());
        assert_eq!(leaf.1.len(), 1);
        assert_eq!(leaf.1["label"].0.as_str(), Some("Select region"));
    }
    #[test]
    fn repeat_cannot_be_invoked_until_the_controller_has_a_region() {
        assert_eq!(action(6, false), None);
        assert_eq!(action(6, true), Some(TrayCommand::RepeatRegion));
        for id in [-1, 0, 9, i32::MAX] {
            assert_eq!(action(id, true), None);
        }
        assert_eq!(action(7, false), Some(TrayCommand::Show));
        assert_eq!(action(8, false), Some(TrayCommand::Quit));
    }
}
