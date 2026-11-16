//! Explicit remote image input; the desktop executor isolates DNS, HTTP and codecs.
use crate::{
    CaptureError, CapturedImage, NativeCaptureCall, NativeCaptureCheck, NativeCaptureReply,
};
use serde::{Deserialize, Serialize};

pub const MAX_IMAGE_URL_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageUrlSource {
    url: String,
    #[serde(default)]
    allow_http: bool,
}
impl std::fmt::Debug for ImageUrlSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ImageUrlSource")
            .field("allow_http", &self.allow_http)
            .finish_non_exhaustive()
    }
}
impl ImageUrlSource {
    pub fn new(url: String, allow_http: bool) -> Result<Self, CaptureError> {
        let source = Self { url, allow_http };
        source.validate()?;
        Ok(source)
    }
    pub(crate) fn validate(&self) -> Result<(), CaptureError> {
        if self.url.is_empty() || self.url.len() > 4096 || self.url.chars().any(char::is_control) {
            return Err(invalid_url());
        }
        let parsed = reqwest::Url::parse(&self.url).map_err(|_| invalid_url())?;
        if parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.fragment().is_some()
            || !(parsed.scheme() == "https" || self.allow_http && parsed.scheme() == "http")
        {
            return Err(invalid_url());
        }
        Ok(())
    }
    pub fn capture_with_cancel(
        &self,
        check: &NativeCaptureCheck<'_>,
    ) -> Result<CapturedImage, CaptureError> {
        check()?;
        self.validate()?;
        let executor = crate::native_capture_job::executor().ok_or_else(|| {
            CaptureError::Backend(
                "URL image input requires the desktop's isolated operation executor".into(),
            )
        })?;
        let reply = executor(
            NativeCaptureCall::ImageUrl {
                source: self.clone(),
            },
            check,
        )?;
        check()?;
        let NativeCaptureReply::Frames(mut frames) = reply else {
            return Err(invalid_reply());
        };
        if frames.len() != 1 {
            return Err(invalid_reply());
        }
        let frame = frames.pop().unwrap();
        if frame.display_id != "image-url"
            || frame.window_id.is_some()
            || frame.display_geometry.is_some()
        {
            return Err(invalid_reply());
        }
        Ok(frame)
    }
    /// Worker-only implementation. The parent process deadline owns native DNS/codec lifetime.
    pub(crate) fn download_in_worker(
        &self,
        check: &NativeCaptureCheck<'_>,
    ) -> Result<CapturedImage, CaptureError> {
        self.validate()?;
        check()?;
        let started = std::time::Instant::now();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| download_error())?;
        let bytes = runtime.block_on(async {
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .https_only(!self.allow_http)
                .http1_only()
                .http1_max_headers(64)
                .connect_timeout(std::time::Duration::from_secs(5))
                .timeout(std::time::Duration::from_secs(15));
            #[cfg(test)]
            let client = client.no_proxy();
            let client = client.build().map_err(|_| download_error())?;
            let mut response = client
                .get(&self.url)
                .header(
                    reqwest::header::ACCEPT,
                    "image/png,image/jpeg,image/webp,image/bmp,image/tiff",
                )
                .send()
                .await
                .map_err(|_| download_error())?;
            if !response.status().is_success() {
                return Err(download_error());
            }
            if response
                .content_length()
                .is_some_and(|length| length > MAX_IMAGE_URL_BYTES as u64)
            {
                return Err(size_error());
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| download_error())? {
                check()?;
                if chunk.len() > MAX_IMAGE_URL_BYTES.saturating_sub(bytes.len()) {
                    return Err(size_error());
                }
                for part in chunk.chunks(64 * 1024) {
                    check()?;
                    bytes.extend_from_slice(part);
                }
            }
            check()?;
            Ok(bytes)
        })?;
        drop(runtime);
        check()?;
        let pixels = decode_downloaded_image(&bytes, bytes.capacity(), check)?;
        check()?;
        let frame = CapturedImage {
            desktop_layout: None,
            display_id: "image-url".into(),
            window_id: None,
            width: pixels.width(),
            height: pixels.height(),
            pixels: std::sync::Arc::new(pixels),
            png_path: "url-source.png".into(),
            elapsed_ms: started.elapsed().as_millis(),
            source_format: crate::PixelFormat::Rgba8UnormSrgb,
            display_geometry: None,
        };
        Ok(frame)
    }
}
fn decode_downloaded_image(
    bytes: &[u8],
    backing: usize,
    check: &NativeCaptureCheck<'_>,
) -> Result<image::RgbaImage, CaptureError> {
    check()?;
    let pixels = crate::decode_editor_image_bytes_with_backing_and_cancel(bytes, backing, &|| {
        check().is_err()
    });
    // Preserve the operation's cancellation/deadline error rather than recasting it
    // as a format error when the managed decoder stopped partway through.
    check()?;
    pixels.map_err(|error| {
        if matches!(
            error.downcast_ref::<CaptureError>(),
            Some(CaptureError::ImageColorPolicy)
        ) {
            CaptureError::ImageColorPolicy
        } else {
            CaptureError::Backend(
                "Downloaded data is not an admitted PNG, JPEG, WebP, BMP or TIFF image".into(),
            )
        }
    })
}
fn invalid_url() -> CaptureError {
    CaptureError::Backend("Use an explicit HTTPS image URL (HTTP requires allow_http), without credentials or a fragment, at most 4096 bytes".into())
}
fn download_error() -> CaptureError {
    CaptureError::Backend("Image URL download failed or exceeded its transfer deadline".into())
}
fn size_error() -> CaptureError {
    CaptureError::Backend("Image URL response exceeds 32 MiB".into())
}
fn invalid_reply() -> CaptureError {
    CaptureError::Backend("Invalid isolated image URL result".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_loopback_hdr_url_preserves_public_color_policy_error() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            time::{Duration, Instant},
        };
        let bytes = include_bytes!("../tests/fixtures/color/hdr-pq-cicp.png");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    result => panic!("Private image fixture accept failed: {result:?}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = [0u8; 4096];
            let mut used = 0;
            while !request[..used].ends_with(b"\r\n\r\n") {
                assert!(used < request.len());
                let count = stream.read(&mut request[used..]).unwrap();
                assert!(count > 0);
                used += count;
            }
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len()).unwrap();
            stream.write_all(bytes).unwrap();
        });
        let source =
            ImageUrlSource::new(format!("http://{address}/private-hdr.png"), true).unwrap();
        let error = source.download_in_worker(&|| Ok(())).unwrap_err();
        server.join().unwrap();
        assert!(matches!(error, CaptureError::ImageColorPolicy));
        assert!(error.to_string().contains("SDR sRGB"));
        let canceled =
            decode_downloaded_image(bytes, bytes.len(), &|| Err(CaptureError::NativeCanceled));
        assert!(matches!(canceled, Err(CaptureError::NativeCanceled)));
    }
    #[test]
    fn downloaded_decode_observes_signaled_cancellation_without_returning_pixels() {
        use std::cell::Cell;
        let bytes = include_bytes!("../tests/fixtures/color/lcms-p3-rgba16.png");
        assert!(matches!(
            decode_downloaded_image(bytes, bytes.len(), &|| Err(CaptureError::NativeCanceled)),
            Err(CaptureError::NativeCanceled)
        ));
        let calls = Cell::new(0usize);
        let check = || {
            calls.set(calls.get() + 1);
            if calls.get() >= 3 {
                Err(CaptureError::NativeCanceled)
            } else {
                Ok(())
            }
        };
        assert!(matches!(
            decode_downloaded_image(bytes, bytes.len(), &check),
            Err(CaptureError::NativeCanceled)
        ));
        assert!(calls.get() >= 3);
    }
    #[test]
    fn image_url_admission_is_explicit_and_does_not_access_a_source() {
        assert!(
            ImageUrlSource::new("https://example.invalid/image?version=1".into(), false).is_ok()
        );
        assert!(ImageUrlSource::new("http://127.0.0.1/image".into(), true).is_ok());
        for value in [
            "http://example.invalid/x",
            "file:///private/image.png",
            "https://user:private@example.invalid/x",
            "https://example.invalid/x#fragment",
            "https://example.invalid/\n",
        ] {
            assert!(ImageUrlSource::new(value.into(), false).is_err());
        }
        assert!(
            ImageUrlSource::new(
                format!("https://example.invalid/{}", "x".repeat(4096)),
                false
            )
            .is_err()
        );
        let source =
            ImageUrlSource::new("https://example.invalid/private-token".into(), false).unwrap();
        assert!(!format!("{source:?}").contains("private-token"));
        let call = NativeCaptureCall::ImageUrl {
            source: source.clone(),
        };
        call.validate().unwrap();
        assert_eq!(call.frame_count(), Some(1));
        assert!(!call.is_enumeration());
        assert!(
            call.validate_frame_metadata(0, "image-url", None, 2, 1, None)
                .is_ok()
        );
        assert!(
            call.validate_frame_metadata(0, "another-source", None, 2, 1, None)
                .is_err()
        );
        assert!(
            call.validate_frame_metadata(1, "image-url", None, 2, 1, None)
                .is_err()
        );
        assert!(matches!(
            source.capture_with_cancel(&|| Err(CaptureError::NativeCanceled)),
            Err(CaptureError::NativeCanceled)
        ));
    }
}
