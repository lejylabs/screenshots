//! Production Windows terminal transitions against GPUI's real headless drop consumer.
#[path = "../../gpui-pre-windows/src/file_drop.rs"]
mod file_drop;

use crate::{
    AnyWindowHandle, App, AppContext as _, Context, ExternalPaths, FileDropEvent,
    InteractiveElement as _, IntoElement, Render, Styled, TestAppContext, Window, div, point, px,
};
use file_drop::FileDropState;
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    sync::Arc,
};

struct DropView(Rc<RefCell<Vec<PathBuf>>>);

impl Render for DropView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let dropped = self.0.clone();
        div()
            .id("windows-drop-consumer")
            .size_full()
            .on_drop(move |paths: &ExternalPaths, _, _| {
                dropped.borrow_mut().extend_from_slice(paths.paths())
            })
    }
}

#[derive(Default)]
struct Helper {
    active: Cell<Option<&'static str>>,
    notifications: RefCell<Vec<(&'static str, &'static str)>>,
}

impl Helper {
    fn enter(&self, label: &'static str, accepted: bool) {
        assert!(self.active.replace(Some(label)).is_none());
        self.notifications
            .borrow_mut()
            .push((label, if accepted { "enter-copy" } else { "enter-none" }));
    }

    fn terminal(&self, kind: &'static str, reenter: impl FnOnce()) {
        let label = self
            .active
            .take()
            .expect("exactly one terminal per helper Enter");
        self.notifications.borrow_mut().push((label, kind));
        reenter();
    }
}

struct Target<'a, 'b> {
    state: FileDropState,
    helper: Helper,
    consumer: RefCell<(&'a mut Window, &'b mut App)>,
}

impl Target<'_, '_> {
    fn dispatch(&self, event: FileDropEvent) {
        let mut consumer = self.consumer.borrow_mut();
        let (window, cx) = &mut *consumer;
        window.dispatch_event(crate::PlatformInput::FileDrop(event), cx);
    }

    fn active_path(&self) -> Option<PathBuf> {
        let consumer = self.consumer.borrow();
        consumer
            .1
            .active_drag
            .as_ref()
            .and_then(|drag| drag.value.downcast_ref::<ExternalPaths>())
            .and_then(|paths| paths.paths().first())
            .cloned()
    }

    fn enter(&self, label: &'static str, accepted: bool) {
        let generation = self
            .state
            .begin_enter(
                || self.dispatch(FileDropEvent::Exited),
                || self.helper.terminal("leave", || {}),
            )
            .unwrap();
        assert!(self.state.helper_enter(generation));
        self.helper.enter(label, accepted);
        if accepted {
            assert!(self.state.accept(generation));
            self.dispatch(FileDropEvent::Entered {
                position: point(px(10.), px(10.)),
                paths: ExternalPaths([PathBuf::from(label)].into_iter().collect()),
            });
        }
    }

    fn leave(&self, reenter: impl FnOnce(&Self)) {
        self.state.finish_leave(
            || self.dispatch(FileDropEvent::Exited),
            || self.helper.terminal("leave", || reenter(self)),
        );
    }

    fn drop(&self, reenter: impl FnOnce(&Self)) -> bool {
        self.state.finish_drop(
            |accepted| {
                self.helper
                    .terminal(if accepted { "drop-copy" } else { "drop-none" }, || {
                        reenter(self)
                    })
            },
            |accepted| {
                if accepted {
                    self.dispatch(FileDropEvent::Submit {
                        position: point(px(10.), px(10.)),
                    });
                }
                accepted
            },
        )
    }

    fn drop_missing_data(&self, reenter: impl FnOnce(&Self)) -> bool {
        self.state.finish_drop(
            |_| self.helper.terminal("leave", || reenter(self)),
            |accepted| {
                if accepted {
                    self.dispatch(FileDropEvent::Exited);
                }
                false
            },
        )
    }
}

fn with_target(cx: &mut TestAppContext, check: impl FnOnce(&Target, &Rc<RefCell<Vec<PathBuf>>>)) {
    cx.update(|cx| cx.set_quit_mode(crate::QuitMode::Explicit));
    let dropped = Rc::new(RefCell::new(Vec::new()));
    let handle: AnyWindowHandle = cx
        .add_window({
            let dropped = dropped.clone();
            move |_, _| DropView(dropped)
        })
        .into();
    cx.update_window(handle, |_, window, cx| {
        window.draw(cx).clear(cx);
        let target = Target {
            state: FileDropState::default(),
            helper: Helper::default(),
            consumer: RefCell::new((window, cx)),
        };
        check(&target, &dropped);
    })
    .unwrap();
}

#[gpui::test]
fn normal_drop_submits_once_and_balances_helper(cx: &mut TestAppContext) {
    with_target(cx, |target, dropped| {
        target.enter("A", true);
        assert_eq!(target.active_path(), Some(PathBuf::from("A")));
        assert!(target.drop(|_| {}));
        assert_eq!(dropped.borrow().as_slice(), [PathBuf::from("A")]);
        assert_eq!(target.active_path(), None);
        target.leave(|_| panic!("duplicate terminal"));
        assert_eq!(
            *target.helper.notifications.borrow(),
            [("A", "enter-copy"), ("A", "drop-copy")]
        );
    });
}

#[gpui::test]
fn leave_clears_a_before_helper_reenters_b(cx: &mut TestAppContext) {
    with_target(cx, |target, dropped| {
        target.enter("A", true);
        target.leave(|target| {
            assert_eq!(target.active_path(), None);
            target.enter("B", true);
        });
        assert_eq!(target.active_path(), Some(PathBuf::from("B")));
        assert!(target.drop(|_| {}));
        assert_eq!(dropped.borrow().as_slice(), [PathBuf::from("B")]);
        assert_eq!(target.active_path(), None);
        assert_eq!(
            *target.helper.notifications.borrow(),
            [
                ("A", "enter-copy"),
                ("A", "leave"),
                ("B", "enter-copy"),
                ("B", "drop-copy")
            ]
        );
    });
}

#[gpui::test]
fn drop_helper_reentry_submits_b_without_retaining_or_submitting_a(cx: &mut TestAppContext) {
    with_target(cx, |target, dropped| {
        target.enter("A", true);
        assert!(!target.drop(|target| target.enter("B", true)));
        assert!(dropped.borrow().is_empty());
        assert_eq!(target.active_path(), Some(PathBuf::from("B")));
        assert!(target.drop(|_| {}));
        assert_eq!(dropped.borrow().as_slice(), [PathBuf::from("B")]);
        assert_eq!(target.active_path(), None);
        assert_eq!(
            *target.helper.notifications.borrow(),
            [
                ("A", "enter-copy"),
                ("A", "drop-copy"),
                ("B", "enter-copy"),
                ("B", "drop-copy")
            ]
        );
    });
}

#[gpui::test]
fn drop_helper_reentrant_leave_suppresses_old_submit(cx: &mut TestAppContext) {
    with_target(cx, |target, dropped| {
        target.enter("A", true);
        assert!(!target.drop(|target| target.leave(|_| panic!("helper already terminated"))));
        assert_eq!(target.active_path(), None);
        assert!(dropped.borrow().is_empty());
        target.enter("B", true);
        assert!(target.drop(|_| {}));
        assert_eq!(dropped.borrow().as_slice(), [PathBuf::from("B")]);
    });
}

#[gpui::test]
fn refused_and_missing_data_drops_end_helper_without_submit(cx: &mut TestAppContext) {
    with_target(cx, |target, dropped| {
        target.enter("A", true);
        assert!(!target.drop(|target| target.enter("B", false)));
        assert_eq!(target.active_path(), None);
        assert!(!target.drop(|_| {}));
        target.enter("C", false);
        assert!(!target.drop_missing_data(|_| {}));
        target.enter("D", true);
        assert!(!target.drop_missing_data(|_| {}));
        assert_eq!(target.active_path(), None);
        assert!(dropped.borrow().is_empty());
        assert_eq!(
            *target.helper.notifications.borrow(),
            [
                ("A", "enter-copy"),
                ("A", "drop-copy"),
                ("B", "enter-none"),
                ("B", "drop-none"),
                ("C", "enter-none"),
                ("C", "leave"),
                ("D", "enter-copy"),
                ("D", "leave")
            ]
        );
        assert!(target.helper.active.get().is_none());
    });
}

#[gpui::test]
fn refused_drop_helper_reentry_keeps_b_payload_and_terminal_lease(cx: &mut TestAppContext) {
    with_target(cx, |target, dropped| {
        target.enter("A", false);
        assert!(!target.drop(|target| target.enter("B", true)));
        assert_eq!(target.active_path(), Some(PathBuf::from("B")));
        assert!(target.drop(|_| {}));
        assert_eq!(dropped.borrow().as_slice(), [PathBuf::from("B")]);
        assert_eq!(
            *target.helper.notifications.borrow(),
            [
                ("A", "enter-none"),
                ("A", "drop-none"),
                ("B", "enter-copy"),
                ("B", "drop-copy")
            ]
        );
    });
}

#[gpui::test]
fn closed_target_terminal_releases_helper_without_dispatching_old_input(cx: &mut TestAppContext) {
    let old_payload = RefCell::new(None);
    with_target(cx, |target, dropped| {
        target.enter("A", true);
        old_payload.replace(Some({
            let consumer = target.consumer.borrow();
            Arc::downgrade(&consumer.1.active_drag.as_ref().unwrap().value)
        }));
        target.consumer.borrow_mut().0.remove_window();
        target.leave(|_| {});
        assert!(!target.drop(|_| panic!("helper already ended")));
        assert!(dropped.borrow().is_empty());
        assert_eq!(
            *target.helper.notifications.borrow(),
            [("A", "enter-copy"), ("A", "leave")]
        );
        assert!(!target.state.helper_is_open());
    });
    assert!(old_payload.borrow().as_ref().unwrap().upgrade().is_none());
    assert!(!cx.read(|cx| cx.has_active_drag()));
}
