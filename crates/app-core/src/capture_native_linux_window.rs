//! Explicit X11 window policies read existing off-screen storage, never the root.
use super::*;
use crate::{CaptureWindowFrame, CaptureWindowTransparency};
use x11rb::{
    protocol::{
        composite::ConnectionExt as _,
        render::{ConnectionExt as _, PictType},
        shape::ConnectionExt as _,
        xproto::{ImageFormat, ImageOrder, MapState, WindowClass},
    },
    rust_connection::RustConnection,
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Geometry {
    id: u32,
    parent: u32,
    x: i32,
    y: i32,
    width: u16,
    height: u16,
    depth: u8,
    visual: u32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Snapshot {
    client: Geometry,
    drawable: Geometry,
    pid: u32,
    extents: Option<[u32; 4]>,
}
#[derive(Clone)]
pub(crate) struct Admission {
    bytes: u64,
    snapshot: Option<Snapshot>,
}
impl Admission {
    pub(crate) fn bytes(&self) -> u64 {
        self.bytes
    }
}
fn explicit(options: &CaptureOptions) -> bool {
    options.window_frame == CaptureWindowFrame::Include
        || options.window_transparency == CaptureWindowTransparency::Preserve
}
fn geometry(conn: &RustConnection, root: u32, id: u32) -> Result<Geometry> {
    let geo = conn.get_geometry(id)?.reply()?;
    let attr = conn.get_window_attributes(id)?.reply()?;
    let tree = conn.query_tree(id)?.reply()?;
    ensure!(
        id != root
            && geo.root == root
            && tree.root == root
            && geo.border_width == 0
            && attr.class == WindowClass::INPUT_OUTPUT
            && attr.map_state == MapState::VIEWABLE,
        "Explicit X11 window policies require a viewable zero-border InputOutput window"
    );
    let shape = conn.shape_query_extents(id)?.reply()?;
    ensure!(
        !shape.bounding_shaped && !shape.clip_shaped,
        "Explicit X11 window policies do not support shaped windows"
    );
    let origin = conn.translate_coordinates(id, root, 0, 0)?.reply()?;
    ensure!(origin.same_screen, "X11 window changed screen");
    crate::validate_image_size(u32::from(geo.width), u32::from(geo.height))?;
    Ok(Geometry {
        id,
        parent: tree.parent,
        x: i32::from(origin.dst_x),
        y: i32::from(origin.dst_y),
        width: geo.width,
        height: geo.height,
        depth: geo.depth,
        visual: attr.visual,
    })
}
fn frame_matches(client: &Geometry, frame: &Geometry, extents: [u32; 4]) -> bool {
    let [left, right, top, bottom] = extents.map(i64::from);
    i64::from(client.x) - left == i64::from(frame.x)
        && i64::from(client.y) - top == i64::from(frame.y)
        && i64::from(client.width) + left + right == i64::from(frame.width)
        && i64::from(client.height) + top + bottom == i64::from(frame.height)
}
fn snapshot(
    conn: &RustConnection,
    screen: usize,
    window: &xcap::Window,
    options: &CaptureOptions,
) -> Result<Snapshot> {
    let root = conn.setup().roots[screen].root;
    let id = window.id()?;
    ensure!(
        !window.is_minimized()?,
        "Explicit X11 capture refuses a minimized window"
    );
    let client = geometry(conn, root, id)?;
    ensure!(
        conn.query_tree(id)?.reply()?.children.is_empty(),
        "Explicit X11 window policies require a client without child windows"
    );
    let (drawable, extents) = if options.window_frame == CaptureWindowFrame::Include {
        let atom = conn.intern_atom(true, b"_NET_FRAME_EXTENTS")?.reply()?.atom;
        ensure!(
            atom != x11rb::NONE,
            "X11 frame inclusion requires native _NET_FRAME_EXTENTS"
        );
        let prop = conn
            .get_property(false, id, atom, AtomEnum::CARDINAL, 0, 4)?
            .reply()?;
        ensure!(
            prop.type_ == u32::from(AtomEnum::CARDINAL)
                && prop.format == 32
                && prop.bytes_after == 0
                && prop.value.len() == 16,
            "X11 frame inclusion requires exactly four native CARDINAL frame extents"
        );
        let values: Vec<u32> = prop
            .value32()
            .context("Invalid X11 frame extents")?
            .collect();
        let extents: [u32; 4] = values
            .try_into()
            .map_err(|_| anyhow::anyhow!("Invalid X11 frame extent count"))?;
        let frame = if client.parent == root {
            client.clone()
        } else {
            geometry(conn, root, client.parent)?
        };
        ensure!(
            frame.parent == root && frame_matches(&client, &frame, extents),
            "Native X11 parent does not exactly match the declared decorated frame"
        );
        if frame.id != client.id {
            ensure!(
                conn.query_tree(frame.id)?.reply()?.children == [id],
                "X11 frame inclusion requires one direct client and no sibling child windows"
            );
            // An independently redirected child is absent from its parent's backing pixmap.
            let probe = conn.generate_id()?;
            match conn.composite_name_window_pixmap(id, probe)?.check() {
                Ok(()) => {
                    conn.free_pixmap(probe)?.check()?;
                    anyhow::bail!(
                        "X11 frame inclusion cannot use an independently redirected client"
                    );
                }
                Err(x11rb::errors::ReplyError::X11Error(error))
                    if error.error_kind == x11rb::protocol::ErrorKind::Match => {}
                Err(error) => return Err(error.into()),
            }
        }
        (frame, Some(extents))
    } else {
        (client.clone(), None)
    };
    let pid = window.pid()?;
    ensure!(
        pid != 0 && pid != std::process::id(),
        "Invalid or own-process X11 window identity"
    );
    Ok(Snapshot {
        client,
        drawable,
        pid,
        extents,
    })
}
fn prepare_connection() -> Result<(RustConnection, usize)> {
    let (conn, screen) = connection()?;
    let version = conn.composite_query_version(0, 4)?.reply()?;
    ensure!(
        version.major_version > 0 || version.minor_version >= 2,
        "Explicit X11 frame/transparency policies require Composite 0.2 or later"
    );
    conn.render_query_version(0, 11)?.reply()?;
    conn.shape_query_version()?.reply()?;
    Ok((conn, screen))
}
pub(crate) fn admit(window: &xcap::Window, options: &CaptureOptions) -> Result<Admission> {
    if !explicit(options) {
        let (w, h) = (window.width()?, window.height()?);
        crate::validate_image_size(w, h)?;
        return Ok(Admission {
            bytes: u64::from(w) * u64::from(h) * 4,
            snapshot: None,
        });
    }
    let (conn, screen) = prepare_connection()?;
    let before = snapshot(&conn, screen, window, options)?;
    let bytes = u64::from(before.drawable.width) * u64::from(before.drawable.height) * 4;
    // Negotiate/validate format before native pixel allocation.
    format(&conn, screen, &before.drawable)?;
    Ok(Admission {
        bytes,
        snapshot: Some(before),
    })
}
pub(crate) fn verify(
    window: &xcap::Window,
    options: &CaptureOptions,
    admitted: &Admission,
) -> Result<()> {
    if let Some(expected) = &admitted.snapshot {
        let (conn, screen) = prepare_connection()?;
        ensure!(
            &snapshot(&conn, screen, window, options)? == expected,
            "X11 window identity, frame or geometry changed after admission"
        );
    } else {
        ensure!(
            admit(window, options)?.bytes == admitted.bytes,
            "X11 window size changed after admission"
        );
    }
    Ok(())
}
#[derive(Clone, Copy)]
struct Format {
    shifts: [u16; 4],
    alpha: bool,
    little: bool,
}
fn format(conn: &RustConnection, screen: usize, geo: &Geometry) -> Result<Format> {
    let formats = conn.render_query_pict_formats()?.reply()?;
    let mapping = formats
        .screens
        .get(screen)
        .context("XRender screen is missing")?
        .depths
        .iter()
        .filter(|depth| depth.depth == geo.depth)
        .flat_map(|depth| depth.visuals.iter())
        .find(|visual| visual.visual == geo.visual)
        .context("XRender does not describe the native window visual")?;
    let info = formats
        .formats
        .iter()
        .find(|format| format.id == mapping.format)
        .context("XRender window format is missing")?;
    let d = info.direct;
    let alpha = d.alpha_mask != 0;
    let shifts = [d.red_shift, d.green_shift, d.blue_shift, d.alpha_shift];
    ensure!(
        info.type_ == PictType::DIRECT
            && info.depth == geo.depth
            && d.red_mask == 255
            && d.green_mask == 255
            && d.blue_mask == 255
            && ((!alpha && geo.depth == 24) || (alpha && geo.depth == 32 && d.alpha_mask == 255))
            && shifts[..3]
                .iter()
                .all(|shift| [0, 8, 16, 24].contains(shift))
            && (!alpha || [0, 8, 16, 24].contains(&shifts[3]))
            && shifts[0] != shifts[1]
            && shifts[0] != shifts[2]
            && shifts[1] != shifts[2]
            && (!alpha || shifts[..3].iter().all(|shift| *shift != shifts[3])),
        "Explicit X11 policies support only verified direct RGB8 or ARGB8 visuals"
    );
    let packing = conn
        .setup()
        .pixmap_formats
        .iter()
        .find(|format| format.depth == geo.depth)
        .context("Native X11 pixmap packing is missing")?;
    ensure!(
        packing.bits_per_pixel == 32 && packing.scanline_pad == 32,
        "Explicit X11 policies require packed 32-bit pixels and 32-bit scanlines"
    );
    let order = conn.setup().image_byte_order;
    ensure!(
        order == ImageOrder::LSB_FIRST || order == ImageOrder::MSB_FIRST,
        "Unknown X11 image byte order"
    );
    Ok(Format {
        shifts,
        alpha,
        little: order == ImageOrder::LSB_FIRST,
    })
}
fn decode(
    bytes: &[u8],
    width: u16,
    height: u16,
    format: Format,
    check: &crate::NativeCaptureCheck<'_>,
) -> Result<RgbaImage> {
    crate::validate_image_size(u32::from(width), u32::from(height))?;
    let row = usize::from(width) * 4;
    ensure!(
        bytes.len() == row * usize::from(height),
        "Invalid native X11 pixmap extent"
    );
    check()?;
    let mut pixels = Vec::with_capacity(bytes.len());
    for source in bytes.chunks_exact(row) {
        check()?;
        for packed in source.chunks_exact(4) {
            let packed: [u8; 4] = packed.try_into().unwrap();
            let value = if format.little {
                u32::from_le_bytes(packed)
            } else {
                u32::from_be_bytes(packed)
            };
            let mut rgba = format.shifts.map(|shift| ((value >> shift) & 255) as u8);
            if !format.alpha {
                rgba[3] = 255;
            }
            let alpha = u32::from(rgba[3]);
            for channel in &mut rgba[..3] {
                ensure!(
                    u32::from(*channel) <= alpha,
                    "X11 ARGB pixels violate native premultiplied-alpha semantics"
                );
                *channel = if alpha == 0 {
                    0
                } else {
                    ((u32::from(*channel) * 255 + alpha / 2) / alpha) as u8
                };
            }
            pixels.extend_from_slice(&rgba);
        }
    }
    check()?;
    RgbaImage::from_raw(u32::from(width), u32::from(height), pixels)
        .context("Invalid native X11 image dimensions")
}
pub(crate) fn capture(
    window: &xcap::Window,
    options: &CaptureOptions,
    admitted: &Admission,
    check: &crate::NativeCaptureCheck<'_>,
) -> Result<RgbaImage> {
    check()?;
    let Some(expected) = &admitted.snapshot else {
        let image = super::capture_window_legacy(window, options)?;
        check()?;
        verify(window, options, admitted)?;
        return Ok(image);
    };
    let (conn, screen) = prepare_connection()?;
    ensure!(
        &snapshot(&conn, screen, window, options)? == expected,
        "X11 window identity, frame or geometry changed before capture"
    );
    let geo = &expected.drawable;
    let format = format(&conn, screen, geo)?;
    check()?;
    let pixmap = conn.generate_id()?;
    conn.composite_name_window_pixmap(geo.id, pixmap)?.check()
        .context("Explicit X11 policies require an already redirected visible drawable; no compositor storage is available")?;
    // Free this reference on every return; closing the operation connection also releases it.
    let result = (|| {
        let backing = conn.get_geometry(pixmap)?.reply()?;
        ensure!(
            backing.width == geo.width
                && backing.height == geo.height
                && backing.depth == geo.depth
                && backing.border_width == 0,
            "X11 backing pixmap does not match admitted physical geometry"
        );
        check()?;
        let reply = conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                pixmap,
                0,
                0,
                geo.width,
                geo.height,
                u32::MAX,
            )?
            .reply()?;
        ensure!(reply.depth == geo.depth, "X11 backing depth changed");
        check()?;
        ensure!(
            &snapshot(&conn, screen, window, options)? == expected,
            "X11 window identity, frame or geometry changed during readback"
        );
        let mut image = decode(&reply.data, geo.width, geo.height, format, check)?;
        drop(reply);
        // Flatten with cancellation at row granularity, avoiding a second image allocation.
        if let Some(background) = options.background {
            for row in image.rows_mut() {
                check()?;
                for pixel in row {
                    let alpha = u32::from(pixel[3]);
                    for (channel, bg) in pixel.0[..3].iter_mut().zip(background) {
                        *channel =
                            ((u32::from(*channel) * alpha + u32::from(bg) * (255 - alpha) + 127)
                                / 255) as u8;
                    }
                    pixel[3] = 255;
                }
            }
        }
        if options.cursor == CaptureCursor::Include {
            check()?;
            apply_cursor(&mut image, geo.x, geo.y)?;
        }
        ensure!(
            &snapshot(&conn, screen, window, options)? == expected,
            "X11 window identity, frame or geometry changed before publication"
        );
        check()?;
        Ok(image)
    })();
    let released = (|| -> Result<()> {
        conn.free_pixmap(pixmap)?.check()?;
        Ok(())
    })();
    let image = result?;
    released?;
    check()?;
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_x11_argb_decodes_straight_alpha_endian_and_rejects_invalid_pixels() {
        let little = Format {
            shifts: [16, 8, 0, 24],
            alpha: true,
            little: true,
        };
        let big = Format {
            little: false,
            ..little
        };
        let values: [u32; 4] = [0, 0x80402010, 0xff123456, 0x01010000];
        let le: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        let be: Vec<u8> = values.iter().flat_map(|v| v.to_be_bytes()).collect();
        let expected = vec![0, 0, 0, 0, 128, 64, 32, 128, 18, 52, 86, 255, 255, 0, 0, 1];
        assert_eq!(
            decode(&le, 2, 2, little, &|| Ok(())).unwrap().as_raw(),
            &expected
        );
        assert_eq!(
            decode(&be, 2, 2, big, &|| Ok(())).unwrap().as_raw(),
            &expected
        );
        assert!(decode(&[0, 0, 129, 128], 1, 1, little, &|| Ok(())).is_err());
        assert!(decode(&le[..15], 2, 2, little, &|| Ok(())).is_err());
        assert!(
            decode(&le, 2, 2, little, &|| Err(
                crate::CaptureError::NativeCanceled
            ))
            .is_err()
        );
    }
    #[test]
    fn x11_frame_extents_require_exact_native_geometry_without_padding_guesses() {
        let client = Geometry {
            id: 4,
            parent: 3,
            x: -40,
            y: 22,
            width: 96,
            height: 64,
            depth: 24,
            visual: 10,
        };
        let mut frame = Geometry {
            id: 3,
            parent: 1,
            x: -44,
            y: 2,
            width: 104,
            height: 88,
            ..client.clone()
        };
        assert!(frame_matches(&client, &frame, [4, 4, 20, 4]));
        frame.width += 1;
        assert!(!frame_matches(&client, &frame, [4, 4, 20, 4]));
        assert!(!frame_matches(&client, &frame, [u32::MAX; 4]));
    }
    #[test]
    #[ignore = "requires scripts/test-native-window-policy.py authenticated private owned-window fixture"]
    fn private_x11_policy_rejects_changes_after_readback_and_cancels_decoding() -> Result<()> {
        use std::{cell::Cell, os::unix::fs::MetadataExt};
        use x11rb::{
            protocol::xproto::{ConfigureWindowAux, PropMode},
            wrapper::ConnectionExt as _,
        };
        ensure!(
            std::env::var("SCREENSHOTS_PRIVATE_WINDOW_POLICY").as_deref()
                == Ok("owned-composite-frame-argb"),
            "Missing private policy fixture"
        );
        let display = std::env::var("DISPLAY")?;
        let number: u16 = display
            .strip_prefix(':')
            .context("Invalid private display")?
            .parse()?;
        ensure!(
            (420..480).contains(&number) && !crate::linux_wayland_session(),
            "Not a private X11 fixture"
        );
        let auth = std::path::PathBuf::from(
            std::env::var_os("XAUTHORITY").context("Missing private authority")?,
        );
        let parent = auth.parent().context("Missing authority directory")?;
        ensure!(
            parent
                .file_name()
                .context("Missing fixture directory")?
                .to_string_lossy()
                .starts_with("screenshots-window-policy-private-"),
            "Wrong fixture directory"
        );
        let metadata = std::fs::symlink_metadata(parent)?;
        ensure!(
            metadata.is_dir()
                && metadata.mode() & 0o077 == 0
                && metadata.uid() == unsafe { libc::getuid() },
            "Fixture directory is not privately owned"
        );
        let metadata = std::fs::symlink_metadata(&auth)?;
        ensure!(
            metadata.is_file()
                && metadata.mode() & 0o077 == 0
                && metadata.uid() == unsafe { libc::getuid() },
            "Authority is not a private regular file"
        );
        let id: u32 = std::env::var("SCREENSHOTS_PRIVATE_WINDOW_POLICY_CLIENT")?.parse()?;
        let pid: u32 = std::env::var("SCREENSHOTS_PRIVATE_WINDOW_POLICY_PID")?.parse()?;
        let window = xcap::Window::all()?
            .into_iter()
            .find(|window| window.id().ok() == Some(id))
            .context("Owned fixture client not listed")?;
        ensure!(
            window.pid()? == pid && pid != std::process::id(),
            "Wrong fixture client owner"
        );
        let options = CaptureOptions {
            window_frame: CaptureWindowFrame::Include,
            window_transparency: CaptureWindowTransparency::Preserve,
            ..Default::default()
        };
        let admitted = admit(&window, &options)?;
        let snap = admitted
            .snapshot
            .as_ref()
            .context("Missing explicit admission")?;
        let (conn, _) = prepare_connection()?;
        let pid_atom = conn.intern_atom(true, b"_NET_WM_PID")?.reply()?.atom;
        struct Restore<'a> {
            conn: &'a RustConnection,
            snapshot: &'a Snapshot,
            pid_atom: u32,
        }
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                if let Ok(cookie) = self.conn.configure_window(
                    self.snapshot.drawable.id,
                    &ConfigureWindowAux::new()
                        .x(self.snapshot.drawable.x)
                        .y(self.snapshot.drawable.y),
                ) {
                    let _ = cookie.check();
                }
                if let Ok(cookie) = self.conn.change_property32(
                    PropMode::REPLACE,
                    self.snapshot.client.id,
                    self.pid_atom,
                    AtomEnum::CARDINAL,
                    &[self.snapshot.pid],
                ) {
                    let _ = cookie.check();
                }
            }
        }
        // Call four occurs after native readback, before the post-readback identity check.
        for change_pid in [false, true] {
            let restore = Restore {
                conn: &conn,
                snapshot: snap,
                pid_atom,
            };
            let calls = Cell::new(0);
            let outcome = capture(&window, &options, &admitted, &|| {
                let call = calls.get() + 1;
                calls.set(call);
                if call == 4 {
                    let changed: Result<()> = (|| {
                        if change_pid {
                            conn.change_property32(
                                PropMode::REPLACE,
                                id,
                                pid_atom,
                                AtomEnum::CARDINAL,
                                &[pid.checked_add(1).context("Invalid fixture PID")?],
                            )?
                            .check()?;
                        } else {
                            conn.configure_window(
                                snap.drawable.id,
                                &ConfigureWindowAux::new().x(snap.drawable.x + 1),
                            )?
                            .check()?;
                        }
                        Ok(())
                    })();
                    changed.map_err(|error| crate::CaptureError::Backend(error.to_string()))?;
                }
                Ok(())
            });
            assert!(outcome.is_err(), "Changed source must not publish pixels");
            assert_eq!(
                calls.get(),
                4,
                "Mutation must follow actual native readback"
            );
            drop(restore);
            verify(&window, &options, &admitted)?;
        }
        let calls = Cell::new(0);
        let outcome = capture(&window, &options, &admitted, &|| {
            calls.set(calls.get() + 1);
            if calls.get() == 7 {
                Err(crate::CaptureError::NativeCanceled)
            } else {
                Ok(())
            }
        });
        let error = outcome.unwrap_err();
        assert!(matches!(
            error.downcast_ref::<crate::CaptureError>(),
            Some(crate::CaptureError::NativeCanceled)
        ));
        assert_eq!(
            calls.get(),
            7,
            "Cancellation must interrupt decoded row work"
        );
        verify(&window, &options, &admitted)?;
        Ok(())
    }
}
