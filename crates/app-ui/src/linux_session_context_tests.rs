use super::*;

#[test]
fn focused_paste_uses_the_actual_client_independently_of_session_authority() {
    for client in ["Wayland", "headless", ""] {
        assert_eq!(
            focused_clipboard_backend(false, client),
            app_core::NativeClipboardBackend::Automatic
        );
    }
    assert_eq!(
        focused_clipboard_backend(false, "X11"),
        app_core::NativeClipboardBackend::FocusedX11
    );
    assert_eq!(
        focused_clipboard_backend(true, "X11"),
        app_core::NativeClipboardBackend::Automatic
    );
    assert_eq!(
        focused_clipboard_backend(true, "Wayland"),
        app_core::NativeClipboardBackend::Automatic
    );
}
