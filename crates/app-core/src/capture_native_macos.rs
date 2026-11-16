use crate::{
    CaptureCursor, CaptureMonitorBinding, CaptureMonitorIdentity, CaptureOptions, DisplayInfo,
};
use anyhow::{Context, Result, bail, ensure};
use core_foundation::{
    base::TCFType,
    string::CFString,
    uuid::{CFUUID, CFUUIDCreateString, CFUUIDRef},
};
use image::RgbaImage;
use std::ptr::null;

#[link(name = "ColorSync", kind = "framework")]
unsafe extern "C" {
    fn CGDisplayCreateUUIDFromDisplayID(display: u32) -> CFUUIDRef;
}
pub(super) fn display_identities(displays: &[DisplayInfo]) -> Result<Vec<CaptureMonitorIdentity>> {
    displays
        .iter()
        .map(|display| {
            let id = display
                .id
                .parse::<u32>()
                .context("Invalid CoreGraphics display ID")?;
            let raw = unsafe { CGDisplayCreateUUIDFromDisplayID(id) };
            ensure!(
                !raw.is_null(),
                "Persistent CoreGraphics display UUID is unavailable"
            );
            let uuid = unsafe { CFUUID::wrap_under_create_rule(raw) };
            let raw = unsafe { CFUUIDCreateString(null(), uuid.as_concrete_TypeRef()) };
            ensure!(
                !raw.is_null(),
                "Cannot represent the CoreGraphics display UUID"
            );
            let name = unsafe { CFString::wrap_under_create_rule(raw) }.to_string();
            ensure!(name.len() == 36, "Invalid CoreGraphics display UUID");
            Ok(CaptureMonitorIdentity {
                display_id: display.id.clone(),
                binding: CaptureMonitorBinding::Physical {
                    key: format!("macos:{}", name.to_lowercase()),
                },
            })
        })
        .collect()
}
pub(super) fn validate_options(options: &CaptureOptions, window: bool) -> Result<()> {
    if window {
        super::validate_native_window_policy(options, false, true, true, true)?;
    }
    ensure!(
        window || options.cursor != CaptureCursor::Include,
        "Native macOS display cursor source is not yet wired"
    );
    Ok(())
}
pub(super) fn validate_template_session() -> Result<()> {
    Ok(())
}
pub(super) fn apply_cursor(_: &mut RgbaImage, _: i32, _: i32) -> Result<()> {
    bail!("Native macOS still cursor composition is not implemented")
}
pub(super) fn capture_window(window: &xcap::Window, options: &CaptureOptions) -> Result<RgbaImage> {
    validate_options(options, true)?;
    let id = window.id()?;
    let pid = window.pid()?;
    let proof = crate::capture_macos_ax::window(id)?;
    ensure!(
        proof.pid == pid,
        "Native macOS window process changed before capture"
    );
    let pixels = crate::capture_macos_image::window(proof, options)?;
    ensure!(
        window.id()? == id && window.pid()? == pid,
        "Native macOS window process changed after capture"
    );
    Ok(pixels)
}
