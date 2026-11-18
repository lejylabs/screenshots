//! Portable immutable Windows source proofs; no SDK calls or pixel allocation.
use crate::{CaptureError, DisplayGeometry, PixelRegion, validate_image_size};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Color {
    pub hdr: bool,
    pub white_level: u32,
    pub adapter_low: u32,
    pub adapter_high: i32,
    pub target: u32,
    pub flags: u32,
    pub encoding: i32,
    pub bits: u32,
    pub output_space: i32,
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WindowColor {
    pub selected: Option<Color>,
    pub displays: Vec<(String, DisplayGeometry, Option<Color>)>,
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Proof {
    Display {
        id: u32,
        name: String,
        geometry: DisplayGeometry,
        area: Option<PixelRegion>,
        color: Option<Color>,
    },
    Window {
        id: u32,
        geometry: ([i32; 4], [i32; 4], u32),
        color: WindowColor,
    },
}
#[derive(Clone, Debug)]
pub(crate) struct Admission {
    proof: Proof,
    bytes: u64,
}
impl Admission {
    pub fn new(proof: Proof) -> Result<Self, CaptureError> {
        let bytes = match &proof {
            Proof::Display {
                geometry,
                area,
                color,
                ..
            } => {
                validate_image_size(geometry.width, geometry.height)?;
                if let Some(area) = area {
                    validate_image_size(area.width, area.height)?;
                    if area
                        .x
                        .checked_add(area.width)
                        .is_none_or(|end| end > geometry.width)
                        || area
                            .y
                            .checked_add(area.height)
                            .is_none_or(|end| end > geometry.height)
                    {
                        return Err(CaptureError::InvalidRegion);
                    }
                }
                if color.is_some() {
                    u64::from(geometry.width) * u64::from(geometry.height) * 8
                } else {
                    area.map_or(
                        u64::from(geometry.width) * u64::from(geometry.height) * 4,
                        |area| u64::from(area.width) * u64::from(area.height) * 4,
                    )
                }
            }
            Proof::Window {
                geometry, color, ..
            } => {
                let full = geometry.0;
                let width = u32::try_from(i64::from(full[2]) - i64::from(full[0]))
                    .map_err(|_| CaptureError::MemoryLimit)?;
                let height = u32::try_from(i64::from(full[3]) - i64::from(full[1]))
                    .map_err(|_| CaptureError::MemoryLimit)?;
                validate_image_size(width, height)?;
                u64::from(width) * u64::from(height) * if color.selected.is_some() { 8 } else { 4 }
            }
        };
        if bytes == 0 || bytes > crate::MAX_CAPTURE_BYTES {
            return Err(CaptureError::MemoryLimit);
        }
        Ok(Self { proof, bytes })
    }
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
    pub fn proof(&self) -> &Proof {
        &self.proof
    }
    pub fn verify(&self, current: &Proof) -> Result<(), CaptureError> {
        if self.proof == *current {
            Ok(())
        } else {
            Err(CaptureError::StaleRegion)
        }
    }
}
/// Duplicate mirrored paths are safe for legacy capture only when every target
/// proves SDR. An Advanced Color mirror has no unique native reference white.
pub(crate) fn matching_color(matches: &[Option<Color>]) -> Result<Option<Color>, CaptureError> {
    match matches {
        [] => Err(CaptureError::StaleRegion),
        [color] => Ok(*color),
        many if many.iter().all(Option::is_none) => Ok(None),
        _ => Err(CaptureError::Unsupported(
            "Mirrored Advanced Color source is ambiguous; select an unmirrored source",
        )),
    }
}
pub(crate) fn legacy_query_unavailable(
    status: i32,
    major: u32,
    build: u32,
    modern_dpi_api: bool,
) -> bool {
    !modern_dpi_api
        && matches!(status, 50 | 87)
        && build > 0
        && ((6..10).contains(&major) || (major == 10 && build < 15063))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn color(white: u32) -> Color {
        Color {
            hdr: true,
            white_level: white,
            adapter_low: 1,
            adapter_high: 0,
            target: 1,
            flags: 2,
            encoding: 0,
            bits: 10,
            output_space: 12,
        }
    }
    fn display(color: Option<Color>) -> Proof {
        Proof::Display {
            id: 7,
            name: "fixture".into(),
            geometry: DisplayGeometry {
                x: 0,
                y: 0,
                width: 4096,
                height: 4096,
                scale_factor: 1.,
                rotation: 0.,
                mac_coordinates: None,
            },
            area: None,
            color,
        }
    }
    #[test]
    fn mirrored_sdr_is_preserved_and_every_ambiguous_hdr_mix_is_refused() {
        assert_eq!(matching_color(&[None, None]).unwrap(), None);
        assert_eq!(matching_color(&[None, None, None]).unwrap(), None);
        assert_eq!(
            matching_color(&[Some(color(2000))]).unwrap(),
            Some(color(2000))
        );
        for values in [
            vec![],
            vec![None, Some(color(2000))],
            vec![Some(color(1000)), Some(color(2000))],
            vec![Some(color(2000)), Some(color(2000))],
        ] {
            assert!(matching_color(&values).is_err());
        }
    }
    #[test]
    fn unavailable_query_is_legacy_only_with_a_proven_pre_hdr_os() {
        for status in [50, 87] {
            assert!(legacy_query_unavailable(status, 10, 14393, false));
            assert!(!legacy_query_unavailable(status, 10, 15063, false));
            assert!(!legacy_query_unavailable(status, 10, 26100, false));
            assert!(!legacy_query_unavailable(status, 11, 1, false));
        }
        assert!(!legacy_query_unavailable(5, 10, 14393, false));
        assert!(!legacy_query_unavailable(87, 0, 0, false));
        // A modern export defeats a version number virtualized by an old manifest.
        assert!(!legacy_query_unavailable(87, 6, 9200, true));
        assert!(!legacy_query_unavailable(87, 10, 14393, true));
    }
    #[test]
    fn native_group_charges_are_independent_of_normalized_output_bytes() {
        let one = Admission::new(display(Some(color(2000)))).unwrap();
        let two = Admission::new(display(Some(color(2000)))).unwrap();
        let three = Admission::new(display(Some(color(2000)))).unwrap();
        let native = one.bytes() + two.bytes();
        assert_eq!(native, crate::MAX_CAPTURE_BYTES);
        let normalized_outputs = native / 2;
        assert!(normalized_outputs + three.bytes() <= crate::MAX_CAPTURE_BYTES);
        assert!(native + three.bytes() > crate::MAX_CAPTURE_BYTES);
        let window = Admission::new(Proof::Window {
            id: 11,
            geometry: ([0, 0, 4096, 4096], [1, 1, 4095, 4095], 7),
            color: WindowColor {
                selected: Some(color(2000)),
                displays: Vec::new(),
            },
        })
        .unwrap();
        assert_eq!(window.bytes(), 128 * 1024 * 1024);
    }
    #[test]
    fn prepared_sdr_to_hdr_and_white_geometry_roi_transitions_cannot_change_reservation() {
        let frozen = Admission::new(display(None)).unwrap();
        assert_eq!(frozen.bytes(), 64 * 1024 * 1024);
        assert_eq!(frozen.proof(), &display(None));
        assert!(frozen.verify(&display(Some(color(2000)))).is_err());
        assert_eq!(frozen.bytes(), 64 * 1024 * 1024);
        let hdr = Admission::new(display(Some(color(2000)))).unwrap();
        assert_eq!(hdr.bytes(), 128 * 1024 * 1024);
        assert!(hdr.verify(&display(Some(color(4000)))).is_err());
        let mut roi = display(Some(color(2000)));
        if let Proof::Display { area, .. } = &mut roi {
            *area = Some(PixelRegion {
                x: 1,
                y: 1,
                width: 1,
                height: 1,
            });
        }
        assert!(hdr.verify(&roi).is_err());
        let tiny = Admission::new(roi).unwrap();
        assert_eq!(tiny.bytes(), 128 * 1024 * 1024);
        let mut moved = display(None);
        if let Proof::Display { geometry, .. } = &mut moved {
            geometry.x = 1;
        }
        assert!(frozen.verify(&moved).is_err());
    }
}
