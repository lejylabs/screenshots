//! One admitted native display source; points never masquerade as native pixels.
use crate::{CaptureCursor, CaptureOptions, MacDisplayCoordinates, PixelRegion};
use anyhow::{Context, Result, ensure};
use std::{
    ffi::c_void,
    time::{Duration, Instant},
};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGDisplayCreateImage(display: u32) -> *const c_void;
}

pub(crate) struct PreparedDisplay {
    coordinates: MacDisplayCoordinates,
    source: Option<crate::capture_macos_sck::Display>,
    options: CaptureOptions,
    until: Instant,
}

impl PreparedDisplay {
    pub(crate) fn new(id: u32, options: &CaptureOptions) -> Result<Self> {
        crate::capture_macos_ax::main_thread()?;
        crate::capture_macos_ax::screen_permission()?;
        options.validate_for_capture(false)?;
        let until = Instant::now() + Duration::from_secs(20);
        let proof = crate::capture_macos_ax::displays()?
            .into_iter()
            .find(|display| display.id == id)
            .context("Native display is unavailable")?;
        let source = crate::capture_macos_sck::Display::new(id, until)?;
        let coordinates = if let Some(source) = &source {
            ensure!(
                source.proof == proof,
                "Native display identity changed before admission"
            );
            source.coordinates
        } else {
            MacDisplayCoordinates::from_legacy(proof)?
        };
        let result = Self {
            coordinates,
            source,
            options: *options,
            until,
        };
        result.check()?;
        Ok(result)
    }

    pub(crate) fn coordinates(&self) -> MacDisplayCoordinates {
        self.coordinates
    }

    pub(crate) fn check(&self) -> Result<()> {
        ensure!(
            Instant::now() < self.until,
            "Prepared native display deadline exceeded"
        );
        crate::capture_macos_loop::Topology::check()?;
        let current = crate::capture_macos_ax::displays()?
            .into_iter()
            .find(|display| display.id == self.coordinates.display_id)
            .context("Native display disappeared")?;
        self.coordinates.verify_native_proof(&current)?;
        Ok(())
    }

    pub(crate) fn capture(&self, region: Option<PixelRegion>) -> Result<image::RgbaImage> {
        self.check()?;
        if let Some(area) = region {
            self.coordinates.validate_region(area)?;
        }
        let selected_bytes = region.map_or_else(
            || {
                u64::from(self.coordinates.pixel_width)
                    * u64::from(self.coordinates.pixel_height)
                    * 4
            },
            |area| u64::from(area.width) * u64::from(area.height) * 4,
        );
        crate::capture_macos_model::screenshot_buffer_admission(
            self.coordinates.pixel_width,
            self.coordinates.pixel_height,
            selected_bytes,
        )?;
        let native = match &self.source {
            Some(source) => source.capture(&self.options, self.until, selected_bytes)?,
            None => {
                crate::capture_macos_sck::verify_legacy_app_hidden()?;
                crate::capture_macos_sck::Image(unsafe {
                    CGDisplayCreateImage(self.coordinates.display_id)
                })
            }
        };
        ensure!(!native.0.is_null(), "Native display image is unavailable");
        let native_region =
            if self.source.is_none() && self.options.cursor == CaptureCursor::Include {
                None
            } else {
                region
            };
        let mut pixels = match native_region {
            Some(area) => unsafe {
                crate::capture_macos_image::decode_region(
                    native.0,
                    area,
                    (self.coordinates.pixel_width, self.coordinates.pixel_height),
                )
            }?,
            None => unsafe { crate::capture_macos_image::decode(native.0) }?,
        };
        ensure!(
            pixels.dimensions()
                == native_region.map_or(
                    (self.coordinates.pixel_width, self.coordinates.pixel_height),
                    |area| (area.width, area.height)
                ),
            "Native display pixel extent changed"
        );
        if self.source.is_none() && self.options.cursor == CaptureCursor::Include {
            crate::capture_macos_cursor::compose(
                &mut pixels,
                self.coordinates.logical,
                self.coordinates.pixels_per_point,
            )?;
        }
        self.check()?;
        if let Some(area) = region.filter(|_| native_region.is_none()) {
            Ok(
                image::imageops::crop_imm(&pixels, area.x, area.y, area.width, area.height)
                    .to_image(),
            )
        } else {
            Ok(pixels)
        }
    }
}
