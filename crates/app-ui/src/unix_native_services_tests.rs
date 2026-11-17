//! Isolated GPUI lifecycle checks: synthetic pixels, no native registrations or user clipboard.
use super::*;
use gpui::test::TestWindowExt;
use gpui::{AppContext, TestAppContext};

fn workspace(cx: &mut TestAppContext) -> WindowHandle<CaptureWorkspace> {
    cx.update(|cx| {
        cx.open_kit_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    Default::default(),
                    size(px(1060.), px(760.)),
                ))),
                ..app_window_options("Fixture")
            },
            |window, cx| cx.new(|cx| CaptureWorkspace::isolated_ui(window, cx)),
        )
        .unwrap()
    })
}
fn shot(index: usize) -> CapturedImage {
    CapturedImage {
        desktop_layout: None,
        display_id: "fixture".into(),
        window_id: None,
        width: 16,
        height: 16,
        pixels: Arc::new(image::RgbaImage::from_pixel(
            16,
            16,
            image::Rgba([index as u8, 37, 53, 255]),
        )),
        png_path: PathBuf::from(format!("fixture-{index}.png")),
        elapsed_ms: 0,
        source_format: app_core::PixelFormat::Rgba8UnormSrgb,
        display_geometry: None,
    }
}
#[gpui::test]
fn repeated_minimize_and_reopen_releases_the_control_thumbnail_atlas_entry(
    cx: &mut TestAppContext,
) {
    let handle = workspace(cx);
    let raw: AnyWindowHandle = handle.into();
    for index in 0..8 {
        let capture = shot(index);
        let render = render_thumbnail(&capture);
        handle
            .update(cx, |this, _, cx| {
                this.last_capture = Some(capture);
                this.last_render = Some(render.clone());
                this.page = ControlPage::Captures;
                cx.notify();
            })
            .unwrap();
        cx.update_window(raw, |_, window, cx| {
            window.render_frame(cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(raw, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.has_image_atlas_entry(&render),
                "Fixture must upload before testing release"
            );
        })
        .unwrap();
        handle
            .update(cx, |this, window, cx| {
                this.minimize_unix_control(window, cx);
                assert!(this.last_render.is_none());
                assert!(!window.has_image_atlas_entry(&render));
                show_control_window(window).unwrap();
                cx.notify();
            })
            .unwrap();
        cx.update_window(raw, |_, window, cx| {
            window.render_frame(cx);
            assert!(!window.has_image_atlas_entry(&render));
        })
        .unwrap();
    }
}
#[gpui::test]
fn output_save_failure_aborts_quit_before_irreversible_native_teardown(cx: &mut TestAppContext) {
    let handle = workspace(cx);
    handle
        .update(cx, |this, _, cx| {
            this.quitting = true;
            this.saving = true;
            this.set_unix_quitting(true);
            assert!(this.unix_quit_paused);
            assert!(this.unix_quit_work_pending());
            this.stop_unix_native_services(cx);
            assert!(!this.unix_services_stopping);
            this.saving = false;
            let capture = shot(11);
            let render = render_thumbnail(&capture);
            this.finish_capture_save_batch(
                vec![CaptureSaveAttempt {
                    capture,
                    result: Err("fixture save failure".into()),
                    history_warning: None,
                }],
                Some(render),
                cx,
            );
            assert!(!this.quitting && !this.unix_quit_paused);
            assert_eq!(this.failed_capture_saves.len(), 1);
            assert!(this.save_status.contains("unsaved screenshot"));
            this.unix_reopen(cx);
            assert!(!this.quitting);
        })
        .unwrap();
    cx.run_until_parked();
}
#[gpui::test]
fn provisional_quit_show_exposes_busy_window_and_editor_failure_keeps_recovery_possible(
    cx: &mut TestAppContext,
) {
    let handle = workspace(cx);
    handle
        .update(cx, |this, _, cx| {
            this.quitting = true;
            this.editor_closing = true;
            this.set_unix_quitting(true);
            this.stop_unix_native_services(cx);
            assert!(this.unix_quit_work_pending() && !this.unix_services_stopping);
            this.unix_reopen(cx);
            assert!(this.capture_status.contains("waiting for active work"));
            this.editor_closing = false;
            this.abort_quit(cx);
            assert!(!this.quitting && !this.unix_quit_paused);
        })
        .unwrap();
    cx.run_until_parked();
}

#[gpui::test]
fn tray_readiness_seen_by_close_survives_loss_before_first_receiver_dispatch(
    cx: &mut TestAppContext,
) {
    let handle = workspace(cx);
    handle
        .update(cx, |this, _, _| {
            assert!(!this.unix_tray_observed);
            // Close admits minimization using native readiness before receiver dispatch.
            this.unix_tray_observed = true;
            assert!(this.observe_unix_tray(false));
            assert!(!this.observe_unix_tray(false));
            assert!(!this.observe_unix_tray(true));
            assert!(this.observe_unix_tray(false));
        })
        .unwrap();
}
#[gpui::test]
fn prepared_shortcuts_roll_back_before_quit_recovery_releases_settings_admission(
    cx: &mut TestAppContext,
) {
    let handle = workspace(cx);
    for fail_cleanup in [false, true] {
        let pending = std::rc::Rc::new(std::cell::RefCell::new((
            None::<app_core::UnixShortcutReport>,
            None::<std::task::Waker>,
        )));
        handle
            .update(cx, |this, _, cx| {
                this.saving = true;
                this.quitting = true;
                this.set_unix_quitting(true);
                this.settings_saving = true;
                this.unix_shortcut_preparing = true;
                let admitted = this.admit_unix_shortcut_preparation(
                    app_core::UnixShortcutReport {
                        status: "Reserved".into(),
                        outcome: Ok(()),
                    },
                    cx,
                );
                assert!(!admitted && this.settings_saving && !this.unix_shortcut_preparing);
                let receive = pending.clone();
                let operation: app_core::UnixShortcutOperation =
                    Box::pin(std::future::poll_fn(move |cx| {
                        let mut pending = receive.borrow_mut();
                        if let Some(report) = pending.0.take() {
                            std::task::Poll::Ready(report)
                        } else {
                            pending.1 = Some(cx.waker().clone());
                            std::task::Poll::Pending
                        }
                    }));
                cx.spawn(async move |this, cx| {
                    rollback_unix_shortcuts(this, cx, Some(operation)).await
                })
                .detach();
            })
            .unwrap();
        cx.run_until_parked();
        handle
            .update(cx, |this, _, cx| {
                assert!(this.settings_saving && this.unix_quit_work_pending());
                this.saving = false;
                this.finish_capture_save_batch(
                    vec![CaptureSaveAttempt {
                        capture: shot(17),
                        result: Err("save failure".into()),
                        history_warning: None,
                    }],
                    None,
                    cx,
                );
                assert!(!this.quitting && !this.unix_quit_paused);
                assert!(this.settings_saving && this.unix_quit_work_pending());
            })
            .unwrap();
        let wake = {
            let mut pending = pending.borrow_mut();
            pending.0 = Some(app_core::UnixShortcutReport {
                status: "Old bindings retained".into(),
                outcome: if fail_cleanup {
                    Err("owned cleanup failure".into())
                } else {
                    Ok(())
                },
            });
            pending
                .1
                .take()
                .expect("Rollback must be parked awaiting completion")
        };
        wake.wake();
        cx.run_until_parked();
        handle
            .update(cx, |this, _, _| {
                assert!(!this.settings_saving && !this.quitting);
                assert_eq!(this.hotkey_status, "Old bindings retained");
                assert_eq!(
                    this.settings_status.contains("rollback failed"),
                    fail_cleanup
                );
            })
            .unwrap();
    }
}
