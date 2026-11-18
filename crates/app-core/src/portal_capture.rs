//! An explicitly selected portal source has no saved physical monitor identity.
use crate::{
    CaptureError, CapturedImage, NativeCaptureCall, NativeCaptureCheck, NativeCaptureReply,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortalCaptureSource {
    Display,
    Window,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn portal_identity_is_explicit_without_physical_geometry_or_window_ids() {
        let call = NativeCaptureCall::Portal {
            source: PortalCaptureSource::Display,
            cursor: PortalCaptureCursor::Include,
        };
        call.validate().unwrap();
        assert_eq!(call.frame_count(), Some(1));
        assert!(!call.is_enumeration());
        call.validate_frame_metadata(0, "portal:display", None, 4, 3, None)
            .unwrap();
        assert!(
            call.validate_frame_metadata(0, "123", None, 4, 3, None)
                .is_err()
        );
        assert!(
            call.validate_frame_metadata(0, "portal:display", Some("123"), 4, 3, None)
                .is_err()
        );
        assert!(
            call.validate_frame_metadata(1, "portal:display", None, 4, 3, None)
                .is_err()
        );
        assert!(
            serde_json::from_str::<NativeCaptureCall>(
                r#"{"operation":"portal","source":"display","cursor":"metadata"}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<NativeCaptureCall>(
                r#"{"operation":"portal","source":"virtual","cursor":"exclude"}"#
            )
            .is_err()
        );
    }
    #[test]
    fn canceled_portal_capture_never_initializes_permission_or_native_work() {
        assert!(matches!(
            capture_portal_with_cancel(
                PortalCaptureSource::Display,
                PortalCaptureCursor::Exclude,
                &|| Err(CaptureError::NativeCanceled)
            ),
            Err(CaptureError::NativeCanceled)
        ));
    }
}
impl PortalCaptureSource {
    pub fn identifier(self) -> &'static str {
        match self {
            Self::Display => "portal:display",
            Self::Window => "portal:window",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortalCaptureCursor {
    #[default]
    Exclude,
    Include,
}

/// Permission selection and one frame run only in the installed operation-owned executor.
pub fn capture_portal_with_cancel(
    source: PortalCaptureSource,
    cursor: PortalCaptureCursor,
    check: &NativeCaptureCheck<'_>,
) -> Result<CapturedImage, CaptureError> {
    check()?;
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (source, cursor);
        return Err(CaptureError::Unsupported(
            "Portal still capture requires Linux",
        ));
    }
    #[cfg(target_os = "linux")]
    {
        let call = NativeCaptureCall::Portal { source, cursor };
        let executor = crate::native_capture_job::executor().ok_or(CaptureError::Unsupported(
            "Portal capture requires the desktop process executor",
        ))?;
        let reply = executor(call.clone(), check, crate::NativeCaptureContext::default())?;
        call.validate_reply(&reply)?;
        check()?;
        let NativeCaptureReply::Frames(mut frames) = reply else {
            return Err(CaptureError::Backend(
                "Portal capture returned no frame".into(),
            ));
        };
        frames.pop().ok_or(CaptureError::EmptyFrame)
    }
}

/// Exact negotiated stream raster; coordinates never imply physical desktop geometry.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortalStreamSource {
    pub source: PortalCaptureSource,
    pub cursor: PortalCaptureCursor,
    pub width: u32,
    pub height: u32,
}
impl PortalStreamSource {
    pub fn validate(self) -> Result<(), CaptureError> {
        if !(1..=1920).contains(&self.width) || !(2..=1080).contains(&self.height) {
            return Err(CaptureError::InvalidRegion);
        }
        crate::validate_image_size(self.width, self.height)
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortalVideoRequest {
    pub stream: PortalStreamSource,
    pub seconds: u32,
    pub fps: u32,
}
impl PortalVideoRequest {
    pub fn validate(self) -> Result<(), CaptureError> {
        self.stream.validate()?;
        if !(1..=60).contains(&self.seconds) || !(1..=30).contains(&self.fps) {
            return Err(CaptureError::InvalidRegion);
        }
        Ok(())
    }
    pub fn frames(self) -> Result<u32, CaptureError> {
        self.validate()?;
        Ok(self.seconds * self.fps)
    }
}
/// Output samples have their own monotonic clock; unchanged compositor frames may repeat.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortalVideoSummary {
    pub frames: u32,
    pub decoded_frames: u32,
    pub repeated_frames: u32,
    pub sample_span_ns: u64,
}
impl PortalVideoSummary {
    pub fn validate(self, request: PortalVideoRequest) -> Result<(), CaptureError> {
        let expected = request.frames()?;
        let nominal = u64::from(expected - 1) * 1_000_000_000 / u64::from(request.fps);
        let slack = 1_000_000_000u64.div_ceil(u64::from(request.fps)) + 50_000_000;
        if self.frames != expected
            || !(1..=16384).contains(&self.decoded_frames)
            || self.repeated_frames >= expected
            || expected - self.repeated_frames > self.decoded_frames
            || self.sample_span_ns.abs_diff(nominal) > slack
        {
            return Err(CaptureError::Backend(
                "Invalid portal video completion".into(),
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortalScrollRequest {
    pub stream: PortalStreamSource,
    pub frames: u32,
    pub interval_ms: u32,
    pub minimum_overlap: u32,
}
impl PortalScrollRequest {
    pub fn validate(self) -> Result<(), CaptureError> {
        self.stream.validate()?;
        if !(2..=129).contains(&self.frames)
            || !(100..=5000).contains(&self.interval_ms)
            || u64::from(self.frames - 1) * u64::from(self.interval_ms) > 60_000
            || self.minimum_overlap == 0
            || self.minimum_overlap > 1024
            || self.minimum_overlap >= self.stream.height
        {
            return Err(CaptureError::InvalidRegion);
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortalScrollSummary {
    pub captured_frames: u32,
    pub vertical_advances: Vec<u32>,
}
impl PortalScrollSummary {
    pub fn validate(
        &self,
        request: PortalScrollRequest,
        width: u32,
        height: u32,
    ) -> Result<(), CaptureError> {
        request.validate()?;
        let added = self.vertical_advances.iter().try_fold(0u32, |sum, value| {
            if *value == 0 || *value > request.stream.height - request.minimum_overlap {
                None
            } else {
                sum.checked_add(*value)
            }
        });
        if self.captured_frames != request.frames
            || self.vertical_advances.len() >= request.frames as usize
            || width != request.stream.width
            || added.and_then(|added| request.stream.height.checked_add(added)) != Some(height)
        {
            return Err(CaptureError::Backend(
                "Invalid manual portal scroll completion".into(),
            ));
        }
        crate::validate_image_size(width, height)
    }
}
/// Manual scrolling has no input authority and never implies that the document end was found.
pub fn capture_portal_scroll_with_cancel(
    request: PortalScrollRequest,
    check: &NativeCaptureCheck<'_>,
) -> Result<(CapturedImage, PortalScrollSummary), CaptureError> {
    check()?;
    request.validate()?;
    if !cfg!(target_os = "linux") {
        return Err(CaptureError::Unsupported("Portal scrolling requires Linux"));
    }
    let call = NativeCaptureCall::PortalScroll { request };
    let executor = crate::native_capture_job::executor().ok_or(CaptureError::Unsupported(
        "Portal scrolling requires the desktop process executor",
    ))?;
    let reply = executor(call.clone(), check, crate::NativeCaptureContext::default())?;
    call.validate_reply(&reply)?;
    check()?;
    let NativeCaptureReply::PortalScroll { image, summary } = reply else {
        return Err(CaptureError::Backend(
            "Portal scrolling returned no image".into(),
        ));
    };
    Ok((image, summary))
}

#[cfg(test)]
mod stream_tests {
    use super::*;
    fn stream() -> PortalStreamSource {
        PortalStreamSource {
            source: PortalCaptureSource::Window,
            cursor: PortalCaptureCursor::Exclude,
            width: 16,
            height: 12,
        }
    }
    #[test]
    fn finite_portal_admission_and_truthful_summary() {
        let request = PortalVideoRequest {
            stream: stream(),
            seconds: 2,
            fps: 5,
        };
        request.validate().unwrap();
        let summary = PortalVideoSummary {
            frames: 10,
            decoded_frames: 1,
            repeated_frames: 9,
            sample_span_ns: 1_800_000_000,
        };
        summary.validate(request).unwrap();
        assert!(
            PortalVideoSummary {
                frames: 9,
                ..summary
            }
            .validate(request)
            .is_err()
        );
        assert!(
            PortalVideoSummary {
                sample_span_ns: 0,
                ..summary
            }
            .validate(request)
            .is_err()
        );
        assert!(PortalVideoRequest { fps: 0, ..request }.validate().is_err());
        assert!(
            PortalVideoRequest {
                stream: PortalStreamSource {
                    width: 1921,
                    ..stream()
                },
                ..request
            }
            .validate()
            .is_err()
        );
        let scroll = PortalScrollRequest {
            stream: stream(),
            frames: 3,
            interval_ms: 500,
            minimum_overlap: 4,
        };
        scroll.validate().unwrap();
        let summary = PortalScrollSummary {
            captured_frames: 3,
            vertical_advances: vec![3, 4],
        };
        summary.validate(scroll, 16, 19).unwrap();
        assert!(summary.validate(scroll, 16, 18).is_err());
        assert!(
            PortalScrollRequest {
                frames: 129,
                interval_ms: 5000,
                ..scroll
            }
            .validate()
            .is_err()
        );
        let call = NativeCaptureCall::PortalScroll { request: scroll };
        call.validate_frame_metadata(0, "portal:window", None, 16, 19, None)
            .unwrap();
        assert!(
            call.validate_frame_metadata(0, "other-window", None, 16, 19, None)
                .is_err()
        );
    }
}
