//! Opt-in owned-Xvfb fixture. No extra production controls, services or input injection.
use super::*;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    io::{BufRead, Read, Write},
    time::Duration,
};

fn reply(value: impl std::fmt::Display) {
    println!("DROP_PROBE {value}");
    std::io::stdout().flush().unwrap();
}

fn open(cx: &mut App, root: &std::path::Path) -> WindowHandle<CaptureWorkspace> {
    let output = root.join("output");
    let handle = cx
        .open_kit_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(100.), px(100.)),
                    size(px(900.), px(600.)),
                ))),
                ..app_window_options("Owned native drop fixture")
            },
            |window, cx| {
                let view = cx.new(|cx| {
                    CaptureWorkspace::with_native_services(
                        Some(gpui::Window::window_handle(window)),
                        false,
                        cx,
                    )
                });
                view.update(cx, |state, _| {
                    state.backend = XcapBackend::new(&output);
                    state.output_directory = Some(output);
                });
                let focus = view.read(cx).focus_handle.clone();
                window.focus(&focus, cx);
                view
            },
        )
        .unwrap();
    handle
        .update(cx, |_, window, _| {
            match HasWindowHandle::window_handle(window).unwrap().as_raw() {
                RawWindowHandle::Xcb(handle) => reply(format!("target {}", handle.window.get())),
                RawWindowHandle::Xlib(handle) => reply(format!("target {}", handle.window)),
                _ => panic!("Owned drop fixture requires X11"),
            }
        })
        .unwrap();
    handle
}

struct OutgoingProbe(PathBuf);
impl gpui::Render for OutgoingProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("owned-native-drag-source")
            .size_full()
            .on_drag(self.0.clone(), |_, _, _, cx| cx.new(|_| gpui::Empty))
            .external_drag_payload(|path: &PathBuf, _, _| {
                Some(gpui::ExternalDragPayload::Files(gpui::FileDragPaths::new(
                    [(path.clone(), false)],
                )))
            })
    }
}
fn outgoing(cx: &mut App, root: &std::path::Path) -> WindowHandle<OutgoingProbe> {
    use std::os::unix::ffi::OsStrExt;
    let path = root.join(std::ffi::OsStr::from_bytes(b"synthetic-\xff.png"));
    let handle = cx
        .open_kit_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(40.), px(40.)),
                    size(px(240.), px(180.)),
                ))),
                ..app_window_options("Owned outgoing drag fixture")
            },
            |_, cx| cx.new(|_| OutgoingProbe(path)),
        )
        .unwrap();
    handle
        .update(cx, |_, window, _| {
            match HasWindowHandle::window_handle(window).unwrap().as_raw() {
                RawWindowHandle::Xcb(handle) => {
                    reply(format!("outgoing-source {}", handle.window.get()))
                }
                RawWindowHandle::Xlib(handle) => {
                    reply(format!("outgoing-source {}", handle.window))
                }
                _ => panic!("Owned outgoing fixture requires X11"),
            }
        })
        .unwrap();
    handle
}

fn command() -> String {
    let mut bytes = Vec::new();
    std::io::stdin()
        .lock()
        .take(129)
        .read_until(b'\n', &mut bytes)
        .unwrap();
    assert!(!bytes.is_empty() && bytes.len() <= 128 && bytes.last() == Some(&b'\n'));
    String::from_utf8(bytes).unwrap().trim().to_owned()
}

/// Opens only the existing control/editor surfaces under the fixture's isolated XDG paths.
pub fn run_native_x11_drop_probe() {
    assert_eq!(
        std::env::var("SCREENSHOTS_NATIVE_DROP_PRIVATE").as_deref(),
        Ok("1")
    );
    assert!(std::env::var_os("WAYLAND_DISPLAY").is_none());
    assert!(std::env::var_os("WAYLAND_SOCKET").is_none());
    let root = PathBuf::from(std::env::var_os("SCREENSHOTS_NATIVE_DROP_ROOT").unwrap());
    assert!(root.is_absolute());
    for name in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"] {
        assert!(PathBuf::from(std::env::var_os(name).unwrap()).starts_with(&root));
    }
    let platform = gpui_platform::current_platform(false);
    let fonts = caption::CaptionFonts(platform.text_system());
    gpui::Application::with_platform(platform)
        .with_assets(assets::AppAssets)
        .run(move |cx| {
            cx.set_global(fonts);
            cx.set_quit_mode(gpui::QuitMode::Explicit);
            redaction_editor::bind_keys(cx);
            let mut handle = Some(open(cx, &root));
            let mut outgoing_handle: Option<WindowHandle<OutgoingProbe>> = None;
            cx.spawn(async |cx| {
                cx.background_executor()
                    .timer(Duration::from_secs(120))
                    .await;
                panic!("Owned native drop fixture exceeded its absolute deadline");
            })
            .detach();
            cx.spawn(async move |cx| {
                loop {
                    let next = cx.background_executor().spawn(async { command() }).await;
                    match next.as_str() {
                        "state" => {
                            let snapshot = cx.update(|cx| {
                                handle.and_then(|handle| {
                                    handle
                                        .update(cx, |state, _, cx| {
                                            let ready = !state.settings_loading
                                                && !state.image_input_busy();
                                            let pixels = state.editor.and_then(|editor| {
                                                editor
                                                    .update(cx, |editor, _, _| {
                                                        editor.source.pixels.clone()
                                                    })
                                                    .ok()
                                            });
                                            (ready, pixels)
                                        })
                                        .ok()
                                })
                            });
                            let (ready, pixels) = snapshot.unwrap_or((false, None));
                            let (width, height, hash) = cx
                                .background_executor()
                                .spawn(async move {
                                    let Some(pixels) = pixels else {
                                        return (0, 0, 0);
                                    };
                                    assert!(
                                        pixels.width() <= 8 && pixels.height() <= 8,
                                        "Only tiny synthetic fixture pixels"
                                    );
                                    let hash = pixels.as_raw().iter().fold(
                                        0xcbf29ce484222325u64,
                                        |hash, byte| {
                                            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
                                        },
                                    );
                                    (pixels.width(), pixels.height(), hash)
                                })
                                .await;
                            reply(format!(
                                "state {} {width} {height} {hash:016x}",
                                u8::from(ready)
                            ));
                        }
                        "cancel" => {
                            cx.update(|cx| {
                                if let Some(handle) = handle {
                                    // This is the production Escape/common cancellation handler.
                                    handle
                                        .update(cx, |state, _, cx| state.close_overlays(cx))
                                        .unwrap();
                                }
                            });
                            reply("cancelled");
                        }
                        "new-input" => {
                            let path = root.join("new-input.png");
                            cx.update(|cx| {
                                handle
                                    .unwrap()
                                    .update(cx, |state, _, cx| {
                                        state.open_redaction_source(EditorImage::File(path), cx)
                                    })
                                    .unwrap();
                            });
                            reply("new-input-admitted");
                        }
                        "close" => {
                            cx.update(|cx| {
                                if let Some(handle) = handle.take() {
                                    handle
                                        .update(cx, |state, window, cx| {
                                            state.close_overlays(cx);
                                            state.close_editor(cx);
                                            window.remove_window();
                                        })
                                        .unwrap();
                                }
                            });
                            reply("closed");
                        }
                        "new" => {
                            cx.update(|cx| {
                                assert!(handle.is_none());
                                handle = Some(open(cx, &root));
                            });
                        }
                        "outgoing" => {
                            cx.update(|cx| {
                                assert!(outgoing_handle.is_none());
                                outgoing_handle = Some(outgoing(cx, &root));
                            });
                        }
                        "outgoing-close" => {
                            cx.update(|cx| {
                                if let Some(handle) = outgoing_handle.take() {
                                    handle
                                        .update(cx, |_, window, _| window.remove_window())
                                        .unwrap();
                                }
                            });
                            reply("outgoing-closed");
                        }
                        "quit" => {
                            cx.update(|cx| cx.quit());
                            break;
                        }
                        _ => panic!("Unknown owned native drop fixture command"),
                    }
                }
            })
            .detach();
        });
}
