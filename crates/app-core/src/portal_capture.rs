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
