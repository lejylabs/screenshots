//! Explicit still-capture policies; omitted fields preserve the existing backend.
use anyhow::{Result, ensure};
use image::RgbaImage;
use serde::{Deserialize, Serialize};

// Exercise the exact production vendor helper on hosts without Windows/DWM.
#[cfg(test)]
#[path = "../../../vendor/xcap/src/windows/window_crop.rs"]
mod xcap_window_crop;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureCursor {
    #[default]
    Exclude,
    Include,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureWindowFrame {
    #[default]
    Native,
    Client,
    Include,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureWindowShadow {
    #[default]
    Native,
    Exclude,
    Include,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureWindowTransparency {
    #[default]
    Native,
    Preserve,
    Opaque,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CaptureOptions {
    pub cursor: CaptureCursor,
    pub window_frame: CaptureWindowFrame,
    pub window_shadow: CaptureWindowShadow,
    pub window_transparency: CaptureWindowTransparency,
    pub background: Option<[u8; 3]>,
    pub delay_ms: u64,
}
impl CaptureOptions {
    pub fn validate_for_capture(&self, window: bool) -> Result<()> {
        crate::capture_native::validate_options(self, window)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.delay_ms <= 3_600_000,
            "Capture delay must be 0–3600000 milliseconds"
        );
        ensure!(
            (self.window_transparency == CaptureWindowTransparency::Opaque)
                == self.background.is_some(),
            "Opaque window transparency requires an explicit RGB background, used only with opaque mode"
        );
        ensure!(
            self.window_frame != CaptureWindowFrame::Client
                || self.window_shadow != CaptureWindowShadow::Include,
            "Client-only window capture cannot include a window shadow"
        );
        Ok(())
    }

    pub(crate) fn flatten_window(&self, pixels: &mut RgbaImage) {
        let Some(background) = self.background else {
            return;
        };
        for pixel in pixels.pixels_mut() {
            let alpha = u32::from(pixel[3]);
            for (channel, background) in pixel.0[..3].iter_mut().zip(background) {
                *channel =
                    ((u32::from(*channel) * alpha + u32::from(background) * (255 - alpha) + 127)
                        / 255) as u8;
            }
            pixel[3] = 255;
        }
    }
}

/// An operation-owned cursor in premultiplied RGBA, matching native XFixes/ARGB.
#[cfg(any(target_os = "linux", target_os = "windows", test))]
pub(crate) struct CaptureCursorImage {
    pub pixels: RgbaImage,
    pub screen_x: i32,
    pub screen_y: i32,
    pub hotspot_x: u32,
    pub hotspot_y: u32,
}
#[cfg(any(target_os = "linux", target_os = "windows", test))]
impl CaptureCursorImage {
    pub fn validate(&self) -> Result<()> {
        let (width, height) = self.pixels.dimensions();
        ensure!(
            (1..=1024).contains(&width) && (1..=1024).contains(&height),
            "Native cursor exceeds 1024×1024"
        );
        ensure!(
            self.pixels.as_raw().len() == width as usize * height as usize * 4,
            "Invalid native cursor backing"
        );
        ensure!(
            self.hotspot_x < width && self.hotspot_y < height,
            "Invalid native cursor hotspot"
        );
        ensure!(
            self.pixels
                .pixels()
                .all(|p| p.0[..3].iter().all(|channel| *channel <= p[3])),
            "Invalid premultiplied native cursor"
        );
        Ok(())
    }

    pub fn compose(&self, pixels: &mut RgbaImage, origin_x: i32, origin_y: i32) -> Result<()> {
        self.validate()?;
        let left = i64::from(self.screen_x) - i64::from(self.hotspot_x) - i64::from(origin_x);
        let top = i64::from(self.screen_y) - i64::from(self.hotspot_y) - i64::from(origin_y);
        let start_x = (-left).max(0).min(i64::from(self.pixels.width())) as u32;
        let start_y = (-top).max(0).min(i64::from(self.pixels.height())) as u32;
        let end_x = (i64::from(pixels.width()) - left)
            .max(0)
            .min(i64::from(self.pixels.width())) as u32;
        let end_y = (i64::from(pixels.height()) - top)
            .max(0)
            .min(i64::from(self.pixels.height())) as u32;
        for y in start_y..end_y {
            for x in start_x..end_x {
                let source = self.pixels.get_pixel(x, y);
                let alpha = u32::from(source[3]);
                if alpha == 0 {
                    continue;
                }
                let destination =
                    pixels.get_pixel_mut((left + i64::from(x)) as u32, (top + i64::from(y)) as u32);
                let destination_alpha = u32::from(destination[3]);
                let output_alpha = alpha * 255 + destination_alpha * (255 - alpha);
                for (channel, source) in destination.0[..3].iter_mut().zip(source.0) {
                    *channel = ((u32::from(source) * 255 * 255
                        + u32::from(*channel) * destination_alpha * (255 - alpha)
                        + output_alpha / 2)
                        / output_alpha)
                        .min(255) as u8;
                }
                destination[3] = ((output_alpha + 127) / 255) as u8;
            }
        }
        Ok(())
    }
}

/// Windows legacy cursors combine an AND plane with either XOR bits or color.
#[cfg(any(target_os = "windows", test))]
pub(crate) fn compose_mask_cursor(
    image: &mut RgbaImage,
    offset: (i64, i64),
    width: u32,
    height: u32,
    mask: &[u8],
    stride: usize,
    colors: Option<&RgbaImage>,
) -> Result<()> {
    ensure!(
        (1..=1024).contains(&width)
            && (1..=1024).contains(&height)
            && stride == (width as usize).div_ceil(32) * 4
            && mask.len() == stride * height as usize * if colors.is_some() { 1 } else { 2 },
        "Invalid native cursor mask backing"
    );
    if let Some(colors) = colors {
        ensure!(
            colors.dimensions() == (width, height)
                && colors.as_raw().len() == width as usize * height as usize * 4
                && colors.pixels().all(|pixel| pixel[3] == 0),
            "Invalid legacy cursor color backing"
        );
    }
    for y in 0..height {
        for x in 0..width {
            let destination = (offset.0 + i64::from(x), offset.1 + i64::from(y));
            if destination.0 < 0
                || destination.1 < 0
                || destination.0 >= i64::from(image.width())
                || destination.1 >= i64::from(image.height())
            {
                continue;
            }
            let bit = |y: u32| mask[y as usize * stride + x as usize / 8] & (0x80 >> (x % 8)) != 0;
            let and = if bit(y) { 255 } else { 0 };
            let xor = colors.map_or_else(
                || if bit(y + height) { [255; 3] } else { [0; 3] },
                |pixels| {
                    let p = pixels.get_pixel(x, y);
                    [p[0], p[1], p[2]]
                },
            );
            if and == 255 && xor == [0; 3] {
                continue;
            }
            let target = image.get_pixel_mut(destination.0 as u32, destination.1 as u32);
            ensure!(
                and == 0 || target[3] == 255,
                "Inverting a native cursor over transparent pixels is unsupported"
            );
            for (channel, xor) in target.0[..3].iter_mut().zip(xor) {
                *channel = (*channel & and) ^ xor;
            }
            target[3] = 255;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;
    #[test]
    fn capture_options_keep_legacy_defaults_and_reject_inconsistent_policies() {
        let options: CaptureOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(options, CaptureOptions::default());
        options.validate().unwrap();
        for text in [
            r#"{"delay_ms":3600001}"#,
            r#"{"background":[1,2,3]}"#,
            r#"{"window_transparency":"opaque"}"#,
            r#"{"window_frame":"client","window_shadow":"include"}"#,
        ] {
            assert!(
                serde_json::from_str::<CaptureOptions>(text)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        assert!(
            serde_json::from_str::<CaptureOptions>(r#"{"cursor":"include","cursor":"exclude"}"#)
                .is_err()
        );
        assert!(serde_json::from_str::<CaptureOptions>(r#"{"unknown":true}"#).is_err());
    }
    #[test]
    fn cursor_uses_native_hotspot_clipping_and_premultiplied_alpha_without_touching_other_pixels() {
        let image = CaptureCursorImage {
            pixels: RgbaImage::from_raw(
                2,
                2,
                vec![
                    200, 40, 10, 255, 64, 32, 0, 128, 0, 0, 0, 0, 20, 30, 40, 255,
                ],
            )
            .unwrap(),
            screen_x: 10,
            screen_y: -2,
            hotspot_x: 1,
            hotspot_y: 1,
        };
        let mut result = RgbaImage::from_pixel(3, 3, Rgba([40, 80, 120, 255]));
        image.compose(&mut result, 9, -3).unwrap();
        assert_eq!(*result.get_pixel(0, 0), Rgba([200, 40, 10, 255]));
        assert_eq!(*result.get_pixel(1, 0), Rgba([84, 72, 60, 255]));
        assert_eq!(*result.get_pixel(0, 1), Rgba([40, 80, 120, 255]));
        assert_eq!(*result.get_pixel(1, 1), Rgba([20, 30, 40, 255]));
        assert_eq!(*result.get_pixel(2, 2), Rgba([40, 80, 120, 255]));
        let mut clipped = RgbaImage::from_pixel(1, 1, Rgba([1, 2, 3, 0]));
        image.compose(&mut clipped, 10, -3).unwrap();
        assert_eq!(*clipped.get_pixel(0, 0), Rgba([128, 64, 0, 128]));
        image.compose(&mut clipped, i32::MAX, i32::MIN).unwrap();
        assert_eq!(*clipped.get_pixel(0, 0), Rgba([128, 64, 0, 128]));
    }
    #[test]
    fn opaque_policy_requires_and_uses_the_selected_background() {
        let options = CaptureOptions {
            window_transparency: CaptureWindowTransparency::Opaque,
            background: Some([100, 110, 120]),
            ..Default::default()
        };
        options.validate().unwrap();
        let mut pixels = RgbaImage::from_raw(2, 1, vec![9, 19, 29, 0, 20, 40, 60, 128]).unwrap();
        options.flatten_window(&mut pixels);
        assert_eq!(pixels.as_raw(), &[100, 110, 120, 255, 60, 75, 90, 255]);
    }
    #[test]
    fn invalid_cursor_metadata_and_alpha_fail_before_changing_destination() {
        for (width, bytes, hotspot) in [
            (1, vec![0; 8], 0),
            (1, vec![2, 0, 0, 1], 0),
            (1, vec![0; 4], 1),
            (1025, vec![0; 4100], 0),
        ] {
            let cursor = CaptureCursorImage {
                pixels: RgbaImage::from_raw(width, 1, bytes).unwrap(),
                screen_x: 0,
                screen_y: 0,
                hotspot_x: hotspot,
                hotspot_y: 0,
            };
            let mut output = RgbaImage::from_pixel(1, 1, Rgba([7, 11, 13, 17]));
            assert!(cursor.compose(&mut output, 0, 0).is_err());
            assert_eq!(*output.get_pixel(0, 0), Rgba([7, 11, 13, 17]));
        }
    }
    #[test]
    fn monochrome_and_color_cursor_masks_keep_native_and_xor_semantics_and_clip() {
        let mask = [0x90, 0, 0, 0, 0x30, 0, 0, 0];
        let mut image = RgbaImage::from_pixel(4, 1, Rgba([20, 40, 60, 255]));
        compose_mask_cursor(&mut image, (0, 0), 4, 1, &mask, 4, None).unwrap();
        assert_eq!(
            image.as_raw(),
            &[
                20, 40, 60, 255, 0, 0, 0, 255, 255, 255, 255, 255, 235, 215, 195, 255
            ]
        );
        let mut clipped = RgbaImage::from_pixel(1, 1, Rgba([20, 40, 60, 255]));
        compose_mask_cursor(&mut clipped, (-3, 0), 4, 1, &mask, 4, None).unwrap();
        assert_eq!(*clipped.get_pixel(0, 0), Rgba([235, 215, 195, 255]));
        let colors = RgbaImage::from_raw(2, 1, vec![1, 2, 3, 0, 7, 11, 13, 0]).unwrap();
        let mut image = RgbaImage::from_pixel(2, 1, Rgba([20, 40, 60, 255]));
        compose_mask_cursor(&mut image, (0, 0), 2, 1, &[0x80, 0, 0, 0], 4, Some(&colors)).unwrap();
        assert_eq!(image.as_raw(), &[21, 42, 63, 255, 7, 11, 13, 255]);
    }
    #[test]
    fn legacy_cursor_refuses_ambiguous_transparent_inversion_and_bad_planes() {
        let mut image = RgbaImage::from_pixel(1, 1, Rgba([20, 40, 60, 128]));
        assert!(
            compose_mask_cursor(
                &mut image,
                (0, 0),
                1,
                1,
                &[0x80, 0, 0, 0, 0x80, 0, 0, 0],
                4,
                None
            )
            .is_err()
        );
        assert_eq!(*image.get_pixel(0, 0), Rgba([20, 40, 60, 128]));
        assert!(compose_mask_cursor(&mut image, (0, 0), 1, 1, &[0; 4], 4, None).is_err());
        assert!(compose_mask_cursor(&mut image, (0, 0), 1, 1, &[0; 8], 5, None).is_err());
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn unsupported_window_policy_rejects_before_delay_or_native_enumeration() {
        let backend = crate::XcapBackend::new(std::path::PathBuf::new())
            .with_options(CaptureOptions {
                window_shadow: CaptureWindowShadow::Include,
                delay_ms: 3600000,
                ..Default::default()
            })
            .unwrap();
        let started = std::time::Instant::now();
        assert!(
            backend
                .capture_batch(vec![crate::CaptureRequest::Window("not-native".into())])
                .is_err()
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }
}
