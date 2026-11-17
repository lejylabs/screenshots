//! Session-context checks use an empty-environment child and a synthetic executor only.
use super::*;
use std::process::Command;

const CASE: &str = "SCREENSHOTS_PRIVATE_CAPTURE_CONTEXT_CASE";

#[test]
fn owned_gui_authority_survives_consumed_environment_without_rejecting_plain_x11() {
    for case in [
        "gui_wayland",
        "gui_xwayland",
        "plain_x11",
        "raw_cli_wayland",
    ] {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "capture::capture_session_context_tests::private_capture_context_case",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env_clear()
            .env(CASE, case)
            .env("DISPLAY", ":private-context-never-connected")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{case}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
#[ignore = "fresh private subprocess; installs a synthetic executor and never opens a display"]
fn private_capture_context_case() {
    let case = std::env::var(CASE).expect("private capture context case required");
    assert!(std::env::var_os("WAYLAND_DISPLAY").is_none());
    assert!(std::env::var_os("WAYLAND_SOCKET").is_none());
    assert!(std::env::var_os("XDG_RUNTIME_DIR").is_none());
    if case == "raw_cli_wayland" {
        // No platform or native service runs in this single selected test process.
        unsafe { std::env::set_var("WAYLAND_SOCKET", "invalid-owned-test-context") };
    }
    let owned = matches!(case.as_str(), "gui_wayland" | "gui_xwayland");
    let backend =
        XcapBackend::new(PathBuf::new()).with_native_context(crate::NativeCaptureContext {
            wayland_session: owned,
        });
    let restricted = owned || case == "raw_cli_wayland";
    let capabilities = backend.capabilities();
    assert_eq!(capabilities.capture_display, !restricted);
    assert_eq!(capabilities.capture_region, !restricted);
    assert!(capabilities.list_windows && capabilities.capture_window);
    if restricted {
        let refused = backend.capture_batch(vec![CaptureRequest::Display("private".into())]);
        assert!(matches!(refused, Err(CaptureError::Unsupported(_))));
    }
    if case == "raw_cli_wayland" {
        let refused = crate::capture_clipboard_image_with_cancel(
            crate::NativeClipboardBackend::Automatic,
            &|| Ok(()),
        );
        assert!(matches!(refused, Err(CaptureError::Clipboard)));
    }
    crate::install_native_capture_executor(assert_context_executor).unwrap();
    assert!(matches!(
        backend.list_sources_with_cancel(&|| Ok(())),
        Err(CaptureError::NativeCanceled)
    ));
}

fn assert_context_executor(
    call: crate::NativeCaptureCall,
    check: &crate::NativeCaptureCheck<'_>,
    context: crate::NativeCaptureContext,
) -> Result<crate::NativeCaptureReply, CaptureError> {
    check()?;
    assert!(matches!(call, crate::NativeCaptureCall::Inventory));
    let case = std::env::var(CASE).unwrap();
    assert_eq!(
        context.wayland_session,
        matches!(case.as_str(), "gui_wayland" | "gui_xwayland")
    );
    Err(CaptureError::NativeCanceled)
}
