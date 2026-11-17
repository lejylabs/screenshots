//! Actual WM_CLOSE routing helper with private callbacks and an inert HWND.
use super::route_window_close;
use crate::{Callbacks, HWND, dialog::DialogOwner};
use std::{cell::Cell, rc::Rc};

#[test]
fn accepted_close_routes_once_before_any_default_native_destroy() {
    let callbacks = Rc::new(Callbacks::default());
    let owner = DialogOwner::new(HWND::default());
    let queued = Rc::new(Cell::new(0));
    callbacks.close.set(Some(Box::new({
        let callbacks = callbacks.clone();
        let owner = owner.clone();
        let queued = queued.clone();
        move || {
            assert!(owner.is_closed());
            queued.set(queued.get() + 1);
            assert_eq!(route_window_close(&callbacks, &owner), Some(0));
        }
    })));
    assert_eq!(route_window_close(&callbacks, &owner), Some(0));
    assert_eq!(queued.get(), 1);
    assert_eq!(route_window_close(&callbacks, &owner), Some(0));
    assert_eq!(queued.get(), 1);
    // Before GPUI installs on_close, preserve the existing default-procedure fallback.
    assert_eq!(
        route_window_close(&Callbacks::default(), &DialogOwner::new(HWND::default())),
        None
    );
}

#[test]
fn veto_keeps_owner_and_callbacks_available_for_later_close() {
    let callbacks = Rc::new(Callbacks::default());
    let owner = DialogOwner::new(HWND::default());
    let accepted = Rc::new(Cell::new(false));
    let queued = Rc::new(Cell::new(0));
    callbacks.should_close.set(Some(Box::new({
        let accepted = accepted.clone();
        let callbacks = Rc::downgrade(&callbacks);
        let owner = owner.clone();
        let queued = queued.clone();
        move || {
            assert_eq!(
                route_window_close(&callbacks.upgrade().unwrap(), &owner),
                Some(0)
            );
            assert_eq!(queued.get(), 0);
            accepted.get()
        }
    })));
    callbacks.close.set(Some(Box::new({
        let queued = queued.clone();
        move || queued.set(queued.get() + 1)
    })));
    assert_eq!(route_window_close(&callbacks, &owner), Some(0));
    assert!(!owner.is_closed());
    assert_eq!(queued.get(), 0);
    accepted.set(true);
    assert_eq!(route_window_close(&callbacks, &owner), Some(0));
    assert!(owner.is_closed());
    assert_eq!(queued.get(), 1);
}

#[test]
fn reentrant_owner_close_during_veto_callback_never_falls_through_to_destroy() {
    let callbacks = Callbacks::default();
    let owner = DialogOwner::new(HWND::default());
    let queued = Rc::new(Cell::new(0));
    callbacks.should_close.set(Some(Box::new({
        let owner = owner.clone();
        move || {
            owner.close();
            true
        }
    })));
    callbacks.close.set(Some(Box::new({
        let queued = queued.clone();
        move || queued.set(queued.get() + 1)
    })));
    assert_eq!(route_window_close(&callbacks, &owner), Some(0));
    assert_eq!(queued.get(), 0);
    assert!(owner.is_closed());
}
