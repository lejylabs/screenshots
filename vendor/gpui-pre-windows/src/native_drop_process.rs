//! Desktop-owned atomic process creation, installed without starting a runtime.
use anyhow::{Result, ensure};
use std::{
    fs::File,
    os::windows::io::{BorrowedHandle, OwnedHandle},
    process::Command,
    sync::OnceLock,
};

pub const CHILD_ARGUMENT: &str = "--native-drop-receiver";

/// Only the background controller owns this; foreground delivery holds plain data.
pub struct NativeDropProcess {
    pub process: OwnedHandle,
    pub job: OwnedHandle,
    pub read: File,
    pub write: File,
    pub nonce: [u8; 16],
}

pub type NativeDropSpawner =
    for<'a> fn(&mut Command, &[BorrowedHandle<'a>]) -> Result<NativeDropProcess>;
static SPAWNER: OnceLock<NativeDropSpawner> = OnceLock::new();

/// Stores a function pointer only; no process, clipboard access or native call.
pub fn install_native_drop_spawner(spawner: NativeDropSpawner) -> Result<()> {
    ensure!(
        SPAWNER.set(spawner).is_ok(),
        "Native drop spawner is already installed"
    );
    Ok(())
}
pub(crate) fn installed() -> bool {
    SPAWNER.get().is_some()
}
pub(crate) fn spawn(
    command: &mut Command,
    inherited: &[BorrowedHandle<'_>],
) -> Result<NativeDropProcess> {
    SPAWNER
        .get()
        .ok_or_else(|| anyhow::anyhow!("Native drop process creation is unavailable"))?(
        command, inherited,
    )
}
