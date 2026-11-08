//! Application core. This crate must not depend on GPUI.

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
