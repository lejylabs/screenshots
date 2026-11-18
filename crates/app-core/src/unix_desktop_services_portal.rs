//! GlobalShortcuts uses saved accelerators as preferences. The compositor owns actual triggers.
//! The portal exposes descriptions, not machine-verifiable assigned accelerators.
use super::*;
use anyhow::{Context, Result, ensure};
use dbus::{
    Path,
    arg::{PropMap, RefArg, Variant},
    blocking::Connection,
    channel::MatchingReceiver,
    message::MatchRule,
};
use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    os::fd::RawFd,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
const SERVICE: &str = "org.freedesktop.portal.Desktop";
const OBJECT: &str = "/org/freedesktop/portal/desktop";
const INTERFACE: &str = "org.freedesktop.portal.GlobalShortcuts";
/// Resolve the authoritative unique sender on the caller's existing private session connection.
#[doc(hidden)]
pub fn shortcut_portal_owner(connection: &Connection) -> Result<String> {
    let daemon = connection.with_proxy(
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        Duration::from_secs(2),
    );
    let (activation,): (u32,) = daemon.method_call(
        "org.freedesktop.DBus",
        "StartServiceByName",
        (SERVICE, 0u32),
    )?;
    ensure!(
        matches!(activation, 1 | 2),
        "Shortcut portal activation failed"
    );
    let (owner,): (String,) = connection
        .with_proxy(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            Duration::from_secs(2),
        )
        .method_call("org.freedesktop.DBus", "GetNameOwner", (SERVICE,))?;
    ensure!(
        owner.starts_with(':') && owner.len() <= 255,
        "Invalid shortcut portal owner"
    );
    Ok(owner)
}

fn option(map: &mut PropMap, name: &str, value: impl RefArg + 'static) {
    map.insert(name.into(), Variant(Box::new(value)));
}
pub(super) fn preferred(value: &str) -> String {
    value
        .split('+')
        .map(|part| match part {
            "Ctrl" => "CTRL".into(),
            "Alt" => "ALT".into(),
            "Shift" => "SHIFT".into(),
            "Super" => "LOGO".into(),
            "PrintScreen" => "Print".into(),
            v if v.len() == 1 => v.to_ascii_lowercase(),
            v => v.into(),
        })
        .collect::<Vec<_>>()
        .join("+")
}
fn command(index: usize) -> CaptureCommand {
    [
        CaptureCommand::Primary,
        CaptureCommand::Region,
        CaptureCommand::EditRegion,
        CaptureCommand::ActiveWindow,
        CaptureCommand::RepeatRegion,
        CaptureCommand::ActiveDisplay,
    ][index]
}
fn descriptions(
    result: &PropMap,
    wanted: &BTreeMap<String, (CaptureCommand, String)>,
    complete: bool,
) -> Result<BTreeMap<String, String>> {
    let values = result
        .get("shortcuts")
        .and_then(|v| v.0.as_iter())
        .context("Portal did not confirm shortcuts")?;
    let mut accepted = BTreeMap::new();
    for item in values.take(7) {
        ensure!(accepted.len() < 6, "Portal returned excessive shortcuts");
        let mut fields = item.as_iter().context("Invalid portal shortcut")?;
        let id = fields
            .next()
            .and_then(RefArg::as_str)
            .context("Invalid portal shortcut ID")?;
        ensure!(
            id.len() <= 32 && wanted.contains_key(id),
            "Unexpected portal shortcut ID"
        );
        let mut fields = fields
            .next()
            .and_then(RefArg::as_iter)
            .context("Missing portal shortcut properties")?;
        let mut trigger = None;
        for _ in 0..16 {
            let Some(name) = fields.next() else {
                break;
            };
            let name = name
                .as_str()
                .context("Invalid portal shortcut property name")?;
            ensure!(name.len() <= 64, "Portal property name exceeds its limit");
            let value = fields
                .next()
                .context("Missing portal shortcut property value")?;
            if name == "trigger_description" {
                let text = value
                    .as_str()
                    .context("Invalid portal trigger description")?;
                ensure!(
                    !text.is_empty() && text.len() <= 256 && !text.chars().any(char::is_control),
                    "Portal trigger description exceeds its limit"
                );
                ensure!(
                    trigger.replace(text.to_owned()).is_none(),
                    "Duplicate portal trigger description"
                );
            }
        }
        ensure!(
            fields.next().is_none(),
            "Portal returned excessive shortcut properties"
        );
        ensure!(
            accepted
                .insert(
                    id.to_owned(),
                    trigger.context("Portal did not describe the assigned shortcut")?
                )
                .is_none(),
            "Duplicate portal shortcut"
        );
    }
    ensure!(
        !complete || accepted.len() == wanted.len(),
        "Portal did not grant every configured shortcut"
    );
    Ok(accepted)
}

pub(super) enum Signal {
    Key(Path<'static>, String, bool),
    State(Path<'static>, String, bool),
    Closed(Path<'static>),
    Changed(Path<'static>, PropMap),
    OwnerGone,
    Overflow,
}
pub(super) type Signals = Arc<Mutex<VecDeque<Signal>>>;
fn enqueue(queue: &Signals, signal: Signal) {
    let mut queue = queue.lock().unwrap();
    match &signal {
        Signal::Key(path, id, pressed) => {
            if *pressed {
                if queue.iter().any(|e| matches!(e, Signal::Key(p, key, true) if p == path && key == id)) {
                    queue.retain(|e| !matches!(e, Signal::Key(p, key, false) if p == path && key == id));
                    return;
                }
            } else { queue.retain(|e| !matches!(e, Signal::Key(p, key, false) if p == path && key == id)); }
        }
        Signal::State(path, id, _) => queue.retain(|event| !matches!(event, Signal::State(p, key, _) if p == path && key == id)),
        Signal::Changed(path, _) => queue.retain(|e| !matches!(e, Signal::Changed(p, _) if p == path)),
        Signal::Closed(path) => queue.retain(|e| !matches!(e, Signal::Key(p, _, _) | Signal::State(p, _, _) | Signal::Changed(p, _) | Signal::Closed(p) if p == path)),
        Signal::OwnerGone => queue.clear(),
        Signal::Overflow => {},
    }
    if queue.len() >= 32 {
        queue.clear();
        queue.push_back(Signal::Overflow);
        return;
    }
    queue.push_back(signal);
}
pub(super) fn install(
    connection: &Connection,
    owner: &str,
    events: mpsc::Sender<UnixDesktopEvent>,
    status: Arc<Mutex<String>>,
    accepting: Arc<AtomicBool>,
) -> Result<(Signals, Arc<AtomicBool>)> {
    let alive = Arc::new(AtomicBool::new(true));
    let signals: Signals = Arc::new(Mutex::new(VecDeque::new()));
    let prefix = format!(
        "/org/freedesktop/portal/desktop/session/{}/",
        connection
            .unique_name()
            .to_string()
            .trim_start_matches(':')
            .replace('.', "_")
    );
    for (member, pressed) in [("Activated", true), ("Deactivated", false)] {
        let queue = signals.clone();
        let prefix = prefix.clone();
        let accepting = accepting.clone();
        connection.add_match(
            MatchRule::new_signal(INTERFACE, member)
                .with_sender(owner.to_owned())
                .with_path(OBJECT),
            move |(session, id, _, _): (Path<'static>, String, u64, PropMap), _, _| {
                if session.len() <= 512
                    && session.to_string().starts_with(&prefix)
                    && matches!(
                        id.as_str(),
                        "capture_0"
                            | "capture_1"
                            | "capture_2"
                            | "capture_3"
                            | "capture_4"
                            | "capture_5"
                    )
                {
                    enqueue(
                        &queue,
                        if accepting.load(Ordering::Acquire) {
                            Signal::Key(session, id, pressed)
                        } else {
                            Signal::State(session, id, pressed)
                        },
                    );
                }
                true
            },
        )?;
    }
    let queue = signals.clone();
    let session_prefix = prefix.clone();
    let closed_status = status.clone();
    let closed_events = events.clone();
    connection.add_match(
        MatchRule::new_signal("org.freedesktop.portal.Session", "Closed")
            .with_sender(owner.to_owned()),
        move |(): (), _, message| {
            if let Some(path) = message
                .path()
                .filter(|path| path.len() <= 512 && path.to_string().starts_with(&session_prefix))
            {
                let message =
                    "Wayland shortcut session state changed; native status update pending"
                        .to_owned();
                *closed_status.lock().unwrap() = message.clone();
                deliver(&closed_events, UnixDesktopEvent::ShortcutStatus(message));
                enqueue(&queue, Signal::Closed(path.into_static()));
            }
            true
        },
    )?;
    let queue = signals.clone();
    let owner_alive = alive.clone();
    let owner_rule = super::daemon_owner_rule();
    connection.add_match_no_cb(&format!("{},arg0='{}'", owner_rule.match_str(), SERVICE))?;
    connection.start_receive(
        owner_rule,
        Box::new(move |message, _| {
            let Ok((name, old, new)) = message.read3::<String, String, String>() else {
                return true;
            };
            if name == SERVICE && !old.is_empty() && old != new {
                owner_alive.store(false, Ordering::Release);
                let message =
                    "Wayland shortcut portal disappeared; restart to register again".to_owned();
                *status.lock().unwrap() = message.clone();
                deliver(&events, UnixDesktopEvent::ShortcutStatus(message));
                enqueue(&queue, Signal::OwnerGone);
            }
            true
        }),
    );
    // Changes are bounded after parsing, and replace any older descriptions for that session.
    let queue = signals.clone();
    connection.add_match(
        MatchRule::new_signal(INTERFACE, "ShortcutsChanged")
            .with_sender(owner.to_owned())
            .with_path(OBJECT),
        move |(session, shortcuts): (Path<'static>, Vec<(String, PropMap)>), _, _| {
            let mut result = PropMap::new();
            if session.len() <= 512
                && session.to_string().starts_with(&prefix)
                && shortcuts.len() <= 6
            {
                let sanitized: Option<Vec<_>> = shortcuts
                    .into_iter()
                    .map(|(id, properties)| {
                        if id.len() > 32 {
                            return None;
                        }
                        let text = properties.get("trigger_description")?.0.as_str()?;
                        if text.is_empty() || text.len() > 256 || text.chars().any(char::is_control)
                        {
                            return None;
                        }
                        let mut fields = PropMap::new();
                        option(&mut fields, "trigger_description", text.to_owned());
                        Some((id, fields))
                    })
                    .collect();
                if let Some(shortcuts) = sanitized {
                    option(&mut result, "shortcuts", shortcuts);
                    enqueue(&queue, Signal::Changed(session, result));
                } else {
                    enqueue(&queue, Signal::Overflow);
                }
            }
            true
        },
    )?;
    Ok((signals, alive))
}

struct Session {
    owner: String,
    path: Path<'static>,
    keys: BTreeMap<String, (CaptureCommand, String)>,
    triggers: BTreeMap<String, String>,
    down: HashSet<String>,
}
impl Session {
    fn close(&self, connection: &Connection) -> Result<()> {
        connection
            .with_proxy(
                self.owner.clone(),
                self.path.clone(),
                Duration::from_secs(2),
            )
            .method_call("org.freedesktop.portal.Session", "Close", ())
            .map_err(Into::into)
    }
}
pub(super) struct Shortcuts {
    owner: String,
    alive: Arc<AtomicBool>,
    active: Option<Session>,
    staged: Option<Option<Session>>,
    failed_cleanup: bool,
    initialized: bool,
    status: String,
}
impl Shortcuts {
    pub(super) fn new(owner: String, alive: Arc<AtomicBool>) -> Self {
        Self {
            owner,
            alive,
            active: None,
            staged: None,
            failed_cleanup: false,
            initialized: false,
            status: "Waiting for settings; no global shortcuts active".into(),
        }
    }
    pub(super) fn alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }
    pub(super) fn status(&self) -> &str {
        &self.status
    }
    fn refresh_status(&mut self) {
        self.status = if let Some(active) = &self.active {
            format!(
                "Wayland portal assignments (saved keys are preferences): {}",
                active
                    .keys
                    .iter()
                    .map(|(id, (command, preference))| format!(
                        "{}: {} (requested {})",
                        command.label(),
                        active.triggers[id],
                        preference
                    ))
                    .collect::<Vec<_>>()
                    .join(" | ")
            )
        } else {
            "No Wayland portal shortcuts active".into()
        };
    }
    pub(super) fn initialize(
        &mut self,
        connection: &Connection,
        settings: &CaptureHotkeys,
        stop: &AtomicBool,
        wake: RawFd,
    ) -> Result<()> {
        ensure!(!self.initialized, "Shortcuts already initialized");
        self.initialized = true;
        let result = self
            .prepare(connection, settings, stop, wake)
            .and_then(|()| self.finish(connection, true));
        if let Err(error) = &result {
            self.status =
                bounded_status(&format!("Wayland global shortcuts unavailable: {error:#}"));
        }
        result
    }
    pub(super) fn prepare(
        &mut self,
        connection: &Connection,
        settings: &CaptureHotkeys,
        stop: &AtomicBool,
        wake: RawFd,
    ) -> Result<()> {
        ensure!(
            self.alive(),
            "Shortcut portal owner disappeared; restart to retry"
        );
        ensure!(
            !self.failed_cleanup && self.staged.is_none(),
            "Shortcut change or cleanup pending; restart to retry"
        );
        let settings = settings.validated()?;
        let keys: BTreeMap<_, _> = settings
            .0
            .iter()
            .enumerate()
            .filter(|(_, v)| !v.is_empty())
            .map(|(i, v)| (format!("capture_{i}"), (command(i), v.clone())))
            .collect();
        if keys.is_empty() {
            self.staged = Some(None);
            return Ok(());
        }
        let token = format!("screenshots_{}", uuid::Uuid::new_v4().simple());
        let sender = connection
            .unique_name()
            .to_string()
            .trim_start_matches(':')
            .replace('.', "_");
        let expected = Path::new(format!(
            "/org/freedesktop/portal/desktop/session/{sender}/{token}"
        ))
        .map_err(anyhow::Error::msg)?;
        let mut options = PropMap::new();
        option(&mut options, "session_handle_token", token);
        let mut session = Session {
            owner: self.owner.clone(),
            path: expected.clone(),
            keys,
            triggers: BTreeMap::new(),
            down: HashSet::new(),
        };
        let result = (|| {
            let created = request(
                connection,
                &self.owner,
                "CreateSession",
                options,
                None,
                stop,
                wake,
            )?;
            ensure!(
                created.get("session_handle").and_then(|v| v.0.as_str()) == Some(expected.as_ref()),
                "Portal returned an unexpected session handle"
            );
            let result = request(
                connection,
                &self.owner,
                "BindShortcuts",
                PropMap::new(),
                Some(&session),
                stop,
                wake,
            )?;
            session.triggers = descriptions(&result, &session.keys, true)?;
            Ok(())
        })();
        if let Err(error) = result {
            if let Err(cleanup) = session.close(connection) {
                self.failed_cleanup = true;
                anyhow::bail!("{error:#}; portal cleanup failed: {cleanup:#}; restart to retry");
            }
            return Err(error);
        }
        self.staged = Some(Some(session));
        Ok(())
    }
    pub(super) fn finish(&mut self, connection: &Connection, commit: bool) -> Result<()> {
        ensure!(
            self.alive(),
            "Shortcut portal owner disappeared; restart to retry"
        );
        let staged = self.staged.take().context("No prepared shortcuts")?;
        if commit {
            let previous = std::mem::replace(&mut self.active, staged);
            self.refresh_status();
            if let Some(previous) = previous {
                if let Err(error) = previous.close(connection) {
                    self.failed_cleanup = true;
                    self.status
                        .push_str(" | Previous portal session cleanup failed; restart required");
                    return Err(error);
                }
            }
        } else if let Some(staged) = staged {
            if let Err(error) = staged.close(connection) {
                self.failed_cleanup = true;
                self.status
                    .push_str(" | Reserved portal session cleanup failed; restart required");
                return Err(error);
            }
        }
        Ok(())
    }
    pub(super) fn signal(&mut self, signal: Signal) -> Result<Option<CaptureCommand>> {
        match signal {
            Signal::Overflow => {
                anyhow::bail!("Shortcut portal event overflow; restart to register again")
            }
            Signal::OwnerGone => {
                self.active = None;
                self.staged = None;
                self.status =
                    "Wayland shortcut portal disappeared; restart to register again".into();
                anyhow::bail!("Shortcut portal disconnected")
            }
            Signal::State(path, id, pressed) => {
                let update = |session: &mut Session| {
                    if session.path == path && session.keys.contains_key(&id) {
                        if pressed {
                            session.down.insert(id.clone());
                        } else {
                            session.down.remove(&id);
                        }
                    }
                };
                if let Some(session) = self.active.as_mut() {
                    update(session);
                }
                if let Some(Some(session)) = self.staged.as_mut() {
                    update(session);
                }
                Ok(None)
            }
            Signal::Key(path, id, pressed) => {
                let Some(active) = self.active.as_mut().filter(|s| s.path == path) else {
                    return Ok(None);
                };
                let Some((command, _)) = active.keys.get(&id) else {
                    return Ok(None);
                };
                if !pressed {
                    active.down.remove(&id);
                    return Ok(None);
                }
                Ok(active.down.insert(id).then_some(*command))
            }
            Signal::Closed(path) => {
                if self
                    .staged
                    .as_ref()
                    .and_then(|s| s.as_ref())
                    .is_some_and(|s| s.path == path)
                {
                    self.staged = None;
                    self.failed_cleanup = true;
                    anyhow::bail!("Reserved Wayland shortcut session was closed");
                }
                if self.active.as_ref().is_some_and(|s| s.path == path) {
                    self.active = None;
                    self.refresh_status();
                    anyhow::bail!("Wayland shortcut session was closed");
                }
                Ok(None)
            }
            Signal::Changed(path, values) => {
                if let Some(active) = self.active.as_mut().filter(|s| s.path == path) {
                    active
                        .triggers
                        .extend(descriptions(&values, &active.keys, false)?);
                    self.refresh_status();
                }
                Ok(None)
            }
        }
    }
    pub(super) fn close(&mut self, connection: &Connection) -> Result<()> {
        let mut failure = None;
        if let Some(Some(session)) = self.staged.take() {
            if let Err(error) = session.close(connection) {
                failure = Some(error);
            }
        }
        if let Some(session) = self.active.take() {
            if let Err(error) = session.close(connection) {
                failure = Some(error);
            }
        }
        if let Some(error) = failure {
            Err(error)
        } else {
            Ok(())
        }
    }
}

fn request(
    connection: &Connection,
    owner: &str,
    method: &str,
    options: PropMap,
    session: Option<&Session>,
    stop: &AtomicBool,
    wake: RawFd,
) -> Result<PropMap> {
    let binding = session.map(|session| {
        let shortcuts = session
            .keys
            .iter()
            .map(|(id, (command, key))| {
                let mut properties = PropMap::new();
                option(&mut properties, "description", command.label().to_owned());
                option(&mut properties, "preferred_trigger", preferred(key));
                (id.clone(), properties)
            })
            .collect::<Vec<_>>();
        (session.path.clone(), shortcuts)
    });
    shortcut_portal_request(connection, owner, method, options, binding, stop, wake)
}

/// Shared by the main application and the separate task host; never call on a UI executor.
#[doc(hidden)]
pub fn shortcut_portal_request(
    connection: &Connection,
    owner: &str,
    method: &str,
    mut options: PropMap,
    binding: Option<(Path<'static>, Vec<(String, PropMap)>)>,
    stop: &AtomicBool,
    wake: RawFd,
) -> Result<PropMap> {
    let token = format!("request_{}", uuid::Uuid::new_v4().simple());
    option(&mut options, "handle_token", token.clone());
    let sender = connection
        .unique_name()
        .to_string()
        .trim_start_matches(':')
        .replace('.', "_");
    let path = Path::new(format!(
        "/org/freedesktop/portal/desktop/request/{sender}/{token}"
    ))
    .map_err(anyhow::Error::msg)?;
    let response = Arc::new(Mutex::new(None));
    let output = response.clone();
    let subscription = connection.add_match(
        MatchRule::new_signal("org.freedesktop.portal.Request", "Response")
            .with_sender(owner.to_owned())
            .with_path(path.clone()),
        move |value: (u32, PropMap), _, _| {
            let mut slot = output.lock().unwrap();
            if slot.is_none() {
                *slot = Some(value);
            }
            true
        },
    )?;
    let result = (|| {
        let proxy = connection.with_proxy(owner.to_owned(), OBJECT, Duration::from_secs(5));
        ensure!(
            method == "CreateSession" || method == "BindShortcuts",
            "Unsupported shortcut portal operation"
        );
        let handle: (Path<'static>,) = if let Some((session, shortcuts)) = binding {
            ensure!(
                shortcuts.len() <= 16,
                "Shortcut request exceeds its binding limit"
            );
            proxy.method_call(INTERFACE, method, (session, shortcuts, "", options))?
        } else {
            proxy.method_call(INTERFACE, method, (options,))?
        };
        ensure!(
            handle.0 == path,
            "Portal returned an unexpected request handle"
        );
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            let mut full_batch = true;
            for _ in 0..64 {
                if !connection.process(Duration::ZERO)? {
                    full_batch = false;
                    break;
                }
            }
            if let Some((code, result)) = response.lock().unwrap().take() {
                ensure!(code == 0, "Shortcut registration canceled or denied");
                return Ok(result);
            }
            ensure!(
                !stop.load(Ordering::Acquire),
                "Shortcut registration canceled during shutdown"
            );
            ensure!(Instant::now() < deadline, "Shortcut registration timed out");
            if full_batch {
                continue;
            }
            super::wait(connection, None, wake, Some(deadline))?;
        }
    })();
    if result.is_err() {
        let _: Result<(), dbus::Error> = connection
            .with_proxy(owner.to_owned(), path, Duration::from_secs(2))
            .method_call("org.freedesktop.portal.Request", "Close", ());
    }
    connection.remove_match(subscription).ok();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wanted() -> BTreeMap<String, (CaptureCommand, String)> {
        [
            (
                "capture_0".into(),
                (CaptureCommand::Primary, "Ctrl+Alt+F9".into()),
            ),
            (
                "capture_1".into(),
                (CaptureCommand::Region, "Super+A".into()),
            ),
        ]
        .into()
    }
    fn result(values: &[(&str, &str)]) -> PropMap {
        let shortcuts = values
            .iter()
            .map(|(id, trigger)| {
                let mut fields = PropMap::new();
                option(&mut fields, "trigger_description", trigger.to_string());
                (id.to_string(), fields)
            })
            .collect::<Vec<_>>();
        let mut result = PropMap::new();
        option(&mut result, "shortcuts", shortcuts);
        result
    }
    #[test]
    fn portal_preferences_use_xdg_syntax() {
        assert_eq!(preferred("Ctrl+Alt+Shift+Super+A"), "CTRL+ALT+SHIFT+LOGO+a");
        assert_eq!(preferred("PrintScreen"), "Print");
        assert_eq!(preferred("Super+F24"), "LOGO+F24");
    }
    #[test]
    fn grant_requires_all_ids_while_change_signal_can_describe_a_subset() -> Result<()> {
        let values = result(&[("capture_0", "Portal choice")]);
        assert!(descriptions(&values, &wanted(), true).is_err());
        assert_eq!(
            descriptions(&values, &wanted(), false)?["capture_0"],
            "Portal choice"
        );
        Ok(())
    }
    #[test]
    fn malformed_assignments_are_not_reported_active() {
        for values in [
            result(&[("capture_0", "A"), ("capture_0", "B")]),
            result(&[("unknown", "A")]),
            result(&[("capture_0", "")]),
            result(&[("capture_0", "A\nB")]),
            result(&[("capture_0", &"A".repeat(257))]),
        ] {
            assert!(descriptions(&values, &wanted(), false).is_err());
        }
    }
    #[test]
    fn repeated_edges_coalesce_without_losing_the_final_release() {
        let queue: Signals = Arc::new(Mutex::new(VecDeque::new()));
        let path = Path::from("/org/freedesktop/portal/desktop/session/fixture/active");
        for _ in 0..100 {
            enqueue(&queue, Signal::Key(path.clone(), "capture_0".into(), true));
            enqueue(&queue, Signal::Key(path.clone(), "capture_0".into(), false));
        }
        let queue = queue.lock().unwrap();
        assert_eq!(queue.len(), 2);
        assert!(matches!(queue.front(), Some(Signal::Key(_, _, true))));
        assert!(matches!(queue.back(), Some(Signal::Key(_, _, false))));
    }
    #[test]
    fn closed_session_retires_its_queued_edges() {
        let queue: Signals = Arc::new(Mutex::new(VecDeque::new()));
        let path = Path::from("/org/freedesktop/portal/desktop/session/fixture/active");
        enqueue(&queue, Signal::Key(path.clone(), "capture_0".into(), true));
        enqueue(&queue, Signal::Closed(path));
        let queue = queue.lock().unwrap();
        assert_eq!(queue.len(), 1);
        assert!(matches!(queue.front(), Some(Signal::Closed(_))));
    }
}

#[cfg(test)]
mod consent_state_tests {
    use super::*;
    #[test]
    fn paused_key_state_never_becomes_a_late_capture_and_release_remains_usable() -> Result<()> {
        let path = Path::from("/org/freedesktop/portal/desktop/session/fixture/active");
        let mut shortcuts = Shortcuts::new(":1.42".into(), Arc::new(AtomicBool::new(true)));
        shortcuts.active = Some(Session {
            owner: ":1.42".into(),
            path: path.clone(),
            keys: [(
                "capture_0".into(),
                (CaptureCommand::Primary, "Ctrl+F9".into()),
            )]
            .into(),
            triggers: [("capture_0".into(), "Portal trigger".into())].into(),
            down: HashSet::new(),
        });
        assert_eq!(
            shortcuts.signal(Signal::State(path.clone(), "capture_0".into(), true))?,
            None
        );
        assert_eq!(
            shortcuts.signal(Signal::Key(path.clone(), "capture_0".into(), true))?,
            None
        );
        assert_eq!(
            shortcuts.signal(Signal::State(path.clone(), "capture_0".into(), false))?,
            None
        );
        assert_eq!(
            shortcuts.signal(Signal::Key(path, "capture_0".into(), true))?,
            Some(CaptureCommand::Primary)
        );
        Ok(())
    }
    #[test]
    fn paused_edges_coalesce_to_state_without_an_action_backlog() {
        let queue: Signals = Arc::new(Mutex::new(VecDeque::new()));
        let path = Path::from("/org/freedesktop/portal/desktop/session/fixture/active");
        for _ in 0..100 {
            enqueue(
                &queue,
                Signal::State(path.clone(), "capture_0".into(), true),
            );
            enqueue(
                &queue,
                Signal::State(path.clone(), "capture_0".into(), false),
            );
        }
        let queue = queue.lock().unwrap();
        assert_eq!(queue.len(), 1);
        assert!(matches!(queue.front(), Some(Signal::State(_, _, false))));
    }
}
