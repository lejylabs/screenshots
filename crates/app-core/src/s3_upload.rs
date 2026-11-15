//! Explicit bounded single-object S3-compatible PUT uploads.

use anyhow::{Context, Result, ensure};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2_10::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{File, Metadata, OpenOptions},
    io::{Read, Seek},
    path::Path,
    time::{Duration, Instant},
};
use tokio::io::AsyncReadExt;
use zeroize::Zeroizing;

const MAX_PROFILE_BYTES: u64 = 16 * 1024;
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(60);

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct S3Profile {
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    #[serde(default)]
    pub key_prefix: String,
    pub credential: String,
}

impl S3Profile {
    pub fn load(path: &Path) -> Result<Self> {
        let mut file = open_regular(path)?;
        ensure!(
            file.metadata()?.len() <= MAX_PROFILE_BYTES,
            "S3 profile exceeds 16 KiB"
        );
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_PROFILE_BYTES + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= MAX_PROFILE_BYTES,
            "S3 profile exceeds 16 KiB"
        );
        let profile: Self = serde_json::from_slice(&bytes).context("Invalid S3 profile")?;
        profile.validate()?;
        Ok(profile)
    }

    pub fn validate(&self) -> Result<reqwest::Url> {
        ensure!(
            self.endpoint.len() <= 2048 && !self.endpoint.chars().any(char::is_control),
            "Invalid S3 endpoint"
        );
        let url = reqwest::Url::parse(&self.endpoint).context("Invalid S3 endpoint")?;
        ensure!(
            url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && self.endpoint == url.origin().ascii_serialization(),
            "S3 endpoint must be a canonical HTTPS origin without path, query or credentials"
        );
        ensure!(
            !self.region.is_empty()
                && self.region.len() <= 64
                && self
                    .region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "Invalid S3 region"
        );
        let bucket = self.bucket.as_bytes();
        ensure!(
            (3..=63).contains(&bucket.len())
                && bucket.first().is_some_and(u8::is_ascii_alphanumeric)
                && bucket.last().is_some_and(u8::is_ascii_alphanumeric)
                && bucket.iter().all(|b| b.is_ascii_lowercase()
                    || b.is_ascii_digit()
                    || matches!(*b, b'-' | b'.'))
                && !self.bucket.contains("..")
                && !self.bucket.contains(".-")
                && !self.bucket.contains("-.")
                && self.bucket.parse::<std::net::Ipv4Addr>().is_err(),
            "Invalid DNS-style S3 bucket name"
        );
        ensure!(
            self.key_prefix.len() <= 512
                && (self.key_prefix.is_empty()
                    || self.key_prefix.split('/').all(|part| !part.is_empty()
                        && part != "."
                        && part != ".."
                        && part
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b)))),
            "S3 key prefix must contain safe nonempty ASCII path segments"
        );
        ensure!(
            (1..=64).contains(&self.credential.len())
                && self
                    .credential
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
            "Invalid S3 credential reference"
        );
        Ok(url)
    }

    pub fn credential_target(&self) -> Result<String> {
        let url = self.validate()?;
        Ok(format!(
            "Screenshots/s3/{}/{}/{}",
            url.origin().ascii_serialization(),
            self.bucket,
            self.credential
        ))
    }
}

pub struct S3Credentials {
    pub access_key_id: Zeroizing<String>,
    pub secret_access_key: Zeroizing<String>,
}

impl S3Credentials {
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Borrowed<'a> {
            access_key_id: &'a str,
            secret_access_key: &'a str,
        }
        ensure!(
            bytes.len() <= 4096,
            "S3 credential input exceeds 4096 bytes"
        );
        let parsed: Borrowed<'_> = serde_json::from_slice(bytes)
            .map_err(|_| anyhow::anyhow!("Invalid S3 credential JSON"))?;
        let result = Self {
            access_key_id: Zeroizing::new(parsed.access_key_id.to_owned()),
            secret_access_key: Zeroizing::new(parsed.secret_access_key.to_owned()),
        };
        result.validate()?;
        Ok(result)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=128).contains(&self.access_key_id.len())
                && self
                    .access_key_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric()),
            "S3 access key ID must be 1–128 ASCII letters or digits"
        );
        ensure!(
            (1..=256).contains(&self.secret_access_key.len())
                && self
                    .secret_access_key
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b)),
            "S3 secret access key must be 1–256 ASCII letters, digits, plus, slash or equals"
        );
        Ok(())
    }
}

#[derive(Debug, serde::Serialize)]
pub struct S3Outcome {
    pub bucket: String,
    pub key: String,
    pub etag: Option<String>,
}

fn open_regular(path: &Path) -> Result<File> {
    ensure!(
        path.metadata()?.is_file(),
        "S3 input must be a regular file"
    );
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    ensure!(
        file.metadata()?.is_file(),
        "S3 input must be a regular file"
    );
    Ok(file)
}

fn same_metadata(a: &Metadata, b: &Metadata) -> bool {
    a.len() == b.len() && a.modified().ok() == b.modified().ok()
}

fn response_etag(headers: &reqwest::header::HeaderMap) -> Result<Option<String>> {
    ensure!(
        headers.get_all(reqwest::header::ETAG).iter().count() <= 1,
        "S3 returned multiple ETag headers"
    );
    headers
        .get(reqwest::header::ETAG)
        .map(|value| {
            let bytes = value.as_bytes();
            ensure!(
                bytes.len() <= 256 && bytes.iter().all(|byte| (0x20..=0x7e).contains(byte)),
                "S3 ETag exceeds its bound or is not printable ASCII"
            );
            Ok(std::str::from_utf8(bytes)?.to_owned())
        })
        .transpose()
}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}
fn hmac(key: &[u8], value: &str) -> Zeroizing<Vec<u8>> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(value.as_bytes());
    Zeroizing::new(mac.finalize().into_bytes().to_vec())
}

fn authorization(
    credentials: &S3Credentials,
    region: &str,
    date: &str,
    method: &str,
    path: &str,
    headers: &BTreeMap<String, String>,
    payload: &str,
) -> String {
    let canonical_headers = headers
        .iter()
        .map(|(name, value)| {
            format!(
                "{name}:{}\n",
                value.split_whitespace().collect::<Vec<_>>().join(" ")
            )
        })
        .collect::<String>();
    let signed = headers.keys().cloned().collect::<Vec<_>>().join(";");
    let canonical = format!("{method}\n{path}\n\n{canonical_headers}\n{signed}\n{payload}");
    let scope = format!("{}/{region}/s3/aws4_request", &date[..8]);
    let text = format!(
        "AWS4-HMAC-SHA256\n{date}\n{scope}\n{}",
        hex(Sha256::digest(canonical.as_bytes()))
    );
    let initial = Zeroizing::new(format!("AWS4{}", credentials.secret_access_key.as_str()));
    let day = hmac(initial.as_bytes(), &date[..8]);
    let region = hmac(&day, region);
    let service = hmac(&region, "s3");
    let key = hmac(&service, "aws4_request");
    format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope},SignedHeaders={signed},Signature={}",
        credentials.access_key_id.as_str(),
        hex(hmac(&key, &text))
    )
}

pub fn upload_s3_file(
    profile: &S3Profile,
    path: &Path,
    credentials: S3Credentials,
) -> Result<S3Outcome> {
    let endpoint = profile.validate()?;
    credentials.validate()?;
    upload_at(
        profile,
        path,
        credentials,
        endpoint,
        chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string(),
    )
}

fn upload_at(
    profile: &S3Profile,
    path: &Path,
    credentials: S3Credentials,
    mut endpoint: reqwest::Url,
    date: String,
) -> Result<S3Outcome> {
    let deadline = Instant::now() + DEADLINE;
    let mut file = open_regular(path)?;
    let metadata = file.metadata()?;
    ensure!(metadata.len() <= MAX_FILE_BYTES, "S3 input exceeds 64 MiB");
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut remaining = metadata.len();
    while remaining > 0 {
        ensure!(
            Instant::now() < deadline,
            "S3 upload exceeded its 60 second deadline before sending"
        );
        let wanted = buffer.len().min(remaining as usize);
        let count = file.read(&mut buffer[..wanted])?;
        ensure!(count > 0, "S3 input ended while hashing");
        hash.update(&buffer[..count]);
        remaining -= count as u64;
    }
    ensure!(
        same_metadata(&metadata, &file.metadata()?),
        "S3 input changed while hashing"
    );
    file.rewind()?;
    let payload_hash = hex(hash.finalize());
    let key = if profile.key_prefix.is_empty() {
        format!("{}.bin", uuid::Uuid::new_v4())
    } else {
        format!("{}/{}.bin", profile.key_prefix, uuid::Uuid::new_v4())
    };
    let path = format!("/{}/{key}", profile.bucket);
    endpoint.set_path(&path);
    ensure!(
        endpoint.path() == path,
        "S3 object path changed during URL construction"
    );
    let host = endpoint
        .origin()
        .ascii_serialization()
        .split_once("://")
        .expect("validated endpoint has scheme")
        .1
        .to_owned();
    let headers = BTreeMap::from([
        ("content-type".into(), "application/octet-stream".into()),
        ("host".into(), host),
        ("if-none-match".into(), "*".into()),
        ("x-amz-content-sha256".into(), payload_hash.clone()),
        ("x-amz-date".into(), date.clone()),
    ]);
    let authorization = Zeroizing::new(authorization(
        &credentials,
        &profile.region,
        &date,
        "PUT",
        &path,
        &headers,
        &payload_hash,
    ));
    let mut auth = reqwest::header::HeaderValue::from_str(&authorization)?;
    auth.set_sensitive(true);
    drop(credentials);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()?;
    let result = runtime.block_on(async {
        let remaining = deadline.saturating_duration_since(Instant::now());
        ensure!(
            !remaining.is_zero(),
            "S3 upload exceeded its 60 second deadline before sending"
        );
        tokio::time::timeout(remaining, async {
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .connect_timeout(Duration::from_secs(5))
                .timeout(remaining)
                .http1_only()
                .pool_max_idle_per_host(0)
                .no_proxy()
                .no_gzip()
                .no_brotli()
                .no_deflate()
                .no_zstd()
                .build()?;
            let mut file = tokio::fs::File::from_std(file);
            file.set_max_buf_size(64 * 1024);
            let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::with_capacity(
                file.take(metadata.len()),
                64 * 1024,
            ));
            let mut request = client
                .put(endpoint)
                .header(reqwest::header::AUTHORIZATION, auth)
                .header(reqwest::header::CONTENT_LENGTH, metadata.len())
                .header(reqwest::header::ACCEPT_ENCODING, "identity")
                .body(body);
            for (name, value) in headers {
                request = request.header(name, value);
            }
            let mut response = request
                .send()
                .await
                .map_err(|error| error.without_url())
                .context("S3 request failed; the remote object may exist and no retry was attempted")?;
            ensure!(
                response.status().as_u16() == 200,
                "S3 returned HTTP {}; the remote object may exist and no retry was attempted",
                response.status().as_u16()
            );
            ensure!(
                response.content_length().is_none_or(|length| length <= 1024 * 1024),
                "S3 response exceeds 1 MiB; the object may exist"
            );
            let etag = response_etag(response.headers())?;
            let mut received = 0usize;
            while let Some(chunk) = response.chunk().await.map_err(|error| error.without_url())? {
                received += chunk.len();
                ensure!(received <= 1024 * 1024, "S3 response exceeds 1 MiB; the object may exist");
            }
            Ok::<_, anyhow::Error>(S3Outcome { bucket: profile.bucket.clone(), key, etag })
        })
        .await
        .context("S3 upload exceeded its 60 second deadline; the remote object may exist and no retry was attempted")?
    });
    result.context("S3 upload did not return a confirmed result; the remote object may exist and no retry was attempted")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, Write};

    fn profile() -> S3Profile {
        S3Profile {
            endpoint: "https://example.invalid".into(),
            region: "auto".into(),
            bucket: "fixture-bucket".into(),
            key_prefix: "safe/path".into(),
            credential: "main".into(),
        }
    }
    fn credentials() -> S3Credentials {
        S3Credentials::from_json(br#"{"access_key_id":"AKIAIOSFODNN7EXAMPLE","secret_access_key":"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"}"#).unwrap()
    }
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("screenshots-s3-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn aws_published_put_vector_matches_exact_authorization_signature() {
        let payload = "44ce7dd67c959e0d3524ffac1771dfbba87d2b6b4b4e99e42034a8b803f8b072";
        let headers = BTreeMap::from([
            ("date".into(), "Fri, 24 May 2013 00:00:00 GMT".into()),
            ("host".into(), "examplebucket.s3.amazonaws.com".into()),
            ("x-amz-content-sha256".into(), payload.into()),
            ("x-amz-date".into(), "20130524T000000Z".into()),
            ("x-amz-storage-class".into(), "REDUCED_REDUNDANCY".into()),
        ]);
        let result = authorization(
            &credentials(),
            "us-east-1",
            "20130524T000000Z",
            "PUT",
            "/test%24file.text",
            &headers,
            payload,
        );
        assert!(
            result.ends_with(
                "Signature=98ad721746da40c64f1a55b78f14c238d841ea1380cd77a1b5971af0ece108bd"
            ),
            "{result}"
        );
    }

    #[test]
    fn profiles_and_secrets_are_strict_and_scoped_without_networking() {
        let p = profile();
        assert_eq!(
            p.credential_target().unwrap(),
            "Screenshots/s3/https://example.invalid/fixture-bucket/main"
        );
        for endpoint in [
            "http://example.invalid",
            "https://example.invalid/",
            "https://user:secret@example.invalid",
            "https://example.invalid/path",
            "https://example.invalid?x=y",
            "https://EXAMPLE.invalid",
        ] {
            let mut p = profile();
            p.endpoint = endpoint.into();
            assert!(p.validate().is_err(), "{endpoint}");
        }
        for bucket in [
            "a",
            "ABC",
            "-abc",
            "abc-",
            "abc..def",
            "abc.-def",
            "abc-.def",
            "127.0.0.1",
        ] {
            let mut p = profile();
            p.bucket = bucket.into();
            assert!(p.validate().is_err(), "{bucket}");
        }
        for prefix in [
            "/a", "a/", "a//b", ".", "a/../b", "a b", "a%20b", "ü", "{key}",
        ] {
            let mut p = profile();
            p.key_prefix = prefix.into();
            assert!(p.validate().is_err(), "{prefix}");
        }
        for text in [
            b"{}".as_slice(),
            br#"{"access_key_id":"A","secret_access_key":"B","token":"C"}"#,
            br#"{"access_key_id":"A","access_key_id":"B","secret_access_key":"C"}"#,
            br#"{"access_key_id":"A","secret_access_key":"has space"}"#,
        ] {
            assert!(S3Credentials::from_json(text).is_err());
        }
        assert!(S3Credentials::from_json(&vec![b' '; 4097]).is_err());
        let fixture = Fixture::new();
        let path = fixture.0.join("profile.json");
        std::fs::write(&path,br#"{"endpoint":"https://example.invalid","region":"auto","bucket":"abc","credential":"main","unknown":1}"#).unwrap();
        assert!(S3Profile::load(&path).is_err());
        std::fs::write(&path, vec![b' '; 16385]).unwrap();
        assert!(S3Profile::load(&path).is_err());
        assert!(S3Profile::load(&fixture.0).is_err());
    }

    #[test]
    fn file_admission_is_bounded_and_preserves_source() {
        let fixture = Fixture::new();
        let path = fixture.0.join("too-large");
        File::create(&path)
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();
        let error = upload_s3_file(&profile(), &path, credentials()).unwrap_err();
        assert!(error.to_string().contains("64 MiB"));
        assert_eq!(path.metadata().unwrap().len(), MAX_FILE_BYTES + 1);
        assert!(upload_s3_file(&profile(), &fixture.0, credentials()).is_err());
    }

    #[test]
    fn response_etag_rejects_duplicates_and_non_ascii() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::ETAG,
            reqwest::header::HeaderValue::from_static("\"etag\""),
        );
        assert_eq!(
            response_etag(&headers).unwrap().as_deref(),
            Some("\"etag\"")
        );
        headers.append(
            reqwest::header::ETAG,
            reqwest::header::HeaderValue::from_static("other"),
        );
        assert!(response_etag(&headers).is_err());
        headers.clear();
        headers.insert(
            reqwest::header::ETAG,
            reqwest::header::HeaderValue::from_bytes(&[0xff]).unwrap(),
        );
        assert!(response_etag(&headers).is_err());
        headers.clear();
        headers.insert(
            reqwest::header::ETAG,
            reqwest::header::HeaderValue::from_str(&"x".repeat(257)).unwrap(),
        );
        assert!(response_etag(&headers).is_err());
    }

    fn server(
        status: u16,
        body: &'static [u8],
    ) -> (reqwest::Url, std::thread::JoinHandle<Vec<u8>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url =
            reqwest::Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let worker = std::thread::spawn(move || {
            let started = Instant::now();
            let (mut socket, _) = loop {
                match listener.accept() {
                    Ok(value) => break value,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            && started.elapsed() < Duration::from_secs(5) =>
                    {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = std::io::BufReader::new(&mut socket);
            let mut request = Vec::new();
            let mut length = 0;
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                request.extend_from_slice(line.as_bytes());
                assert!(request.len() < 16384);
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
            }
            assert!(length < 16384);
            let mut bytes = vec![0; length];
            reader.read_exact(&mut bytes).unwrap();
            request.extend(bytes);
            drop(reader);
            write!(socket,"HTTP/1.1 {status} Result\r\nContent-Length: {}\r\nETag: \"fixture-etag\"\r\nLocation: http://127.0.0.1:1/never\r\nConnection: close\r\n\r\n",body.len()).unwrap();
            socket.write_all(body).unwrap();
            request
        });
        (url, worker)
    }

    #[test]
    fn loopback_put_has_exact_signature_body_precondition_and_no_redirect_retry() {
        let fixture = Fixture::new();
        let path = fixture.0.join("source-private-name");
        let payload = b"synthetic signed source\0with bytes";
        std::fs::write(&path, payload).unwrap();
        for status in [200, 302, 409, 412, 202] {
            let (url, worker) = server(status, b"");
            let expected_host =
                url.host_str().unwrap().to_owned() + ":" + &url.port().unwrap().to_string();
            let result = upload_at(
                &profile(),
                &path,
                credentials(),
                url,
                "20261115T120000Z".into(),
            );
            let request = worker.join().unwrap();
            let split = request.windows(4).position(|p| p == b"\r\n\r\n").unwrap() + 4;
            assert_eq!(&request[split..], payload);
            let text = std::str::from_utf8(&request[..split]).unwrap();
            let line = text.lines().next().unwrap();
            let uri = line.split(' ').nth(1).unwrap();
            assert!(uri.starts_with("/fixture-bucket/safe/path/"));
            assert!(uri.ends_with(".bin"));
            assert!(!uri.contains("source-private-name"));
            let headers: BTreeMap<String, String> = BTreeMap::from([
                ("content-type".into(), "application/octet-stream".into()),
                ("host".into(), expected_host),
                ("if-none-match".into(), "*".into()),
                ("x-amz-content-sha256".into(), hex(Sha256::digest(payload))),
                ("x-amz-date".into(), "20261115T120000Z".into()),
            ]);
            for (name, value) in &headers {
                assert!(
                    text.lines().any(|line| {
                        line.split_once(':')
                            .is_some_and(|(actual_name, actual_value)| {
                                actual_name.eq_ignore_ascii_case(name)
                                    && actual_value.trim() == value
                            })
                    }),
                    "missing signed header {name}"
                );
            }
            assert!(text.lines().any(|l| l.eq_ignore_ascii_case(&format!(
                "authorization: {}",
                authorization(
                    &credentials(),
                    "auto",
                    "20261115T120000Z",
                    "PUT",
                    uri,
                    &headers,
                    &hex(Sha256::digest(payload))
                )
            ))));
            if status == 200 {
                let outcome = result.unwrap();
                assert_eq!(outcome.etag.as_deref(), Some("\"fixture-etag\""));
                assert_eq!(outcome.bucket, "fixture-bucket");
            } else {
                assert!(result.is_err(), "{status}");
            }
            assert_eq!(std::fs::read(&path).unwrap(), payload);
        }
    }

    #[test]
    fn empty_file_upload_and_wire_response_limits_are_enforced() {
        let fixture = Fixture::new();
        let path = fixture.0.join("empty");
        std::fs::write(&path, []).unwrap();
        let (url, worker) = server(200, b"");
        let result = upload_at(
            &profile(),
            &path,
            credentials(),
            url,
            "20261115T120000Z".into(),
        );
        let request = worker.join().unwrap();
        assert!(result.is_ok());
        assert!(
            std::str::from_utf8(&request)
                .unwrap()
                .to_lowercase()
                .contains("content-length: 0")
        );
        for headers in [
            "Content-Length: 1048577\r\n",
            "Content-Length: 0\r\nETag: one\r\nETag: two\r\n",
            &format!("Content-Length: 0\r\nETag: {}\r\n", "x".repeat(257)),
            "Transfer-Encoding: chunked\r\n",
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url =
                reqwest::Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
            let headers = headers.to_owned();
            let server = std::thread::spawn(move || {
                let start = Instant::now();
                let (mut socket, _) = loop {
                    match listener.accept() {
                        Ok(pair) => break pair,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                && start.elapsed() < Duration::from_secs(5) =>
                        {
                            std::thread::sleep(Duration::from_millis(5))
                        }
                        Err(error) => panic!("{error}"),
                    }
                };
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = std::io::BufReader::new(&mut socket);
                loop {
                    let mut line = String::new();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                }
                drop(reader);
                let _ = write!(
                    socket,
                    "HTTP/1.1 200 OK\r\n{headers}Connection: close\r\n\r\n"
                );
                if headers.starts_with("Transfer-Encoding") {
                    let _ = write!(socket, "100001\r\n");
                    let chunk = [b'x'; 65536];
                    for _ in 0..16 {
                        if socket.write_all(&chunk).is_err() {
                            break;
                        }
                    }
                    let _ = socket.write_all(b"x\r\n0\r\n\r\n");
                }
            });
            assert!(
                upload_at(
                    &profile(),
                    &path,
                    credentials(),
                    url,
                    "20261115T120000Z".into()
                )
                .is_err()
            );
            server.join().unwrap();
        }
        assert_eq!(std::fs::metadata(path).unwrap().len(), 0);
    }
}
