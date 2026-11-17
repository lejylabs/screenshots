//! Real headless windows, shared drag storage and removal; no native desktop input.
use crate::{
    AnyWindowHandle, AppContext as _, Context, Empty, ExternalDragPayload, ExternalPaths,
    FileDragPaths, FileDropEvent, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, PlatformInput, Render, StatefulInteractiveElement as _, Styled,
    TestAppContext, Window, div, point, px,
};
use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Weak},
};

struct DropView(Rc<RefCell<Vec<PathBuf>>>);
impl Render for DropView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let dropped = self.0.clone();
        div()
            .id("incoming-owner")
            .size_full()
            .on_drop(move |paths: &ExternalPaths, _, _| {
                dropped.borrow_mut().extend_from_slice(paths.paths());
            })
    }
}

fn target(cx: &mut TestAppContext, dropped: &Rc<RefCell<Vec<PathBuf>>>) -> AnyWindowHandle {
    cx.update(|cx| cx.set_quit_mode(crate::QuitMode::Explicit));
    let dropped = dropped.clone();
    cx.add_window(move |_, _| DropView(dropped)).into()
}

fn dispatch(cx: &mut TestAppContext, handle: AnyWindowHandle, event: FileDropEvent) {
    cx.update_window(handle, |_, window, cx| {
        window.draw(cx).clear(cx);
        window.dispatch_event(PlatformInput::FileDrop(event), cx);
    })
    .unwrap();
}

fn enter(label: &str) -> FileDropEvent {
    FileDropEvent::Entered {
        position: point(px(10.), px(10.)),
        paths: ExternalPaths([PathBuf::from(label)].into_iter().collect()),
    }
}

fn active_path(cx: &TestAppContext) -> Option<PathBuf> {
    cx.read(|cx| {
        cx.active_drag
            .as_ref()
            .and_then(|drag| drag.value.downcast_ref::<ExternalPaths>())
            .and_then(|paths| paths.paths().first())
            .cloned()
    })
}

fn payload_weak(cx: &TestAppContext) -> Weak<dyn std::any::Any> {
    cx.read(|cx| Arc::downgrade(&cx.active_drag.as_ref().unwrap().value))
}

#[gpui::test]
fn close_reopen_releases_incoming_a_and_submits_only_b(cx: &mut TestAppContext) {
    let dropped = Rc::new(RefCell::new(Vec::new()));
    let a = target(cx, &dropped);
    dispatch(cx, a, enter("A"));
    let old_payload = payload_weak(cx);
    cx.update_window(a, |_, window, _| window.remove_window())
        .unwrap();
    assert!(old_payload.upgrade().is_none());
    assert_eq!(active_path(cx), None);
    let b = target(cx, &dropped);
    assert_ne!(a.window_id(), b.window_id());
    dispatch(cx, b, enter("B"));
    assert_eq!(active_path(cx), Some(PathBuf::from("B")));
    // The platform callback resolves its original handle; a removed window cannot update B.
    assert!(
        cx.update_window(a, |_, window, cx| {
            window.dispatch_event(PlatformInput::FileDrop(FileDropEvent::Exited), cx);
        })
        .is_err()
    );
    assert_eq!(active_path(cx), Some(PathBuf::from("B")));
    dispatch(
        cx,
        b,
        FileDropEvent::Submit {
            position: point(px(10.), px(10.)),
        },
    );
    assert_eq!(dropped.borrow().as_slice(), [PathBuf::from("B")]);
    assert_eq!(active_path(cx), None);
}

#[gpui::test]
fn older_window_callbacks_and_removal_preserve_live_b(cx: &mut TestAppContext) {
    let dropped = Rc::new(RefCell::new(Vec::new()));
    let a = target(cx, &dropped);
    let b = target(cx, &dropped);
    dispatch(cx, a, enter("A"));
    let old_payload = payload_weak(cx);
    dispatch(cx, b, enter("B"));
    assert!(old_payload.upgrade().is_none());
    for event in [
        FileDropEvent::Exited,
        FileDropEvent::Pending {
            position: point(px(90.), px(90.)),
        },
        FileDropEvent::Submit {
            position: point(px(10.), px(10.)),
        },
        FileDropEvent::Ended,
    ] {
        dispatch(cx, a, event);
        assert_eq!(active_path(cx), Some(PathBuf::from("B")));
        assert!(dropped.borrow().is_empty());
    }
    cx.update_window(a, |_, window, cx| {
        window.remove_window();
        for event in [
            enter("stale-A"),
            FileDropEvent::Exited,
            FileDropEvent::Submit {
                position: point(px(10.), px(10.)),
            },
        ] {
            window.dispatch_event(PlatformInput::FileDrop(event), cx);
        }
    })
    .unwrap();
    assert_eq!(active_path(cx), Some(PathBuf::from("B")));
    dispatch(
        cx,
        b,
        FileDropEvent::Submit {
            position: point(px(10.), px(10.)),
        },
    );
    assert_eq!(dropped.borrow().as_slice(), [PathBuf::from("B")]);
}

#[gpui::test]
fn image_and_url_incoming_payloads_release_on_owner_removal(cx: &mut TestAppContext) {
    let dropped = Rc::new(RefCell::new(Vec::new()));
    let keeper = target(cx, &dropped);
    for image in [true, false] {
        let window = target(cx, &dropped);
        let bytes = Arc::new(vec![1u8, 2, 3]);
        let raw = Arc::downgrade(&bytes);
        let event = if image {
            FileDropEvent::ImageEntered {
                position: point(px(10.), px(10.)),
                image: crate::ExternalImageDrop {
                    format: crate::ImageFormat::Png,
                    bytes: Some(bytes),
                },
            }
        } else {
            drop(bytes);
            FileDropEvent::UrlEntered {
                position: point(px(10.), px(10.)),
                url: crate::ExternalUrlDrop {
                    url: Arc::from("https://example.invalid/private.png"),
                },
            }
        };
        dispatch(cx, window, event);
        let payload = payload_weak(cx);
        cx.update_window(window, |_, window, _| window.remove_window())
            .unwrap();
        assert!(payload.upgrade().is_none());
        assert!(raw.upgrade().is_none());
        assert!(!cx.read(|cx| cx.has_active_drag()));
    }
    assert!(cx.update_window(keeper, |_, _, _| ()).is_ok());
}

struct InternalView {
    outgoing: bool,
}
impl Render for InternalView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let view = div().id("internal-owner").size_full().on_drag(
            ExternalPaths([PathBuf::from("internal")].into_iter().collect()),
            |_, _, _, cx| cx.new(|_| Empty),
        );
        if self.outgoing {
            view.external_drag_payload(|paths: &ExternalPaths, _, _| {
                Some(ExternalDragPayload::Files(FileDragPaths::new(
                    paths.paths().iter().cloned().map(|path| (path, false)),
                )))
            })
        } else {
            view
        }
    }
}

fn start_internal(cx: &mut TestAppContext, outgoing: bool) -> AnyWindowHandle {
    let handle: AnyWindowHandle = cx.add_window(move |_, _| InternalView { outgoing }).into();
    cx.test_window(handle).set_start_external_drag_result(true);
    cx.update_window(handle, |_, window, cx| {
        window.draw(cx).clear(cx);
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                position: point(px(10.), px(10.)),
                button: MouseButton::Left,
                modifiers: Default::default(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: point(px(20.), px(20.)),
                pressed_button: Some(MouseButton::Left),
                modifiers: Default::default(),
            }),
            cx,
        );
    })
    .unwrap();
    assert_eq!(active_path(cx), Some(PathBuf::from("internal")));
    handle
}

#[gpui::test]
fn incoming_cancellation_and_other_window_removal_preserve_internal_drag(cx: &mut TestAppContext) {
    let dropped = Rc::new(RefCell::new(Vec::new()));
    let other = target(cx, &dropped);
    let internal = start_internal(cx, false);
    cx.update(|cx| cx.cancel_pending_native_drop());
    assert_eq!(active_path(cx), Some(PathBuf::from("internal")));
    dispatch(cx, other, FileDropEvent::Exited);
    dispatch(
        cx,
        other,
        FileDropEvent::Submit {
            position: point(px(10.), px(10.)),
        },
    );
    cx.update_window(other, |_, window, _| window.remove_window())
        .unwrap();
    assert_eq!(active_path(cx), Some(PathBuf::from("internal")));
    assert!(dropped.borrow().is_empty());
    cx.update_window(internal, |_, window, _| window.remove_window())
        .unwrap();
    assert!(!cx.read(|cx| cx.has_active_drag()));
}

#[gpui::test]
fn closing_suspended_outgoing_source_preserves_incoming_target_b(cx: &mut TestAppContext) {
    let dropped = Rc::new(RefCell::new(Vec::new()));
    let b = target(cx, &dropped);
    let source = start_internal(cx, true);
    cx.update_window(source, |_, window, cx| {
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: point(px(-1.), px(20.)),
                pressed_button: Some(MouseButton::Left),
                modifiers: Default::default(),
            }),
            cx,
        );
    })
    .unwrap();
    assert!(!cx.read(|cx| cx.has_active_drag()));
    dispatch(cx, b, enter("B"));
    cx.update_window(source, |_, window, _| window.remove_window())
        .unwrap();
    assert_eq!(active_path(cx), Some(PathBuf::from("B")));
    assert!(!cx.update(|cx| cx.end_platform_drag(source.window_id())));
    assert_eq!(active_path(cx), Some(PathBuf::from("B")));
    dispatch(
        cx,
        b,
        FileDropEvent::Submit {
            position: point(px(10.), px(10.)),
        },
    );
    assert_eq!(dropped.borrow().as_slice(), [PathBuf::from("B")]);
}
