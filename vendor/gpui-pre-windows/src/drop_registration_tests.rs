use super::DropRegistration;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
};

#[derive(Default)]
struct Native {
    next: Cell<usize>,
    current_handle_owner: Cell<Option<usize>>,
    // Models OLE's AddRef: this is the only owner after the application lets go.
    retained: RefCell<BTreeMap<usize, Rc<Target>>>,
    trace: RefCell<Vec<(usize, &'static str)>>,
    released: Cell<usize>,
    freed: Cell<usize>,
}

impl Native {
    fn window(self: &Rc<Self>) -> Rc<Target> {
        let id = self.next.get() + 1;
        self.next.set(id);
        assert!(self.current_handle_owner.replace(Some(id)).is_none());
        Rc::new(Target {
            id,
            native: self.clone(),
            registration: DropRegistration::default(),
            destruction_started: Cell::new(false),
            destroyed: Cell::new(false),
            visible: Cell::new(true),
        })
    }

    fn call(&self, id: usize, name: &'static str) {
        assert_eq!(
            self.current_handle_owner.get(),
            Some(id),
            "stale HWND call reached a reused handle"
        );
        self.trace.borrow_mut().push((id, name));
    }
}

struct Target {
    id: usize,
    native: Rc<Native>,
    registration: DropRegistration,
    destruction_started: Cell<bool>,
    destroyed: Cell<bool>,
    visible: Cell<bool>,
}

impl Drop for Target {
    fn drop(&mut self) {
        self.native.freed.set(self.native.freed.get() + 1);
    }
}

impl Target {
    fn register(self: &Rc<Self>, fail: bool) -> Result<(), &'static str> {
        self.registration.register(|| {
            if fail {
                return Err("registration failed");
            }
            self.native.call(self.id, "register-addref");
            assert!(
                self.native
                    .retained
                    .borrow_mut()
                    .insert(self.id, self.clone())
                    .is_none()
            );
            Ok(())
        })
    }

    fn revoke(&self, reenter: impl FnOnce()) -> Result<bool, &'static str> {
        self.registration.revoke(|| {
            self.native.call(self.id, "revoke-release");
            let registration_reference =
                self.native.retained.borrow_mut().remove(&self.id).unwrap();
            self.native.released.set(self.native.released.get() + 1);
            drop(registration_reference);
            reenter();
            Ok(())
        })
    }

    fn unexpected_destroy(&self, reenter_revoke: impl FnOnce()) {
        self.destruction_started.set(true);
        self.revoke(reenter_revoke).unwrap();
        if !self.destroyed.replace(true) {
            self.native.call(self.id, "nc-destroy");
            self.native.current_handle_owner.set(None);
        }
    }

    fn queued_teardown(&self, reenter_revoke: impl FnOnce()) {
        if !self.destruction_started.get() {
            self.native.call(self.id, "hide");
            self.visible.set(false);
        }
        if !self.destruction_started.get() && self.revoke(reenter_revoke).unwrap() {
            if !self.destruction_started.get() {
                self.native.call(self.id, "destroy");
                self.unexpected_destroy(|| panic!("already revoked"));
            }
        }
    }
}

#[test]
fn unexpected_destroy_releases_registered_target_without_queued_window_drop() {
    let native = Rc::new(Native::default());
    let target = native.window();
    target.register(false).unwrap();
    let weak = Rc::downgrade(&target);
    drop(target);
    assert!(
        weak.upgrade().is_some(),
        "OLE must really own the resource in this fixture"
    );
    {
        let message_guard = weak.upgrade().unwrap();
        message_guard.unexpected_destroy(|| {});
        assert!(native.retained.borrow().is_empty());
    }
    assert!(weak.upgrade().is_none());
    assert_eq!(native.released.get(), 1);
    assert_eq!(native.freed.get(), 1);
    assert_eq!(
        *native.trace.borrow(),
        [
            (1, "register-addref"),
            (1, "revoke-release"),
            (1, "nc-destroy")
        ]
    );
}

#[test]
fn duplicate_and_reentrant_destroy_revoke_release_only_once() {
    let native = Rc::new(Native::default());
    let a = native.window();
    a.register(false).unwrap();
    assert!(
        a.revoke(|| {
            assert!(
                !a.revoke(|| panic!("nested revoke must not call OLE"))
                    .unwrap()
            );
            a.unexpected_destroy(|| panic!("nested destroy must not revoke twice"));
        })
        .unwrap()
    );
    a.unexpected_destroy(|| panic!("duplicate destroy must not call OLE"));
    a.queued_teardown(|| panic!("destroyed target must not call OLE"));
    assert_eq!(native.released.get(), 1);
    assert!(native.retained.borrow().is_empty());
    drop(a);
    assert_eq!(native.freed.get(), 1);
}

#[test]
fn already_destroyed_queued_teardown_never_hides_revokes_or_destroys_reused_b() {
    let native = Rc::new(Native::default());
    let a = native.window();
    a.register(false).unwrap();
    a.unexpected_destroy(|| {});
    let b = native.window(); // Deliberately reuses the same native handle slot.
    b.register(false).unwrap();
    let before = native.trace.borrow().clone();
    a.queued_teardown(|| panic!("old A must not call OLE for B's handle"));
    assert_eq!(*native.trace.borrow(), before);
    assert!(b.visible.get());
    assert!(native.retained.borrow().contains_key(&b.id));
    b.queued_teardown(|| {});
    assert_eq!(native.released.get(), 2);
    assert!(native.retained.borrow().is_empty());
}

#[test]
fn registration_failure_rolls_back_and_revoke_failure_keeps_the_real_resource_lease() {
    let native = Rc::new(Native::default());
    let failed = native.window();
    assert_eq!(failed.register(true), Err("registration failed"));
    assert!(
        failed
            .revoke(|| panic!("failed Register acquired no native reference"))
            .unwrap()
    );
    failed.queued_teardown(|| panic!("failed Register acquired no native reference"));
    drop(failed);
    let target = native.window();
    target.register(false).unwrap();
    assert_eq!(
        target.registration.revoke(|| Err("native failure")),
        Err("native failure")
    );
    let weak = Rc::downgrade(&target);
    drop(target);
    assert!(
        weak.upgrade().is_some(),
        "failed Revoke must not pretend Release happened"
    );
    {
        let retry = weak.upgrade().unwrap();
        assert!(retry.revoke(|| {}).unwrap());
        retry.unexpected_destroy(|| panic!("successful revoke must not repeat"));
    }
    assert!(weak.upgrade().is_none());
    assert_eq!(native.released.get(), 1);
    assert_eq!(native.freed.get(), 2);
}
