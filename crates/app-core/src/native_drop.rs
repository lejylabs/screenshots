//! Private native drop acquisition, executed only in the existing isolated worker.
use crate::{CaptureError, NativeCaptureCall, NativeCaptureCheck, NativeCaptureReply};
use serde::{Deserialize, Serialize};

/// Native descriptor captured from an actual drop gesture, never an image source URL.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "backend", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeDropSource {
    X11 {
        display: String,
        source: u32,
        target: u32,
        types: [u32; 3],
        extended: bool,
        timestamp: u32,
    },
    MacPasteboard {
        name: String,
        change_count: i64,
    },
}
impl std::fmt::Debug for NativeDropSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::X11 { .. } => f.write_str("NativeDropSource::X11"),
            Self::MacPasteboard { name, .. } => f
                .debug_struct("NativeDropSource::MacPasteboard")
                .field("name_bytes", &name.len())
                .finish_non_exhaustive(),
        }
    }
}
impl NativeDropSource {
    pub fn validate(&self) -> Result<(), CaptureError> {
        let valid = match self {
            Self::X11 {
                display,
                source,
                target,
                timestamp,
                ..
            } => {
                cfg!(target_os = "linux")
                    && *source != 0
                    && *target != 0
                    && *timestamp != 0
                    && display.len() <= 64
                    && display.starts_with(':')
                    && display.len() > 1
                    && display.as_bytes()[1..]
                        .iter()
                        .all(|byte| byte.is_ascii_digit() || *byte == b'.')
            }
            Self::MacPasteboard { name, change_count } => {
                cfg!(target_os = "macos")
                    && (1..=128).contains(&name.len())
                    && !name.contains('\0')
                    && *change_count >= 0
                    && !matches!(
                        name.as_str(),
                        "NSGeneralPboard" | "com.apple.pasteboard.clipboard"
                    )
            }
        };
        if valid {
            Ok(())
        } else {
            Err(CaptureError::Backend(
                "Invalid native drop descriptor".into(),
            ))
        }
    }
}

/// Finite supported wire shapes; images retain their original encoded representation.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NativeDropMime {
    UriList,
    Png,
    Jpeg,
    Webp,
    Bmp,
    Tiff,
}
impl NativeDropMime {
    pub fn maximum(self) -> usize {
        if self == Self::UriList {
            64 * 1024
        } else {
            32 * 1024 * 1024
        }
    }
}
pub struct NativeDropReply {
    pub mime: NativeDropMime,
    /// Empty image bytes denote negotiation; URI metadata is always explicit and nonempty.
    pub bytes: Vec<u8>,
    /// Native root-to-target translation for X11, without a foreground X round trip.
    pub root_offset: Option<[i16; 2]>,
}
impl std::fmt::Debug for NativeDropReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeDropReply")
            .field("mime", &self.mime)
            .field("bytes", &self.bytes.len())
            .finish_non_exhaustive()
    }
}
impl NativeDropReply {
    pub fn validate_shape(mime: NativeDropMime, length: usize, image: bool) -> anyhow::Result<()> {
        anyhow::ensure!(
            length <= mime.maximum(),
            "Native drop exceeds its admitted extent"
        );
        anyhow::ensure!(
            if mime == NativeDropMime::UriList {
                !image && length > 0
            } else if image {
                length > 0
            } else {
                length == 0
            },
            "Invalid native drop phase"
        );
        Ok(())
    }
    pub fn validate(&self, source: &NativeDropSource, image: bool) -> anyhow::Result<()> {
        Self::validate_shape(self.mime, self.bytes.len(), image)?;
        anyhow::ensure!(
            self.root_offset.is_some() == matches!(source, NativeDropSource::X11 { .. }),
            "Native drop coordinate authority differs from its source"
        );
        Ok(())
    }
}

/// The caller invokes this on a background executor and retains its admission through delivery.
pub fn acquire_native_drop(
    source: NativeDropSource,
    image: bool,
    check: &NativeCaptureCheck<'_>,
) -> Result<NativeDropReply, CaptureError> {
    check()?;
    let call = NativeCaptureCall::NativeDrop { source, image };
    call.validate()?;
    let executor = crate::native_capture_job::executor().ok_or(CaptureError::Unsupported(
        "Native drops require the isolated desktop executor",
    ))?;
    let reply = executor(call.clone(), check, crate::NativeCaptureContext::default())?;
    call.validate_reply(&reply)?;
    check()?;
    let NativeCaptureReply::NativeDrop(reply) = reply else {
        return Err(CaptureError::Backend("Invalid native drop reply".into()));
    };
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drop_phase_and_body_bounds_are_checked_before_allocation() {
        for (mime, length, image) in [
            (NativeDropMime::UriList, 0, false),
            (NativeDropMime::UriList, 65537, false),
            (NativeDropMime::UriList, 1, true),
            (NativeDropMime::Png, 1, false),
            (NativeDropMime::Png, 0, true),
            (NativeDropMime::Png, 32 * 1024 * 1024 + 1, true),
        ] {
            assert!(NativeDropReply::validate_shape(mime, length, image).is_err());
        }
        assert!(NativeDropReply::validate_shape(NativeDropMime::Png, 0, false).is_ok());
        assert!(
            NativeDropReply::validate_shape(NativeDropMime::Png, 32 * 1024 * 1024, true).is_ok()
        );
        assert!(
            !format!(
                "{:?}",
                NativeDropSource::MacPasteboard {
                    name: "private-synthetic-token".into(),
                    change_count: 1,
                }
            )
            .contains("synthetic-token")
        );
    }
}
