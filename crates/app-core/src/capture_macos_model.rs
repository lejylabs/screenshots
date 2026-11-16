//! Portable admission for the macOS native adapters; no framework/runtime initialization.
use crate::{PixelRegion, ScrollMatchOptions, validate_image_size};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

const NS_PER_SECOND: u64 = 1_000_000_000;
pub const MAC_AX_PARENT_LIMIT: usize = 16;
pub const MAC_AX_ARRAY_LIMIT: usize = 32;
pub const MAC_AX_MESSAGE_MILLIS: u32 = 150;
pub const MAC_VIDEO_GAP_SLACK_NS: u64 = 20_000_000;
pub const MAC_VIDEO_DRIFT_SLACK_NS: u64 = 50_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacLogicalRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
impl MacLogicalRect {
    pub fn validate(self) -> Result<()> {
        ensure!(
            [self.x, self.y, self.width, self.height]
                .iter()
                .all(|value| value.is_finite()),
            "Nonfinite macOS geometry"
        );
        ensure!(
            self.width > 0.0
                && self.height > 0.0
                && self.width <= u32::MAX as f64
                && self.height <= u32::MAX as f64
                && self.x.abs() <= i32::MAX as f64
                && self.y.abs() <= i32::MAX as f64
                && (self.x + self.width).abs() <= u32::MAX as f64
                && (self.y + self.height).abs() <= u32::MAX as f64,
            "Invalid macOS geometry"
        );
        Ok(())
    }
    pub fn same_bounds(self, other: Self) -> bool {
        const TOLERANCE: f64 = 1.0 / 1024.0;
        [
            self.x - other.x,
            self.y - other.y,
            self.width - other.width,
            self.height - other.height,
        ]
        .iter()
        .all(|value| value.is_finite() && value.abs() <= TOLERANCE)
    }
    pub fn contains(self, other: Self) -> bool {
        self.validate().is_ok()
            && other.validate().is_ok()
            && other.x >= self.x
            && other.y >= self.y
            && other.x + other.width <= self.x + self.width
            && other.y + other.height <= self.y + self.height
    }
    pub fn pixel_size(self, scale: f64) -> Result<(u32, u32)> {
        self.validate()?;
        ensure!(
            scale.is_finite() && scale > 0.0 && scale <= 8.0,
            "Invalid macOS pixel scale"
        );
        let integral = |value: f64| -> Result<u32> {
            let pixels = value * scale;
            ensure!(
                pixels >= 1.0
                    && pixels <= u32::MAX as f64
                    && (pixels - pixels.round()).abs() <= 1.0 / 1024.0,
                "Ambiguous macOS pixel extent"
            );
            Ok(pixels.round() as u32)
        };
        let size = (integral(self.width)?, integral(self.height)?);
        validate_image_size(size.0, size.1)?;
        Ok(size)
    }
    pub fn region_center(self, scale: f64, region: PixelRegion) -> Result<(f64, f64)> {
        let (width, height) = self.pixel_size(scale)?;
        validate_region(region, width, height)?;
        Ok((
            self.x + (f64::from(region.x) + f64::from(region.width) / 2.0) / scale,
            self.y + (f64::from(region.y) + f64::from(region.height) / 2.0) / scale,
        ))
    }
}
fn validate_region(region: PixelRegion, width: u32, height: u32) -> Result<()> {
    validate_image_size(region.width, region.height)?;
    ensure!(
        region
            .x
            .checked_add(region.width)
            .is_some_and(|end| end <= width)
            && region
                .y
                .checked_add(region.height)
                .is_some_and(|end| end <= height),
        "macOS region is outside its source"
    );
    Ok(())
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn screenshot_buffer_admission(
    width: u32,
    height: u32,
    selected_bytes: u64,
) -> Result<()> {
    validate_image_size(width, height)?;
    ensure!(
        selected_bytes > 0 && selected_bytes <= crate::MAX_CAPTURE_BYTES,
        "Invalid native screenshot memory admission"
    );
    let source = u64::from(width) * u64::from(height) * 4;
    let scratch = 32 * 1024 * 1024;
    // Native CGImage + provider bytes + decoded RGBA can coexist while reading.
    ensure!(
        3 * source + selected_bytes <= 2 * source + 2 * selected_bytes + scratch,
        "Native macOS screenshot exceeds active-image memory admission; choose a smaller source"
    );
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacDisplayProof {
    pub id: u32,
    pub uuid: String,
    pub logical: MacLogicalRect,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub rotation: f64,
}
impl MacDisplayProof {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.id != 0
                && self.uuid.len() == 36
                && self.uuid.bytes().enumerate().all(|(at, byte)| {
                    if [8, 13, 18, 23].contains(&at) {
                        byte == b'-'
                    } else {
                        byte.is_ascii_hexdigit()
                    }
                }),
            "Invalid macOS display identity"
        );
        self.logical.validate()?;
        validate_image_size(self.pixel_width, self.pixel_height)?;
        ensure!(self.rotation.is_finite(), "Invalid macOS display rotation");
        Ok(())
    }
    pub fn unrotated_scale(&self) -> Result<f64> {
        self.validate()?;
        ensure!(
            self.rotation.rem_euclid(360.0) == 0.0,
            "This operation requires an unrotated macOS display"
        );
        let x = f64::from(self.pixel_width) / self.logical.width;
        let y = f64::from(self.pixel_height) / self.logical.height;
        ensure!(
            x.is_finite() && x > 0.0 && x <= 8.0 && (x - y).abs() <= 1.0 / 1024.0,
            "Ambiguous macOS display scale"
        );
        Ok(x)
    }
    #[cfg(any(target_os = "macos", test))]
    pub(crate) fn screenshot_dimensions(
        &self,
        display_frame: MacLogicalRect,
        content: MacLogicalRect,
        scale: f64,
    ) -> Result<(u32, u32)> {
        self.validate()?;
        ensure!(
            self.logical.same_bounds(display_frame),
            "Native display logical geometry changed"
        );
        let size = content.pixel_size(scale)?;
        ensure!(
            size == (self.pixel_width, self.pixel_height),
            "Native display content scale/pixel geometry changed or needs rotation calibration"
        );
        Ok(size)
    }
    /// Actual macOS SDK AVCaptureInput.h declares display-local POINTS,
    /// bottom-left; pair this rectangle with scaleFactor=unrotated_scale().
    pub fn screen_input_crop(&self, region: PixelRegion) -> Result<MacLogicalRect> {
        let scale = self.unrotated_scale()?;
        validate_region(region, self.pixel_width, self.pixel_height)?;
        let top = region
            .y
            .checked_add(region.height)
            .context("macOS crop overflow")?;
        Ok(MacLogicalRect {
            x: f64::from(region.x) / scale,
            y: f64::from(self.pixel_height - top) / scale,
            width: f64::from(region.width) / scale,
            height: f64::from(region.height) / scale,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacWindowProof {
    pub id: u32,
    pub pid: u32,
    pub process_start_seconds: u64,
    pub process_start_micros: u32,
    pub bounds: MacLogicalRect,
    pub onscreen: bool,
    pub layer: i32,
}
impl MacWindowProof {
    pub fn validate(self, originating_pid: u32, helper_pid: u32) -> Result<()> {
        self.bounds.validate()?;
        ensure!(
            originating_pid != 0
                && helper_pid != 0
                && originating_pid != helper_pid
                && self.id != 0
                && self.pid != 0
                && self.pid <= i32::MAX as u32
                && self.process_start_seconds > 0
                && self.process_start_micros < 1_000_000
                && self.pid != originating_pid
                && self.pid != helper_pid
                && self.onscreen
                && self.layer == 0,
            "macOS window is unavailable or not an eligible external target"
        );
        Ok(())
    }
    pub fn matches_focused_window(
        self,
        focused_pid: u32,
        focused_bounds: MacLogicalRect,
        matching_windows: usize,
    ) -> Result<()> {
        ensure!(
            focused_pid == self.pid
                && matching_windows == 1
                && self.bounds.same_bounds(focused_bounds),
            "macOS focus is ambiguous or belongs to another window"
        );
        Ok(())
    }
    pub fn verify_unchanged(self, current: Self) -> Result<()> {
        ensure!(
            self.id == current.id
                && self.pid == current.pid
                && self.process_start_seconds == current.process_start_seconds
                && self.process_start_micros == current.process_start_micros
                && self.bounds.same_bounds(current.bounds)
                && current.onscreen
                && current.layer == 0,
            "macOS window geometry/process changed"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacScrollRequest {
    pub window: u32,
    pub region: PixelRegion,
    pub steps: u32,
    pub wheel: u32,
    pub settle_ms: u32,
    pub minimum_overlap: u32,
    pub tolerance: u8,
    pub trim_top: u32,
    pub trim_bottom: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MacScrollStop {
    Unchanged,
    NoScroll,
    StepLimit,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacScrollSummary {
    pub captured_frames: u32,
    pub scroll_actions: u32,
    pub vertical_advances: Vec<u32>,
    pub stop_reason: MacScrollStop,
    pub complete: bool,
}
impl MacScrollSummary {
    pub fn validate(&self, request: &MacScrollRequest, width: u32, height: u32) -> Result<()> {
        request.validate()?;
        let base = request.region.height - request.trim_top - request.trim_bottom;
        ensure!(
            self.scroll_actions <= request.steps
                && self.captured_frames == self.scroll_actions + 1
                && self.vertical_advances.len() <= self.scroll_actions as usize
                && self.vertical_advances.len() <= 128
                && self
                    .vertical_advances
                    .iter()
                    .all(|value| *value > 0 && *value < base),
            "Invalid native scrolling counters"
        );
        let total = self
            .vertical_advances
            .iter()
            .try_fold(base, |total, value| total.checked_add(*value))
            .context("Native scroll height overflow")?;
        ensure!(
            width == request.region.width && height == total,
            "Native scrolling result dimensions are inconsistent"
        );
        validate_image_size(width, height)?;
        match self.stop_reason {
            MacScrollStop::Unchanged => ensure!(
                self.complete
                    && !self.vertical_advances.is_empty()
                    && self.scroll_actions >= self.vertical_advances.len() as u32 + 2,
                "Native scroll completion is unproven"
            ),
            MacScrollStop::NoScroll => ensure!(
                !self.complete && self.vertical_advances.is_empty() && self.scroll_actions >= 2,
                "Native no-scroll result is inconsistent"
            ),
            MacScrollStop::StepLimit => ensure!(
                !self.complete && self.scroll_actions == request.steps,
                "Native step-limit result is inconsistent"
            ),
        }
        Ok(())
    }
}
impl MacScrollRequest {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.window > 0
                && (1..=128).contains(&self.steps)
                && (1..=10).contains(&self.wheel)
                && (20..=2000).contains(&self.settle_ms)
                && self.steps * self.settle_ms <= 100_000,
            "Invalid bounded macOS scrolling options"
        );
        validate_image_size(self.region.width, self.region.height)?;
        ensure!(
            self.region.width <= 4096
                && self.region.height <= 4096
                && u64::from(self.region.width) * u64::from(self.region.height) * 4
                    <= 16 * 1024 * 1024
                && self.region.x.checked_add(self.region.width).is_some()
                && self.region.y.checked_add(self.region.height).is_some(),
            "Invalid macOS scroll viewport"
        );
        let height = self
            .region
            .height
            .checked_sub(
                self.trim_top
                    .checked_add(self.trim_bottom)
                    .context("macOS trim overflow")?,
            )
            .filter(|height| *height >= 2)
            .context("macOS trim leaves no scroll viewport")?;
        ensure!(
            (1..=1024).contains(&self.minimum_overlap)
                && self.minimum_overlap < height
                && self.tolerance <= 16,
            "Invalid macOS scroll matching options"
        );
        Ok(())
    }
    pub fn matcher(&self) -> ScrollMatchOptions {
        ScrollMatchOptions {
            minimum_overlap: self.minimum_overlap,
            tolerance: self.tolerance,
            trim_top: self.trim_top,
            trim_bottom: self.trim_bottom,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacVideoRequest {
    pub display: u32,
    pub region: PixelRegion,
    pub seconds: u32,
    pub fps: u32,
    pub cursor: bool,
    #[serde(default)]
    pub microphone: bool,
}
impl MacVideoRequest {
    pub fn validate(self) -> Result<()> {
        ensure!(
            self.display > 0
                && (1..=60).contains(&self.seconds)
                && (1..=30).contains(&self.fps)
                && (1..=1920).contains(&self.region.width)
                && (1..=1080).contains(&self.region.height)
                && self.region.x.checked_add(self.region.width).is_some()
                && self.region.y.checked_add(self.region.height).is_some(),
            "Invalid bounded macOS video request"
        );
        validate_image_size(self.region.width, self.region.height)?;
        Ok(())
    }
    pub fn frames(self) -> Result<u32> {
        self.validate()?;
        Ok(self.seconds * self.fps)
    }
    pub fn frame_bytes(self) -> Result<usize> {
        self.validate()?;
        Ok(self.region.width as usize * self.region.height as usize * 4)
    }
    pub fn total_raw_bytes(self) -> Result<u64> {
        Ok(self.frame_bytes()? as u64 * u64::from(self.frames()?))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MacMediaTimestamp {
    pub value: i64,
    pub timescale: i32,
    pub epoch: i64,
    pub flags: u32,
}
impl MacMediaTimestamp {
    pub fn nanoseconds(self) -> Result<u64> {
        ensure!(
            self.timescale > 0
                && self.value >= 0
                && self.epoch == 0
                && self.flags & 1 != 0
                && self.flags & !3 == 0,
            "Invalid/non-numeric native media timestamp"
        );
        let value = i128::from(self.value)
            .checked_mul(i128::from(NS_PER_SECOND))
            .context("Native timestamp overflow")?
            / i128::from(self.timescale);
        ensure!(
            value <= i128::from(i64::MAX),
            "Native timestamp exceeds its wire bound"
        );
        Ok(value as u64)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacVideoSummary {
    pub frames: u32,
    pub first_pts_ns: u64,
    pub last_pts_ns: u64,
}
impl MacVideoSummary {
    pub fn validate(self, request: MacVideoRequest) -> Result<()> {
        let expected = request.frames()?;
        ensure!(
            self.frames == expected
                && self.last_pts_ns >= self.first_pts_ns
                && self.last_pts_ns <= i64::MAX as u64,
            "Invalid native video completion count/timestamps"
        );
        let actual = self.last_pts_ns - self.first_pts_ns;
        let nominal = u64::from(expected - 1) * NS_PER_SECOND / u64::from(request.fps);
        let tolerance =
            (2 * NS_PER_SECOND).div_ceil(u64::from(request.fps)) + MAC_VIDEO_DRIFT_SLACK_NS;
        ensure!(
            actual.abs_diff(nominal) <= tolerance,
            "Native video completion clock drift exceeds its limit"
        );
        Ok(())
    }
}
pub struct MacFrameClock {
    fps: u32,
    expected: u32,
    accepted: u32,
    first: Option<u64>,
    last: Option<u64>,
    failed: bool,
}
impl MacFrameClock {
    pub fn new(request: MacVideoRequest) -> Result<Self> {
        Ok(Self {
            fps: request.fps,
            expected: request.frames()?,
            accepted: 0,
            first: None,
            last: None,
            failed: false,
        })
    }
    pub fn accept(&mut self, timestamp: MacMediaTimestamp) -> Result<()> {
        let result = (|| {
            ensure!(
                !self.failed && self.accepted < self.expected,
                "Native media count is failed or complete"
            );
            let pts = timestamp.nanoseconds()?;
            let two_periods = (2 * NS_PER_SECOND).div_ceil(u64::from(self.fps));
            if let Some(last) = self.last {
                ensure!(
                    pts > last && pts - last <= two_periods + MAC_VIDEO_GAP_SLACK_NS,
                    "Native media timestamp gap/reordering"
                );
            }
            if let Some(first) = self.first {
                let elapsed = pts
                    .checked_sub(first)
                    .context("Native media timestamp regressed")?;
                let expected = u64::from(self.accepted) * NS_PER_SECOND / u64::from(self.fps);
                ensure!(
                    elapsed.abs_diff(expected) <= two_periods + MAC_VIDEO_DRIFT_SLACK_NS,
                    "Native media clock drift exceeds its limit"
                );
            }
            self.first.get_or_insert(pts);
            self.last = Some(pts);
            self.accepted += 1;
            Ok(())
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    pub fn fail(&mut self) {
        self.failed = true;
    }
    pub fn expected_reached(&self) -> bool {
        !self.failed && self.accepted == self.expected
    }
    pub fn finish(&self) -> Result<MacVideoSummary> {
        ensure!(
            self.expected_reached(),
            "Native media ended early, dropped or failed"
        );
        Ok(MacVideoSummary {
            frames: self.accepted,
            first_pts_ns: self.first.unwrap(),
            last_pts_ns: self.last.unwrap(),
        })
    }
}

/// A reservation remains busy until the writer acknowledges its complete raw frame.
/// A failed/busy/abandoned reservation is terminal rather than a hidden frame drop.
#[derive(Clone, Default)]
pub struct MacFrameSlot(Arc<AtomicU8>);
pub struct MacFrameLease {
    state: Arc<AtomicU8>,
    completed: bool,
}
impl MacFrameSlot {
    pub fn reserve(&self) -> Result<MacFrameLease> {
        if self
            .0
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            self.0.store(2, Ordering::Release);
            anyhow::bail!("Native frame backpressure or terminal source");
        }
        Ok(MacFrameLease {
            state: self.0.clone(),
            completed: false,
        })
    }
    pub fn fail(&self) {
        self.0.store(2, Ordering::Release);
    }
    pub fn finish(&self) -> Result<()> {
        self.0
            .compare_exchange(0, 3, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| anyhow::anyhow!("Native frame still owned or source failed"))
    }
}
impl MacFrameLease {
    pub fn complete(mut self) -> Result<()> {
        self.state
            .compare_exchange(1, 0, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| anyhow::anyhow!("Native frame source failed during output"))?;
        self.completed = true;
        Ok(())
    }
}
impl Drop for MacFrameLease {
    fn drop(&mut self) {
        if !self.completed {
            self.state.store(2, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn display() -> MacDisplayProof {
        MacDisplayProof {
            id: 7,
            uuid: "12345678-1234-1234-1234-123456789abc".into(),
            logical: MacLogicalRect {
                x: -1440.0,
                y: 90.0,
                width: 1440.0,
                height: 900.0,
            },
            pixel_width: 2880,
            pixel_height: 1800,
            rotation: 0.0,
        }
    }
    fn video() -> MacVideoRequest {
        MacVideoRequest {
            display: 7,
            region: PixelRegion {
                x: 0,
                y: 0,
                width: 640,
                height: 480,
            },
            seconds: 1,
            fps: 30,
            cursor: false,
            microphone: false,
        }
    }
    #[test]
    fn screenshot_display_uses_content_point_scale_and_never_guesses_from_point_width() {
        let source = display();
        let mut content = source.logical;
        content.x = 0.0;
        content.y = 0.0;
        assert_eq!(
            source
                .screenshot_dimensions(source.logical, content, 2.0)
                .unwrap(),
            (2880, 1800)
        );
        assert!(
            source
                .screenshot_dimensions(source.logical, content, 1.0)
                .is_err()
        );
        let mut moved = source.logical;
        moved.x += 1.0;
        assert!(source.screenshot_dimensions(moved, content, 2.0).is_err());
        content.width += 1.0;
        assert!(
            source
                .screenshot_dimensions(source.logical, content, 2.0)
                .is_err()
        );
    }
    #[test]
    fn screenshot_provider_and_crop_admission_preserve_the_plan_cpu_memory_budget() {
        let source = 8192 * 1024 * 4;
        screenshot_buffer_admission(8192, 1024, source).unwrap();
        screenshot_buffer_admission(3840, 2160, 4).unwrap();
        screenshot_buffer_admission(8192, 1024, 4).unwrap();
        assert!(screenshot_buffer_admission(8193, 1024, 4).is_err());
        screenshot_buffer_admission(16384, 2048, 16384 * 2048 * 4).unwrap();
        assert!(screenshot_buffer_admission(1, 1, 0).is_err());
    }
    #[test]
    fn scroll_summary_never_claims_completion_without_progress_and_two_end_frames() {
        let request = MacScrollRequest {
            window: 42,
            region: PixelRegion {
                x: 0,
                y: 0,
                width: 2,
                height: 4,
            },
            steps: 4,
            wheel: 1,
            settle_ms: 20,
            minimum_overlap: 1,
            tolerance: 0,
            trim_top: 0,
            trim_bottom: 0,
        };
        request.validate().unwrap();
        let summary = MacScrollSummary {
            captured_frames: 4,
            scroll_actions: 3,
            vertical_advances: vec![2],
            stop_reason: MacScrollStop::Unchanged,
            complete: true,
        };
        summary.validate(&request, 2, 6).unwrap();
        assert!(summary.validate(&request, 2, 7).is_err());
        assert!(
            MacScrollSummary {
                complete: false,
                ..summary.clone()
            }
            .validate(&request, 2, 6)
            .is_err()
        );
        assert!(
            MacScrollSummary {
                captured_frames: 2,
                scroll_actions: 1,
                ..summary
            }
            .validate(&request, 2, 6)
            .is_err()
        );
        let mut invalid = request.clone();
        invalid.trim_top = u32::MAX;
        assert!(invalid.validate().is_err());
        invalid = request;
        invalid.minimum_overlap = 4;
        assert!(invalid.validate().is_err());
    }
    fn timestamp(nanos: u64) -> MacMediaTimestamp {
        MacMediaTimestamp {
            value: nanos as i64,
            timescale: NS_PER_SECOND as i32,
            epoch: 0,
            flags: 1,
        }
    }
    #[test]
    fn retina_negative_origin_and_avfoundation_crop_use_their_declared_coordinate_spaces() {
        let display = display();
        assert_eq!(display.unrotated_scale().unwrap(), 2.0);
        let region = PixelRegion {
            x: 20,
            y: 30,
            width: 100,
            height: 80,
        };
        assert_eq!(
            display.screen_input_crop(region).unwrap(),
            MacLogicalRect {
                x: 10.0,
                y: 845.0,
                width: 50.0,
                height: 40.0
            }
        );
        assert_eq!(
            display.logical.region_center(2.0, region).unwrap(),
            (-1405.0, 125.0)
        );
        assert!(
            display
                .screen_input_crop(PixelRegion {
                    x: 0,
                    y: 1790,
                    width: 10,
                    height: 11
                })
                .is_err()
        );
        let mut changed = display.clone();
        changed.rotation = 90.0;
        assert!(changed.unrotated_scale().is_err());
        changed = display.clone();
        changed.pixel_height = 1801;
        assert!(changed.unrotated_scale().is_err());
        assert!(
            MacLogicalRect {
                width: f64::NAN,
                ..display.logical
            }
            .pixel_size(2.0)
            .is_err()
        );
        assert!(
            MacLogicalRect {
                width: 1.2,
                ..display.logical
            }
            .pixel_size(2.0)
            .is_err()
        );
    }
    #[test]
    fn window_binding_rejects_pid_only_focus_ambiguity_and_reused_process_identity() {
        let window = MacWindowProof {
            id: 42,
            pid: 900,
            process_start_seconds: 100,
            process_start_micros: 7,
            bounds: MacLogicalRect {
                x: 10.0,
                y: 20.0,
                width: 800.0,
                height: 600.0,
            },
            onscreen: true,
            layer: 0,
        };
        window.validate(2, 3).unwrap();
        window
            .matches_focused_window(900, window.bounds, 1)
            .unwrap();
        assert!(window.validate(900, 3).is_err());
        assert!(window.validate(0, 3).is_err());
        assert!(
            window
                .matches_focused_window(900, window.bounds, 2)
                .is_err()
        );
        assert!(
            window
                .matches_focused_window(
                    900,
                    MacLogicalRect {
                        x: 12.0,
                        ..window.bounds
                    },
                    1
                )
                .is_err()
        );
        assert!(
            window
                .verify_unchanged(MacWindowProof {
                    process_start_micros: 8,
                    ..window
                })
                .is_err()
        );
        assert!(
            window
                .verify_unchanged(MacWindowProof {
                    onscreen: false,
                    ..window
                })
                .is_err()
        );
    }
    #[test]
    fn stream_count_byte_admission_is_finite_without_aggregating_frames() {
        let mut request = video();
        request.seconds = 60;
        request.fps = 30;
        request.region.width = 1920;
        request.region.height = 1080;
        assert_eq!(request.frames().unwrap(), 1800);
        assert_eq!(request.frame_bytes().unwrap(), 8_294_400);
        assert_eq!(request.total_raw_bytes().unwrap(), 14_929_920_000);
        request.region.width = 1921;
        assert!(request.frames().is_err());
        request = video();
        request.display = 0;
        assert!(request.validate().is_err());
    }
    #[test]
    fn source_clock_checks_rational_pts_exact_count_and_terminal_failure() {
        let mut clock = MacFrameClock::new(video()).unwrap();
        for frame in 0..30 {
            clock
                .accept(MacMediaTimestamp {
                    value: frame,
                    timescale: 30,
                    epoch: 0,
                    flags: 1,
                })
                .unwrap();
        }
        assert_eq!(
            clock.finish().unwrap(),
            MacVideoSummary {
                frames: 30,
                first_pts_ns: 0,
                last_pts_ns: 966_666_666
            }
        );
        clock.finish().unwrap().validate(video()).unwrap();
        assert!(
            MacVideoSummary {
                frames: 29,
                first_pts_ns: 0,
                last_pts_ns: 966_666_666
            }
            .validate(video())
            .is_err()
        );
        assert!(clock.accept(timestamp(1_000_000_000)).is_err());
        assert!(clock.finish().is_err());
        for stamp in [
            MacMediaTimestamp {
                value: -1,
                ..timestamp(0)
            },
            MacMediaTimestamp {
                timescale: 0,
                ..timestamp(0)
            },
            MacMediaTimestamp {
                flags: 17,
                ..timestamp(0)
            },
            MacMediaTimestamp {
                epoch: 1,
                ..timestamp(0)
            },
        ] {
            assert!(stamp.nanoseconds().is_err());
        }
        let mut clock = MacFrameClock::new(video()).unwrap();
        clock.accept(timestamp(100)).unwrap();
        assert!(clock.accept(timestamp(100)).is_err());
        assert!(clock.accept(timestamp(33_333_333)).is_err());
        assert!(clock.finish().is_err());
    }
    #[test]
    fn source_gap_and_prefix_drift_have_fixed_boundary_checks() {
        let request = video();
        let gap = (2 * NS_PER_SECOND).div_ceil(30) + MAC_VIDEO_GAP_SLACK_NS;
        let mut clock = MacFrameClock::new(request).unwrap();
        clock.accept(timestamp(0)).unwrap();
        clock.accept(timestamp(gap)).unwrap();
        let mut clock = MacFrameClock::new(request).unwrap();
        clock.accept(timestamp(0)).unwrap();
        assert!(clock.accept(timestamp(gap + 1)).is_err());
        let mut clock = MacFrameClock::new(request).unwrap();
        for frame in 0..5 {
            clock.accept(timestamp(frame)).unwrap_or_else(|_| {
                assert_eq!(frame, 4);
            });
        }
        assert!(clock.finish().is_err());
    }
    #[test]
    fn slot_reservation_is_retained_through_output_and_backpressure_is_terminal() {
        let slot = MacFrameSlot::default();
        let lease = slot.reserve().unwrap();
        assert!(slot.reserve().is_err());
        assert!(lease.complete().is_err());
        assert!(slot.finish().is_err());
        let slot = MacFrameSlot::default();
        slot.reserve().unwrap().complete().unwrap();
        slot.reserve().unwrap().complete().unwrap();
        slot.finish().unwrap();
        assert!(slot.reserve().is_err());
        let slot = MacFrameSlot::default();
        drop(slot.reserve().unwrap());
        assert!(slot.reserve().is_err());
    }
}
