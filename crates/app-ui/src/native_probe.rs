//! Explicit native X11 verification: only our own windows and synthetic pixels.
use super::*;
use gpui::test::TestWindowExt;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{path::Path, time::Duration};

async fn pause(cx: &mut gpui::AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(100))
        .await;
}
async fn snapshot(
    handle: AnyWindowHandle,
    name: &str,
    root: &Path,
    cx: &mut gpui::AsyncApp,
) -> Result<(), String> {
    handle
        .update(cx, |_, window, _| window.refresh())
        .map_err(|e| e.to_string())?;
    pause(cx).await;
    let id = handle
        .update(cx, |_, window, _| {
            let raw = HasWindowHandle::window_handle(window)
                .map_err(|e| e.to_string())?
                .as_raw();
            match raw {
                RawWindowHandle::Xcb(handle) => Ok(handle.window.get().to_string()),
                RawWindowHandle::Xlib(handle) => Ok(handle.window.to_string()),
                _ => Err("Native probe needs X11; unset WAYLAND_DISPLAY".to_string()),
            }
        })
        .map_err(|e| e.to_string())??;
    let target = root.join(name);
    cx.background_executor()
        .spawn(async move {
            let status = std::process::Command::new("import")
                .args(["-window", &id])
                .arg(target)
                .status()
                .map_err(|e| e.to_string())?;
            if status.success() {
                Ok(())
            } else {
                Err("Own-window screenshot failed".to_string())
            }
        })
        .await
}
async fn ready(
    handle: WindowHandle<CaptureWorkspace>,
    cx: &mut gpui::AsyncApp,
    predicate: impl Fn(&CaptureWorkspace) -> bool,
) -> Result<(), String> {
    let start = std::time::Instant::now();
    loop {
        if handle
            .update(cx, |state, _, _| predicate(state))
            .map_err(|e| e.to_string())?
        {
            return Ok(());
        }
        if start.elapsed() > Duration::from_secs(10) {
            return Err("Native work did not finish".into());
        }
        pause(cx).await;
    }
}

#[test]
#[ignore = "opens native X11 windows with synthetic pixels; requires isolated XDG paths and ImageMagick"]
fn native_kit_workspace_and_editor() {
    let root = PathBuf::from(
        std::env::var_os("SCREENSHOTS_NATIVE_PROBE_ROOT").expect("explicit probe root"),
    );
    assert!(root.is_absolute() && root.parent().is_some());
    for variable in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"] {
        assert!(
            PathBuf::from(std::env::var_os(variable).expect("isolated XDG path"))
                .starts_with(&root)
        );
    }
    let directory = root.join(format!("run-{}", std::process::id()));
    let captures = directory.join("captures");
    std::fs::create_dir_all(&captures).unwrap();
    let source = directory.join("Synthetic image.png");
    image::RgbaImage::from_fn(1280, 720, |x, y| {
        let value = if (x / 160 + y / 120) % 2 == 0 {
            225
        } else {
            175
        };
        image::Rgba([value, value, value, 255])
    })
    .save(&source)
    .unwrap();
    let original = std::fs::read(&source).unwrap();
    let database = directory.join("history.sqlite3");
    app_core::import_capture_files(&database, std::slice::from_ref(&source)).unwrap();
    let result = Arc::new(Mutex::new(None));
    let report = result.clone();
    let platform = gpui_platform::current_platform(false);
    let fonts = caption::CaptionFonts(platform.text_system());
    gpui::Application::with_platform(platform)
        .with_assets(assets::AppAssets)
        .run(move |cx| {
            cx.set_global(fonts);
            cx.set_quit_mode(gpui::QuitMode::Explicit);
            redaction_editor::bind_keys(cx);
            let handle = cx
                .open_kit_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                            None,
                            size(px(1060.), px(760.)),
                            cx,
                        ))),
                        ..app_window_options("screenshots")
                    },
                    |window, cx| {
                        let view = cx.new(|cx| CaptureWorkspace::isolated_ui(window, cx));
                        view.update(cx, |state, _| {
                            state.backend = XcapBackend::new(&captures);
                            state.output_directory = Some(captures.clone());
                            state.history_database = Some(database.clone());
                        });
                        let focus = view.read(cx).focus_handle.clone();
                        window.focus(&focus, cx);
                        view
                    },
                )
                .unwrap();
            let deadline_report = report.clone();
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_secs(45))
                    .await;
                *deadline_report.lock().unwrap() =
                    Some(Err("Native probe/normal Quit timed out".to_string()));
                cx.update(|cx| cx.quit());
            })
            .detach();
            cx.spawn(async move |cx| {
                let checked = async {
                    pause(cx).await;
                    snapshot(handle.into(), "dark.png", &directory, cx).await?;
                    AnyWindowHandle::from(handle)
                        .update(cx, |_, window, cx| window.press("ctrl-2", cx))
                        .map_err(|e| e.to_string())?;
                    ready(handle, cx, |state| {
                        !state.history_loading
                            && state.history_entries.len() == 1
                            && state.history_thumb_loading.is_none()
                            && (state
                                .history_thumbs
                                .contains_key(&state.history_entries[0].id)
                                || state
                                    .history_thumb_failed
                                    .contains(&state.history_entries[0].id))
                    })
                    .await?;
                    snapshot(handle.into(), "library.png", &directory, cx).await?;
                    AnyWindowHandle::from(handle)
                        .update(cx, |_, window, cx| window.click("history-more-actions", cx))
                        .map_err(|e| e.to_string())?;
                    snapshot(handle.into(), "menu.png", &directory, cx).await?;
                    AnyWindowHandle::from(handle)
                        .update(cx, |_, window, cx| {
                            window.press("escape", cx);
                            window.press("ctrl-,", cx);
                        })
                        .map_err(|e| e.to_string())?;
                    snapshot(handle.into(), "settings.png", &directory, cx).await?;
                    AnyWindowHandle::from(handle)
                        .update(cx, |_, window, cx| {
                            window.within("appearance").click(1usize, cx)
                        })
                        .map_err(|e| e.to_string())?;
                    ready(handle, cx, |state| !state.settings_saving).await?;
                    let stored = cx
                        .background_executor()
                        .spawn(async {
                            app_core::CaptureSettings::load().expect("isolated saved settings")
                        })
                        .await;
                    assert_eq!(stored.theme, app_core::UiTheme::Oldschool);
                    AnyWindowHandle::from(handle)
                        .update(cx, |_, window, cx| window.press("ctrl-1", cx))
                        .map_err(|e| e.to_string())?;
                    snapshot(handle.into(), "oldschool.png", &directory, cx).await?;
                    AnyWindowHandle::from(handle)
                        .update(cx, |_, window, cx| {
                            window.press("ctrl-,", cx);
                            window.within("appearance").click(0usize, cx);
                        })
                        .map_err(|e| e.to_string())?;
                    ready(handle, cx, |state| !state.settings_saving).await?;
                    let stored = cx
                        .background_executor()
                        .spawn(async {
                            app_core::CaptureSettings::load().expect("isolated saved settings")
                        })
                        .await;
                    assert_eq!(stored.theme, app_core::UiTheme::Default);
                    AnyWindowHandle::from(handle)
                        .update(cx, |_, window, _| window.resize(size(px(680.), px(480.))))
                        .map_err(|e| e.to_string())?;
                    snapshot(handle.into(), "compact.png", &directory, cx).await?;
                    AnyWindowHandle::from(handle)
                        .update(cx, |_, window, cx| {
                            window.resize(size(px(1060.), px(760.)));
                            window.press("ctrl-2", cx);
                        })
                        .map_err(|e| e.to_string())?;
                    ready(handle, cx, |state| {
                        !state.history_loading
                            && state.history_entries.len() == 1
                            && state.history_thumb_loading.is_none()
                    })
                    .await?;
                    AnyWindowHandle::from(handle)
                        .update(cx, |_, window, cx| window.click("edit-history-file", cx))
                        .map_err(|e| e.to_string())?;
                    ready(handle, cx, |state| {
                        state.editor.is_some() && !state.capturing
                    })
                    .await?;
                    let editor = handle
                        .update(cx, |state, _, _| state.editor.unwrap())
                        .map_err(|e| e.to_string())?;
                    AnyWindowHandle::from(editor)
                        .update(cx, |_, window, cx| {
                            window.resize(size(px(1000.), px(760.)));
                            window.render_frame(cx);
                            window.click("tool-text", cx);
                            window.click_at("editor-image-area", point(px(64.), px(64.)), cx);
                        })
                        .map_err(|e| e.to_string())?;
                    pause(cx).await;
                    AnyWindowHandle::from(editor)
                        .update(cx, |_, window, cx| {
                            window.click("caption-input", cx);
                            window.input("Kit label", cx);
                            window.press("shift-enter", cx);
                            window.input("Native Grüße", cx);
                            assert_eq!(
                                window.find("caption-input").value(),
                                Some("Kit label\nNative Grüße")
                            );
                            window.press("enter", cx);
                        })
                        .map_err(|e| e.to_string())?;
                    let start = std::time::Instant::now();
                    while AnyWindowHandle::from(editor)
                        .update(cx, |_, window, cx| {
                            window.render_frame(cx);
                            window.find("export-save-only").disabled() == Some(true)
                        })
                        .map_err(|e| e.to_string())?
                    {
                        if start.elapsed() > Duration::from_secs(10) {
                            return Err("Label rasterization failed".into());
                        }
                        pause(cx).await;
                    }
                    snapshot(editor.into(), "editor.png", &directory, cx).await?;
                    AnyWindowHandle::from(editor)
                        .update(cx, |_, window, _| window.resize(size(px(760.), px(500.))))
                        .map_err(|e| e.to_string())?;
                    snapshot(editor.into(), "editor-compact.png", &directory, cx).await?;
                    AnyWindowHandle::from(editor)
                        .update(cx, |_, window, cx| window.click("export-save-only", cx))
                        .map_err(|e| e.to_string())?;
                    ready(handle, cx, |state| {
                        !state.saving && state.editor.is_none() && state.last_saved.is_some()
                    })
                    .await?;
                    assert_eq!(std::fs::read(&source).unwrap(), original);
                    let exported = handle
                        .update(cx, |state, _, _| state.last_saved.clone().unwrap())
                        .map_err(|e| e.to_string())?;
                    assert_eq!(
                        image::ImageReader::open(&exported)
                            .unwrap()
                            .into_dimensions()
                            .unwrap(),
                        (1280, 720)
                    );
                    assert_ne!(std::fs::read(&exported).unwrap(), original);
                    assert_ne!(
                        image::open(&exported).unwrap().into_rgba8(),
                        image::open(&source).unwrap().into_rgba8()
                    );
                    AnyWindowHandle::from(handle)
                        .update(cx, |_, window, cx| window.press("ctrl-1", cx))
                        .map_err(|e| e.to_string())?;
                    snapshot(handle.into(), "saved.png", &directory, cx).await?;
                    println!("Native Kit probe: {}", directory.display());
                    Ok::<(), String>(())
                }
                .await;
                let success = checked.is_ok();
                *report.lock().unwrap() = Some(checked);
                if success {
                    AnyWindowHandle::from(handle)
                        .update(cx, |_, window, cx| window.click("workspace-footer-2", cx))
                        .ok();
                } else {
                    cx.update(|cx| cx.quit());
                }
            })
            .detach();
        });
    result
        .lock()
        .unwrap()
        .take()
        .expect("probe result")
        .unwrap();
}
