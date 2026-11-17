#![cfg(target_os = "windows")]

#[cfg(screenshots_native_drop_acceptance)]
mod native_drop_acceptance;
#[cfg(screenshots_native_drop_acceptance)]
pub use native_drop_acceptance::{
    arm_consumer as native_drop_acceptance_arm_consumer,
    await_consumer as native_drop_acceptance_await_consumer,
    busy_admission as native_drop_acceptance_busy_admission,
    hold_delivery as hold_native_drop_acceptance_delivery,
    release_delivery as release_native_drop_acceptance_delivery, run as run_native_drop_acceptance,
    run_ui as run_native_drop_ui_acceptance, ui_drive as native_drop_acceptance_drive,
    ui_input as native_drop_acceptance_input, ui_rearm as native_drop_acceptance_rearm,
    wait_reaped as native_drop_acceptance_wait_reaped,
};
#[cfg(screenshots_native_drop_acceptance)]
pub use window::native_drop_acceptance_window_state;
mod clipboard;
mod native_drop_broker;
mod native_drop_deadline;
mod native_drop_events;
mod native_drop_process;
mod native_drop_receiver;
mod native_drop_wire;
pub use native_drop_process::CHILD_ARGUMENT as NATIVE_DROP_CHILD_ARGUMENT;
pub use native_drop_process::{NativeDropProcess, NativeDropSpawner, install_native_drop_spawner};
pub use native_drop_receiver::serve as native_drop_receiver;
mod destination_list;
mod dialog;
mod direct_manipulation;
mod direct_write;
mod directx_atlas;
mod directx_devices;
mod directx_renderer;
mod dispatcher;
mod display;
mod events;
mod keyboard;
mod platform;
mod system_notifications;
mod system_settings;
mod util;
mod vsync;
mod window;
mod wrapper;

pub(crate) use clipboard::*;
pub(crate) use destination_list::*;
pub(crate) use direct_write::*;
pub(crate) use directx_atlas::*;
pub(crate) use directx_devices::*;
pub(crate) use directx_renderer::*;
pub(crate) use dispatcher::*;
pub(crate) use display::*;
pub(crate) use events::*;
pub(crate) use keyboard::*;
pub(crate) use platform::*;
pub(crate) use system_notifications::*;
pub(crate) use system_settings::*;
pub(crate) use util::*;
pub(crate) use vsync::*;
pub(crate) use window::*;
pub(crate) use wrapper::*;

pub use platform::WindowsPlatform;

pub(crate) use windows::Win32::Foundation::HWND;
