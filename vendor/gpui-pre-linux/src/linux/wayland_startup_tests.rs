//! Pure startup API regressions in fresh child processes with owned Unix socket pairs.
//! No compositor, graphical platform, clipboard, user display or user input is opened.
use super::*;
use std::{
    os::fd::{AsRawFd, IntoRawFd},
    os::unix::net::UnixStream,
    process::Command,
};

const CASE: &str = "GPUI_PRIVATE_WAYLAND_STARTUP_CASE";

#[test]
fn descriptor_startup_preserves_authority_without_recapturing_an_fd() {
    let module = module_path!().split_once("::").unwrap().1;
    for scenario in [
        "descriptor_only",
        "descriptor_with_x11",
        "descriptor_with_named_wayland_and_x11",
        "descriptor_consumed_before_detect",
        "descriptor_before_logger_thread",
        "descriptor_disallowed_wayland",
        "descriptor_closed",
        "descriptor_peer_closed",
        "descriptor_invalid",
        "descriptor_empty",
        "descriptor_headless_valid",
        "descriptor_headless_invalid",
        "descriptor_headless_closed",
        "named_wayland_with_x11",
        "named_x11",
        "x11_client_in_wayland_session",
    ] {
        #[allow(
            clippy::disallowed_methods,
            reason = "wait for one private API-only child"
        )]
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!("{module}::private_startup_api_case"),
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env_clear()
            .env(CASE, scenario)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{scenario}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
#[ignore = "invoked only by the private API parent with an empty process environment"]
fn private_startup_api_case() {
    let scenario = std::env::var(CASE).expect("private API case required");
    assert!(std::env::var_os("DISPLAY").is_none());
    assert!(std::env::var_os("WAYLAND_DISPLAY").is_none());
    assert!(std::env::var_os("WAYLAND_SOCKET").is_none());
    assert!(std::env::var_os("XDG_RUNTIME_DIR").is_none());
    let mut descriptor = None;
    let mut peer = None;
    // This child has no native platform or optional services, and one selected test thread.
    unsafe {
        if scenario != "descriptor_only" {
            std::env::set_var("DISPLAY", ":private-startup-api-never-connected");
        }
        if matches!(
            scenario.as_str(),
            "named_wayland_with_x11" | "descriptor_with_named_wayland_and_x11"
        ) {
            std::env::set_var("WAYLAND_DISPLAY", "/private-startup-api-never-connected");
        }
        if scenario == "x11_client_in_wayland_session" {
            std::env::set_var("XDG_SESSION_TYPE", "wayland");
        }
        if scenario == "descriptor_before_logger_thread" {
            std::env::set_var("XDG_ACTIVATION_TOKEN", "private-owned-startup-token");
        }
        if scenario.starts_with("descriptor_") {
            if matches!(
                scenario.as_str(),
                "descriptor_invalid" | "descriptor_empty" | "descriptor_headless_invalid"
            ) {
                std::env::set_var(
                    "WAYLAND_SOCKET",
                    if scenario == "descriptor_empty" {
                        ""
                    } else {
                        "invalid"
                    },
                );
            } else {
                let (socket, other) = UnixStream::pair().unwrap();
                let fd = socket.as_raw_fd();
                assert!(fd >= 0);
                if matches!(
                    scenario.as_str(),
                    "descriptor_closed" | "descriptor_headless_closed"
                ) {
                    drop(socket);
                    assert_eq!(libc::fcntl(fd, libc::F_GETFD), -1);
                } else {
                    let flags = libc::fcntl(fd, libc::F_GETFD);
                    assert!(flags >= 0);
                    assert_eq!(libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC), 0);
                    assert_eq!(socket.into_raw_fd(), fd);
                    descriptor = Some(fd);
                }
                if scenario != "descriptor_peer_closed" {
                    peer = Some(other);
                }
                std::env::set_var("WAYLAND_SOCKET", fd.to_string());
            }
        }
    }
    let mut startup = StartupEnvironment::take();
    assert!(std::env::var_os("WAYLAND_SOCKET").is_none());
    if scenario.starts_with("descriptor_headless_") {
        assert!(matches!(
            startup.initial_request(WindowingModes::HEADLESS),
            WindowingRequest::Headless
        ));
        assert!(matches!(
            startup.initial_request(WindowingModes::all()),
            WindowingRequest::Windowed(_)
        ));
        // Headless-only startup must not weaken windowed authority admission.
        assert!(
            startup
                .backend(
                    WindowingModes::X11 | WindowingModes::HEADLESS,
                    &startup.environment
                )
                .is_err()
        );
        if let Some(fd) = descriptor {
            assert_eq!(
                startup
                    .wayland_socket
                    .as_ref()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .as_raw_fd(),
                fd,
                "headless bootstrap duplicated the inherited descriptor"
            );
        }
        drop(startup);
        if let Some(fd) = descriptor {
            assert_eq!(unsafe { libc::fcntl(fd, libc::F_GETFD) }, -1);
            let mut peer = peer.unwrap();
            peer.set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            assert_eq!(std::io::Read::read(&mut peer, &mut [0u8; 1]).unwrap(), 0);
        }
        return;
    }
    let logger = if scenario == "descriptor_before_logger_thread" {
        assert_eq!(
            startup.activation_token.as_deref(),
            Some("private-owned-startup-token")
        );
        let (started_sender, started) = std::sync::mpsc::sync_channel(0);
        let (finish, finished) = std::sync::mpsc::sync_channel(0);
        let thread = std::thread::spawn(move || {
            assert!(std::env::var_os("WAYLAND_SOCKET").is_none());
            assert!(std::env::var_os("XDG_ACTIVATION_TOKEN").is_none());
            started_sender.send(()).unwrap();
            finished.recv().unwrap();
        });
        started.recv().unwrap();
        // The event-loop stage must use owned evidence while a service thread is alive.
        assert!(matches!(
            startup
                .backend(WindowingModes::all(), &GraphicalEnvironment::detect())
                .unwrap(),
            Some(Backend::Wayland)
        ));
        Some((finish, thread))
    } else {
        None
    };
    let expected_authority = scenario != "named_x11";
    assert_eq!(startup.environment.wayland_session, expected_authority);
    let modes = if scenario == "descriptor_disallowed_wayland" {
        WindowingModes::X11 | WindowingModes::HEADLESS
    } else {
        WindowingModes::all()
    };
    let selected = startup.backend(modes, &startup.environment);
    if matches!(
        scenario.as_str(),
        "descriptor_closed"
            | "descriptor_invalid"
            | "descriptor_empty"
            | "descriptor_disallowed_wayland"
    ) {
        assert!(
            selected.is_err(),
            "invalid authority must not select the supplied X11 display"
        );
    } else if matches!(
        scenario.as_str(),
        "named_x11" | "x11_client_in_wayland_session"
    ) {
        assert!(matches!(selected.unwrap(), Some(Backend::X11)));
    } else {
        assert!(matches!(selected.unwrap(), Some(Backend::Wayland)));
    }
    if scenario == "descriptor_consumed_before_detect" {
        let detected_after_consumption = GraphicalEnvironment::detect();
        assert!(!detected_after_consumption.wayland_session);
        assert!(matches!(
            startup.backend(modes, &detected_after_consumption).unwrap(),
            Some(Backend::Wayland)
        ));
        let second = StartupEnvironment::take();
        assert!(
            second.wayland_socket.is_none(),
            "the owned descriptor must only be taken once"
        );
    }
    if let Some(fd) = descriptor {
        assert_ne!(
            unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        let stream = startup.wayland_socket.take().unwrap().unwrap();
        assert_eq!(
            stream.as_raw_fd(),
            fd,
            "bootstrap duplicated or replaced the authorized fd"
        );
        if scenario == "descriptor_peer_closed" {
            // Exercise the native Wayland client on an owned disconnected socket only.
            let native = wayland_client::Connection::from_socket(stream);
            if let Ok(native) = native {
                assert!(native.roundtrip().is_err());
            }
        } else {
            drop(stream);
        }
        assert_eq!(unsafe { libc::fcntl(fd, libc::F_GETFD) }, -1);
    }
    drop(peer);
    if let Some((finish, thread)) = logger {
        finish.send(()).unwrap();
        thread.join().unwrap();
    }
}
