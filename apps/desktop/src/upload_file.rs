//! Explicit byte-preserving file uploads through the existing transport.

use anyhow::Result;
use std::ffi::OsString;

pub fn run(args: &[OsString]) -> Result<()> {
    crate::sharex::single(args, false)
}
