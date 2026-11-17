//! The separate task host uses the same supervised helper and bounded portal request protocol.
use super::*;
use dbus::{
    Path,
    arg::{PropMap, RefArg, Variant},
    blocking::Connection,
    message::MatchRule,
};
use std::collections::{HashMap, HashSet};

const SERVICE: &str = "org.freedesktop.portal.Desktop";
const OBJECT: &str = "/org/freedesktop/portal/desktop";
const INTERFACE: &str = "org.freedesktop.portal.GlobalShortcuts";
fn option(map: &mut PropMap, name: &str, value: impl RefArg + 'static) {
    map.insert(name.into(), Variant(Box::new(value)));
}
pub(super) fn run(state: Arc<Shared>, bindings: Vec<(String, String)>) -> Result<()> {
    let mut channel = dbus::channel::Channel::get_private(dbus::channel::BusType::Session)?;
    channel.set_watch_enabled(true);
    let connection = Connection::from(channel);
    connection.set_signal_match_mode(true);
    process::bus_ready()?;
    let owner = portal::shortcut_portal_owner(&connection)?;
    let (control, _keep_control) = UnixStream::pair()?;
    control.set_nonblocking(true)?;
    let token = format!("screenshots_{}", uuid::Uuid::new_v4().simple());
    let sender = connection
        .unique_name()
        .to_string()
        .trim_start_matches(':')
        .replace('.', "_");
    let session = Path::new(format!(
        "/org/freedesktop/portal/desktop/session/{sender}/{token}"
    ))
    .map_err(anyhow::Error::msg)?;
    let mut options = PropMap::new();
    option(&mut options, "session_handle_token", token);
    let result = (|| {
        let created = portal::shortcut_portal_request(
            &connection,
            &owner,
            "CreateSession",
            options,
            None,
            &state.stop,
            control.as_raw_fd(),
        )?;
        ensure!(
            created.get("session_handle").and_then(|v| v.0.as_str()) == Some(session.as_ref()),
            "Task portal returned an unexpected session"
        );
        let mut wanted = HashMap::new();
        let shortcuts = bindings
            .iter()
            .map(|(id, key)| -> Result<_> {
                let canonical = CaptureHotkeys([
                    key.clone(),
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                ])
                .validated()?
                .0[0]
                    .clone();
                let mut info = PropMap::new();
                option(&mut info, "description", id.clone());
                option(
                    &mut info,
                    "preferred_trigger",
                    portal::preferred(&canonical),
                );
                wanted.insert(id.clone(), canonical);
                Ok((id.clone(), info))
            })
            .collect::<Result<Vec<_>>>()?;
        let granted = portal::shortcut_portal_request(
            &connection,
            &owner,
            "BindShortcuts",
            PropMap::new(),
            Some((session.clone(), shortcuts)),
            &state.stop,
            control.as_raw_fd(),
        )?;
        let values = granted
            .get("shortcuts")
            .and_then(|v| v.0.as_iter())
            .context("Task portal did not confirm bindings")?;
        let mut ids = HashSet::new();
        for value in values.take(17) {
            ensure!(
                ids.len() < wanted.len(),
                "Task portal returned excessive bindings"
            );
            let id = value
                .as_iter()
                .and_then(|mut f| f.next())
                .and_then(RefArg::as_str)
                .context("Invalid task portal ID")?;
            ensure!(
                id.len() <= 64 && wanted.contains_key(id) && ids.insert(id.to_owned()),
                "Unexpected or duplicate task portal ID"
            );
        }
        ensure!(
            ids.len() == wanted.len(),
            "Task portal did not grant every configured binding"
        );
        let ids = Arc::new(ids);
        let down = Arc::new(Mutex::new(HashSet::<String>::new()));
        for (name, pressed) in [("Activated", true), ("Deactivated", false)] {
            let session = session.clone();
            let ids = ids.clone();
            let down = down.clone();
            let mut rule = MatchRule::new_signal(INTERFACE, name)
                .with_sender(owner.clone())
                .with_path(OBJECT);
            rule.strict_sender = true;
            connection.add_match(
                rule,
                move |(actual, id, _, _): (Path<'static>, String, u64, PropMap), _, _| {
                    if actual == session && id.len() <= 64 && ids.contains(&id) {
                        let mut down = down.lock().unwrap();
                        if !pressed {
                            down.remove(&id);
                        } else if down.insert(id.clone()) {
                            process::task_event(id);
                        }
                    }
                    true
                },
            )?;
        }
        let closed = state.clone();
        let mut rule = MatchRule::new_signal("org.freedesktop.portal.Session", "Closed")
            .with_sender(owner.clone())
            .with_path(session.clone());
        rule.strict_sender = true;
        connection.add_match(rule, move |(): (), _, _| {
            closed.stop.store(true, Ordering::Release);
            true
        })?;
        let gone = state.clone();
        let expected = owner.clone();
        let rule = super::daemon_owner_rule();
        connection.add_match_no_cb(&format!("{},arg0='{}'", rule.match_str(), SERVICE))?;
        use dbus::channel::MatchingReceiver;
        connection.start_receive(
            rule,
            Box::new(move |message, _| {
                if let Ok((name, old, new)) = message.read3::<String, String, String>() {
                    if name == SERVICE && old == expected && old != new {
                        gone.stop.store(true, Ordering::Release);
                    }
                }
                true
            }),
        );
        // Retire input queued before the task host became ready.
        loop {
            process::pump()?;
            ensure!(
                !state.stop.load(Ordering::Acquire),
                "Task portal startup canceled"
            );
            let mut full = true;
            for _ in 0..64 {
                if !connection.process(Duration::ZERO)? {
                    full = false;
                    break;
                }
            }
            if !full {
                break;
            }
        }
        state.allow_capture.store(true, Ordering::Release);
        process::ready()?;
        while !state.stop.load(Ordering::Acquire) {
            process::pump()?;
            let mut full = true;
            for _ in 0..64 {
                if !connection.process(Duration::ZERO)? {
                    full = false;
                    break;
                }
            }
            if state.stop.load(Ordering::Acquire) {
                break;
            }
            if !full {
                super::wait(&connection, None, control.as_raw_fd(), None)?;
            }
        }
        Ok(())
    })();
    let cleanup: Result<(), dbus::Error> = connection
        .with_proxy(owner, session, Duration::from_secs(2))
        .method_call("org.freedesktop.portal.Session", "Close", ());
    result.and(cleanup.map_err(Into::into))
}
