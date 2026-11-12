use super::*;
use gpui::test::{TestQueryExt, TestWindowExt};
use gpui::{AppContext, TestAppContext};

fn shortcut(key: &str) -> String {
    format!(
        "{}-{key}",
        if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        }
    )
}

#[gpui::test]
fn transparent_overlays_keep_their_alpha_surface_across_theme_changes(cx: &mut TestAppContext) {
    struct OverlayContent;
    impl Render for OverlayContent {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }
    cx.update(|cx| {
        let handle = cx
            .open_kit_window(
                WindowOptions {
                    window_background: WindowBackgroundAppearance::Transparent,
                    ..Default::default()
                },
                |_, cx| cx.new(|_| OverlayContent),
            )
            .unwrap();
        let window: AnyWindowHandle = handle.into();
        for appearance in [app_core::UiTheme::Oldschool, app_core::UiTheme::Default] {
            theme::apply(appearance, cx);
            window
                .update(cx, |_, window, cx| {
                    window.render_frame(cx);
                    gpui::base::Root::update(window, cx, |root, _, _| {
                        assert_eq!(root.style().background, Some(gpui::rgba(0).into()));
                    });
                })
                .unwrap();
        }
    });
}

fn open_workspace(cx: &mut TestAppContext) -> WindowHandle<CaptureWorkspace> {
    cx.update(|cx| {
        cx.open_kit_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    Default::default(),
                    size(px(1060.), px(760.)),
                ))),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| CaptureWorkspace::isolated_ui(window, cx));
                view.update(cx, |view, _| {
                    view.history_database = None;
                    view.output_directory = None;
                });
                let focus = view.read(cx).focus_handle.clone();
                window.focus(&focus, cx);
                view
            },
        )
        .unwrap()
    })
}

#[cfg(target_os = "windows")]
#[gpui::test]
fn shortcut_fields_are_named_and_released_with_settings(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let window: AnyWindowHandle = handle.into();
    cx.update_window(window, |_, window, cx| {
        window.press(&shortcut(","), cx);
        window.render_frame(cx);
        for (index, expected) in app_core::CaptureHotkeys::default().0.iter().enumerate() {
            assert_eq!(window.find(("capture-hotkey", index)).value(), Some(expected.as_str()));
        }
        // Isolated UI tests own no native hotkey manager or settings destination.
        assert_eq!(window.find("save-hotkeys").disabled(), Some(true));
        window.press(&shortcut("1"), cx);
        window.render_frame(cx);
    }).unwrap();
    cx.update(|cx| assert!(handle.read(cx).unwrap().hotkey_inputs.is_none()));
}

#[gpui::test]
fn kit_root_preserves_native_window_content_and_navigation(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let window: AnyWindowHandle = handle.into();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("workspace-heading").label(),
            Some("Recent capture")
        );
        assert_eq!(window.find("new-capture").disabled(), Some(true));
        window.click("new-capture", cx);
        assert_eq!(
            window.find("workspace-heading").label(),
            Some("Recent capture")
        );
        window.press(&shortcut(","), cx);
        assert_eq!(window.find("workspace-heading").label(), Some("Settings"));
        window.press(&shortcut("3"), cx);
        assert_eq!(
            window.find("workspace-heading").label(),
            Some("Displays & windows")
        );
    })
    .unwrap();
    cx.update(|cx| {
        assert!(WindowHandle::<CaptureWorkspace>::from_handle(window, cx).is_some());
        assert!(WindowHandle::<RedactionEditor>::from_handle(window, cx).is_none());
        assert_eq!(handle.read(cx).unwrap().page, ControlPage::Sources);
    });
    cx.update(|cx| {
        let content = window
            .downcast::<gpui::base::Root>()
            .unwrap()
            .read(cx)
            .unwrap()
            .view()
            .clone()
            .downcast::<CaptureWorkspace>()
            .unwrap();
        content.update(cx, |_, cx| {
            assert!(WindowHandle::<CaptureWorkspace>::from_handle(window, cx).is_some());
        });
    });
}

#[gpui::test]
fn kit_search_keeps_bounds_and_releases_state_on_page_change(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let window: AnyWindowHandle = handle.into();
    cx.update_window(window, |_, window, cx| window.press(&shortcut("2"), cx))
        .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click("library-search", cx);
        window.input("Grüße 😀", cx);
        assert_eq!(window.find("library-search").value(), Some("Grüße 😀"));
        window.input(&"x".repeat(513), cx);
        assert!(window.find("library-search").value().unwrap().len() <= 512);
        window.press("escape", cx);
        assert_eq!(window.find("library-search").value(), Some(""));
        window.press(&shortcut("1"), cx);
        assert_eq!(
            window.find("workspace-heading").label(),
            Some("Recent capture")
        );
    })
    .unwrap();
    cx.update(|cx| {
        let state = handle.read(cx).unwrap();
        assert!(state.history_search_input.is_none());
        assert!(state.history_search_subscription.is_none());
        assert!(state.history_query.is_empty());
    });
}

#[gpui::test]
fn stale_library_menu_cannot_mutate_a_new_page(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let window: AnyWindowHandle = handle.into();
    cx.update_window(window, |_, window, cx| window.press(&shortcut("2"), cx))
        .unwrap();
    cx.run_until_parked();
    handle
        .update(cx, |state, _, _| {
            state.history_status.clear();
            state.history_action_status.clear();
            state.history_entries = vec![app_core::HistoryEntry {
                id: 42,
                path: PathBuf::from("Synthetic.png"),
                width: 2,
                height: 3,
                created_at: "2026-11-11".into(),
            }];
        })
        .unwrap();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click("history-more-actions", cx);
        assert!(window.find_by_label("Remove from library").visible());
    })
    .unwrap();
    handle
        .update(cx, |state, _, _| {
            state.history_revision = state.history_revision.wrapping_add(1)
        })
        .unwrap();
    cx.update_window(window, |_, window, cx| {
        let item = window.find_by_label("Remove from library");
        let bounds = item.bounds();
        // Popup items expose semantic labels rather than caller-assigned IDs.
        use gpui::test::TestEventExt;
        window.pointer_down(bounds.center(), gpui::MouseButton::Left, cx);
        window.pointer_up(bounds.center(), gpui::MouseButton::Left, cx);
    })
    .unwrap();
    cx.update(|cx| {
        let state = handle.read(cx).unwrap();
        assert_eq!(state.history_entries.len(), 1);
        assert!(state.history_action_status.is_empty());
        assert!(!state.history_mutating);
    });
}

#[gpui::test]
fn quit_waits_for_work_inside_a_kit_root(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    handle
        .update(cx, |state, _, _| state.saving = true)
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("workspace-footer-2", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let state = handle.read(cx).unwrap();
        assert!(state.quitting);
        assert!(state.saving);
    });
}

#[gpui::test]
fn escape_releases_an_overlay_inside_its_kit_root(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    cx.update(|cx| cx.bind_keys([KeyBinding::new("escape", CloseOverlay, Some("Overlay"))]));
    handle
        .update(cx, |state, _, cx| {
            state.show_overlays(OverlayMode::Probe, &[], &HashMap::new(), cx);
        })
        .unwrap();
    let overlay = cx.update(|cx| {
        let state = handle.read(cx).unwrap();
        assert_eq!(state.live_overlay_count(cx), 1);
        state.overlays[0]
    });
    cx.update_window(overlay.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let state = handle.read(cx).unwrap();
        assert_eq!(state.live_overlay_count(cx), 0);
        assert!(state.overlays.is_empty());
        assert!(state.last_saved.is_none());
        assert!(state.last_capture.is_none());
    });
}

#[gpui::test]
fn library_checkbox_keeps_controlled_selection_and_disabled_state(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let window: AnyWindowHandle = handle.into();
    cx.update_window(window, |_, window, cx| window.press(&shortcut("2"), cx))
        .unwrap();
    cx.run_until_parked();
    handle
        .update(cx, |state, _, _| {
            state.history_entries = vec![app_core::HistoryEntry {
                id: 42,
                path: PathBuf::from("Synthetic.png"),
                width: 2,
                height: 3,
                created_at: "2026-11-11".into(),
            }];
        })
        .unwrap();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click("select-history-entry", cx);
        assert_eq!(window.find("select-history-entry").checked(), Some(true));
    })
    .unwrap();
    cx.update(|cx| assert_eq!(handle.read(cx).unwrap().history_selected, vec![42]));
    handle
        .update(cx, |state, _, _| state.history_mutating = true)
        .unwrap();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("select-history-entry").disabled(), Some(true));
        window.click("select-history-entry", cx);
    })
    .unwrap();
    cx.update(|cx| assert_eq!(handle.read(cx).unwrap().history_selected, vec![42]));
}
