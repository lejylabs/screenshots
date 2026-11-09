use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{io::Read, path::Path, sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

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
}

fn default_field() -> String {
    "file".into()
}

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "UPPERCASE")]
enum UploadMethod {
    #[default]
    Post,
    Put,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum UploadResponse {
    Text,
    Header { name: String },
}

impl UploadDestination {
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
        if let Some(response) = &self.response {
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
            "Upload URLs must have a host and no credentials, query or fragment; authenticated destinations are not supported yet"
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
    let url = destination.validate()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()?;
    runtime.block_on(async {
        tokio::select! {
            biased;
            _ = cancel.0.notified() => anyhow::bail!("Upload canceled. The server may already have received the file."),
            result = tokio::time::timeout(Duration::from_secs(60), transfer(destination, path, url)) => {
                result.context("Upload exceeded its 60 second deadline; the server may have received the file")?
            }
        }
    })
}

async fn transfer(
    destination: &UploadDestination,
    path: &Path,
    url: reqwest::Url,
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
    let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::with_capacity(
        file.take(length),
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
    let request = match destination.method {
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

fn validate_link(value: &str) -> Result<String> {
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

    fn fixture() -> (std::path::PathBuf, std::path::PathBuf) {
        let root =
            std::env::temp_dir().join(format!("screenshots-upload-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("private-source-name.png");
        image::RgbaImage::new(3, 2).save(&path).unwrap();
        (root, path)
    }
    fn destination(url: String) -> UploadDestination {
        UploadDestination {
            url,
            method: UploadMethod::Post,
            field: "file".into(),
            json_pointer: "/result/url".into(),
            response: None,
            allow_http: true,
        }
    }
    fn read_request(stream: &mut TcpStream) -> Vec<u8> {
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
