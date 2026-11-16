//! Verify ownership, logger ordering and teardown without constructing a native platform.
use super::with_desktop_startup;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

struct OwnedToken {
    trace: Rc<RefCell<Vec<&'static str>>>,
    drop_event: &'static str,
}
impl Drop for OwnedToken {
    fn drop(&mut self) {
        self.trace.borrow_mut().push(self.drop_event);
    }
}

#[test]
fn desktop_prepares_once_before_services_and_keeps_the_guard_until_owned_run_finishes() {
    let trace = Rc::new(RefCell::new(Vec::new()));
    let factory_calls = Cell::new(0);
    let original = Cell::new(std::ptr::null());
    with_desktop_startup(
        || {
            factory_calls.set(factory_calls.get() + 1);
            trace.borrow_mut().push("native_startup_taken");
            let platform = Box::new(OwnedToken {
                trace: trace.clone(),
                drop_event: "platform_dropped",
            });
            original.set((&*platform) as *const OwnedToken);
            platform
        },
        || {
            assert_eq!(*trace.borrow(), ["native_startup_taken"]);
            trace.borrow_mut().push("logger_started");
            OwnedToken {
                trace: trace.clone(),
                drop_event: "logger_guard_dropped",
            }
        },
        |platform| {
            assert_eq!((&*platform) as *const OwnedToken, original.get());
            assert_eq!(*trace.borrow(), ["native_startup_taken", "logger_started"]);
            trace.borrow_mut().push("run");
            drop(platform);
        },
    );
    assert_eq!(factory_calls.get(), 1);
    assert_eq!(
        *trace.borrow(),
        [
            "native_startup_taken",
            "logger_started",
            "run",
            "platform_dropped",
            "logger_guard_dropped"
        ]
    );
}

#[test]
fn a_platform_constructor_failure_never_starts_services_or_run() {
    let services = Cell::new(false);
    let run = Cell::new(false);
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        with_desktop_startup(
            || panic!("synthetic platform constructor failure"),
            || services.set(true),
            |_: ()| run.set(true),
        );
    }));
    assert!(failure.is_err());
    assert!(!services.get() && !run.get());
}

#[test]
fn a_service_start_failure_retires_the_owned_platform_without_running_it() {
    let trace = Rc::new(RefCell::new(Vec::new()));
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        with_desktop_startup(
            || {
                Box::new(OwnedToken {
                    trace: trace.clone(),
                    drop_event: "platform_dropped",
                })
            },
            || panic!("synthetic service startup failure"),
            |_| trace.borrow_mut().push("run"),
        );
    }));
    assert!(failure.is_err());
    assert_eq!(*trace.borrow(), ["platform_dropped"]);
}

#[test]
fn run_failure_drops_the_owned_platform_before_the_logger_guard() {
    let trace = Rc::new(RefCell::new(Vec::new()));
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        with_desktop_startup(
            || {
                Box::new(OwnedToken {
                    trace: trace.clone(),
                    drop_event: "platform_dropped",
                })
            },
            || OwnedToken {
                trace: trace.clone(),
                drop_event: "logger_guard_dropped",
            },
            |_| panic!("synthetic GUI run failure"),
        );
    }));
    assert!(failure.is_err());
    assert_eq!(
        *trace.borrow(),
        ["platform_dropped", "logger_guard_dropped"]
    );
}
