use crate::{XCapError, error::XCapResult};
use image::RgbaImage;

fn wlroots_screenshot(
    x_coordinate: i32,
    y_coordinate: i32,
    width: i32,
    height: i32,
) -> XCapResult<RgbaImage> {
    let wayshot_connection = libwayshot_xcap::WayshotConnection::new()?;
    let capture_region = libwayshot_xcap::region::LogicalRegion {
        inner: libwayshot_xcap::region::Region {
            position: libwayshot_xcap::region::Position {
                x: x_coordinate,
                y: y_coordinate,
            },
            size: libwayshot_xcap::region::Size {
                width: width as u32,
                height: height as u32,
            },
        },
    };
    let rgba_image = wayshot_connection.screenshot(capture_region, false)?;

    // libwayshot returns image 0.24 RgbaImage
    // we need image 0.25 RgbaImage
    let image = image::RgbaImage::from_raw(
        rgba_image.width(),
        rgba_image.height(),
        rgba_image.to_rgba8().into_vec(),
    )
    .ok_or_else(|| XCapError::new("Invalid in-memory Wayland frame extent"))?;

    Ok(image)
}

pub fn wayland_capture(x: i32, y: i32, width: i32, height: i32) -> XCapResult<RgbaImage> {
    if width <= 0 || height <= 0 || (width as u64) * (height as u64) * 4 > 256 * 1024 * 1024 {
        return Err(XCapError::new("Invalid bounded Wayland region"));
    }
    // Only the native in-memory wlroots path remains. A caller must admit logical
    // coordinates against its real Wayland outputs; XWayland monitor IDs do not do so.
    wlroots_screenshot(x, y, width, height)
}
