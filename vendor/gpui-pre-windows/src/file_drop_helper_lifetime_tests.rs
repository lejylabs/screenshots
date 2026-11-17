//! Per-window helper factory/endpoints; foreign callbacks run without a borrowed trace.
use super::FileDropState;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

#[derive(Default)]
struct Factory {
    next: Cell<usize>,
    trace: Rc<RefCell<Vec<(usize, &'static str)>>>,
}

impl Factory {
    fn window(&self) -> Rc<Window> {
        let id = self.next.get() + 1;
        self.next.set(id);
        Rc::new(Window {
            state: FileDropState::default(),
            helper: Helper {
                id,
                active: Cell::new(false),
                trace: self.trace.clone(),
            },
            native_destroyed: Cell::new(false),
        })
    }
}

struct Helper {
    id: usize,
    active: Cell<bool>,
    trace: Rc<RefCell<Vec<(usize, &'static str)>>>,
}

impl Helper {
    fn enter(&self, reenter: impl FnOnce()) {
        assert!(!self.active.replace(true));
        self.record("enter");
        reenter();
    }

    fn terminal(&self, kind: &'static str, reenter: impl FnOnce()) {
        assert!(self.active.replace(false), "terminal without helper Enter");
        self.record(kind);
        reenter();
    }

    fn record(&self, kind: &'static str) {
        self.trace.borrow_mut().push((self.id, kind));
    }
}

struct Window {
    state: FileDropState,
    helper: Helper,
    native_destroyed: Cell<bool>,
}

impl Window {
    fn enter(&self, reenter: impl FnOnce()) -> Option<u64> {
        let generation = self
            .state
            .begin_enter(|| {}, || self.helper.terminal("leave", || {}))?;
        assert!(self.state.helper_enter(generation));
        self.helper.enter(reenter);
        self.state.accept(generation).then_some(generation)
    }

    fn leave(&self, reenter: impl FnOnce()) {
        self.state
            .finish_leave(|| {}, || self.helper.terminal("leave", reenter));
    }

    fn submit(&self, reenter: impl FnOnce()) -> bool {
        self.state.finish_drop(
            |_| self.helper.terminal("drop", reenter),
            |accepted| accepted,
        )
    }

    fn close(window: &Rc<Self>) -> Option<Teardown> {
        window.state.close().map(|helper_leave| Teardown {
            window: window.clone(),
            helper_leave,
        })
    }
}

struct Teardown {
    window: Rc<Window>,
    helper_leave: bool,
}

impl Teardown {
    fn run(self, reenter: impl FnOnce()) {
        if self.helper_leave {
            self.window.helper.terminal("leave", reenter);
        }
        if !self.window.native_destroyed.get() {
            self.window.helper.record("revoke");
            if !self.window.native_destroyed.get() {
                self.window.helper.record("destroy");
                self.window.native_destroyed.set(true);
            }
        }
    }
}

#[test]
fn close_without_os_leave_owns_a_helper_while_b_and_late_a_callbacks_are_independent() {
    let factory = Factory::default();
    let a = factory.window();
    let b = factory.window();
    assert_ne!(a.helper.id, b.helper.id);
    a.enter(|| {}).unwrap();
    let teardown = Window::close(&a).unwrap();
    assert!(!a.state.helper_is_open());
    assert!(a.state.accepted().is_none());
    b.enter(|| {}).unwrap();
    a.leave(|| panic!("late A must not call a helper"));
    assert!(!a.submit(|| panic!("late A must not call a helper")));
    assert!(a.enter(|| panic!("closed A must not reenter")).is_none());
    assert!(Window::close(&a).is_none());
    teardown.run(|| {});
    assert!(b.helper.active.get());
    assert!(b.state.accepted().is_some());
    assert!(b.submit(|| {}));
    assert_eq!(
        *factory.trace.borrow(),
        [
            (1, "enter"),
            (2, "enter"),
            (1, "leave"),
            (1, "revoke"),
            (1, "destroy"),
            (2, "drop")
        ]
    );
    a.leave(|| panic!("duplicate late A terminal"));
}

#[test]
fn helper_enter_reentrant_close_claims_one_leave_without_affecting_new_endpoint() {
    let factory = Factory::default();
    let a = factory.window();
    let b = factory.window();
    let teardown = RefCell::new(None);
    assert!(
        a.enter(|| {
            teardown.replace(Window::close(&a));
            b.enter(|| {}).unwrap();
        })
        .is_none()
    );
    teardown.take().unwrap().run(|| {});
    assert!(b.helper.active.get());
    assert!(b.submit(|| {}));
    assert_eq!(
        *factory.trace.borrow(),
        [
            (1, "enter"),
            (2, "enter"),
            (1, "leave"),
            (1, "revoke"),
            (1, "destroy"),
            (2, "drop")
        ]
    );
}

#[test]
fn helper_terminal_reentrant_close_never_schedules_a_second_terminal() {
    for drop in [false, true] {
        let factory = Factory::default();
        let a = factory.window();
        let b = factory.window();
        let teardown = RefCell::new(None);
        a.enter(|| {}).unwrap();
        let close_and_enter_b = || {
            teardown.replace(Window::close(&a));
            b.enter(|| {}).unwrap();
        };
        if drop {
            assert!(!a.submit(close_and_enter_b));
        } else {
            a.leave(close_and_enter_b);
        }
        teardown
            .take()
            .unwrap()
            .run(|| panic!("terminal was already consumed"));
        assert!(Window::close(&a).is_none());
        a.leave(|| panic!("closed A terminal"));
        assert!(b.helper.active.get());
        assert!(b.submit(|| {}));
        let terminal = if drop { "drop" } else { "leave" };
        assert_eq!(
            *factory.trace.borrow(),
            [
                (1, "enter"),
                (1, terminal),
                (2, "enter"),
                (1, "revoke"),
                (1, "destroy"),
                (2, "drop")
            ]
        );
    }
}

#[test]
fn close_leave_reentry_cannot_destroy_reused_native_target_or_end_b_helper() {
    let factory = Factory::default();
    let a = factory.window();
    let b = factory.window();
    a.enter(|| {}).unwrap();
    let teardown = Window::close(&a).unwrap();
    teardown.run(|| {
        // Models native destruction delivered while Leave pumps messages.
        a.native_destroyed.set(true);
        assert!(Window::close(&a).is_none());
        b.enter(|| {}).unwrap();
    });
    assert!(b.helper.active.get());
    assert!(b.submit(|| {}));
    assert_eq!(
        *factory.trace.borrow(),
        [(1, "enter"), (1, "leave"), (2, "enter"), (2, "drop")]
    );
}
