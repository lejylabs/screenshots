use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{io::Read, path::Path, sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

#[derive(Clone, Copy, Default)]
pub struct UploadProgress {
    pub prepared: u64,
    pub total: u64,
}

#[derive(Clone)]
pub struct UploadProgressReporter(tokio::sync::watch::Sender<UploadProgress>);
pub struct UploadProgressReceiver(tokio::sync::watch::Receiver<UploadProgress>);

pub fn upload_progress_channel() -> (UploadProgressReporter, UploadProgressReceiver) {
    let (sender, receiver) = tokio::sync::watch::channel(UploadProgress::default());
    (
        UploadProgressReporter(sender),
        UploadProgressReceiver(receiver),
    )
}
impl UploadProgressReceiver {
    pub async fn next(&mut self) -> Option<UploadProgress> {
        self.0.changed().await.ok()?;
        Some(*self.0.borrow_and_update())
    }
}

struct ProgressReader<R> {
    reader: R,
    progress: Option<UploadProgressReporter>,
    prepared: u64,
    total: u64,
    last: std::time::Instant,
}
impl<R: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for ProgressReader<R> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let before = buffer.filled().len();
        let result = std::pin::Pin::new(&mut this.reader).poll_read(cx, buffer);
        if this.progress.is_some()
            && matches!(result, std::task::Poll::Ready(Ok(())))
            && buffer.filled().len() > before
        {
            this.prepared += (buffer.filled().len() - before) as u64;
            if this.prepared == this.total || this.last.elapsed() >= Duration::from_millis(250) {
                if let Some(progress) = &this.progress {
                    progress.0.send_replace(UploadProgress {
                        prepared: this.prepared,
                        total: this.total,
                    });
                }
                this.last = std::time::Instant::now();
            }
        }
        result
    }
}

const MAX_RESPONSE: usize = 1024 * 1024;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UploadDestination {
    url: String,
    #[serde(default)]
    method: UploadMethod,
    #[serde(default = "default_field")]
    field: String,
    #[serde(default)]
    json_pointer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    response: Option<UploadResponse>,
    #[serde(default)]
    allow_http: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    credential: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    authentication: Option<UploadAuthentication>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadAuthentication {
    #[default]
    Bearer,
    ApiKey,
    Basic,
}
impl UploadAuthentication {
    pub fn header_name(self) -> &'static str {
        match self {
            Self::Bearer | Self::Basic => "authorization",
            Self::ApiKey => "x-api-key",
        }
    }
}

fn default_field() -> String {
    "file".into()
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum UploadMethod {
    #[default]
    Post,
    Put,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum UploadResponse {
    Text,
    Header { name: String },
    RequestUrl,
}

impl UploadDestination {
    /// Build a credential-free manual destination without performing IO or networking.
    pub fn manual(url: String, method: UploadMethod, field: String, json_pointer: Option<String>, allow_http: bool) -> Result<Self> {
        let response = json_pointer.is_none().then_some(UploadResponse::Text);
        let field = if method == UploadMethod::Post { field.trim().into() } else { default_field() };
        let destination = Self { url: url.trim().into(), method, field,
            json_pointer: json_pointer.unwrap_or_default().trim().into(), response, allow_http,
            credential: None, authentication: None };
        destination.validate()?;
        Ok(destination)
    }

    /// Attach a credential reference without reading or writing secrets.
    pub fn with_authentication(
        mut self,
        reference: String,
        authentication: UploadAuthentication,
    ) -> Result<Self> {
        self.credential = Some(reference.trim().into());
        self.authentication = Some(authentication);
        self.validate()?;
        Ok(self)
    }

    /// Validate, store the secret in the OS credential store, and return the updated profile.
    /// The secret is never written to the destination JSON.
    pub fn attach_credential(
        self,
        reference: String,
        authentication: UploadAuthentication,
        username: String,
        secret: String,
    ) -> Result<Self> {
        let username = zeroize::Zeroizing::new(username);
        let secret = zeroize::Zeroizing::new(secret);
        let destination = self.with_authentication(reference, authentication)?;
        let target = destination
            .credential_target()
            .context("Authenticated destinations require a Windows credential target")?;
        crate::upload_credentials::save(&target, authentication, &username, &secret)?;
        Ok(destination)
    }

    pub fn credential_reference(&self) -> Option<&str> {
        self.credential.as_deref()
    }

    pub fn authentication(&self) -> Option<UploadAuthentication> {
        self.authentication
    }

    pub fn credential_target(&self) -> Option<String> {
        let name = self.credential.as_ref()?;
        let origin = self.validate().ok()?.origin().ascii_serialization();
        Some(format!("Screenshots/upload/{origin}/{name}"))
    }
    pub fn request_method(&self) -> &'static str {
        match self.method {
            UploadMethod::Post => "POST",
            UploadMethod::Put => "PUT",
        }
    }
    fn remembered_path() -> Result<std::path::PathBuf> {
        Ok(crate::CaptureSettings::config_path()?.with_file_name("upload-destination.json"))
    }
    pub fn load_remembered() -> Result<Option<Self>> {
        Self::load_optional(&Self::remembered_path()?)
    }
    fn load_optional(path: &Path) -> Result<Option<Self>> {
        match Self::load(path) {
            Ok(value) => Ok(Some(value)),
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
    pub fn remember(&self) -> Result<()> {
        self.save_at(&Self::remembered_path()?)
    }
    fn save_at(&self, path: &Path) -> Result<()> {
        use std::io::Write;
        self.validate()?;
        let parent = path.parent().context("Invalid upload settings path")?;
        std::fs::create_dir_all(parent)?;
        let temporary = parent.join(format!("upload-{}.part", uuid::Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(&serde_json::to_vec_pretty(self)?)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temporary, path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(temporary);
        }
        result
    }
    pub fn forget_remembered() -> Result<()> {
        Self::remove_at(&Self::remembered_path()?)
    }
    fn remove_at(path: &Path) -> Result<()> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(16385)
            .read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 16384, "Upload configuration exceeds 16 KiB");
        let destination: Self =
            serde_json::from_slice(&bytes).context("Invalid upload configuration")?;
        destination.validate()?;
        Ok(destination)
    }

    fn validate(&self) -> Result<reqwest::Url> {
        ensure!(
            self.authentication.is_none() || self.credential.is_some(),
            "Authentication requires an OS credential reference"
        );
        if let Some(response) = &self.response {
            if matches!(response, UploadResponse::RequestUrl) {
                ensure!(
                    matches!(self.method, UploadMethod::Put),
                    "request_url response mode requires PUT to a resource URL"
                );
            }
            ensure!(
                self.json_pointer.is_empty(),
                "Choose either json_pointer or response, not both"
            );
            if let UploadResponse::Header { name } = response {
                ensure!(
                    name.len() <= 64
                        && reqwest::header::HeaderName::from_bytes(name.as_bytes()).is_ok(),
                    "Use a valid response header name of at most 64 bytes"
                );
            }
        }
        ensure!(
            self.url.len() <= 2048 && !self.url.chars().any(char::is_control),
            "Upload URL is invalid or exceeds 2048 bytes"
        );
        let url = reqwest::Url::parse(&self.url).context("Invalid upload URL")?;
        if let Some(name) = &self.credential {
            ensure!(
                url.scheme() == "https",
                "Upload credentials require HTTPS, including for self-hosted destinations"
            );
            ensure!(
                !name.is_empty()
                    && name.len() <= 64
                    && name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte)),
                "Credential reference must contain 1–64 letters, digits, underscores or hyphens"
            );
            ensure!(
                cfg!(target_os = "windows"),
                "OS upload credential lookup is currently supported on Windows only"
            );
        }
        ensure!(
            url.scheme() == "https" || (url.scheme() == "http" && self.allow_http),
            "Use HTTPS, or explicitly enable allow_http for your HTTP destination"
        );
        ensure!(
            url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "Upload URLs must have a host and no embedded credentials, query or fragment"
        );
        ensure!(
            !self.field.is_empty()
                && self.field.len() <= 64
                && self
                    .field
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'),
            "Use a multipart field of 1–64 letters, digits, underscores or hyphens"
        );
        ensure!(
            self.json_pointer.len() <= 256
                && (self.json_pointer.is_empty() || self.json_pointer.starts_with('/')),
            "Use a JSON pointer up to 256 bytes, such as /url"
        );
        ensure!(
            self.json_pointer
                .split('~')
                .skip(1)
                .all(|part| part.starts_with('0') || part.starts_with('1')),
            "Invalid JSON pointer escape; use ~0 for ~ and ~1 for /"
        );
        if matches!(self.response, Some(UploadResponse::RequestUrl)) {
            validate_link(url.as_str())?;
        }
        Ok(url)
    }

    pub fn origin(&self) -> String {
        self.validate()
            .map(|url| url.origin().ascii_serialization())
            .unwrap_or_default()
    }
}

#[derive(Clone, Default)]
pub struct UploadCancellation(Arc<tokio::sync::Notify>);
impl UploadCancellation {
    pub fn cancel(&self) {
        self.0.notify_one();
    }
}

/// Run on a background executor. The operation owns and drops its network runtime/client.
pub fn upload_png(
    destination: &UploadDestination,
    path: &Path,
    cancel: UploadCancellation,
) -> Result<String> {
    run_upload(destination, path, cancel, None)
}

pub fn upload_png_with_progress(
    destination: &UploadDestination,
    path: &Path,
    cancel: UploadCancellation,
    progress: UploadProgressReporter,
) -> Result<String> {
    run_upload(destination, path, cancel, Some(progress))
}

/// Upload a throwaway 1×1 PNG through the existing transfer path, then delete it.
/// The destination profile, capture library and last-saved image are not modified.
pub fn probe_upload(
    destination: &UploadDestination,
    cancel: UploadCancellation,
    progress: Option<UploadProgressReporter>,
) -> Result<String> {
    let root = std::env::temp_dir().join(format!(
        "screenshots-upload-probe-{}",
        uuid::Uuid::new_v4()
    ));
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    std::fs::create_dir_all(&root)?;
    let cleanup = Cleanup(root.clone());
    let path = write_probe_png(&root)?;
    let result = run_upload(destination, &path, cancel, progress);
    drop(cleanup);
    result
}

fn write_probe_png(root: &Path) -> Result<std::path::PathBuf> {
    std::fs::create_dir_all(root)?;
    let path = root.join("probe.png");
    image::RgbaImage::new(1, 1)
        .save(&path)
        .context("Could not write the destination test image")?;
    Ok(path)
}

fn run_upload(
    destination: &UploadDestination,
    path: &Path,
    cancel: UploadCancellation,
    progress: Option<UploadProgressReporter>,
) -> Result<String> {
    let url = destination.validate()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()?;
    runtime.block_on(async {
        tokio::select! {
            biased;
            _ = cancel.0.notified() => anyhow::bail!("Upload canceled. The server may already have received the file."),
            result = tokio::time::timeout(Duration::from_secs(60), transfer(destination, path, url, progress)) => {
                result.context("Upload exceeded its 60 second deadline; the server may have received the file")?
            }
        }
    })
}

async fn transfer(
    destination: &UploadDestination,
    path: &Path,
    url: reqwest::Url,
    progress: Option<UploadProgressReporter>,
) -> Result<String> {
    ensure!(path.is_absolute(), "Upload source must be an absolute path");
    let mut file = tokio::fs::File::open(path)
        .await
        .context("Could not open the saved PNG")?;
    file.set_max_buf_size(64 * 1024);
    let metadata = file.metadata().await?;
    ensure!(
        metadata.is_file()
            && metadata.len() >= 8
            && metadata.len() <= crate::MAX_CAPTURE_BYTES + 16 * 1024 * 1024,
        "PNG file exceeds the upload size limit or is not a regular file"
    );
    let mut signature = [0; 8];
    file.read_exact(&mut signature).await?;
    ensure!(
        signature == [137, 80, 78, 71, 13, 10, 26, 10],
        "Upload source is not a PNG"
    );
    file.rewind().await?;
    let length = metadata.len();
    if let Some(progress) = &progress {
        progress.0.send_replace(UploadProgress {
            prepared: 0,
            total: length,
        });
    }
    let reader = ProgressReader {
        reader: file.take(length),
        progress,
        prepared: 0,
        total: length,
        last: std::time::Instant::now(),
    };
    let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::with_capacity(
        reader,
        64 * 1024,
    ));
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(60))
        .http1_only()
        .pool_max_idle_per_host(0)
        .no_proxy()
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .build()?;
    let resource_url =
        matches!(destination.response, Some(UploadResponse::RequestUrl)).then(|| url.to_string());
    let mut request = match destination.method {
        UploadMethod::Post => {
            let part = reqwest::multipart::Part::stream_with_length(body, length)
                .file_name("screenshot.png")
                .mime_str("image/png")?;
            client
                .post(url)
                .multipart(reqwest::multipart::Form::new().part(destination.field.clone(), part))
        }
        UploadMethod::Put => client
            .put(url)
            .header(reqwest::header::CONTENT_TYPE, "image/png")
            .header(reqwest::header::CONTENT_LENGTH, length)
            .header(reqwest::header::IF_NONE_MATCH, "*")
            .body(body),
    };
    if let Some(target) = destination.credential_target() {
        let authentication = destination.authentication.unwrap_or_default();
        request = request.header(
            authentication.header_name(),
            crate::upload_credentials::read_header(&target, authentication)?,
        );
    }
    let mut response = request
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .send()
        .await
        .map_err(|error| error.without_url())
        .context("Upload request failed; the server may have received the file")?;
    ensure!(
        response.status().is_success(),
        "Upload returned HTTP {}; redirects are not followed and uploads are not retried",
        response.status().as_u16()
    );
    ensure!(
        response
            .content_length()
            .is_none_or(|length| length <= MAX_RESPONSE as u64),
        "Upload response exceeds 1 MiB; the file may already be uploaded"
    );
    if let Some(url) = resource_url {
        ensure!(
            matches!(response.status().as_u16(), 200 | 201 | 204),
            "PUT did not confirm resource completion (HTTP {}); no completed resource link is available",
            response.status().as_u16()
        );
        return validate_link(&url);
    }
    if let Some(UploadResponse::Header { name }) = &destination.response {
        let mut values = response.headers().get_all(name.as_str()).iter();
        let value = values
            .next()
            .context("Upload completed, but the configured response header is missing")?;
        ensure!(
            values.next().is_none(),
            "Upload completed, but the response link header is repeated"
        );
        return validate_link(
            value
                .to_str()
                .context("Upload response header is not valid text")?
                .trim(),
        );
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| error.without_url())
        .context("Upload response interrupted; the server may have received the file")?
    {
        ensure!(
            bytes.len().saturating_add(chunk.len()) <= MAX_RESPONSE,
            "Upload response exceeds 1 MiB; the file may already be uploaded"
        );
        bytes.extend_from_slice(&chunk);
    }
    if matches!(destination.response, Some(UploadResponse::Text)) {
        return validate_link(
            std::str::from_utf8(&bytes)
                .context("Upload completed, but its response is not UTF-8 text")?
                .trim(),
        );
    }
    let json: serde_json::Value = serde_json::from_slice(&bytes)
        .context("Upload completed, but its response is not valid JSON")?;
    let value = json
        .pointer(&destination.json_pointer)
        .and_then(|value| value.as_str())
        .context("Upload completed, but the configured JSON pointer contains no link")?;
    validate_link(value)
}

pub(crate) fn validate_link(value: &str) -> Result<String> {
    ensure!(
        value.len() <= 2048 && !value.chars().any(char::is_control),
        "Upload response link is invalid or too long"
    );
    let link = reqwest::Url::parse(value).context("Upload response link is not an absolute URL")?;
    ensure!(
        matches!(link.scheme(), "http" | "https")
            && link.host_str().is_some()
            && link.username().is_empty()
            && link.password().is_none(),
        "Upload response link must be an HTTP(S) URL without embedded credentials"
    );
    Ok(link.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        net::{TcpListener, TcpStream},
        sync::mpsc,
        thread,
    };

    #[test]
    fn manual_destinations_validate_and_reuse_saved_profile_policy() -> Result<()> {
        for method in [UploadMethod::Post, UploadMethod::Put] {
            for pointer in [None, Some("/result/url".to_string()), Some(String::new())] {
                let config = UploadDestination::manual(" https://example.invalid/upload ".into(), method,
                    " file ".into(), pointer.clone(), false)?;
                assert_eq!(config.url, "https://example.invalid/upload");
                assert_eq!(config.field, "file");
                assert!(config.credential_target().is_none() && config.authentication.is_none());
                assert_eq!(config.request_method(), if method == UploadMethod::Post { "POST" } else { "PUT" });
                assert_eq!(config.json_pointer, pointer.clone().unwrap_or_default());
                assert_eq!(matches!(config.response, Some(UploadResponse::Text)), pointer.is_none());
                let root = std::env::temp_dir().join(format!("manual-destination-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&root)?;
                let path = root.join("destination.json");
                config.save_at(&path)?;
                let loaded = UploadDestination::load(&path)?;
                assert_eq!(loaded.request_method(), config.request_method());
                assert_eq!(loaded.json_pointer, config.json_pointer);
                assert!(loaded.credential_target().is_none());
                std::fs::remove_file(path)?; std::fs::remove_dir(root)?;
            }
        }
        for url in ["", "file:///tmp/image", "http://localhost/upload", "https://user:pass@example.invalid/", "https://example.invalid/?token=x", "https://example.invalid/#fragment"] {
            assert!(UploadDestination::manual(url.into(), UploadMethod::Post, "file".into(), None, false).is_err());
        }
        assert!(UploadDestination::manual("http://localhost/upload".into(), UploadMethod::Put, "file".into(), None, true).is_ok());
        assert!(UploadDestination::manual("https://example.invalid/".into(), UploadMethod::Post, "bad field".into(), None, false).is_err());
        assert!(UploadDestination::manual("https://example.invalid/".into(), UploadMethod::Post, "file".into(), Some("/bad~2".into()), false).is_err());
        Ok(())
    }

    #[test]
    fn progress_coalesces_closes_and_counts_only_consumed_png_bytes() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (reporter, mut receiver) = upload_progress_channel();
            for prepared in 0..1000 {
                reporter.0.send_replace(UploadProgress {
                    prepared,
                    total: 2000,
                });
            }
            let latest = tokio::time::timeout(Duration::from_secs(1), receiver.next())
                .await
                .unwrap()
                .unwrap();
            assert_eq!((latest.prepared, latest.total), (999, 2000));
            drop(reporter);
            assert!(
                tokio::time::timeout(Duration::from_secs(1), receiver.next())
                    .await
                    .unwrap()
                    .is_none()
            );
            let (reporter, mut receiver) = upload_progress_channel();
            let bytes = vec![12u8; 128 * 1024];
            let mut reader = ProgressReader {
                reader: bytes.as_slice(),
                progress: Some(reporter),
                prepared: 0,
                total: bytes.len() as u64,
                last: std::time::Instant::now(),
            };
            let mut output = Vec::new();
            reader.read_to_end(&mut output).await.unwrap();
            assert_eq!(output, bytes);
            let progress = tokio::time::timeout(Duration::from_secs(1), receiver.next())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                (progress.prepared, progress.total),
                (bytes.len() as u64, bytes.len() as u64)
            );
            drop(reader);
            assert!(receiver.next().await.is_none());
        });
    }

    fn fixture() -> (std::path::PathBuf, std::path::PathBuf) {
        let root =
            std::env::temp_dir().join(format!("screenshots-upload-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("private-source-name.png");
        image::RgbaImage::new(3, 2).save(&path).unwrap();
        (root, path)
    }
    fn destination(url: String) -> UploadDestination {
        UploadDestination::manual(url, UploadMethod::Post, "file".into(), Some("/result/url".into()), true).unwrap()
    }
    fn read_request(stream: &mut TcpStream) -> Vec<u8> {
        // Accepted sockets can inherit the listener's nonblocking mode on Windows.
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = Vec::new();
        let mut byte = [0; 1];
        while !request.ends_with(b"\r\n\r\n") {
            assert!(request.len() < 8192);
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        let headers = String::from_utf8(request.clone()).unwrap();
        let length: usize = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().unwrap())
            })
            .expect("Multipart request must have a bounded known length");
        assert!(length < 1024 * 1024);
        let offset = request.len();
        request.resize(offset + length, 0);
        stream.read_exact(&mut request[offset..]).unwrap();
        request
    }
    fn server(
        response: Vec<u8>,
        received: Option<mpsc::SyncSender<()>>,
        stall: Duration,
    ) -> (String, thread::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/upload", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let thread = thread::spawn(move || {
            let started = std::time::Instant::now();
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && started.elapsed() < Duration::from_secs(3) =>
                    {
                        thread::sleep(Duration::from_millis(1))
                    }
                    error => panic!("Loopback upload server accept failed: {error:?}"),
                }
            };
            let request = read_request(&mut stream);
            if let Some(received) = received {
                received.send(()).unwrap();
            }
            if !stall.is_zero() {
                thread::sleep(stall);
            }
            let _ = stream.write_all(&response);
            request
        });
        (url, thread)
    }
    fn response(json: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
            json.len()
        )
        .into_bytes()
    }
    fn probe_temp_names() -> std::collections::HashSet<String> {
        std::fs::read_dir(std::env::temp_dir())
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                name.starts_with("screenshots-upload-probe-")
                    .then_some(name)
            })
            .collect()
    }

    #[test]
    fn probe_png_is_tiny_and_probe_upload_cleans_up_without_connecting() -> Result<()> {
        let root = std::env::temp_dir().join(format!(
            "screenshots-probe-png-check-{}",
            uuid::Uuid::new_v4()
        ));
        let path = write_probe_png(&root)?;
        let bytes = std::fs::read(&path)?;
        assert!(bytes.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10]));
        assert!(bytes.len() < 1024);
        let (width, height) = image::image_dimensions(&path)?;
        assert_eq!((width, height), (1, 1));
        std::fs::remove_dir_all(&root)?;

        let before = probe_temp_names();
        let mut config = destination("https://example.invalid/upload".into());
        config.url = "https://user:pass@example.invalid/upload".into();
        assert!(probe_upload(&config, UploadCancellation::default(), None).is_err());
        assert_eq!(before, probe_temp_names());
        Ok(())
    }

    #[test]
    fn remembered_destination_round_trips_replaces_and_forgets_without_touching_other_settings()
    -> Result<()> {
        let root =
            std::env::temp_dir().join(format!("screenshots-profile-{}", uuid::Uuid::new_v4()));
        let path = root.join("upload-destination.json");
        assert!(UploadDestination::load_optional(&path)?.is_none());
        assert!(!root.exists());
        let first = destination("https://first.example.invalid/upload".into());
        first.save_at(&path)?;
        assert_eq!(
            UploadDestination::load_optional(&path)?.unwrap().origin(),
            first.origin()
        );
        let second = destination("https://second.example.invalid/upload".into());
        second.save_at(&path)?;
        let bytes = std::fs::read(&path)?;
        assert_eq!(
            UploadDestination::load_optional(&path)?.unwrap().origin(),
            second.origin()
        );
        let mut invalid = first;
        invalid.url = "https://user:secret@example.invalid/".into();
        assert!(invalid.save_at(&path).is_err());
        assert_eq!(std::fs::read(&path)?, bytes);
        let other = root.join("settings.json");
        std::fs::write(&other, b"keep")?;
        std::fs::write(&path, b"invalid JSON")?;
        assert!(UploadDestination::load_optional(&path).is_err());
        UploadDestination::remove_at(&path)?;
        UploadDestination::remove_at(&path)?;
        assert!(UploadDestination::load_optional(&path)?.is_none());
        assert_eq!(std::fs::read(&other)?, b"keep");
        assert_eq!(std::fs::read_dir(&root)?.count(), 1);
        let blocked = other.join("upload-destination.json");
        assert!(second.save_at(&blocked).is_err());
        assert_eq!(std::fs::read(&other)?, b"keep");
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn response_modes_validate_configuration_and_share_safe_link_rules() -> Result<()> {
        let (root, _) = fixture();
        let path = root.join("destination.json");
        for response in [
            r#"{"type":"text"}"#,
            r#"{"type":"header","name":"Location"}"#,
        ] {
            std::fs::write(
                &path,
                format!(
                    r#"{{"url":"https://example.invalid/upload","field":"file","response":{response}}}"#
                ),
            )?;
            let profile = UploadDestination::load(&path)?;
            profile.save_at(&path)?;
            assert!(UploadDestination::load(&path)?.response.is_some());
        }
        let mut profile = destination("https://example.invalid/upload".into());
        profile.response = Some(UploadResponse::Text);
        assert!(profile.validate().is_err());
        profile.json_pointer.clear();
        assert!(profile.validate().is_ok());
        for name in ["", "bad header", "Location\r\nInjected", &"x".repeat(65)] {
            profile.response = Some(UploadResponse::Header { name: name.into() });
            assert!(profile.validate().is_err());
        }
        for value in [
            "javascript:alert(1)",
            "file:///private",
            "/relative",
            "https://user:pass@example.invalid/",
            "https://example.invalid/a\nb",
            &format!("https://example.invalid/{}", "x".repeat(2048)),
        ] {
            assert!(validate_link(value).is_err());
        }
        assert_eq!(
            validate_link("https://example.invalid/image")?,
            "https://example.invalid/image"
        );
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    #[ignore = "local loopback response-format tests with synthetic PNG; no external connections"]
    fn upload_reads_plain_text_and_header_links_without_following_them() {
        let (root, path) = fixture();
        for (mode, response_bytes, expected) in [
            (UploadResponse::Text, response(" \r\nhttps://example.invalid/plain\n"), Some("https://example.invalid/plain")),
            (UploadResponse::Text, response("javascript:alert(1)"), None),
            (UploadResponse::Header{name:"location".into()}, b"HTTP/1.1 201 Created\r\nLocation: https://example.invalid/header\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(), Some("https://example.invalid/header")),
            (UploadResponse::Header{name:"Location".into()}, response("{}"), None),
            (UploadResponse::Header{name:"Location".into()}, b"HTTP/1.1 200 OK\r\nLocation: file:///private\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(), None),
            (UploadResponse::Header{name:"Location".into()}, b"HTTP/1.1 200 OK\r\nLocation: https://example.invalid/one\r\nLocation: https://example.invalid/two\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(), None),
        ] {
            let (url, task) = server(response_bytes,None,Duration::ZERO);
            let mut config = destination(url); config.json_pointer.clear(); config.response = Some(mode);
            let started = std::time::Instant::now();
            let result = upload_png(&config,&path,UploadCancellation::default());
            if expected.is_some() { eprintln!("Synthetic PNG text/header response including runtime teardown: {:?}", started.elapsed()); }
            match expected { Some(link) => assert_eq!(result.unwrap(), link), None => assert!(result.is_err()) }
            task.join().unwrap();
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn upload_method_defaults_and_remembered_put_profiles_are_validated() -> Result<()> {
        let (root, _) = fixture();
        let path = root.join("destination.json");
        std::fs::write(&path, br#"{"url":"https://example.invalid/image.png","method":"PUT","response":{"type":"header","name":"Location"}}"#)?;
        let profile = UploadDestination::load(&path)?;
        assert!(matches!(profile.method, UploadMethod::Put));
        profile.save_at(&path)?;
        assert!(matches!(
            UploadDestination::load(&path)?.method,
            UploadMethod::Put
        ));
        std::fs::write(
            &path,
            br#"{"url":"https://example.invalid/upload","json_pointer":"/url"}"#,
        )?;
        let legacy = UploadDestination::load(&path)?;
        assert!(matches!(legacy.method, UploadMethod::Post));
        assert_eq!(legacy.field, "file");
        for method in ["GET", "DELETE", "PATCH", "put"] {
            std::fs::write(
                &path,
                format!(r#"{{"url":"https://example.invalid/upload","method":"{method}"}}"#),
            )?;
            assert!(UploadDestination::load(&path).is_err());
        }
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    #[ignore = "local loopback PUT tests with synthetic PNG; no external connections"]
    fn upload_put_streams_exact_png_with_creation_precondition_and_preserves_errors() {
        let (root, path) = fixture();
        let original = std::fs::read(&path).unwrap();
        let (url, task) = server(
            response(r#"{"result":{"url":"https://example.invalid/put.png"}}"#),
            None,
            Duration::ZERO,
        );
        let mut config = destination(url);
        config.method = UploadMethod::Put;
        let started = std::time::Instant::now();
        assert_eq!(
            upload_png(&config, &path, UploadCancellation::default()).unwrap(),
            "https://example.invalid/put.png"
        );
        eprintln!(
            "Synthetic PNG PUT upload including runtime teardown: {:?}",
            started.elapsed()
        );
        let request = task.join().unwrap();
        let boundary = request
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
            .unwrap()
            + 4;
        let headers = String::from_utf8_lossy(&request[..boundary]).to_ascii_lowercase();
        assert!(headers.starts_with("put /upload http/1.1"));
        assert!(headers.contains("content-type: image/png\r\n"));
        assert!(headers.contains("if-none-match: *\r\n"));
        assert!(!headers.contains("multipart"));
        assert_eq!(&request[boundary..], original.as_slice());
        let (url, task) = server(
            b"HTTP/1.1 412 Precondition Failed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_vec(),
            None,
            Duration::ZERO,
        );
        config.url = url;
        assert!(
            upload_png(&config, &path, UploadCancellation::default())
                .unwrap_err()
                .to_string()
                .contains("412")
        );
        task.join().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn credential_references_are_bound_to_https_origin_and_never_contain_tokens() -> Result<()> {
        let mut config = destination("http://example.invalid/upload".into());
        config.credential = Some("main".into());
        assert!(config.validate().is_err());
        config.url = "https://example.invalid/upload".into();
        if cfg!(target_os = "windows") {
            config.validate()?;
            assert_eq!(
                config.credential_target().as_deref(),
                Some("Screenshots/upload/https://example.invalid/main")
            );
            let original = config.credential_target();
            config.url = "https://other.invalid/upload".into();
            assert_ne!(config.credential_target(), original);
            config.url = "https://example.invalid:444/upload".into();
            assert_ne!(config.credential_target(), original);
        } else {
            assert!(config.validate().is_err());
        }
        for name in ["", "../../other", "with spaces", "name\0", &"x".repeat(65)] {
            config.credential = Some(name.into());
            assert!(config.validate().is_err());
        }
        Ok(())
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "checks a unique nonexistent credential and verifies no connection to a loopback listener"]
    fn missing_bearer_credential_prevents_any_request() {
        let (root, path) = fixture();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut config = destination(format!("https://{}/upload", listener.local_addr().unwrap()));
        config.credential = Some(format!("missing-{}", uuid::Uuid::new_v4()));
        for authentication in [
            None,
            Some(UploadAuthentication::ApiKey),
            Some(UploadAuthentication::Basic),
        ] {
            config.authentication = authentication;
            let error = upload_png(&config, &path, UploadCancellation::default()).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("No readable Windows upload credential")
            );
            assert_eq!(
                listener.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn api_key_profiles_require_credentials_and_round_trip_without_secrets() -> Result<()> {
        let mut config = destination("https://example.invalid/upload".into());
        config.authentication = Some(UploadAuthentication::ApiKey);
        assert!(config.validate().is_err());
        config.credential = Some("main".into());
        if cfg!(target_os = "windows") {
            config.validate()?;
            let target = config.credential_target();
            let (root, _) = fixture();
            let path = root.join("destination.json");
            config.save_at(&path)?;
            let loaded = UploadDestination::load(&path)?;
            assert_eq!(loaded.authentication.unwrap().header_name(), "x-api-key");
            assert_eq!(loaded.credential_target(), target);
            config.authentication = None;
            assert_eq!(
                config.authentication.unwrap_or_default().header_name(),
                "authorization"
            );
            assert_eq!(config.credential_target(), target);
            std::fs::remove_dir_all(root)?;
        }
        config.url = "http://example.invalid/upload".into();
        config.allow_http = true;
        assert!(config.validate().is_err());
        Ok(())
    }

    #[test]
    fn basic_profiles_require_an_os_reference_and_persist_only_the_mode() -> Result<()> {
        let mut config = destination("https://example.invalid/upload".into());
        config.authentication = Some(UploadAuthentication::Basic);
        assert!(config.validate().is_err());
        config.credential = Some("account".into());
        if cfg!(target_os = "windows") {
            let (root, _) = fixture();
            let path = root.join("destination.json");
            config.save_at(&path)?;
            let loaded = UploadDestination::load(&path)?;
            assert!(matches!(
                loaded.authentication,
                Some(UploadAuthentication::Basic)
            ));
            assert_eq!(loaded.credential_target(), config.credential_target());
            std::fs::remove_dir_all(root)?;
        }
        config.url = "http://example.invalid/upload".into();
        config.allow_http = true;
        assert!(config.validate().is_err());
        Ok(())
    }

    #[test]
    fn native_credential_attachment_validates_without_storing_secrets() -> Result<()> {
        let destination = UploadDestination::manual(
            "https://example.invalid/upload".into(),
            UploadMethod::Post,
            "file".into(),
            None,
            false,
        )?;
        let attached = destination
            .clone()
            .with_authentication("main".into(), UploadAuthentication::ApiKey);
        if cfg!(target_os = "windows") {
            let attached = attached?;
            assert_eq!(attached.credential_reference(), Some("main"));
            assert_eq!(attached.authentication(), Some(UploadAuthentication::ApiKey));
            assert_eq!(
                attached.credential_target().as_deref(),
                Some("Screenshots/upload/https://example.invalid/main")
            );
            let root = std::env::temp_dir().join(format!("native-cred-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root)?;
            let path = root.join("destination.json");
            attached.save_at(&path)?;
            let text = String::from_utf8(std::fs::read(&path)?)?;
            assert!(text.contains("main"));
            assert!(text.contains("api_key"));
            assert!(!text.contains("secret"));
            std::fs::remove_dir_all(root)?;
        } else {
            assert!(attached.is_err());
        }
        assert!(
            UploadDestination::manual(
                "http://example.invalid/upload".into(),
                UploadMethod::Post,
                "file".into(),
                None,
                true,
            )?
            .with_authentication("main".into(), UploadAuthentication::Bearer)
            .is_err()
        );
        for secret in ["", "two words", "bad\r\ntoken"] {
            let Err(error) = destination.clone().attach_credential(
                "main".into(),
                UploadAuthentication::Bearer,
                String::new(),
                secret.into(),
            ) else {
                panic!("invalid secret must be rejected");
            };
            let error = error.to_string();
            if !secret.is_empty() {
                assert!(!error.contains(secret), "{error}");
            }
        }
        Ok(())
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "creates and deletes one uniquely named synthetic Windows credential; no real secrets or networking"]
    fn attach_credential_writes_an_os_secret_without_embedding_it() -> Result<()> {
        use windows_sys::Win32::Security::Credentials::*;
        let name = format!("test-{}", uuid::Uuid::new_v4());
        let destination = UploadDestination::manual(
            "https://example.invalid/upload".into(),
            UploadMethod::Post,
            "file".into(),
            None,
            false,
        )?
        .attach_credential(
            name,
            UploadAuthentication::Bearer,
            String::new(),
            "synthetic-token_123.abc==".into(),
        )?;
        let target = destination.credential_target().unwrap();
        struct Cleanup(Vec<u16>);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                unsafe {
                    CredDeleteW(self.0.as_ptr(), CRED_TYPE_GENERIC, 0);
                }
            }
        }
        let _cleanup = Cleanup(target.encode_utf16().chain(Some(0)).collect());
        let text = String::from_utf8(serde_json::to_vec(&destination)?)?;
        assert!(!text.contains("synthetic-token"));
        assert_eq!(
            crate::upload_credentials::read_header(&target, UploadAuthentication::Bearer)?
                .to_str()
                .unwrap(),
            "Bearer synthetic-token_123.abc=="
        );
        Ok(())
    }

    #[test]
    fn request_url_profiles_require_put_and_preserve_explicit_mode() -> Result<()> {
        let mut config = destination("https://example.invalid/images/a.png".into());
        config.json_pointer.clear();
        config.response = Some(UploadResponse::RequestUrl);
        assert!(config.validate().is_err());
        config.method = UploadMethod::Put;
        config.validate()?;
        let (root, _) = fixture();
        let path = root.join("destination.json");
        config.save_at(&path)?;
        assert!(matches!(
            UploadDestination::load(&path)?.response,
            Some(UploadResponse::RequestUrl)
        ));
        config.json_pointer = "/url".into();
        assert!(config.validate().is_err());
        config.json_pointer.clear();
        config.url = format!("https://example.invalid/{}", "é".repeat(600));
        assert!(config.validate().is_err());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    #[ignore = "PUT status-only responses from local loopback servers; synthetic PNG only"]
    fn request_url_mode_uses_only_confirmed_put_resource_urls() {
        let (root, path) = fixture();
        let original = std::fs::read(&path).unwrap();
        for (status, success) in [
            (200, true),
            (201, true),
            (204, true),
            (202, false),
            (206, false),
            (307, false),
            (412, false),
        ] {
            let reply=format!("HTTP/1.1 {status} Test\r\nLocation: https://example.invalid/not-the-resource\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes();
            let (url, task) = server(reply, None, Duration::ZERO);
            let mut config = destination(url.clone());
            config.method = UploadMethod::Put;
            config.json_pointer.clear();
            config.response = Some(UploadResponse::RequestUrl);
            let started = std::time::Instant::now();
            let result = upload_png(&config, &path, UploadCancellation::default());
            if success {
                assert_eq!(result.unwrap(), url);
                if status == 204 {
                    eprintln!(
                        "Synthetic PNG status-only PUT including teardown: {:?}",
                        started.elapsed()
                    );
                }
            } else {
                assert!(result.is_err());
            }
            let request = task.join().unwrap();
            assert!(request.starts_with(b"PUT /upload HTTP/1.1"));
        }
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn destination_validation_rejects_implicit_http_secrets_and_unbounded_settings() {
        let mut config = destination("https://example.invalid/upload".into());
        assert!(config.validate().is_ok());
        assert_eq!(config.origin(), "https://example.invalid");
        for url in [
            "file:///tmp/test",
            "https://user:pass@example.invalid/",
            "https://example.invalid/?token=secret",
            "https://example.invalid/#fragment",
        ] {
            config.url = url.into();
            assert!(config.validate().is_err());
        }
        config.url = "http://127.0.0.1/upload".into();
        config.allow_http = false;
        assert!(config.validate().is_err());
        config.allow_http = true;
        config.field = "x\r\nInjected".into();
        assert!(config.validate().is_err());
        config.field = "file".into();
        config.json_pointer = "/bad~2escape".into();
        assert!(config.validate().is_err());
        let (root, _) = fixture();
        let path = root.join("destination.json");
        std::fs::write(
            &path,
            br#"{"url":"https://example.invalid/upload","field":"file","json_pointer":"/url"}"#,
        )
        .unwrap();
        assert!(UploadDestination::load(&path).is_ok());
        std::fs::write(&path, br#"{"url":"https://example.invalid/upload","field":"file","json_pointer":"/url","authorization":"secret"}"#).unwrap();
        assert!(UploadDestination::load(&path).is_err());
        std::fs::write(&path, vec![b' '; 16385]).unwrap();
        assert!(UploadDestination::load(&path).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "uses a local loopback HTTP server and a throwaway 1×1 PNG; no external connections"]
    fn probe_upload_sends_a_throwaway_png_and_parses_the_returned_link() {
        let before = probe_temp_names();
        let (url, server) = server(
            response(r#"{"result":{"url":"https://example.invalid/share/probe"}}"#),
            None,
            Duration::ZERO,
        );
        let started = std::time::Instant::now();
        assert_eq!(
            probe_upload(&destination(url), UploadCancellation::default(), None).unwrap(),
            "https://example.invalid/share/probe"
        );
        eprintln!(
            "Synthetic 1×1 probe including runtime/client setup and teardown: {:?}",
            started.elapsed()
        );
        let request = server.join().unwrap();
        assert!(request.windows(8).any(|chunk| chunk == b"\x89PNG\r\n\x1a\n"));
        assert!(request.len() < 8192);
        assert_eq!(before, probe_temp_names());
    }

    #[test]
    #[ignore = "uses a local loopback HTTP server and synthetic PNG; no external connections"]
    fn upload_streams_png_and_rejects_unsafe_or_oversized_responses() {
        let (root, path) = fixture();
        let original = std::fs::read(&path).unwrap();
        let (url, server) = server(
            response(r#"{"result":{"url":"https://example.invalid/share/abc"}}"#),
            None,
            Duration::ZERO,
        );
        let started = std::time::Instant::now();
        assert_eq!(
            upload_png(&destination(url), &path, UploadCancellation::default()).unwrap(),
            "https://example.invalid/share/abc"
        );
        eprintln!(
            "Synthetic PNG loopback upload including runtime/client setup and teardown: {:?}",
            started.elapsed()
        );
        let request = server.join().unwrap();
        let (url, task) = self::server(
            response(r#"{"result":{"url":"https://example.invalid/share/progress"}}"#),
            None,
            Duration::ZERO,
        );
        let (reporter, mut progress) = upload_progress_channel();
        let started = std::time::Instant::now();
        assert_eq!(
            upload_png_with_progress(
                &destination(url),
                &path,
                UploadCancellation::default(),
                reporter
            )
            .unwrap(),
            "https://example.invalid/share/progress"
        );
        eprintln!(
            "Synthetic PNG loopback upload with progress including teardown: {:?}",
            started.elapsed()
        );
        task.join().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let final_progress = progress.next().await.unwrap();
            assert_eq!(
                (final_progress.prepared, final_progress.total),
                (original.len() as u64, original.len() as u64)
            );
            assert!(progress.next().await.is_none());
        });
        let text = String::from_utf8_lossy(&request);
        assert!(text.starts_with("POST /upload HTTP/1.1"));
        assert!(text.contains("name=\"file\""));
        assert!(text.contains("filename=\"screenshot.png\""));
        assert!(!text.contains("private-source-name.png"));
        assert!(
            request
                .windows(original.len())
                .any(|window| window == original)
        );
        for json in [
            r#"{"result":{"url":"javascript:alert(1)"}}"#,
            r#"{"result":{"url":"https://user:secret@example.invalid/"}}"#,
            r#"{"result":{}}"#,
            "not JSON",
        ] {
            let (url, task) = self::server(response(json), None, Duration::ZERO);
            assert!(upload_png(&destination(url), &path, UploadCancellation::default()).is_err());
            task.join().unwrap();
        }
        let (url, task) = self::server(
            b"HTTP/1.1 200 OK\r\nContent-Length: 1048577\r\nConnection: close\r\n\r\n".to_vec(),
            None,
            Duration::ZERO,
        );
        assert!(
            upload_png(&destination(url), &path, UploadCancellation::default())
                .unwrap_err()
                .to_string()
                .contains("1 MiB")
        );
        task.join().unwrap();
        let mut chunked =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
        for _ in 0..65 {
            chunked.extend_from_slice(b"4000\r\n");
            chunked.extend_from_slice(&[b'x'; 16384]);
            chunked.extend_from_slice(b"\r\n");
        }
        chunked.extend_from_slice(b"0\r\n\r\n");
        let (url, task) = self::server(chunked, None, Duration::ZERO);
        assert!(
            upload_png(&destination(url), &path, UploadCancellation::default())
                .unwrap_err()
                .to_string()
                .contains("1 MiB")
        );
        task.join().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "uses local loopback HTTP servers to verify redirect blocking and cancellation"]
    fn upload_does_not_follow_redirects_and_can_cancel_a_waiting_response() {
        let (root, path) = fixture();
        let forbidden = TcpListener::bind("127.0.0.1:0").unwrap();
        forbidden.set_nonblocking(true).unwrap();
        let redirect = format!(
            "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{}/forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            forbidden.local_addr().unwrap()
        );
        let (url, task) = server(redirect.into_bytes(), None, Duration::ZERO);
        assert!(
            upload_png(&destination(url), &path, UploadCancellation::default())
                .unwrap_err()
                .to_string()
                .contains("307")
        );
        task.join().unwrap();
        assert_eq!(
            forbidden.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        let (sent, received) = mpsc::sync_channel(1);
        let (url, task) = server(Vec::new(), Some(sent), Duration::from_millis(150));
        let cancel = UploadCancellation::default();
        let signal = cancel.clone();
        let image = path.clone();
        let worker = thread::spawn(move || upload_png(&destination(url), &image, signal));
        received.recv_timeout(Duration::from_secs(3)).unwrap();
        cancel.cancel();
        assert!(
            worker
                .join()
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("canceled")
        );
        task.join().unwrap();
        let cancel = UploadCancellation::default();
        cancel.cancel();
        assert!(
            upload_png(
                &destination("http://127.0.0.1:9/upload".into()),
                &path,
                cancel
            )
            .unwrap_err()
            .to_string()
            .contains("canceled")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
