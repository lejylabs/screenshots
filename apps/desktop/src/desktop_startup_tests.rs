//! Exercise the same dispatch boundary as main with pure callbacks only.
use super::launch;
use std::{cell::RefCell, ffi::OsString};

#[test]
fn native_worker_and_cli_early_returns_never_enter_gui_or_logging_startup() {
    for (child, installed, cli) in [
        (Ok(true), Ok(()), Ok(false)),
        (Ok(true), Err("must not install"), Err("must not enter CLI")),
        (Err("native error"), Ok(()), Ok(false)),
        (Ok(false), Err("install error"), Ok(false)),
        (Ok(false), Ok(()), Ok(true)),
        (Ok(false), Ok(()), Err("CLI error")),
        (Ok(false), Ok(()), Ok(false)),
    ] {
        let trace = RefCell::new(Vec::new());
        let arguments = vec![OsString::from("private-dispatch-fixture")];
        let result = launch(
            arguments.clone(),
            |actual| {
                assert_eq!(actual, arguments);
                trace.borrow_mut().push("native");
                child.map_err(anyhow::Error::msg)
            },
            || {
                trace.borrow_mut().push("install");
                installed.map_err(anyhow::Error::msg)
            },
            |actual| {
                assert_eq!(actual, arguments);
                trace.borrow_mut().push("cli");
                cli.map_err(anyhow::Error::msg)
            },
            || trace.borrow_mut().push("gui_and_logging_admitted"),
        );
        let expected = if child != Ok(false) {
            vec!["native"]
        } else if installed.is_err() {
            vec!["native", "install"]
        } else if cli != Ok(false) {
            vec!["native", "install", "cli"]
        } else {
            vec!["native", "install", "cli", "gui_and_logging_admitted"]
        };
        assert_eq!(*trace.borrow(), expected);
        assert_eq!(
            result.is_err(),
            child.is_err() || (child == Ok(false) && (installed.is_err() || cli.is_err()))
        );
    }
}
