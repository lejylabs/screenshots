use image::RgbaImage;
use xcb::{
    Connection,
    x::{
        Drawable, GetGeometry, GetImage, GetWindowAttributes, ImageFormat, ImageOrder, Setup,
        VisualClass, Visualid, Window,
    },
};

use crate::error::{XCapError, XCapResult};

#[path = "zpixmap_decode.rs"]
mod zpixmap_decode;
use zpixmap_decode::{Format, Layout, request_extent};

fn native_format(setup: &Setup, root: Window, depth: u8, visual: Visualid) -> XCapResult<Format> {
    let screen = setup
        .roots()
        .find(|screen| screen.root() == root)
        .ok_or_else(|| XCapError::new("X11 drawable root is absent from setup"))?;
    let visual = screen
        .allowed_depths()
        .filter(|item| item.depth() == depth)
        .flat_map(|item| item.visuals().iter())
        .find(|item| item.visual_id() == visual)
        .ok_or_else(|| XCapError::new("X11 reply visual does not match its setup depth"))?;
    if visual.class() != VisualClass::TrueColor {
        return Err(XCapError::new(
            "X11 capture requires a verified TrueColor visual",
        ));
    }
    let packing = setup
        .pixmap_formats()
        .iter()
        .find(|item| item.depth() == depth)
        .ok_or_else(|| XCapError::new("X11 depth has no native pixmap packing"))?;
    let little_endian = match setup.image_byte_order() {
        ImageOrder::LsbFirst => true,
        ImageOrder::MsbFirst => false,
    };
    Ok(Format {
        depth,
        bits_per_pixel: packing.bits_per_pixel(),
        scanline_pad: packing.scanline_pad(),
        little_endian,
        masks: [visual.red_mask(), visual.green_mask(), visual.blue_mask()],
    })
}

pub fn xorg_capture(
    window: Window,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> XCapResult<RgbaImage> {
    let (x, y, native_width, native_height) =
        request_extent(x, y, width, height).map_err(XCapError::new)?;
    let (conn, _) = Connection::connect(None)?;
    let setup = conn.get_setup();

    // Admit actual native packing before requesting pixels, without a second image copy.
    let geometry_cookie = conn.send_request(&GetGeometry {
        drawable: Drawable::Window(window),
    });
    let attributes_cookie = conn.send_request(&GetWindowAttributes { window });
    let geometry = conn.wait_for_reply(geometry_cookie)?;
    let attributes = conn.wait_for_reply(attributes_cookie)?;
    let format = native_format(
        setup,
        geometry.root(),
        geometry.depth(),
        attributes.visual(),
    )?;
    let layout = Layout::new(width, height, format).map_err(XCapError::new)?;

    let cookie = conn.send_request(&GetImage {
        format: ImageFormat::ZPixmap,
        drawable: Drawable::Window(window),
        x,
        y,
        width: native_width,
        height: native_height,
        plane_mask: u32::MAX,
    });
    let reply = conn.wait_for_reply(cookie)?;
    if reply.depth() != geometry.depth() || reply.visual() != attributes.visual() {
        return Err(XCapError::new(
            "X11 drawable depth or visual changed during capture",
        ));
    }
    // Decode only using the actual reply's proven visual, never depth-based RGB guesses.
    let returned_format = native_format(setup, geometry.root(), reply.depth(), reply.visual())?;
    if returned_format != format {
        return Err(XCapError::new(
            "X11 native pixel format changed during capture",
        ));
    }
    let rgba = layout.decode(reply.data()).map_err(XCapError::new)?;
    RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| XCapError::new("RgbaImage::from_raw failed"))
}
