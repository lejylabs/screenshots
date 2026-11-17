//! Private loopback fixtures only; no credentials, providers or real user files.
use super::*;
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Instant,
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "screenshots-upload-identity-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root.canonicalize().unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Copy)]
enum Kind {
    LegacyPng,
    Image(crate::CaptureFormatPolicy),
    File,
}
#[derive(Clone, Copy)]
enum Mode {
    Put,
    Multipart,
    Json,
}
enum Action {
    Unchanged,
    Mutate(PathBuf),
    Cancel(UploadCancellation),
    Signal(Arc<AtomicBool>),
    Early,
}

fn kinds() -> [Kind; 6] {
    [
        Kind::LegacyPng,
        Kind::Image(crate::CaptureFormatPolicy::Png),
        Kind::Image(crate::CaptureFormatPolicy::Jpeg {
            quality: crate::CaptureImageQuality::new(85).unwrap(),
        }),
        Kind::Image(crate::CaptureFormatPolicy::WebpLossless),
        Kind::Image(crate::CaptureFormatPolicy::WebpLossy {
            quality: crate::CaptureImageQuality::new(70).unwrap(),
        }),
        Kind::File,
    ]
}
fn input(f: &Fixture, kind: Kind) -> (PathBuf, Vec<u8>) {
    let path = f.0.join("private-source.input");
    let mut bytes = Vec::new();
    match kind {
        Kind::File => bytes.extend_from_slice(b"private arbitrary upload bytes"),
        Kind::LegacyPng | Kind::Image(_) => {
            let policy = match kind {
                Kind::Image(policy) => policy,
                _ => crate::CaptureFormatPolicy::Png,
            };
            let pixels = image::RgbaImage::from_fn(31, 17, |x, y| {
                image::Rgba([(x * 7) as u8, (y * 13) as u8, 79, 255])
            });
            policy.encode_to(&pixels, &mut bytes, &|| false).unwrap();
        }
    }
    std::fs::write(&path, &bytes).unwrap();
    (path, bytes)
}
fn headers(stream: &mut TcpStream) -> Vec<u8> {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut byte = [0];
    while !bytes.ends_with(b"\r\n\r\n") {
        assert!(bytes.len() < 8192);
        stream.read_exact(&mut byte).unwrap();
        bytes.push(byte[0]);
    }
    bytes
}
fn server(action: Action) -> (String, thread::JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/upload", listener.local_addr().unwrap());
    let worker = thread::spawn(move || {
        let start = Instant::now();
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && start.elapsed() < Duration::from_secs(3) =>
                {
                    thread::sleep(Duration::from_millis(1))
                }
                error => panic!("Private loopback accept failed: {error:?}"),
            }
        };
        let mut request = headers(&mut stream);
        let header = std::str::from_utf8(&request).unwrap();
        let length: usize = header
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().unwrap())
            })
            .unwrap();
        assert!(length <= 4 * 1024 * 1024);
        let drain = match action {
            Action::Unchanged => true,
            Action::Mutate(path) => {
                // This barrier is after admission/header inspection and before a 200.
                let mut file = OpenOptions::new().write(true).open(path).unwrap();
                let length = file.metadata().unwrap().len();
                file.write_all(b"changed").unwrap();
                file.set_len(length + 1).unwrap();
                file.sync_all().unwrap();
                true
            }
            Action::Cancel(cancel) => {
                cancel.cancel();
                false
            }
            Action::Signal(flag) => {
                flag.store(true, Ordering::Release);
                false
            }
            Action::Early => false,
        };
        if drain {
            let offset = request.len();
            request.resize(offset + length, 0);
            stream.read_exact(&mut request[offset..]).unwrap();
        }
        let json = br#"{"url":"https://example.invalid/private-fixture"}"#;
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            json.len()
        );
        let _ = stream.write_all(reply.as_bytes());
        let _ = stream.write_all(json);
        request
    });
    (url, worker)
}
fn destination(url: String, mode: Mode) -> UploadDestination {
    let value = UploadDestination::manual(
        url,
        if matches!(mode, Mode::Put) {
            UploadMethod::Put
        } else {
            UploadMethod::Post
        },
        "file".into(),
        Some("/url".into()),
        true,
    )
    .unwrap();
    if matches!(mode, Mode::Json) {
        value.with_json([("image", "{base64}")]).unwrap()
    } else {
        value
    }
}
fn upload(
    kind: Kind,
    destination: &UploadDestination,
    path: &Path,
    cancel: UploadCancellation,
) -> Result<UploadOutcome> {
    let (reporter, _receiver) = upload_progress_channel();
    match kind {
        Kind::LegacyPng => upload_png_with_progress(destination, path, cancel, reporter),
        Kind::Image(_) => upload_saved_image_with_progress(destination, path, cancel, reporter),
        Kind::File => upload_file(destination, path, cancel),
    }
}

#[test]
fn unchanged_sources_succeed_and_header_barrier_mutations_never_record_success() -> Result<()> {
    for kind in kinds() {
        for mode in [Mode::Put, Mode::Multipart, Mode::Json] {
            if matches!(kind, Kind::File) && matches!(mode, Mode::Json) {
                continue;
            }
            for mutate in [false, true] {
                let f = Fixture::new();
                let (path, original) = input(&f, kind);
                let (url, worker) = server(if mutate {
                    Action::Mutate(path.clone())
                } else {
                    Action::Unchanged
                });
                let destination = destination(url, mode);
                let result = upload(kind, &destination, &path, UploadCancellation::default());
                let request = worker.join().unwrap();
                let history = f.0.join("upload-history.sqlite3");
                if let Ok(outcome) = &result {
                    crate::record_uploaded_png(
                        &history,
                        &path,
                        &destination.origin(),
                        &outcome.url,
                    )?;
                }
                if mutate {
                    let error = format!("{:#}", result.unwrap_err());
                    assert!(
                        error.contains("changed") && error.contains("server may"),
                        "{error}"
                    );
                    assert!(!history.exists());
                    assert_eq!(std::fs::metadata(&path)?.len(), original.len() as u64 + 1);
                    assert!(std::fs::read(&path)?.starts_with(b"changed"));
                } else {
                    assert!(result.is_ok(), "{:?}", result.err());
                    assert!(history.exists());
                    assert_eq!(std::fs::read(&path)?, original);
                    let split = request
                        .windows(4)
                        .position(|bytes| bytes == b"\r\n\r\n")
                        .unwrap()
                        + 4;
                    if matches!(mode, Mode::Put) {
                        assert_eq!(&request[split..], original.as_slice());
                    }
                }
            }
        }
    }
    Ok(())
}

#[test]
fn loopback_cancel_signal_and_early_200_cannot_create_upload_history() -> Result<()> {
    for kind in [Kind::LegacyPng, kinds()[2]] {
        for event in 0..3 {
            let f = Fixture::new();
            let (path, original) = input(&f, kind);
            let flag = Arc::new(AtomicBool::new(false));
            let cancel = if event == 1 {
                UploadCancellation::with_atomic_flag(flag.clone())
            } else {
                UploadCancellation::default()
            };
            let _body_gate = (event == 2).then(upload_source::pause_next_body_for_test);
            let action = match event {
                0 => Action::Cancel(cancel.clone()),
                1 => Action::Signal(flag),
                _ => Action::Early,
            };
            let (url, worker) = server(action);
            let mut destination = destination(url, Mode::Put);
            if event == 2 {
                destination.response = Some(UploadResponse::RequestUrl);
                destination.json_pointer.clear();
            }
            let result = upload(kind, &destination, &path, cancel);
            worker.join().unwrap();
            let history = f.0.join("upload-history.sqlite3");
            if let Ok(outcome) = &result {
                crate::record_uploaded_png(&history, &path, &destination.origin(), &outcome.url)?;
            }
            assert!(result.is_err());
            assert!(!history.exists());
            assert_eq!(std::fs::read(&path)?, original);
        }
    }
    Ok(())
}

#[test]
fn resource_url_success_return_verifies_the_same_retained_source() -> Result<()> {
    for kind in [Kind::LegacyPng, kinds()[3]] {
        for mutate in [false, true] {
            let f = Fixture::new();
            let (path, original) = input(&f, kind);
            let (url, worker) = server(if mutate {
                Action::Mutate(path.clone())
            } else {
                Action::Unchanged
            });
            let mut destination = destination(url, Mode::Put);
            destination.response = Some(UploadResponse::RequestUrl);
            destination.json_pointer.clear();
            let result = upload(kind, &destination, &path, UploadCancellation::default());
            worker.join().unwrap();
            if mutate {
                let error = format!("{:#}", result.unwrap_err());
                assert!(
                    error.contains("changed") && error.contains("server may"),
                    "{error}"
                );
            } else {
                assert_eq!(result?.url, destination.url);
                assert_eq!(std::fs::read(&path)?, original);
            }
        }
    }
    Ok(())
}
