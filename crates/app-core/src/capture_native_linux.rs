use crate::{
    CaptureCursor, CaptureMonitorBinding, CaptureMonitorIdentity, CaptureOptions, DisplayInfo,
    capture_options::CaptureCursorImage,
};
use anyhow::{Context, Result, ensure};
use image::RgbaImage;
use sha2_10::{Digest, Sha256};
use x11rb::{
    connection::Connection,
    protocol::{
        randr::ConnectionExt as _,
        xfixes::ConnectionExt as _,
        xproto::{AtomEnum, ConnectionExt as _},
    },
};

fn session_display() -> Result<String> {
    ensure!(
        !crate::linux_wayland_session(),
        "Native monitor binding and still cursor composition require an X11 session"
    );
    let display = std::env::var("DISPLAY").context("X11 DISPLAY is unavailable")?;
    ensure!(
        !display.trim().is_empty() && display.len() <= 256,
        "Invalid X11 DISPLAY"
    );
    Ok(display)
}
fn connection() -> Result<(x11rb::rust_connection::RustConnection, usize)> {
    let (connection, screen) = x11rb::connect(Some(&session_display()?))?;
    ensure!(
        connection.setup().roots.len() == 1,
        "Native cursor/monitor binding currently requires one X11 screen (which may contain multiple RandR outputs)"
    );
    Ok((connection, screen))
}
pub(super) fn validate_template_session() -> Result<()> {
    session_display().map(|_| ())
}

pub(super) fn display_identities(displays: &[DisplayInfo]) -> Result<Vec<CaptureMonitorIdentity>> {
    let (connection, _) = connection()?;
    let edid = connection.intern_atom(true, b"EDID")?.reply()?.atom;
    let mut identities = Vec::with_capacity(displays.len() * 2);
    for display in displays {
        let output = display
            .id
            .parse::<u32>()
            .context("Invalid X11 RandR output identity")?;
        let info = connection
            .randr_get_output_info(output, x11rb::CURRENT_TIME)?
            .reply()?;
        let connector =
            std::str::from_utf8(&info.name).context("RandR connector name is not UTF-8")?;
        ensure!(
            !connector.is_empty()
                && connector.len() <= 128
                && !connector.chars().any(char::is_control),
            "Invalid RandR connector name"
        );
        identities.push(CaptureMonitorIdentity {
            display_id: display.id.clone(),
            binding: CaptureMonitorBinding::Connector {
                key: format!("x11:{connector}"),
            },
        });
        if edid != x11rb::NONE {
            let value = connection
                .randr_get_output_property(output, edid, AtomEnum::ANY, 0, 1024, false, false)?
                .reply()?;
            ensure!(
                value.bytes_after == 0 && value.data.len() <= 4096,
                "RandR EDID exceeds 4096 bytes"
            );
            if !value.data.is_empty() {
                ensure!(
                    value.format == 8 && valid_edid(&value.data),
                    "Invalid RandR EDID; use an explicit connector binding if intended"
                );
                let hash = Sha256::digest(&value.data);
                identities.push(CaptureMonitorIdentity {
                    display_id: display.id.clone(),
                    binding: CaptureMonitorBinding::Physical {
                        key: format!("x11:{connector}:{hash:x}"),
                    },
                });
            }
        }
    }
    Ok(identities)
}
fn valid_edid(bytes: &[u8]) -> bool {
    (128..=4096).contains(&bytes.len())
        && bytes.len().is_multiple_of(128)
        && bytes[..8] == [0, 255, 255, 255, 255, 255, 255, 0]
        && (usize::from(bytes[126]) + 1) * 128 == bytes.len()
        && bytes
            .chunks_exact(128)
            .all(|block| block.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)) == 0)
}
pub(super) fn validate_options(options: &CaptureOptions, window: bool) -> Result<()> {
    if window {
        // XCap's X11 drawable is the client's own window, without compositor shadows.
        super::validate_native_window_policy(options, true, false, false, false)?;
    }
    if options.cursor == CaptureCursor::Include {
        ensure!(
            !crate::linux_wayland_session(),
            "Still cursor composition requires an X11 session"
        );
    }
    Ok(())
}
pub(super) fn apply_cursor(image: &mut RgbaImage, x: i32, y: i32) -> Result<()> {
    let (connection, _) = connection()?;
    connection.xfixes_query_version(2, 0)?.reply()?;
    let cursor = connection.xfixes_get_cursor_image()?.reply()?;
    ensure!(
        (1..=1024).contains(&cursor.width) && (1..=1024).contains(&cursor.height),
        "Native cursor exceeds 1024×1024"
    );
    ensure!(
        cursor.cursor_image.len() == usize::from(cursor.width) * usize::from(cursor.height),
        "Invalid XFixes cursor backing"
    );
    let mut bytes = Vec::with_capacity(cursor.cursor_image.len() * 4);
    for argb in cursor.cursor_image {
        bytes.extend_from_slice(&[
            (argb >> 16) as u8,
            (argb >> 8) as u8,
            argb as u8,
            (argb >> 24) as u8,
        ]);
    }
    CaptureCursorImage {
        pixels: RgbaImage::from_raw(u32::from(cursor.width), u32::from(cursor.height), bytes)
            .context("Invalid XFixes cursor dimensions")?,
        screen_x: i32::from(cursor.x),
        screen_y: i32::from(cursor.y),
        hotspot_x: u32::from(cursor.xhot),
        hotspot_y: u32::from(cursor.yhot),
    }
    .compose(image, x, y)
}
pub(super) fn capture_window(window: &xcap::Window, options: &CaptureOptions) -> Result<RgbaImage> {
    let (connection, screen) = connection()?;
    let id = window.id()?;
    let before = connection.get_geometry(id)?.reply()?;
    let origin = connection
        .translate_coordinates(id, connection.setup().roots[screen].root, 0, 0)?
        .reply()?;
    let pid = window.pid()?;
    let mut image = window.capture_image()?;
    ensure!(
        image.dimensions() == (u32::from(before.width), u32::from(before.height)),
        "Native window geometry changed during capture"
    );
    options.flatten_window(&mut image);
    if options.cursor == CaptureCursor::Include {
        apply_cursor(&mut image, i32::from(origin.dst_x), i32::from(origin.dst_y))?;
    }
    let after = connection.get_geometry(id)?.reply()?;
    let current = connection
        .translate_coordinates(id, connection.setup().roots[screen].root, 0, 0)?
        .reply()?;
    ensure!(
        before.width == after.width
            && before.height == after.height
            && origin.dst_x == current.dst_x
            && origin.dst_y == current.dst_y
            && window.pid()? == pid,
        "Native window moved, resized or changed process during capture"
    );
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn monitor_edid_requires_header_extent_and_every_block_checksum() {
        let mut bytes = vec![0u8; 128];
        bytes[..8].copy_from_slice(&[0, 255, 255, 255, 255, 255, 255, 0]);
        bytes[127] = 0u8.wrapping_sub(
            bytes[..127]
                .iter()
                .fold(0u8, |sum, byte| sum.wrapping_add(*byte)),
        );
        assert!(valid_edid(&bytes));
        bytes[20] = 1;
        assert!(!valid_edid(&bytes));
        assert!(!valid_edid(&[]));
    }
}
