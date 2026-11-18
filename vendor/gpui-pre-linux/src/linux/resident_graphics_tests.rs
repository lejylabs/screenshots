//! Opt-in private-compositor probe; never opens a user capture or clipboard.
use super::*;
use gpui::{AppContext as _, Context, IntoElement, Render, Styled, Window, WindowOptions};
use std::{cell::Cell, os::unix::fs::MetadataExt, path::PathBuf};

struct Blank;
impl Render for Blank {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::div().size_full().bg(gpui::rgb(0x314159))
    }
}

fn graphics_slot(platform: &LinuxPlatform) -> gpui_wgpu::GpuContext {
    match &*platform.connection.borrow() {
        #[cfg(feature = "wayland")]
        DisplayConnection::Wayland(connection) => connection.resident_graphics_context(),
        #[cfg(feature = "x11")]
        DisplayConnection::X11(connection) => connection.resident_graphics_context(),
        DisplayConnection::Headless(_) => panic!("native private display is required"),
    }
}

fn compositor_hint(platform: &LinuxPlatform) -> Option<(u32, u32)> {
    let hint = match &*platform.connection.borrow() {
        #[cfg(feature = "wayland")]
        DisplayConnection::Wayland(connection) => connection.resident_compositor_gpu_hint(),
        #[cfg(feature = "x11")]
        DisplayConnection::X11(connection) => connection.resident_compositor_gpu_hint(),
        DisplayConnection::Headless(_) => panic!("native private display is required"),
    };
    hint.map(|hint| (hint.vendor_id, hint.device_id))
}

fn set_compositor_hint(platform: &LinuxPlatform, hint: gpui_wgpu::CompositorGpuHint) {
    match &*platform.connection.borrow() {
        #[cfg(feature = "wayland")]
        DisplayConnection::Wayland(connection) => connection.set_resident_compositor_gpu_hint(hint),
        #[cfg(feature = "x11")]
        DisplayConnection::X11(connection) => connection.set_resident_compositor_gpu_hint(hint),
        DisplayConnection::Headless(_) => panic!("native private display is required"),
    }
}

fn adapter_identity(slot: &gpui_wgpu::GpuContext) -> (u32, u32, String, String) {
    let context = slot.borrow();
    let info = context
        .as_ref()
        .expect("live native window owns its GPU context")
        .adapter
        .get_info();
    (
        info.vendor,
        info.device,
        info.name,
        format!("{:?}", info.backend),
    )
}

#[test]
#[ignore = "requires a fresh process under an owned private compositor and SCREENSHOTS_PRIVATE_GRAPHICS_ROOT"]
fn private_last_window_releases_graphics_and_reopens_on_same_connection() {
    let root = PathBuf::from(
        std::env::var_os("SCREENSHOTS_PRIVATE_GRAPHICS_ROOT")
            .expect("private graphics fixture root required"),
    )
    .canonicalize()
    .unwrap();
    assert!(
        root.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("screenshots-private-graphics-")
    );
    assert_eq!(root.metadata().unwrap().mode() & 0o077, 0);
    for (name, relative) in [
        ("HOME", "home"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_RUNTIME_DIR", "runtime"),
    ] {
        assert_eq!(
            PathBuf::from(std::env::var_os(name).unwrap())
                .canonicalize()
                .unwrap(),
            root.join(relative)
        );
    }
    assert!(
        std::env::var_os("DBUS_SESSION_BUS_ADDRESS")
            .expect("private D-Bus address required")
            .to_string_lossy()
            .contains(root.to_str().unwrap())
    );
    let backend =
        std::env::var("SCREENSHOTS_PRIVATE_GRAPHICS_BACKEND").expect("private backend required");
    match backend.as_str() {
        "wayland" => {
            assert!(std::env::var_os("DISPLAY").is_none());
            if std::env::var_os("WAYLAND_SOCKET").is_none() {
                let display = PathBuf::from(std::env::var_os("WAYLAND_DISPLAY").unwrap());
                let socket = if display.is_absolute() {
                    display
                } else {
                    root.join("runtime").join(display)
                };
                assert!(
                    socket
                        .canonicalize()
                        .unwrap()
                        .starts_with(root.join("runtime"))
                );
            }
        }
        "x11" => {
            assert!(std::env::var_os("WAYLAND_DISPLAY").is_none());
            assert!(std::env::var_os("WAYLAND_SOCKET").is_none());
            assert!(
                std::env::var("DISPLAY")
                    .unwrap()
                    .strip_prefix(':')
                    .is_some_and(|value| !value.is_empty()
                        && value
                            .bytes()
                            .all(|value| value.is_ascii_digit() || value == b'.'))
            );
            assert!(
                PathBuf::from(std::env::var_os("XAUTHORITY").expect("private Xauthority required"))
                    .canonicalize()
                    .unwrap()
                    .starts_with(&root)
            );
        }
        _ => panic!("private backend must be x11 or wayland"),
    }
    assert!(std::env::var_os("ZED_HEADLESS").is_none());
    let platform = Rc::new(LinuxPlatform::new(WindowingModes::all()));
    let finished = Rc::new(Cell::new(false));
    gpui::Application::with_platform(platform.clone())
        .with_quit_mode(gpui::QuitMode::Explicit)
        .run({
            let finished = finished.clone();
            move |cx| {
                assert_eq!(
                    cx.compositor_name(),
                    if backend == "wayland" {
                        "Wayland"
                    } else {
                        "X11"
                    }
                );
                let slot = graphics_slot(&platform);
                assert!(slot.borrow().is_none());
                let mut expected_hint = compositor_hint(&platform);
                let mut expected_adapter = None;
                for _ in 0..8 {
                    let first = cx
                        .open_window(WindowOptions::default(), |_, cx| cx.new(|_| Blank))
                        .unwrap();
                    assert_eq!(
                        compositor_hint(&platform),
                        expected_hint,
                        "window creation must not consume the compositor adapter hint"
                    );
                    let adapter = adapter_identity(&slot);
                    if let Some(expected) = &expected_adapter {
                        assert_eq!(
                            &adapter, expected,
                            "graphics recreation must select the same advertised adapter identity"
                        );
                    } else {
                        expected_adapter = Some(adapter.clone());
                    }
                    if expected_hint.is_none() {
                        // Private software compositors may have no DRM hint; exercise its ownership with the selected adapter.
                        set_compositor_hint(
                            &platform,
                            gpui_wgpu::CompositorGpuHint {
                                vendor_id: adapter.0,
                                device_id: adapter.1,
                            },
                        );
                        expected_hint = Some((adapter.0, adapter.1));
                    }
                    let second = cx
                        .open_window(WindowOptions::default(), |_, cx| cx.new(|_| Blank))
                        .unwrap();
                    assert!(
                        slot.borrow().is_some(),
                        "real native windows must create the GPU context"
                    );
                    assert_eq!(
                        compositor_hint(&platform),
                        expected_hint,
                        "additional windows preserve the next recreation's adapter preference"
                    );
                    assert_eq!(adapter_identity(&slot), adapter);
                    first
                        .update(cx, |_, window, _| window.remove_window())
                        .unwrap();
                    assert!(
                        slot.borrow().is_some(),
                        "the second consumer still owns graphics"
                    );
                    second
                        .update(cx, |_, window, _| window.remove_window())
                        .unwrap();
                    assert!(
                        slot.borrow().is_none(),
                        "last native window must retire the shared graphics owner"
                    );
                    assert_eq!(
                        compositor_hint(&platform),
                        expected_hint,
                        "graphics retirement retains adapter preference metadata"
                    );
                    assert!(
                        Rc::ptr_eq(&slot, &graphics_slot(&platform)),
                        "reopen must retain the native display/clipboard connection"
                    );
                }
                assert!(cx.windows().is_empty());
                finished.set(true);
                cx.quit();
            }
        });
    assert!(finished.get());
}
