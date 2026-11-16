//! Quartz global points and display-local, already oriented native pixels.
use crate::{MacDisplayProof, MacLogicalRect, PixelRegion, validate_image_size};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

const PRECISION: f64 = 1.0 / 1024.0;

/// The mode proof alone does not establish the orientation of a captured raster.
/// Modern sources additionally supply their public filter extent and pixel scale.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacDisplayCoordinates {
    pub display_id: u32,
    pub display_uuid: [u8; 16],
    pub logical: MacLogicalRect,
    pub mode_pixel_width: u32,
    pub mode_pixel_height: u32,
    pub rotation: f64,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub pixels_per_point: f64,
}

impl MacDisplayCoordinates {
    pub fn from_filter(
        proof: MacDisplayProof,
        display_frame: MacLogicalRect,
        content: MacLogicalRect,
        pixels_per_point: f64,
    ) -> Result<Self> {
        ensure!(
            proof.logical.same_bounds(display_frame)
                && (content.width - proof.logical.width).abs() <= PRECISION
                && (content.height - proof.logical.height).abs() <= PRECISION,
            "macOS display/filter point geometry disagrees"
        );
        let (pixel_width, pixel_height) = content.pixel_size(pixels_per_point)?;
        let coordinates = Self {
            display_id: proof.id,
            display_uuid: uuid_bytes(&proof)?,
            logical: proof.logical,
            mode_pixel_width: proof.pixel_width,
            mode_pixel_height: proof.pixel_height,
            rotation: proof.rotation,
            pixel_width,
            pixel_height,
            pixels_per_point,
        };
        coordinates.validate()?;
        Ok(coordinates)
    }

    /// The legacy source has no independent oriented filter proof.
    pub fn from_legacy(proof: MacDisplayProof) -> Result<Self> {
        let pixels_per_point = proof.unrotated_scale()?;
        let coordinates = Self {
            pixel_width: proof.pixel_width,
            pixel_height: proof.pixel_height,
            display_id: proof.id,
            display_uuid: uuid_bytes(&proof)?,
            logical: proof.logical,
            mode_pixel_width: proof.pixel_width,
            mode_pixel_height: proof.pixel_height,
            rotation: proof.rotation,
            pixels_per_point,
        };
        coordinates.validate()?;
        Ok(coordinates)
    }

    pub fn to_proof(self) -> MacDisplayProof {
        let hex = self
            .display_uuid
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        MacDisplayProof {
            id: self.display_id,
            uuid: format!(
                "{}-{}-{}-{}-{}",
                &hex[..8],
                &hex[8..12],
                &hex[12..16],
                &hex[16..20],
                &hex[20..]
            ),
            logical: self.logical,
            pixel_width: self.mode_pixel_width,
            pixel_height: self.mode_pixel_height,
            rotation: self.rotation,
        }
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(self.display_id != 0, "Invalid macOS display identity");
        self.logical.validate()?;
        validate_image_size(self.mode_pixel_width, self.mode_pixel_height)?;
        ensure!(
            matches!(self.rotation, 0.0 | 90.0 | 180.0 | 270.0),
            "Unsupported macOS display rotation"
        );
        let raster = (self.pixel_width, self.pixel_height);
        ensure!(
            self.logical.pixel_size(self.pixels_per_point)? == raster,
            "macOS point/native pixel geometry disagrees"
        );
        let mode = (self.mode_pixel_width, self.mode_pixel_height);
        ensure!(
            raster == mode || (matches!(self.rotation, 90.0 | 270.0) && raster == (mode.1, mode.0)),
            "macOS filter raster disagrees with native display mode"
        );
        Ok(())
    }

    pub fn validate_region(&self, area: PixelRegion) -> Result<()> {
        self.validate()?;
        validate_image_size(area.width, area.height)?;
        ensure!(
            area.x
                .checked_add(area.width)
                .is_some_and(|end| end <= self.pixel_width)
                && area
                    .y
                    .checked_add(area.height)
                    .is_some_and(|end| end <= self.pixel_height),
            "macOS native pixel region is outside its display"
        );
        Ok(())
    }

    pub fn integer_point_origin(&self) -> Result<(i32, i32)> {
        self.validate()?;
        let integer = |value: f64| -> Result<i32> {
            ensure!(
                value >= i32::MIN as f64 && value <= i32::MAX as f64 && value.fract() == 0.0,
                "Quartz origin cannot be represented by legacy integer metadata"
            );
            Ok(value as i32)
        };
        Ok((integer(self.logical.x)?, integer(self.logical.y)?))
    }

    pub fn validate_fields(
        &self,
        id: &str,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        scale: f32,
        rotation: f32,
    ) -> Result<()> {
        self.validate()?;
        ensure!(
            id.parse::<u32>().ok() == Some(self.display_id)
                && self.integer_point_origin()? == (x, y)
                && (width, height) == (self.pixel_width, self.pixel_height)
                && scale == self.pixels_per_point as f32
                && rotation == self.rotation as f32,
            "macOS typed and legacy display metadata disagree"
        );
        Ok(())
    }

    /// Templates bind the physical UUID while permitting a new session ID and
    /// display position. Native scale, layout size and oriented pixels stay exact.
    pub fn same_monitor_raster(&self, current: &Self) -> bool {
        self.validate().is_ok()
            && current.validate().is_ok()
            && self.display_uuid == current.display_uuid
            && self.logical.width == current.logical.width
            && self.logical.height == current.logical.height
            && self.mode_pixel_width == current.mode_pixel_width
            && self.mode_pixel_height == current.mode_pixel_height
            && self.pixel_width == current.pixel_width
            && self.pixel_height == current.pixel_height
            && self.pixels_per_point == current.pixels_per_point
            && self.rotation == current.rotation
    }

    /// A global point addresses the native pixel cell at that point. It does not
    /// create a desktop-wide physical-pixel coordinate plane.
    pub fn pixel_at_quartz_point(&self, x: f64, y: f64) -> Result<(u32, u32)> {
        self.validate()?;
        let bounds = self.logical;
        ensure!(
            x.is_finite()
                && y.is_finite()
                && x >= bounds.x
                && y >= bounds.y
                && x < bounds.x + bounds.width
                && y < bounds.y + bounds.height,
            "Quartz point is outside the selected display"
        );
        let pixel_x = ((x - bounds.x) * self.pixels_per_point).floor();
        let pixel_y = ((y - bounds.y) * self.pixels_per_point).floor();
        ensure!(
            pixel_x >= 0.0
                && pixel_y >= 0.0
                && pixel_x < f64::from(self.pixel_width)
                && pixel_y < f64::from(self.pixel_height),
            "Quartz point/native pixel mapping is ambiguous"
        );
        Ok((pixel_x as u32, pixel_y as u32))
    }

    pub fn verify_unchanged(&self, current: &Self) -> Result<()> {
        self.validate()?;
        current.validate()?;
        ensure!(self == current, "macOS display identity/geometry changed");
        Ok(())
    }

    pub fn verify_native_proof(&self, current: &MacDisplayProof) -> Result<()> {
        self.validate()?;
        ensure!(
            current.id == self.display_id
                && uuid_bytes(current)? == self.display_uuid
                && current.logical == self.logical
                && current.pixel_width == self.mode_pixel_width
                && current.pixel_height == self.mode_pixel_height
                && current.rotation == self.rotation,
            "macOS display native proof changed"
        );
        Ok(())
    }
}

fn uuid_bytes(proof: &MacDisplayProof) -> Result<[u8; 16]> {
    proof.validate()?;
    let mut output = [0_u8; 16];
    let mut digits = proof.uuid.bytes().filter(|byte| *byte != b'-');
    for byte in &mut output {
        let high = (digits.next().unwrap() as char).to_digit(16).unwrap();
        let low = (digits.next().unwrap() as char).to_digit(16).unwrap();
        *byte = (high * 16 + low) as u8;
    }
    Ok(output)
}

/// A derived output grid in Quartz layout, never OS-global physical pixels.
/// Lower-resolution sources are explicitly expanded by nearest-neighbor sampling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MacDesktopPolicy {
    QuartzMaxNativeScale,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacDesktopLayout {
    pub policy: MacDesktopPolicy,
    pub displays: Vec<MacDisplayCoordinates>,
    pub placements: Vec<PixelRegion>,
    pub point_origin: (f64, f64),
    pub pixels_per_point: f64,
    pub width: u32,
    pub height: u32,
}

impl MacDesktopLayout {
    pub fn new(displays: &[MacDisplayCoordinates]) -> Result<Self> {
        ensure!(
            (1..=32).contains(&displays.len()),
            "Select between 1 and 32 macOS displays"
        );
        let mut source_bytes = 0_u64;
        let mut left = f64::INFINITY;
        let mut top = f64::INFINITY;
        let mut right = f64::NEG_INFINITY;
        let mut bottom = f64::NEG_INFINITY;
        let mut scale = 0_f64;
        for (at, display) in displays.iter().enumerate() {
            display.validate()?;
            source_bytes += u64::from(display.pixel_width) * u64::from(display.pixel_height) * 4;
            ensure!(
                source_bytes <= crate::MAX_CAPTURE_BYTES,
                "macOS desktop sources exceed their byte limit"
            );
            let bounds = display.logical;
            for other in &displays[..at] {
                ensure!(
                    display.display_id != other.display_id
                        && display.display_uuid != other.display_uuid,
                    "macOS desktop display identity is ambiguous"
                );
                let other = other.logical;
                ensure!(
                    !(bounds.x < other.x + other.width
                        && bounds.x + bounds.width > other.x
                        && bounds.y < other.y + other.height
                        && bounds.y + bounds.height > other.y),
                    "Overlapping or mirrored Quartz display bounds cannot be composed"
                );
            }
            left = left.min(bounds.x);
            top = top.min(bounds.y);
            right = right.max(bounds.x + bounds.width);
            bottom = bottom.max(bounds.y + bounds.height);
            scale = scale.max(display.pixels_per_point);
        }
        let integer = |points: f64| -> Result<u32> {
            let value = points * scale;
            ensure!(
                value.is_finite()
                    && value >= 0.0
                    && value <= u32::MAX as f64
                    && (value - value.round()).abs() <= PRECISION,
                "Quartz layout has no exact integral output grid"
            );
            Ok(value.round() as u32)
        };
        let width = integer(right - left)?;
        let height = integer(bottom - top)?;
        validate_image_size(width, height)?;
        let placements = displays
            .iter()
            .map(|display| {
                let bounds = display.logical;
                let x = integer(bounds.x - left)?;
                let y = integer(bounds.y - top)?;
                let end_x = integer(bounds.x + bounds.width - left)?;
                let end_y = integer(bounds.y + bounds.height - top)?;
                let area = PixelRegion {
                    x,
                    y,
                    width: end_x - x,
                    height: end_y - y,
                };
                validate_image_size(area.width, area.height)?;
                Ok(area)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            policy: MacDesktopPolicy::QuartzMaxNativeScale,
            displays: displays.to_vec(),
            placements,
            point_origin: (left, top),
            pixels_per_point: scale,
            width,
            height,
        })
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            Self::new(&self.displays)? == *self,
            "Invalid macOS desktop grid metadata"
        );
        Ok(())
    }

    pub fn verify_unchanged(&self, displays: &[MacDisplayCoordinates]) -> Result<()> {
        ensure!(
            displays.len() == self.displays.len(),
            "macOS desktop display count changed"
        );
        for expected in &self.displays {
            let mut matching = displays
                .iter()
                .filter(|display| display.display_id == expected.display_id);
            let current = matching
                .next()
                .ok_or_else(|| anyhow::anyhow!("macOS desktop display disappeared"))?;
            ensure!(
                matching.next().is_none(),
                "macOS desktop display identity is ambiguous"
            );
            expected.verify_unchanged(current)?;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn display(
        id: u32,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        scale: f64,
    ) -> MacDisplayCoordinates {
        let proof = MacDisplayProof {
            id,
            uuid: format!("00000000-0000-0000-0000-{id:012x}"),
            logical: MacLogicalRect {
                x,
                y,
                width,
                height,
            },
            pixel_width: (width * scale) as u32,
            pixel_height: (height * scale) as u32,
            rotation: 0.0,
        };
        MacDisplayCoordinates::from_filter(proof.clone(), proof.logical, proof.logical, scale)
            .unwrap()
    }

    #[test]
    fn retina_global_points_map_after_origin_subtraction_and_local_crop_is_native() {
        let display = display(1, -100.0, 40.0, 100.0, 80.0, 2.0);
        assert_eq!(display.pixel_at_quartz_point(-99.0, 41.0).unwrap(), (2, 2));
        assert_eq!(
            display.pixel_at_quartz_point(-99.75, 40.75).unwrap(),
            (0, 1)
        );
        assert!(display.pixel_at_quartz_point(0.0, 40.0).is_err());
        display
            .validate_region(PixelRegion {
                x: 199,
                y: 159,
                width: 1,
                height: 1,
            })
            .unwrap();
        assert!(
            display
                .validate_region(PixelRegion {
                    x: 200,
                    y: 159,
                    width: 1,
                    height: 1
                })
                .is_err()
        );
    }

    #[test]
    fn filter_proves_orientation_without_rotating_pixels_twice() {
        for rotation in [0.0, 90.0, 180.0, 270.0] {
            let mut source = display(1, -100.0, 0.0, 60.0, 100.0, 2.0);
            source.rotation = rotation;
            if matches!(rotation, 90.0 | 270.0) {
                std::mem::swap(&mut source.mode_pixel_width, &mut source.mode_pixel_height);
            }
            source.validate().unwrap();
            assert_eq!((source.pixel_width, source.pixel_height), (120, 200));
            if rotation != 0.0 {
                assert!(MacDisplayCoordinates::from_legacy(source.to_proof()).is_err());
            }
            source.pixel_width += 1;
            assert!(source.validate().is_err());
        }
    }

    #[test]
    fn native_filter_mode_identity_and_scale_are_independent_observations() {
        let source = display(1, 0.0, 0.0, 100.0, 80.0, 2.0);
        let mut changed = source;
        changed.display_uuid[15] = 2;
        assert!(source.verify_unchanged(&changed).is_err());
        for scale in [f64::NAN, 0.0, 8.1, 1.0] {
            assert!(
                MacDisplayCoordinates {
                    pixels_per_point: scale,
                    ..source
                }
                .validate()
                .is_err()
            );
        }
        let bad_frame = MacLogicalRect {
            x: 1.0,
            ..source.logical
        };
        assert!(
            MacDisplayCoordinates::from_filter(source.to_proof(), bad_frame, source.logical, 2.0)
                .is_err()
        );
        let mut mode = source.to_proof();
        mode.pixel_width = 100;
        assert!(
            MacDisplayCoordinates::from_filter(mode, source.logical, source.logical, 2.0).is_err()
        );
    }

    #[test]
    fn legacy_integer_fields_and_template_rebinding_preserve_coordinate_meaning() {
        let source = display(1, -100.0, 40.0, 100.0, 80.0, 2.0);
        source
            .validate_fields("1", -100, 40, 200, 160, 2.0, 0.0)
            .unwrap();
        assert!(
            source
                .validate_fields("2", -100, 40, 200, 160, 2.0, 0.0)
                .is_err()
        );
        assert!(
            source
                .validate_fields("1", -200, 80, 200, 160, 2.0, 0.0)
                .is_err()
        );
        let mut moved = source;
        moved.display_id = 2;
        moved.logical.x = -250.0;
        assert!(source.same_monitor_raster(&moved));
        assert!(source.verify_unchanged(&moved).is_err());
        moved.display_uuid[0] = 1;
        assert!(!source.same_monitor_raster(&moved));
        let fractional = display(1, 0.25, 0.0, 100.0, 80.0, 2.0);
        assert!(fractional.integer_point_origin().is_err());
        source.verify_native_proof(&source.to_proof()).unwrap();
        let mut wrong = source.to_proof();
        wrong.pixel_width += 1;
        assert!(source.verify_native_proof(&wrong).is_err());
    }

    #[test]
    fn desktop_source_and_output_byte_limits_are_independent_before_pixels_exist() {
        let first = display(1, 0.0, 0.0, 8192.0, 4096.0, 1.0);
        let second = display(2, 8192.0, 0.0, 8192.0, 4096.0, 1.0);
        let boundary = MacDesktopLayout::new(&[first, second]).unwrap();
        assert_eq!(
            u64::from(boundary.width) * u64::from(boundary.height) * 4,
            crate::MAX_CAPTURE_BYTES
        );
        let third = display(3, 16_384.0, 0.0, 1.0, 1.0, 1.0);
        assert!(MacDesktopLayout::new(&[first, second, third]).is_err());
        let sparse = display(2, 8193.0, 0.0, 8192.0, 4096.0, 1.0);
        assert!(MacDesktopLayout::new(&[first, sparse]).is_err());
    }

    #[test]
    fn mixed_dpi_grid_uses_quartz_layout_without_false_overlap_or_scaled_origins() {
        let first = display(1, -100.0, 0.0, 100.0, 80.0, 1.0);
        let second = display(2, 0.0, 0.0, 100.0, 80.0, 2.0);
        let layout = MacDesktopLayout::new(&[first, second]).unwrap();
        assert_eq!((layout.width, layout.height), (400, 160));
        assert_eq!(layout.point_origin, (-100.0, 0.0));
        assert_eq!(layout.placements[1].x, 200);
        assert_eq!(layout.placements[0].width, 200);
        let reversed = vec![second, layout.displays[0]];
        layout.verify_unchanged(&reversed).unwrap();
    }

    #[test]
    fn layout_rejects_overlap_fractional_edges_sparse_bounds_and_duplicate_identity() {
        let first = display(1, 0.0, 0.0, 100.0, 80.0, 1.0);
        for other in [
            first,
            display(2, 99.0, 0.0, 100.0, 80.0, 1.0),
            display(2, 100.25, 0.0, 100.0, 80.0, 1.0),
            display(2, 1_000_000.0, 0.0, 100.0, 80.0, 1.0),
        ] {
            assert!(MacDesktopLayout::new(&[first, other]).is_err());
        }
        assert!(MacDesktopLayout::new(&[]).is_err());
        assert!(MacDesktopLayout::new(&vec![first; 33]).is_err());
        let layout = MacDesktopLayout::new(&[first]).unwrap();
        assert!(layout.verify_unchanged(&[]).is_err());
    }
}
