//! Bounded readback geometry for one held FP16 source, independent of D3D.
use crate::{CaptureError, PixelRegion, validate_image_size};

pub(crate) const TILE_WIDTH: u32 = 2048;
pub(crate) const TILE_HEIGHT: u32 = 256;
pub(crate) const MAX_TILE_MAPPED_BYTES: usize = 8 * 1024 * 1024;
#[derive(Clone, Copy, Debug)]
pub(crate) struct Plan {
    area: PixelRegion,
    source_bytes: u64,
    output_bytes: u64,
    stage_width: u32,
    stage_height: u32,
}
impl Plan {
    pub fn new(width: u32, height: u32, area: PixelRegion) -> Result<Self, CaptureError> {
        validate_image_size(width, height)?;
        validate_image_size(area.width, area.height)?;
        let source_bytes = u64::from(width) * u64::from(height) * 8;
        let output_bytes = u64::from(area.width) * u64::from(area.height) * 4;
        if source_bytes > crate::MAX_CAPTURE_BYTES
            || area.x.checked_add(area.width).is_none_or(|v| v > width)
            || area.y.checked_add(area.height).is_none_or(|v| v > height)
        {
            return Err(CaptureError::MemoryLimit);
        }
        Ok(Self {
            area,
            source_bytes,
            output_bytes,
            stage_width: area.width.min(TILE_WIDTH),
            stage_height: area.height.min(TILE_HEIGHT),
        })
    }
    pub fn stage_width(self) -> u32 {
        self.stage_width
    }
    pub fn stage_height(self) -> u32 {
        self.stage_height
    }
    pub fn output_width(self) -> u32 {
        self.area.width
    }
    pub fn output_height(self) -> u32 {
        self.area.height
    }
    pub fn output_stride(self) -> usize {
        self.area.width as usize * 4
    }
    /// Charge full actual pitch, but never infer an allocation length from DepthPitch.
    pub fn mapped_bytes(self, pitch: usize) -> Result<usize, CaptureError> {
        let bytes = pitch
            .checked_mul(self.stage_height as usize)
            .ok_or(CaptureError::MemoryLimit)?;
        let tile = u64::try_from(bytes).map_err(|_| CaptureError::MemoryLimit)?;
        let scratch = 32 * 1024 * 1024;
        if pitch < self.stage_width as usize * 8
            || bytes > MAX_TILE_MAPPED_BYTES
            || self.source_bytes + tile + self.output_bytes
                > 2 * self.source_bytes + 2 * self.output_bytes + scratch
            || self.source_bytes + tile > self.source_bytes + self.output_bytes + scratch
        {
            return Err(CaptureError::MemoryLimit);
        }
        Ok(bytes)
    }
    pub fn source_bounds(self, tile: PixelRegion) -> Result<[u32; 4], CaptureError> {
        if tile.width == 0
            || tile.height == 0
            || tile.width > self.stage_width
            || tile.height > self.stage_height
            || tile
                .x
                .checked_add(tile.width)
                .is_none_or(|v| v > self.area.width)
            || tile
                .y
                .checked_add(tile.height)
                .is_none_or(|v| v > self.area.height)
        {
            return Err(CaptureError::InvalidRegion);
        }
        let left = self
            .area
            .x
            .checked_add(tile.x)
            .ok_or(CaptureError::InvalidRegion)?;
        let top = self
            .area
            .y
            .checked_add(tile.y)
            .ok_or(CaptureError::InvalidRegion)?;
        Ok([
            left,
            top,
            left.checked_add(tile.width)
                .ok_or(CaptureError::InvalidRegion)?,
            top.checked_add(tile.height)
                .ok_or(CaptureError::InvalidRegion)?,
        ])
    }
    /// The mapped view stops at the last copied pixel, excluding trailing row padding.
    pub fn readable_bytes(self, tile: PixelRegion, pitch: usize) -> Result<usize, CaptureError> {
        self.source_bounds(tile)?;
        self.mapped_bytes(pitch)?;
        (tile.height as usize - 1)
            .checked_mul(pitch)
            .and_then(|v| v.checked_add(tile.width as usize * 8))
            .ok_or(CaptureError::MemoryLimit)
    }
    pub fn output_offset(self, tile: PixelRegion) -> Result<usize, CaptureError> {
        self.source_bounds(tile)?;
        usize::try_from((u64::from(tile.y) * u64::from(self.area.width) + u64::from(tile.x)) * 4)
            .map_err(|_| CaptureError::MemoryLimit)
    }
    #[cfg(test)]
    fn test_tiles(mut self, width: u32, height: u32) -> Self {
        assert!(width > 0 && height > 0 && width <= TILE_WIDTH && height <= TILE_HEIGHT);
        self.stage_width = self.area.width.min(width);
        self.stage_height = self.area.height.min(height);
        self
    }
    pub fn tiles(self) -> Tiles {
        Tiles {
            plan: self,
            x: 0,
            y: 0,
        }
    }
}
pub(crate) struct Tiles {
    plan: Plan,
    x: u32,
    y: u32,
}
impl Iterator for Tiles {
    type Item = PixelRegion;
    fn next(&mut self) -> Option<Self::Item> {
        if self.y >= self.plan.area.height {
            return None;
        }
        let width = (self.plan.area.width - self.x).min(self.plan.stage_width);
        let height = (self.plan.area.height - self.y).min(self.plan.stage_height);
        let tile = PixelRegion {
            x: self.x,
            y: self.y,
            width,
            height,
        };
        self.x += width;
        if self.x == self.plan.area.width {
            self.x = 0;
            self.y += height;
        }
        Some(tile)
    }
}
#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    pub(crate) struct Sample {
        pub half: [u16; 4],
        pub sdr: [u8; 4],
        pub hdr: [u8; 4],
    }
    #[derive(serde::Deserialize)]
    pub(crate) struct Oracle {
        pub white: u32,
        pub samples: Vec<Sample>,
        pub highlight: Sample,
    }
    pub(crate) fn oracles() -> Vec<Oracle> {
        serde_json::from_str(include_str!(
            "../tests/fixtures/color/capture-tile-oracle.json"
        ))
        .unwrap()
    }
    pub(crate) fn pixel<'a>(
        oracle: &'a Oracle,
        x: u32,
        y: u32,
        area: PixelRegion,
        late: bool,
    ) -> &'a Sample {
        if late && x == area.x + area.width - 1 && y == area.y + area.height - 1 {
            &oracle.highlight
        } else if area.x > 0 && x == 0 && y == 0 {
            &oracle.highlight
        } else {
            &oracle.samples[((u64::from(x) * 5
                + u64::from(y) * 7
                + u64::from(x / 131) * 3
                + u64::from(y / 19) * 5)
                % oracle.samples.len() as u64) as usize]
        }
    }
    fn fill(
        plan: Plan,
        tile: PixelRegion,
        oracle: &Oracle,
        late: bool,
        stage: &mut [u8],
        pitch: usize,
    ) {
        let [left, top, ..] = plan.source_bounds(tile).unwrap();
        for row in 0..tile.height {
            for x in 0..tile.width {
                let value = pixel(oracle, left + x, top + row, plan.area, late);
                let offset = row as usize * pitch + x as usize * 8;
                for (i, bits) in value.half.into_iter().enumerate() {
                    stage[offset + i * 2..offset + i * 2 + 2].copy_from_slice(&bits.to_le_bytes());
                }
            }
        }
    }
    fn render(
        plan: Plan,
        oracle: &Oracle,
        source_hdr: bool,
        late: bool,
        check: &crate::NativeCaptureCheck<'_>,
    ) -> Result<image::RgbaImage, CaptureError> {
        let pitch = plan.stage_width() as usize * 8 + 24;
        let mut stage = vec![0xff; plan.mapped_bytes(pitch)?];
        let mut hdr = false;
        if source_hdr {
            for tile in plan.tiles() {
                check()?;
                fill(plan, tile, oracle, late, &mut stage, pitch);
                let mut found = false;
                for row in 0..tile.height {
                    let start = row as usize * pitch;
                    let data = &stage[start..start + tile.width as usize * 8];
                    if crate::capture_color::HalfRows::new(
                        tile.width,
                        1,
                        data.len(),
                        data,
                        oracle.white,
                    )?
                    .has_hdr(check)?
                    {
                        found = true;
                        break;
                    }
                }
                if found {
                    hdr = true;
                    break;
                }
            }
        }
        let mut output = image::RgbaImage::new(plan.output_width(), plan.output_height());
        let target: &mut [u8] = output.as_mut();
        for tile in plan.tiles() {
            check()?;
            fill(plan, tile, oracle, late, &mut stage, pitch);
            for row in 0..tile.height {
                let start = row as usize * pitch;
                let data = &stage[start..start + tile.width as usize * 8];
                let offset = plan.output_offset(tile)? + row as usize * plan.output_stride();
                let len = tile.width as usize * 4;
                crate::capture_color::HalfRows::new(tile.width, 1, data.len(), data, oracle.white)?
                    .write_sdr(hdr, &mut target[offset..offset + len], len, check)?;
            }
        }
        Ok(output)
    }
    #[test]
    fn padded_single_and_multiple_tiles_match_independent_oracle_and_untiled_output() {
        let area = PixelRegion {
            x: 3,
            y: 5,
            width: 67,
            height: 19,
        };
        for oracle in oracles() {
            for source_hdr in [false, true] {
                for late in [false, true] {
                    let one = Plan::new(73, 27, area).unwrap();
                    let many = one.test_tiles(7, 5);
                    let a = render(one, &oracle, source_hdr, late, &|| Ok(())).unwrap();
                    let b = render(many, &oracle, source_hdr, late, &|| Ok(())).unwrap();
                    assert_eq!(a, b);
                    let row = area.width as usize * 8;
                    let mut bytes = vec![199; row * area.height as usize];
                    let mut expected = Vec::new();
                    for y in 0..area.height {
                        for x in 0..area.width {
                            let sample = pixel(&oracle, area.x + x, area.y + y, area, late);
                            for (i, bits) in sample.half.into_iter().enumerate() {
                                let at = y as usize * row + x as usize * 8 + i * 2;
                                bytes[at..at + 2].copy_from_slice(&bits.to_le_bytes());
                            }
                            expected.extend(if source_hdr && late {
                                sample.hdr
                            } else {
                                sample.sdr
                            });
                        }
                    }
                    assert_eq!(a.as_raw(), &expected);
                    assert_eq!(
                        a,
                        crate::capture_color::half_rgba_to_sdr(
                            area.width,
                            area.height,
                            row,
                            &bytes,
                            source_hdr,
                            oracle.white,
                            &|| false
                        )
                        .unwrap()
                    );
                }
            }
        }
    }
    #[test]
    fn final_real_tile_highlight_tones_first_tile_and_cancel_returns_no_image() {
        let oracle = oracles().remove(0);
        let area = PixelRegion {
            x: 3,
            y: 5,
            width: 2053,
            height: 263,
        };
        let plan = Plan::new(2059, 269, area).unwrap();
        let output = render(plan, &oracle, true, true, &|| Ok(())).unwrap();
        assert_eq!(output.get_pixel(0, 0).0, [240, 240, 240, 255]);
        for (x, y, value) in output.enumerate_pixels() {
            assert_eq!(
                value.0,
                pixel(&oracle, area.x + x, area.y + y, area, true).hdr
            );
        }
        let calls = std::cell::Cell::new(0);
        let result = render(plan, &oracle, true, true, &|| {
            calls.set(calls.get() + 1);
            if calls.get() == 12 {
                Err(CaptureError::NativeCanceled)
            } else {
                Ok(())
            }
        });
        assert!(matches!(result, Err(CaptureError::NativeCanceled)));
        let calls = std::cell::Cell::new(0);
        let result = render(plan, &oracle, false, false, &|| {
            calls.set(calls.get() + 1);
            if calls.get() == 12 {
                Err(CaptureError::NativeCanceled)
            } else {
                Ok(())
            }
        });
        assert!(matches!(result, Err(CaptureError::NativeCanceled)));
    }
    #[test]
    fn admitted_4k_5k_8k_and_exact_source_ceiling_need_only_small_staging() {
        for (width, height) in [(3840, 2160), (5120, 2880), (7680, 4320), (8192, 4096)] {
            let plan = Plan::new(
                width,
                height,
                PixelRegion {
                    x: 0,
                    y: 0,
                    width,
                    height,
                },
            )
            .unwrap();
            assert!(plan.mapped_bytes(2048 * 8 + 256).unwrap() <= MAX_TILE_MAPPED_BYTES);
            assert_eq!(
                plan.tiles()
                    .map(|v| u64::from(v.width) * u64::from(v.height))
                    .sum::<u64>(),
                u64::from(width) * u64::from(height)
            );
        }
        assert!(
            Plan::new(
                8192,
                4097,
                PixelRegion {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1
                }
            )
            .is_err()
        );
    }
    #[test]
    fn edges_crop_origin_offsets_and_actual_pitch_are_checked_without_overflow() {
        let plan = Plan::new(
            2059,
            269,
            PixelRegion {
                x: 3,
                y: 5,
                width: 2053,
                height: 263,
            },
        )
        .unwrap();
        let tiles = plan.tiles().collect::<Vec<_>>();
        assert_eq!(tiles.len(), 4);
        assert_eq!(
            tiles[3],
            PixelRegion {
                x: 2048,
                y: 256,
                width: 5,
                height: 7
            }
        );
        assert_eq!(
            plan.source_bounds(tiles[3]).unwrap(),
            [2051, 261, 2056, 268]
        );
        assert_eq!(
            plan.output_offset(tiles[3]).unwrap(),
            (256 * 2053 + 2048) * 4
        );
        assert!(plan.mapped_bytes(2048 * 8 - 1).is_err());
        assert!(plan.mapped_bytes(usize::MAX).is_err());
        assert!(plan.mapped_bytes(MAX_TILE_MAPPED_BYTES / 256 + 1).is_err());
        assert_eq!(
            plan.readable_bytes(tiles[3], 2048 * 8 + 16).unwrap(),
            6 * (2048 * 8 + 16) + 5 * 8
        );
        assert!(
            plan.source_bounds(PixelRegion {
                x: u32::MAX,
                y: 0,
                width: 1,
                height: 1
            })
            .is_err()
        );
        assert!(
            Plan::new(
                u32::MAX,
                u32::MAX,
                PixelRegion {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1
                }
            )
            .is_err()
        );
    }
}
