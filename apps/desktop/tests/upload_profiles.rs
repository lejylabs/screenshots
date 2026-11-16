use image::{Rgba, RgbaImage};
use serde_json::{Value, json};
use std::{
    fs::File,
    io::Read,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};

const COMMANDS: [&str; 3] = ["upload", "upload-images", "upload-file"];
const STATE_DIRS: [&str; 7] = [
    "home",
    "userprofile",
    "config",
    "data",
    "cache",
    "appdata",
    "localappdata",
];

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "screenshots-upload-profiles-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn command(&self, name: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command.arg(name);
        for (variable, directory) in [
            ("HOME", "home"),
            ("USERPROFILE", "userprofile"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_CACHE_HOME", "cache"),
            ("APPDATA", "appdata"),
            ("LOCALAPPDATA", "localappdata"),
        ] {
            command.env(variable, self.path(directory));
        }
        command
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env("NO_PROXY", "*")
            .env("no_proxy", "*");
        command
    }

    fn run(&self, command: &mut Command) -> Output {
        let tag = uuid::Uuid::new_v4();
        let stdout = self.path(&format!("{tag}.stdout"));
        let stderr = self.path(&format!("{tag}.stderr"));
        command
            .stdin(Stdio::null())
            .stdout(File::create(&stdout).unwrap())
            .stderr(File::create(&stderr).unwrap());
        let mut child = ChildGuard(command.spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(15);
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "Child exceeded 15-second deadline"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        Output {
            status,
            stdout: read_bounded(&stdout),
            stderr: read_bounded(&stderr),
        }
    }

    fn profile(&self, name: &str, value: &Value) -> PathBuf {
        let path = self.path(name);
        std::fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
        path
    }

    fn image(&self) -> PathBuf {
        let path = self.path("private-image.png");
        RgbaImage::from_pixel(3, 2, Rgba([31, 87, 140, 255]))
            .save(&path)
            .unwrap();
        path
    }

    fn assert_no_state(&self) {
        for directory in STATE_DIRS {
            assert!(!self.path(directory).exists(), "Created {directory}");
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn read_bounded(path: &Path) -> Vec<u8> {
    let mut bytes = Vec::new();
    File::open(path)
        .unwrap()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .unwrap();
    assert!(bytes.len() <= 1024 * 1024, "Test output exceeded 1 MiB");
    bytes
}

fn rejected(output: &Output) -> String {
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    String::from_utf8(output.stderr.clone()).unwrap()
}

fn sharex(url: &str) -> Value {
    json!({
        "Name": "Temporary profile only",
        "DestinationType": "ImageUploader",
        "RequestMethod": "PUT",
        "RequestURL": url,
        "Body": "Binary",
        "URL": "{response}"
    })
}

#[test]
fn extension_selects_the_parser_and_credentials_remain_native_json_only() {
    let f = Fixture::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let origin = format!("https://{}", listener.local_addr().unwrap());
    let url = format!("{origin}/upload");
    let native = json!({"url":url, "method":"PUT", "response":{"type":"text"}});
    let missing = f.path("missing.png");
    for (name, value, parsed) in [
        ("profile.sxcu", sharex(&url), true),
        ("profile.SXCU", sharex(&url), true),
        ("profile.sXcU", sharex(&url), true),
        ("native.json", native.clone(), true),
        ("native.other", native.clone(), true),
        ("native", native.clone(), true),
        ("native.SXCU", native, false),
        ("sharex.json", sharex(&url), false),
    ] {
        let profile = f.profile(name, &value);
        let original = std::fs::read(&profile).unwrap();
        for command in COMMANDS {
            let output = f.run(f.command(command).arg(&profile).arg(&missing));
            let error = rejected(&output);
            if parsed {
                let source_error = match command {
                    "upload" => "Could not open the saved PNG",
                    "upload-images" => "An upload input is unavailable",
                    _ => "Could not inspect the upload file",
                };
                assert!(error.contains(source_error), "{name}: {error}");
            } else {
                assert!(error.contains("Invalid"), "{name}: {error}");
            }
        }
        assert_eq!(std::fs::read(profile).unwrap(), original);
    }
    let profile = f.profile("credentials.sxcu", &sharex(&url));
    let output = f.run(f.command("credentials").arg("target").arg(&profile));
    assert!(rejected(&output).contains("Invalid upload configuration"));
    let profile = f.profile(
        "native-credentials.sxcu",
        &json!({"url":url, "credential":"synthetic-reference"}),
    );
    let output = f.run(f.command("credentials").arg("target").arg(profile));
    assert!(output.status.success(), "{:?}", output);
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        result["target"],
        format!("Screenshots/upload/{origin}/synthetic-reference")
    );
    assert!(listener.accept().is_err());
    f.assert_no_state();
}

#[test]
fn invalid_and_secret_bearing_profiles_fail_before_connecting() {
    let f = Fixture::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("https://{}/upload", listener.local_addr().unwrap());
    let image = f.image();
    let original_image = std::fs::read(&image).unwrap();
    let mut invalid = Vec::new();
    let mut value = sharex(&url);
    value["Headers"] = json!({"Authorization":"Bearer synthetic-secret-value"});
    invalid.push(value);
    let mut value = sharex(&url);
    value["Parameters"] = json!({"api_key":"synthetic-secret-value"});
    invalid.push(value);
    let mut value = sharex(&url);
    value["DestinationType"] = json!("TextUploader");
    invalid.push(value);
    let mut value = sharex(&url);
    value["RequestMethod"] = json!("GET");
    invalid.push(value);
    invalid.push(sharex(&url.replace("https:", "http:")));
    invalid.push(json!({}));
    for (index, value) in invalid.iter().enumerate() {
        let profile = f.profile(&format!("invalid-{index}.SXCU"), value);
        let original_profile = std::fs::read(&profile).unwrap();
        for command in COMMANDS {
            let output = f.run(f.command(command).arg(&profile).arg(&image));
            let error = rejected(&output);
            assert!(!error.contains("synthetic-secret-value"));
        }
        assert_eq!(std::fs::read(profile).unwrap(), original_profile);
    }
    let profile = f.profile(
        "image-json.sxcu",
        &json!({
            "DestinationType":"ImageUploader", "RequestURL":url,
            "Body":"JSON", "Data":{"image":"{base64}"}, "URL":"{response}"
        }),
    );
    let output = f.run(f.command("upload-file").arg(profile).arg(&image));
    assert!(rejected(&output).contains("not image JSON bodies"));
    assert!(listener.accept().is_err());
    assert_eq!(std::fs::read(image).unwrap(), original_image);
    f.assert_no_state();
}

#[test]
fn file_only_profiles_are_scoped_to_upload_file_and_reject_json_before_connecting() {
    let f = Fixture::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("https://{}/upload", listener.local_addr().unwrap());
    let image = f.image();
    for extension in ["sxcu", "SXCU"] {
        let mut value = sharex(&url);
        value["DestinationType"] = json!("fileuploader");
        let profile = f.profile(&format!("file-only.{extension}"), &value);
        for command in ["upload", "upload-images"] {
            let output = f.run(f.command(command).arg(&profile).arg(&image));
            assert!(rejected(&output).contains("must include ImageUploader"));
        }
        let output = f.run(
            f.command("upload-file")
                .arg(&profile)
                .arg(f.path("missing")),
        );
        assert!(rejected(&output).contains("Could not inspect the upload file"));
    }
    for flags in ["FileUploader", "ImageUploader, FileUploader"] {
        let mut value = sharex(&url);
        value["DestinationType"] = json!(flags);
        value["RequestMethod"] = json!("POST");
        value["Body"] = json!("JSON");
        value["Data"] = json!({"image":"{base64}"});
        let profile = f.profile("file-json.sxcu", &value);
        let output = f.run(f.command("upload-file").arg(profile).arg(&image));
        assert!(rejected(&output).contains("not image JSON bodies"));
    }
    let mut value = sharex(&url);
    value["DestinationType"] = json!("FileUploader");
    value["Headers"] = json!({"Authorization":"synthetic-secret-value"});
    let profile = f.profile("secret-file.sxcu", &value);
    let output = f.run(f.command("upload-file").arg(profile).arg(image));
    assert!(!rejected(&output).contains("synthetic-secret-value"));
    assert!(listener.accept().is_err());
    f.assert_no_state();
}

#[test]
fn unsupported_error_message_fails_before_connecting_without_echoing_its_contents() {
    let f = Fixture::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("https://{}/upload", listener.local_addr().unwrap());
    let source = f.image();
    let original = std::fs::read(&source).unwrap();
    for value in [
        json!("private-server-secret"),
        json!("{json:private_server_secret}"),
        json!({"private-server-secret":true}),
        json!(["private-server-secret"]),
        json!(false),
        json!(17),
    ] {
        let mut profile = sharex(&url);
        profile["ErrorMessage"] = value;
        let path = f.profile("unsupported.sxcu", &profile);
        let before = std::fs::read(&path).unwrap();
        for command in COMMANDS {
            let output = f.run(f.command(command).arg(&path).arg(&source));
            let error = rejected(&output);
            assert!(error.contains("ShareX ErrorMessage parsing is not supported"));
            assert!(!error.contains("private-server-secret"));
        }
        assert_eq!(std::fs::read(path).unwrap(), before);
    }
    assert!(listener.accept().is_err());
    assert_eq!(std::fs::read(source).unwrap(), original);
    f.assert_no_state();
}

// APFS rejects invalid UTF-8 names before the application can open them.
#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn non_utf8_profile_name_keeps_native_extension_selection() {
    use std::os::unix::ffi::OsStringExt;
    let f = Fixture::new();
    let profile =
        f.0.join(std::ffi::OsString::from_vec(b"profile-\xff.SXCU".to_vec()));
    std::fs::write(
        &profile,
        serde_json::to_vec(&sharex("https://127.0.0.1:1/upload")).unwrap(),
    )
    .unwrap();
    for command in COMMANDS {
        let output = f.run(f.command(command).arg(&profile).arg(f.path("missing")));
        let error = rejected(&output);
        assert!(!error.contains("Invalid ShareX profile"), "{error}");
    }
    f.assert_no_state();
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires Python3/OpenSSL for isolated loopback TLS; no system trust changes"]
fn sharex_profiles_upload_exact_bytes_and_batch_ndjson_over_loopback_tls() {
    let f = Fixture::new();
    let cert = f.path("localhost-cert.pem");
    let key = f.path("localhost-key.pem");
    let output = f.run(
        Command::new("openssl")
            .args([
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-sha256",
                "-nodes",
                "-days",
                "2",
                "-subj",
                "/CN=localhost",
                "-addext",
                "subjectAltName=IP:127.0.0.1,DNS:localhost",
                "-addext",
                "basicConstraints=critical,CA:FALSE",
                "-addext",
                "extendedKeyUsage=serverAuth",
                "-keyout",
            ])
            .arg(&key)
            .arg("-out")
            .arg(&cert),
    );
    assert!(output.status.success(), "{:?}", output);
    let server_script = f.path("server.py");
    std::fs::write(&server_script, TLS_SERVER).unwrap();
    let log = f.path("requests.jsonl");
    let errors = f.path("server.stderr");
    let mut server = ChildGuard(
        Command::new("python3")
            .arg(&server_script)
            .arg(&cert)
            .arg(&key)
            .stdin(Stdio::null())
            .stdout(File::create(&log).unwrap())
            .stderr(File::create(&errors).unwrap())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let port = loop {
        let bytes = read_bounded(&log);
        if let Some(line) = bytes.split(|byte| *byte == b'\n').next()
            && let Ok(ready) = serde_json::from_slice::<Value>(line)
        {
            break ready["port"].as_u64().unwrap();
        }
        assert!(server.0.try_wait().unwrap().is_none(), "TLS server exited");
        assert!(Instant::now() < deadline, "TLS server did not become ready");
        std::thread::sleep(Duration::from_millis(10));
    };
    let url = format!("https://127.0.0.1:{port}/upload");
    let image = f.image();
    let png = std::fs::read(&image).unwrap();
    let expected_batch = app_core::CapturedImage {
        display_id: String::new(),
        window_id: None,
        width: 3,
        height: 2,
        png_path: Default::default(),
        pixels: std::sync::Arc::new(image::open(&image).unwrap().into_rgba8()),
        elapsed_ms: 0,
        source_format: app_core::PixelFormat::Rgba8UnormSrgb,
        display_geometry: None,
    }
    .png_bytes()
    .unwrap();
    let file = f.path("private-source.bin");
    let binary = b"arbitrary\0binary\xff\xfe\r\nbytes";
    std::fs::write(&file, binary).unwrap();
    let mut expected = Vec::new();
    let mut request_count = 0;
    for extension in ["sxcu", "SXCU"] {
        let profile = f.profile(&format!("profile.{extension}"), &sharex(&url));
        let original = std::fs::read(&profile).unwrap();
        for command in COMMANDS {
            let source = if command == "upload-file" {
                &file
            } else {
                &image
            };
            let mut process = f.command(command);
            process
                .arg(&profile)
                .arg(source)
                .env("SSL_CERT_FILE", &cert);
            if command == "upload-images" {
                process.arg(source);
            }
            let output = f.run(&mut process);
            assert!(output.status.success(), "{command}: {:?}", output);
            assert!(output.stderr.is_empty());
            let results: Vec<Value> = output
                .stdout
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
                .map(|line| serde_json::from_slice(line).unwrap())
                .collect();
            let count = if command == "upload-images" { 2 } else { 1 };
            for index in 0..count {
                request_count += 1;
                let result_url = format!("https://example.invalid/result/{request_count}");
                assert_eq!(results[index]["url"], result_url);
                if command == "upload-images" {
                    assert_eq!(
                        results[index],
                        json!({
                            "type":"uploaded", "index":index + 1, "total":2,
                            "url":result_url, "thumbnail":null, "deletion":null
                        })
                    );
                } else if command == "upload-file" {
                    assert_eq!(
                        results[index],
                        json!({
                            "url":result_url, "thumbnail":null, "deletion":null
                        })
                    );
                } else {
                    assert_eq!(results[index], json!({"url":result_url}));
                }
                expected.push(match command {
                    "upload-file" => (binary.to_vec(), "application/octet-stream"),
                    "upload-images" => (expected_batch.clone(), "image/png"),
                    _ => (png.clone(), "image/png"),
                });
            }
            if command == "upload-images" {
                assert_eq!(results.len(), 3);
                assert_eq!(results[2], json!({"type":"complete", "uploaded":2}));
            } else {
                assert_eq!(results.len(), 1);
            }
        }
        assert_eq!(std::fs::read(profile).unwrap(), original);
    }
    let records: Vec<Value> = read_bounded(&log)
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .skip(1)
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    assert_eq!(records.len(), expected.len());
    for (record, (body, content_type)) in records.iter().zip(expected) {
        assert_eq!(record["method"], "PUT");
        assert_eq!(record["path"], "/upload");
        assert_eq!(record["headers"]["content-type"], content_type);
        assert_eq!(record["headers"]["if-none-match"], "*");
        let actual: Vec<u8> = record["body"]
            .as_array()
            .unwrap()
            .iter()
            .map(|byte| byte.as_u64().unwrap() as u8)
            .collect();
        assert_eq!(actual, body);
    }
    for (index, method) in ["PUT", "POST"].into_iter().enumerate() {
        let mut value = sharex(&url);
        value["DestinationType"] = json!("FileUploader");
        value["RequestMethod"] = json!(method);
        value["Body"] = json!(if method == "PUT" {
            "Binary"
        } else {
            "MultipartFormData"
        });
        value["FileFormName"] = json!("payload");
        let profile = f.profile(&format!("file-only-{index}.SXCU"), &value);
        let original = std::fs::read(&profile).unwrap();
        let output = f.run(
            f.command("upload-file")
                .arg(&profile)
                .arg(&file)
                .env("SSL_CERT_FILE", &cert),
        );
        assert!(output.status.success(), "{:?}", output);
        request_count += 1;
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            result["url"],
            format!("https://example.invalid/result/{request_count}")
        );
        let records: Vec<Value> = read_bounded(&log)
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .skip(1)
            .map(|line| serde_json::from_slice(line).unwrap())
            .collect();
        assert_eq!(records.len(), request_count);
        let record = records.last().unwrap();
        assert_eq!(record["method"], method);
        let actual: Vec<u8> = record["body"]
            .as_array()
            .unwrap()
            .iter()
            .map(|byte| byte.as_u64().unwrap() as u8)
            .collect();
        if method == "PUT" {
            assert_eq!(actual, binary);
            assert_eq!(
                record["headers"]["content-type"],
                "application/octet-stream"
            );
        } else {
            assert!(
                record["headers"]["content-type"]
                    .as_str()
                    .unwrap()
                    .starts_with("multipart/form-data; boundary=")
            );
            let text = String::from_utf8_lossy(&actual);
            assert!(text.contains("name=\"payload\"; filename=\"file.bin\""));
            assert!(
                text.contains("Content-Type: application/octet-stream")
                    || text.contains("content-type: application/octet-stream")
            );
            assert!(actual.windows(binary.len()).any(|window| window == binary));
            assert!(!text.contains("private-source"));
        }
        assert_eq!(std::fs::read(profile).unwrap(), original);
    }
    assert!(read_bounded(&errors).is_empty());
    assert_eq!(std::fs::read(image).unwrap(), png);
    assert_eq!(std::fs::read(file).unwrap(), binary);
    f.assert_no_state();
}

#[cfg(target_os = "linux")]
const TLS_SERVER: &str = r#"
import json, socket, ssl, sys, time
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain(sys.argv[1], sys.argv[2])
listener = socket.socket()
listener.bind(('127.0.0.1', 0))
listener.listen(1)
listener.settimeout(1)
print(json.dumps({'port': listener.getsockname()[1]}), flush=True)
deadline = time.monotonic() + 60
count = 0
while time.monotonic() < deadline:
    try:
        raw, _ = listener.accept()
    except socket.timeout:
        continue
    raw.settimeout(5)
    with context.wrap_socket(raw, server_side=True) as connection:
        request = connection.makefile('rb')
        line = request.readline(16385)
        assert 0 < len(line) <= 16384
        method, path, version = line.decode('ascii').strip().split()
        headers = {}
        header_bytes = len(line)
        while True:
            line = request.readline(16385)
            header_bytes += len(line)
            assert line and header_bytes <= 16384
            if line == b'\r\n':
                break
            key, value = line.decode('ascii').split(':', 1)
            headers[key.lower()] = value.strip()
        length = int(headers['content-length'])
        assert 0 <= length <= 1024 * 1024
        body = request.read(length)
        assert len(body) == length
        print(json.dumps({'method':method, 'path':path, 'headers':headers, 'body':list(body)}), flush=True)
        count += 1
        response = ('https://example.invalid/result/' + str(count)).encode('ascii')
        connection.sendall(b'HTTP/1.1 200 OK\r\nContent-Length: ' + str(len(response)).encode('ascii') + b'\r\nConnection: close\r\n\r\n' + response)
        request.close()
listener.close()
"#;
