//! Session-portal shortcuts, waiting on the bus fd and a shutdown pipe (no idle timer).
//! Protocol: https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.GlobalShortcuts.html
use super::{Host, Wake};
use anyhow::{Context, Result, ensure};
use dbus::{
    Path,
    arg::{PropMap, RefArg, Variant},
    blocking::Connection,
    message::MatchRule,
};
use std::{
    io::Write,
    os::{fd::AsRawFd, unix::net::UnixStream},
    sync::{Arc, Mutex, atomic::Ordering},
    time::{Duration, Instant},
};

const SERVICE: &str = "org.freedesktop.portal.Desktop";
const OBJECT: &str = "/org/freedesktop/portal/desktop";
const INTERFACE: &str = "org.freedesktop.portal.GlobalShortcuts";
const RESPONSE: &str = "org.freedesktop.portal.Request";

fn option(map: &mut PropMap, key: &str, value: impl RefArg + 'static) {
    map.insert(key.into(), Variant(Box::new(value)));
}
fn preferred(key: &super::Key) -> String {
    let mut parts = Vec::new();
    for (bit, name) in [(1, "CTRL"), (2, "ALT"), (4, "SHIFT"), (8, "LOGO")] {
        if key.modifiers & bit != 0 {
            parts.push(name.to_owned());
        }
    }
    parts.push(if key.name == "PrintScreen" {
        "Print".into()
    } else if key.name.len() == 1 {
        key.name.to_ascii_lowercase()
    } else {
        key.name.clone()
    });
    parts.join("+")
}

pub(super) struct Native {
    connection: Connection,
    session: Path<'static>,
    host: Arc<Host>,
    read: UnixStream,
    write: Arc<UnixStream>,
}

impl Native {
    pub(super) fn new(host: Arc<Host>) -> Result<Self> {
        let mut channel = dbus::channel::Channel::get_private(dbus::channel::BusType::Session)
            .context("Could not connect to the shortcut session portal")?;
        channel.set_watch_enabled(true);
        let connection = Connection::from(channel);
        let (read, write) = UnixStream::pair()?;
        read.set_nonblocking(true)?;
        write.set_nonblocking(true)?;
        let token = format!("screenshots_{}", uuid::Uuid::new_v4().simple());
        let mut options = PropMap::new();
        option(&mut options, "session_handle_token", token);
        let mut native = Self {
            connection,
            session: Path::from("/"),
            host,
            read,
            write: Arc::new(write),
        };
        *native.host.wake.lock().unwrap() = Some(native.wake());
        let result = native.request("CreateSession", options, None)?;
        let path = result
            .get("session_handle")
            .and_then(|v| v.0.as_str())
            .context("Shortcut portal did not provide a session")?;
        ensure!(
            path.len() <= 512 && path.starts_with("/org/freedesktop/portal/desktop/session/"),
            "Invalid shortcut portal session"
        );
        native.session = Path::new(path.to_owned())
            .map_err(|_| anyhow::anyhow!("Invalid shortcut portal session"))?;
        let result = native.request("BindShortcuts", PropMap::new(), Some(&native.session))?;
        // Decode the returned list through the typed message parser, never treat a partial grant as success.
        let shortcuts = result
            .get("shortcuts")
            .and_then(|v| v.0.as_iter())
            .context("Shortcut portal did not confirm bindings")?;
        let mut accepted = std::collections::HashSet::new();
        let mut count = 0;
        for shortcut in shortcuts.take(17) {
            count += 1;
            ensure!(
                count <= native.host.config.keys.len(),
                "Shortcut portal returned excessive bindings"
            );
            let id = shortcut
                .as_iter()
                .and_then(|mut fields| fields.next())
                .and_then(RefArg::as_str)
                .context("Invalid shortcut portal binding")?;
            ensure!(id.len() <= 64, "Shortcut portal ID exceeds its limit");
            ensure!(
                accepted.insert(id.to_owned()),
                "Shortcut portal returned duplicate bindings"
            );
        }
        ensure!(
            accepted.len() == native.host.config.keys.len()
                && native
                    .host
                    .config
                    .keys
                    .iter()
                    .all(|key| accepted.contains(&native.host.config.bindings[key.profile].id)),
            "Shortcut portal did not grant every configured binding"
        );
        let session = native.session.clone();
        let owner = native.host.clone();
        let mut rule = MatchRule::new_signal(INTERFACE, "Activated").with_sender(SERVICE);
        rule.path = Some(Path::from(OBJECT));
        native
            .connection
            .add_match(
                rule,
                move |(actual, id, _time, _options): (Path<'static>, String, u64, PropMap),
                      _,
                      _| {
                    if actual == session && id.len() <= 64 {
                        if let Some(key) = owner
                            .config
                            .keys
                            .iter()
                            .find(|key| owner.config.bindings[key.profile].id == id)
                        {
                            let _ = owner.trigger(key.profile);
                        }
                    }
                    true
                },
            )
            .context("Could not observe portal shortcut activation")?;
        let owner = native.host.clone();
        let closed = MatchRule::new_signal("org.freedesktop.portal.Session", "Closed")
            .with_sender(SERVICE)
            .with_path(native.session.clone());
        native.connection.add_match(closed, move |(): (), _, _| {
            owner.stop();
            true
        })?;
        Ok(native)
    }

    fn request(
        &self,
        method: &str,
        mut options: PropMap,
        session: Option<&Path<'static>>,
    ) -> Result<PropMap> {
        let token = format!("request_{}", uuid::Uuid::new_v4().simple());
        option(&mut options, "handle_token", token.clone());
        let sender = self
            .connection
            .unique_name()
            .to_string()
            .trim_start_matches(':')
            .replace('.', "_");
        let path = Path::new(format!(
            "/org/freedesktop/portal/desktop/request/{sender}/{token}"
        ))
        .unwrap();
        let received = Arc::new(Mutex::new(None));
        let result = received.clone();
        let rule = MatchRule::new_signal(RESPONSE, "Response")
            .with_sender(SERVICE)
            .with_path(path.clone());
        let subscription =
            self.connection
                .add_match(rule, move |value: (u32, PropMap), _, _| {
                    *result.lock().unwrap() = Some(value);
                    true
                })?;
        let proxy = self
            .connection
            .with_proxy(SERVICE, OBJECT, Duration::from_secs(5));
        let handle: Result<(Path<'static>,), dbus::Error> = if let Some(session) = session {
            let shortcuts = self
                .host
                .config
                .keys
                .iter()
                .map(|key| {
                    let binding = &self.host.config.bindings[key.profile];
                    let mut info = PropMap::new();
                    option(&mut info, "description", binding.id.clone());
                    option(&mut info, "preferred_trigger", preferred(key));
                    (binding.id.clone(), info)
                })
                .collect::<Vec<_>>();
            proxy.method_call(INTERFACE, method, (session.clone(), shortcuts, "", options))
        } else {
            proxy.method_call(INTERFACE, method, (options,))
        };
        let result = (|| -> Result<PropMap> {
            ensure!(
                handle.context("Could not request portal shortcuts")?.0 == path,
                "Shortcut portal returned an unexpected request handle"
            );
            let deadline = Instant::now() + Duration::from_secs(180);
            loop {
                while self.connection.process(Duration::ZERO)? {}
                if let Some((code, result)) = received.lock().unwrap().take() {
                    ensure!(code == 0, "Shortcut portal request was canceled or denied");
                    return Ok(result);
                }
                ensure!(
                    !self.host.stopping.load(Ordering::SeqCst),
                    "Shortcut portal registration canceled"
                );
                ensure!(
                    Instant::now() < deadline,
                    "Shortcut portal registration timed out"
                );
                self.wait(Some(deadline))?;
            }
        })();
        self.connection.remove_match(subscription).ok();
        result
    }

    fn wait(&self, deadline: Option<Instant>) -> Result<()> {
        let watch = self.connection.channel().watch();
        let mut fds = [
            libc::pollfd {
                fd: watch.fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self.read.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let timeout = deadline
            .map(|d| {
                d.saturating_duration_since(Instant::now())
                    .as_millis()
                    .min(i32::MAX as u128) as i32
            })
            .unwrap_or(-1);
        let result = unsafe { libc::poll(fds.as_mut_ptr(), 2, timeout) };
        if result < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            anyhow::bail!("Shortcut portal event wait failed");
        }
        ensure!(
            fds[0].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) == 0,
            "Shortcut portal bus disconnected"
        );
        Ok(())
    }
    pub(super) fn wake(&self) -> Wake {
        let pipe = self.write.clone();
        Arc::new(move || {
            let _ = (&*pipe).write(&[1]);
        })
    }
    pub(super) fn run(&mut self) -> Result<()> {
        while !self.host.stopping.load(Ordering::SeqCst) {
            while self.connection.process(Duration::ZERO)? {}
            if !self.host.stopping.load(Ordering::SeqCst) {
                self.wait(None)?;
            }
        }
        Ok(())
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        if self.session.to_string() != "/" {
            let proxy =
                self.connection
                    .with_proxy(SERVICE, self.session.clone(), Duration::from_secs(2));
            let _: Result<(), _> = proxy.method_call("org.freedesktop.portal.Session", "Close", ());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn portal_shortcuts_use_the_xdg_notation() {
        assert_eq!(
            preferred(&super::super::Key {
                profile: 0,
                modifiers: 5,
                name: "F9".into()
            }),
            "CTRL+SHIFT+F9"
        );
        assert_eq!(
            preferred(&super::super::Key {
                profile: 0,
                modifiers: 8,
                name: "A".into()
            }),
            "LOGO+a"
        );
    }
}
