//! Integer, already oriented physical rasters. DPI never scales desktop origins.
use crate::{CaptureError, DisplayGeometry};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PhysicalMode {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub rotation: f32,
}

pub(crate) fn calibrate(
    mode: PhysicalMode,
    rectangle: [i32; 4],
    backend: PhysicalMode,
    dpi: u32,
) -> Result<DisplayGeometry, CaptureError> {
    crate::validate_image_size(mode.width, mode.height)?;
    if mode != backend
        || !matches!(mode.rotation, 0.0 | 90.0 | 180.0 | 270.0)
        || !(48..=960).contains(&dpi)
        || rectangle[0] != mode.x
        || rectangle[1] != mode.y
        || i64::from(rectangle[2]) - i64::from(rectangle[0]) != i64::from(mode.width)
        || i64::from(rectangle[3]) - i64::from(rectangle[1]) != i64::from(mode.height)
    {
        return Err(CaptureError::StaleRegion);
    }
    Ok(DisplayGeometry {
        mac_coordinates: None,
        x: mode.x,
        y: mode.y,
        width: mode.width,
        height: mode.height,
        scale_factor: dpi as f32 / 96.0,
        rotation: mode.rotation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_coordinates_do_not_change_with_scale_or_rotation() {
        for rotation in [0.0, 90.0, 180.0, 270.0] {
            for dpi in [96, 120, 128, 144, 168, 192, 240] {
                let mode = PhysicalMode {
                    x: -1080,
                    y: -432,
                    width: 1080,
                    height: 1920,
                    rotation,
                };
                let calibrated = calibrate(mode, [-1080, -432, 0, 1488], mode, dpi).unwrap();
                assert_eq!((calibrated.x, calibrated.y), (-1080, -432));
                assert_eq!((calibrated.width, calibrated.height), (1080, 1920));
                assert_eq!(calibrated.scale_factor, dpi as f32 / 96.0);
                assert_eq!(calibrated.rotation, rotation);
            }
        }
    }

    #[test]
    fn logical_mode_mismatch_and_invalid_native_geometry_are_rejected() {
        let mode = PhysicalMode {
            x: 1920,
            y: 0,
            width: 3840,
            height: 2160,
            rotation: 0.0,
        };
        for rectangle in [
            [1280, 0, 3840, 1440],
            [1920, 0, 5761, 2160],
            [1920, 0, 1920, 2160],
            [1920, 0, i32::MIN, 2160],
        ] {
            assert!(calibrate(mode, rectangle, mode, 144).is_err());
        }
        let rectangle = [1920, 0, 5760, 2160];
        for backend in [
            PhysicalMode { x: 1280, ..mode },
            PhysicalMode {
                width: 2560,
                ..mode
            },
            PhysicalMode {
                rotation: 90.0,
                ..mode
            },
        ] {
            assert!(calibrate(mode, rectangle, backend, 144).is_err());
        }
        for dpi in [0, 47, 961, u32::MAX] {
            assert!(calibrate(mode, rectangle, mode, dpi).is_err());
        }
        assert!(
            calibrate(
                PhysicalMode {
                    rotation: 45.0,
                    ..mode
                },
                rectangle,
                PhysicalMode {
                    rotation: 45.0,
                    ..mode
                },
                96
            )
            .is_err()
        );
    }
}
