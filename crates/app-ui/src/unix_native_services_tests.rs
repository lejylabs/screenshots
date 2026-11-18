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
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        handle
            .update(cx, |_, window, _| {
                assert_eq!(window.visibility(), gpui::WindowVisibility::Hidden);
                show_control_window(window).unwrap();
            })
            .unwrap();
        cx.run_until_parked();
        cx.update_window(raw, |_, window, cx| {
            assert_eq!(window.visibility(), gpui::WindowVisibility::Visible);
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

#[gpui::test]
fn resident_close_destroys_window_and_preserves_unsaved_pixels_across_reopen(
    cx: &mut TestAppContext,
) {
    let initial = workspace(cx);
    let owner = initial
        .update(cx, |this, _, cx| {
            this.resident = true;
            let owner = cx.entity();
            cx.set_global(ResidentController(owner.clone()));
            owner
        })
        .unwrap();
    cx.update(|cx| cx.set_quit_mode(gpui::QuitMode::Explicit));
    let capture = shot(19);
    let pixels = capture.pixels.clone();
    owner.update(cx, |this, _| {
        this.last_capture = Some(capture.clone());
        this.failed_capture_saves = vec![capture];
    });
    for _ in 0..8 {
        let handle = owner.read_with(cx, |this, _| this.control.unwrap());
        handle
            .update(cx, |_, window, cx| {
                owner.update(cx, |this, cx| this.minimize_unix_control(window, cx));
            })
            .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert!(cx.windows().is_empty()));
        assert!(handle.update(cx, |_, _, _| ()).is_err());
        owner.update(cx, |this, cx| {
            assert!(!this.switching_graphics && this.control.is_none());
            assert!(this.last_render.is_none());
            assert!(Arc::ptr_eq(
                &this.last_capture.as_ref().unwrap().pixels,
                &pixels
            ));
            assert!(Arc::ptr_eq(&this.failed_capture_saves[0].pixels, &pixels));
            this.unix_reopen(cx);
        });
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(cx.windows().len(), 1));
        owner.read_with(cx, |this, _| assert_ne!(this.control, Some(handle)));
    }
}

#[gpui::test]
fn resident_show_during_retirement_is_coalesced_until_window_destruction(cx: &mut TestAppContext) {
    let handle = workspace(cx);
    let owner = handle
        .update(cx, |this, _, cx| {
            this.resident = true;
            let owner = cx.entity();
            cx.set_global(ResidentController(owner.clone()));
            owner
        })
        .unwrap();
    cx.update(|cx| cx.set_quit_mode(gpui::QuitMode::Explicit));
    handle
        .update(cx, |this, window, cx| {
            this.minimize_unix_control(window, cx);
            assert!(this.switching_graphics);
            for _ in 0..32 {
                this.unix_reopen(cx);
            }
            assert!(this.unix_show_pending);
        })
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(cx.windows().len(), 1));
    owner.read_with(cx, |this, _| {
        assert!(!this.unix_show_pending && !this.switching_graphics);
        assert_ne!(this.control, Some(handle.into()));
    });
}

#[gpui::test]
fn provisional_quit_show_opens_busy_status_without_a_previous_control(cx: &mut TestAppContext) {
    let handle = workspace(cx);
    let owner = handle
        .update(cx, |this, _, cx| {
            this.resident = true;
            let owner = cx.entity();
            cx.set_global(ResidentController(owner.clone()));
            owner
        })
        .unwrap();
    cx.update(|cx| cx.set_quit_mode(gpui::QuitMode::Explicit));
    owner.update(cx, |this, cx| this.retire_ui(cx));
    cx.run_until_parked();
    owner.update(cx, |this, cx| {
        this.saving = true;
        this.capturing = true;
        this.quitting = true;
        this.unix_reopen(cx);
    });
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(cx.windows().len(), 1));
    owner.read_with(cx, |this, _| {
        assert!(this.quitting && this.saving && this.capturing);
        assert!(this.capture_status.contains("waiting for active work"));
        assert!(this.control.is_some());
        assert!(!this.unix_services_stopping);
    });
}

#[gpui::test]
fn resident_editor_prevents_window_retirement(cx: &mut TestAppContext) {
    let handle = workspace(cx);
    handle
        .update(cx, |this, window, cx| {
            this.resident = true;
            this.editor_closing = true;
            this.minimize_unix_control(window, cx);
            assert!(this.control.is_some());
            assert!(!this.switching_graphics);
            assert!(this.capture_status.contains("Finish or cancel editing"));
        })
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(cx.windows().len(), 1));
}

#[gpui::test]
fn reopened_resident_close_without_tray_keeps_pending_save_owned(cx: &mut TestAppContext) {
    let initial = workspace(cx);
    let owner = initial
        .update(cx, |this, _, cx| {
            this.resident = true;
            let owner = cx.entity();
            cx.set_global(ResidentController(owner.clone()));
            owner
        })
        .unwrap();
    cx.update(|cx| cx.set_quit_mode(gpui::QuitMode::Explicit));
    owner.update(cx, |this, cx| this.retire_ui(cx));
    cx.run_until_parked();
    owner.update(cx, |this, cx| this.unix_reopen(cx));
    cx.run_until_parked();
    owner.update(cx, |this, _| {
        this.saving = true;
    });
    let reopened = owner.read_with(cx, |this, _| this.control.unwrap());
    let mut window = gpui::VisualTestContext::from_window(reopened, cx);
    assert!(!window.simulate_close());
    drop(window);
    cx.run_until_parked();
    owner.read_with(cx, |this, _| {
        assert!(this.quitting && this.saving);
        assert_eq!(this.control, Some(reopened));
    });
}

// Simulates only the worker-result boundary; all pixels and windows are test-owned.
fn resident_region_preview_with_queued_show(
    cx: &mut TestAppContext,
) -> (
    gpui::Entity<CaptureWorkspace>,
    WindowHandle<Overlay>,
    std::sync::Weak<image::RgbaImage>,
) {
    let handle = workspace(cx);
    let owner = handle
        .update(cx, |this, _, cx| {
            this.resident = true;
            let owner = cx.entity();
            cx.set_global(ResidentController(owner.clone()));
            owner
        })
        .unwrap();
    cx.update(|cx| {
        cx.set_quit_mode(gpui::QuitMode::Explicit);
        cx.bind_keys([KeyBinding::new("escape", CloseOverlay, Some("Overlay"))]);
    });
    owner.update(cx, |this, cx| this.retire_ui(cx));
    cx.run_until_parked();
    cx.update(|cx| assert!(cx.windows().is_empty()));
    let (overlay, pixels) = owner.update(cx, |this, cx| {
        let mut capture = shot(29);
        capture.display_id = u64::from(cx.displays()[0].id()).to_string();
        let pixels = Arc::downgrade(&capture.pixels);
        let renders = HashMap::from([(capture.png_path.clone(), render_frame(&capture))]);
        this.begin_capturing(cx);
        this.unix_reopen(cx);
        assert!(this.unix_show_pending && this.capturing && this.control.is_none());
        this.finish_capture_work(
            CaptureCompletion {
                result: Ok((vec![capture], renders)),
                preview: true,
                edit_selection: false,
                profile: CaptureOutputProfile::default(),
                exact_region: None,
                repeating_region: false,
            },
            cx,
        );
        assert!(this.preview_active && this.unix_show_pending);
        assert!(this.control.is_none() && !this.switching_graphics);
        assert_eq!(this.live_overlay_count(cx), 1);
        (this.overlays[0], pixels)
    });
    cx.run_until_parked();
    (owner, overlay, pixels)
}

#[gpui::test]
fn resident_pending_show_does_not_interrupt_capture_to_preview(cx: &mut TestAppContext) {
    let (owner, overlay, pixels) = resident_region_preview_with_queued_show(cx);
    owner.read_with(cx, |this, cx| {
        assert!(this.control.is_none());
        assert!(this.preview_active && this.unix_show_pending && !this.switching_graphics);
        assert_eq!(this.live_overlay_count(cx), 1);
        assert!(this.last_capture.is_none() && this.last_saved.is_none());
    });
    assert!(pixels.upgrade().is_some());
    cx.update_window(overlay.into(), |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    owner.read_with(cx, |this, _| {
        assert!(this.control.is_none() && this.preview_active)
    });
}

#[gpui::test]
fn resident_preview_escape_action_delivers_queued_show_after_cancel(cx: &mut TestAppContext) {
    let (owner, overlay, pixels) = resident_region_preview_with_queued_show(cx);
    cx.update_window(overlay.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(cx.windows().len(), 1));
    owner.read_with(cx, |this, cx| {
        assert!(this.control.is_some());
        assert!(!this.unix_show_pending && !this.preview_active && !this.switching_graphics);
        assert_eq!(this.live_overlay_count(cx), 0);
        assert!(this.overlays.is_empty());
        assert!(this.last_capture.is_none() && this.last_saved.is_none());
    });
    assert!(
        pixels.upgrade().is_none(),
        "cancel releases the preview's source pixels"
    );
}

fn resident_capture_with_queued_show(
    cx: &mut TestAppContext,
    keep_control: bool,
) -> gpui::Entity<CaptureWorkspace> {
    let handle = workspace(cx);
    let owner = handle
        .update(cx, |this, _, cx| {
            this.resident = true;
            let owner = cx.entity();
            cx.set_global(ResidentController(owner.clone()));
            owner
        })
        .unwrap();
    cx.update(|cx| cx.set_quit_mode(gpui::QuitMode::Explicit));
    if !keep_control {
        owner.update(cx, |this, cx| this.retire_ui(cx));
        cx.run_until_parked();
    }
    // Exercise the real result handler without starting a native capture worker.
    owner.update(cx, |this, cx| {
        this.begin_capturing(cx);
        this.unix_reopen(cx);
        assert!(this.capturing && this.unix_show_pending);
    });
    owner
}

fn close_fixture_to_tray(owner: &gpui::Entity<CaptureWorkspace>, cx: &mut TestAppContext) {
    let control = owner.read_with(cx, |this, _| this.control.unwrap());
    control
        .update(cx, |_, window, cx| {
            // Native tray admission is outside this isolated fixture; use its actual retirement route.
            owner.update(cx, |this, cx| this.minimize_unix_control(window, cx));
        })
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert!(cx.windows().is_empty()));
    owner.read_with(cx, |this, _| {
        assert!(this.control.is_none());
        assert!(!this.unix_show_pending && !this.switching_graphics);
    });
}

#[gpui::test]
fn capture_failures_fulfill_show_before_later_close_to_tray(cx: &mut TestAppContext) {
    for error in [
        app_core::CaptureError::NativeCanceled,
        app_core::CaptureError::PermissionDenied,
        app_core::CaptureError::Backend("synthetic capture failure".into()),
    ] {
        let owner = resident_capture_with_queued_show(cx, true);
        let original_control = owner.read_with(cx, |this, _| this.control);
        owner.update(cx, |this, cx| {
            this.finish_capture_work(
                CaptureCompletion {
                    result: Err(error),
                    preview: true,
                    edit_selection: false,
                    profile: CaptureOutputProfile::default(),
                    exact_region: None,
                    repeating_region: false,
                },
                cx,
            )
        });
        cx.run_until_parked();
        owner.read_with(cx, |this, _| {
            assert_eq!(this.control, original_control);
            assert!(!this.capturing && !this.unix_show_pending && !this.switching_graphics);
            assert!(this.capture_status.starts_with("capture failed:"));
            assert!(this.last_capture.is_none() && this.last_saved.is_none());
        });
        close_fixture_to_tray(&owner, cx);
    }
}

#[gpui::test]
fn empty_capture_fulfills_show_with_or_without_a_previous_control(cx: &mut TestAppContext) {
    for keep_control in [true, false] {
        let owner = resident_capture_with_queued_show(cx, keep_control);
        owner.update(cx, |this, cx| {
            this.finish_capture_work(
                CaptureCompletion {
                    result: Ok((Vec::new(), HashMap::new())),
                    preview: true,
                    edit_selection: false,
                    profile: CaptureOutputProfile::default(),
                    exact_region: None,
                    repeating_region: false,
                },
                cx,
            )
        });
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(cx.windows().len(), 1));
        owner.read_with(cx, |this, _| {
            assert!(this.control.is_some());
            assert!(!this.capturing && !this.unix_show_pending && !this.preview_active);
            assert_eq!(this.capture_status, "no capture produced");
            assert!(this.last_capture.is_none() && this.last_saved.is_none());
        });
        close_fixture_to_tray(&owner, cx);
    }
}

#[gpui::test]
fn failed_fullscreen_install_fulfills_show_after_final_preview_state(cx: &mut TestAppContext) {
    let owner = resident_capture_with_queued_show(cx, false);
    let mut capture = shot(31);
    capture.display_id = "not-an-installed-fixture-display".into();
    let pixels = Arc::downgrade(&capture.pixels);
    let renders = HashMap::from([(capture.png_path.clone(), render_frame(&capture))]);
    owner.update(cx, |this, cx| {
        this.finish_capture_work(
            CaptureCompletion {
                result: Ok((vec![capture], renders)),
                preview: true,
                edit_selection: false,
                profile: CaptureOutputProfile::default(),
                exact_region: None,
                repeating_region: false,
            },
            cx,
        )
    });
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(cx.windows().len(), 1));
    owner.read_with(cx, |this, cx| {
        assert!(this.control.is_some());
        assert!(!this.preview_active && !this.capturing && !this.unix_show_pending);
        assert_eq!(this.live_overlay_count(cx), 0);
        assert!(this.overlays.is_empty());
        assert!(
            this.capture_status
                .starts_with("No captured image for display")
        );
        assert!(this.last_capture.is_none() && this.last_saved.is_none());
    });
    assert!(pixels.upgrade().is_none());
    close_fixture_to_tray(&owner, cx);
}

#[gpui::test]
fn image_input_cancel_and_failure_fulfill_show_before_later_close(cx: &mut TestAppContext) {
    for cancel in [true, false] {
        let owner = resident_capture_with_queued_show(cx, true);
        owner.update(cx, |this, cx| {
            let token = this.native_capture_cancel.clone().unwrap();
            if cancel {
                token.cancel();
            }
            this.deliver_image_input(
                this.capture_generation,
                &token,
                Err("synthetic image input failure".into()),
                cx,
            );
        });
        cx.run_until_parked();
        owner.read_with(cx, |this, _| {
            assert!(this.control.is_some());
            assert!(!this.capturing && !this.unix_show_pending && !this.switching_graphics);
            assert!(this.editor.is_none());
            assert_eq!(this.capture_status.contains("canceled"), cancel);
        });
        close_fixture_to_tray(&owner, cx);
    }
}

#[gpui::test]
fn workspace_cancel_with_restored_control_consumes_show_before_later_tray_close(
    cx: &mut TestAppContext,
) {
    let owner = resident_capture_with_queued_show(cx, true);
    owner.update(cx, |this, cx| {
        let mut capture = shot(49);
        capture.display_id = u64::from(cx.displays()[0].id()).to_string();
        let renders = HashMap::from([(capture.png_path.clone(), render_frame(&capture))]);
        this.finish_capture_work(
            CaptureCompletion {
                result: Ok((vec![capture], renders)),
                preview: true,
                edit_selection: false,
                profile: CaptureOutputProfile::default(),
                exact_region: None,
                repeating_region: false,
            },
            cx,
        );
        assert!(this.preview_active && this.unix_show_pending && this.control.is_some());
        this.restore_control(cx);
    });
    cx.run_until_parked();
    let control = owner.read_with(cx, |this, _| this.control.unwrap());
    cx.update_window(control, |_, window, cx| {
        window.render_frame(cx);
        window.dispatch_action(Box::new(CloseOverlay), cx);
    })
    .unwrap();
    cx.run_until_parked();
    owner.read_with(cx, |this, _| {
        assert!(!this.preview_active && !this.unix_show_pending)
    });
    close_fixture_to_tray(&owner, cx);
}
