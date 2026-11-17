// Included inside capture_worker::tests; all transfers use its private nonce helper.
#[test]
fn owned_wayland_context_reaches_the_private_worker_without_a_descriptor() {
    let _serial = lock(&SERIAL);
    settle();
    let mut command = fake_command("wayland_session_context", None);
    command
        .env("DISPLAY", ":private-owned-context-never-connected")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("WAYLAND_SOCKET")
        .env_remove("XDG_SESSION_TYPE");
    restrict_command_context(
        &mut command,
        app_core::NativeCaptureContext {
            wayland_session: true,
        },
    );
    let result = execute_command(command, call(), &|| Ok(()), Duration::from_secs(5));
    assert!(result.is_ok(), "{:?}", result.err());
    settle();
}

#[test]
fn owned_wayland_context_rejects_automatic_clipboard_before_private_worker_bootstrap() {
    let _serial = lock(&SERIAL);
    settle();
    let folder = Folder::new();
    let marker = folder.0.join("worker-started");
    let mut command = fake_command("success", Some(&marker));
    command
        .env("DISPLAY", ":private-owned-context-never-connected")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("WAYLAND_SOCKET")
        .env_remove("XDG_SESSION_TYPE");
    restrict_command_context(
        &mut command,
        app_core::NativeCaptureContext {
            wayland_session: true,
        },
    );
    let refused = execute_command(
        command,
        NativeCaptureCall::ClipboardInspect {
            kind: app_core::ClipboardInspectKind::Text,
            backend: app_core::NativeClipboardBackend::Automatic,
        },
        &|| Ok(()),
        Duration::from_secs(5),
    );
    assert!(matches!(refused, Err(CaptureError::Clipboard)));
    assert!(!marker.exists());
    settle();
}

#[test]
fn default_job_context_never_erases_ambient_wayland_or_changes_an_x11_command() {
    for (session, socket, wayland) in [
        (Some("wayland"), None, true),
        (None, Some("invalid"), true),
        (None, Some(""), true),
        (Some("x11"), None, false),
        (None, None, false),
    ] {
        let mut command = Command::new("private-command-not-executed");
        command
            .env("DISPLAY", ":private-context-never-connected")
            .env_remove("WAYLAND_DISPLAY");
        if let Some(value) = session {
            command.env("XDG_SESSION_TYPE", value);
        } else {
            command.env_remove("XDG_SESSION_TYPE");
        }
        if let Some(value) = socket {
            command.env("WAYLAND_SOCKET", value);
        } else {
            command.env_remove("WAYLAND_SOCKET");
        }
        let before = command
            .get_envs()
            .map(|(key, value)| (key.to_owned(), value.map(std::ffi::OsStr::to_owned)))
            .collect::<Vec<_>>();
        restrict_command_context(&mut command, app_core::NativeCaptureContext::default());
        assert_eq!(command_wayland_context(&command), wayland);
        assert_eq!(
            command
                .get_envs()
                .map(|(key, value)| (key.to_owned(), value.map(std::ffi::OsStr::to_owned)))
                .collect::<Vec<_>>(),
            before
        );
    }
}
