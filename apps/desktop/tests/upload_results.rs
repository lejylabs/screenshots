use std::{
    ffi::OsString,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

const URL: &str = "https://example.invalid/inert-upload-result";
const SECRET: &str = "DO_NOT_ECHO_RESULT_PAYLOAD";
fn sharex_result_capacity() -> usize {
    6 * (3 * 2048 + 4096 + 16 * (256 + 4096))
        + r#"{"success":false,"status":65535,"url":"","thumbnail":"","deletion":"","error_message":"","events":[]}"#.len()
        + 16 * r#"{"title":"","text":""},"#.len()
}
const STATE: [&str; 7] = [
    "HOME",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
];

#[test]
fn full_sxcu_results_format_links_and_refuse_failed_or_linkless_outcomes() {
    let f = Fixture::new();
    let mut value = serde_json::json!({"success":true,"status":201,"url":URL,"thumbnail":null,"deletion":null,"error_message":null,"events":[{"title":"Result","text":"explicit bounded output"}]});
    assert_eq!(
        success(f.run(&["format", "plain"], &serde_json::to_vec(&value).unwrap()))["text"],
        URL
    );
    value["success"] = false.into();
    value["error_message"] = SECRET.into();
    rejected(f.run(&["format", "plain"], &serde_json::to_vec(&value).unwrap()));
    value["success"] = true.into();
    value["url"] = serde_json::Value::Null;
    rejected(f.run(&["format", "plain"], &serde_json::to_vec(&value).unwrap()));
    f.no_state();
}

#[test]
fn full_sxcu_maximum_events_and_escaped_json_feed_format_and_qr() {
    let f = Fixture::new();
    let events = (0..16)
        .map(|_| {
            serde_json::json!({
                "title":"\u{0001}".repeat(256), "text":"\u{0002}".repeat(4096)
            })
        })
        .collect::<Vec<_>>();
    let mut value = serde_json::json!({"success":true,"status":201,"url":URL,
        "thumbnail":null,"deletion":null,"error_message":null,"events":events});
    let encoded = serde_json::to_vec(&value).unwrap();
    assert!(encoded.len() > 8192);
    assert_eq!(success(f.run(&["format", "plain"], &encoded))["text"], URL);
    let path = f.path("maximum-events.png");
    success(f.qr(&path, &encoded));
    let pixels = image::open(&path).unwrap().into_luma8();
    let mut decoder = quircs::Quirc::default();
    let codes = decoder
        .identify(
            pixels.width() as usize,
            pixels.height() as usize,
            pixels.as_raw(),
        )
        .map(|code| code.unwrap().decode().unwrap().payload)
        .collect::<Vec<_>>();
    assert_eq!(codes, vec![URL.as_bytes().to_vec()]);
    value["events"][0]["title"] = "x".repeat(257).into();
    rejected(f.run(&["format", "plain"], &serde_json::to_vec(&value).unwrap()));
    value["events"][0]["title"] = "x".into();
    value["events"][0]["text"] = "x".repeat(4097).into();
    rejected(f.run(&["format", "plain"], &serde_json::to_vec(&value).unwrap()));
    value["events"] =
        serde_json::to_value(vec![serde_json::json!({"title":"x","text":"x"}); 17]).unwrap();
    rejected(f.run(&["format", "plain"], &serde_json::to_vec(&value).unwrap()));
    let maximum = sharex_result_capacity();
    let long_url = format!(
        "https://example.invalid/{}",
        "x".repeat(2048 - "https://example.invalid/".len())
    );
    let maximum_value = serde_json::json!({"success":true,"status":299,
        "url":long_url,"thumbnail":long_url,"deletion":long_url,
        "error_message":"\u{0003}".repeat(4096),
        "events":vec![serde_json::json!({"title":"\u{0001}".repeat(256),"text":"\u{0002}".repeat(4096)});16]});
    let mut exact = serde_json::to_vec(&maximum_value).unwrap();
    assert!(exact.len() <= maximum);
    exact.resize(maximum, b' ');
    assert_eq!(
        success(f.run(&["format", "plain"], &exact))["text"],
        long_url
    );
    exact.push(b' ');
    rejected(f.run(&["format", "plain"], &exact));
    f.no_state();
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "screenshots-upload-results-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DBUS_SESSION_BUS_ADDRESS");
        for key in STATE {
            command.env(key, self.path(key));
        }
        command
    }
    fn spawn(&self, args: &[OsString]) -> Running {
        Running::new(self.command().arg("link").args(args).spawn().unwrap())
    }
    fn run(&self, args: &[&str], input: &[u8]) -> Output {
        let mut running = self.spawn(&args.iter().map(OsString::from).collect::<Vec<_>>());
        let broken_pipe = running.input(input);
        let output = running.finish();
        assert!(!broken_pipe || !output.status.success());
        output
    }
    fn qr(&self, destination: &Path, input: &[u8]) -> Output {
        let mut running = self.spawn(&["qr".into(), destination.into()]);
        let broken_pipe = running.input(input);
        let output = running.finish();
        assert!(!broken_pipe || !output.status.success());
        output
    }
    fn no_state(&self) {
        for key in STATE {
            assert!(!self.path(key).exists());
        }
        assert!(self.0.read_dir().unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".part")
        }));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn reader(mut stream: impl Read + Send + 'static) -> (JoinHandle<Vec<u8>>, Arc<AtomicUsize>) {
    let length = Arc::new(AtomicUsize::new(0));
    let observed = length.clone();
    let thread = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            let count = stream.read(&mut buffer).unwrap();
            if count == 0 {
                return bytes;
            }
            assert!(
                bytes.len() + count <= 65536,
                "Fixture output exceeded its cap"
            );
            bytes.extend_from_slice(&buffer[..count]);
            observed.store(bytes.len(), Ordering::Relaxed);
        }
    });
    (thread, length)
}

struct Running {
    child: Child,
    stdout: Option<JoinHandle<Vec<u8>>>,
    stderr: Option<JoinHandle<Vec<u8>>>,
    stdout_bytes: Arc<AtomicUsize>,
    stderr_bytes: Arc<AtomicUsize>,
}
impl Running {
    fn new(mut child: Child) -> Self {
        let (stdout, stdout_bytes) = match child.stdout.take() {
            Some(stdout) => reader(stdout),
            None => reader(std::io::empty()),
        };
        let (stderr, stderr_bytes) = reader(child.stderr.take().unwrap());
        Self {
            child,
            stdout: Some(stdout),
            stderr: Some(stderr),
            stdout_bytes,
            stderr_bytes,
        }
    }
    fn input(&mut self, input: &[u8]) -> bool {
        match self.child.stdin.take().unwrap().write_all(input) {
            Ok(()) => false,
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => true,
            Err(error) => panic!("Could not write fixture input: {error}"),
        }
    }
    fn finish(mut self) -> Output {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.child.try_wait().unwrap().is_none() {
            assert!(
                Instant::now() < deadline,
                "Link fixture exceeded its deadline"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let status = self.child.wait().unwrap();
        self.child.stdin.take();
        Output {
            status,
            stdout: self.stdout.take().unwrap().join().unwrap(),
            stderr: self.stderr.take().unwrap().join().unwrap(),
        }
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.child.stdin.take();
        for thread in [&mut self.stdout, &mut self.stderr] {
            if let Some(thread) = thread.take() {
                let _ = thread.join();
            }
        }
    }
}

fn object(url: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({"url":url})).unwrap()
}

fn success(output: Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert!(output.stdout.ends_with(b"\n"));
    serde_json::from_slice(&output.stdout).unwrap()
}

fn rejected(output: Output) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(!error.contains(SECRET), "Input payload appeared in error");
    assert!(
        !error.contains("example.invalid"),
        "Input URL appeared in error"
    );
}

#[test]
fn exact_formats_escape_labels_and_keep_canonical_query_fragment_and_percent_escapes() {
    let f = Fixture::new();
    let canonical = "http://example.invalid/a%20b?q=%22%27&token=one#frag";
    let markdown_url = "http://example.invalid/a%20b?q=%22%27&amp;token=one#frag";
    let input = object("HTTP://EXAMPLE.INVALID:80/a%20b?q=%22%27&token=one#frag");
    for (args, format, text) in [
        (vec!["format", "plain"], "plain", canonical.to_owned()),
        (
            vec!["format", "markdown"],
            "markdown",
            format!("[Screenshot](<{markdown_url}>)"),
        ),
        (
            vec!["format", "markdown", "--label", r#"A [B](C)! "D" & 'E' \ <F>_"#],
            "markdown",
            format!(r#"[A \[B\]\(C\)\! \"D\" \& \'E\' \\ \<F\>\_](<{markdown_url}>)"#),
        ),
        (
            vec!["format", "html", "--label", "A <B> \"C\" 'D' & E"],
            "html",
            "<a href=\"http://example.invalid/a%20b?q=%22%27&amp;token=one#frag\">A &lt;B&gt; &quot;C&quot; &#39;D&#39; &amp; E</a>".to_owned(),
        ),
        (
            vec!["format", "html"],
            "html",
            "<a href=\"http://example.invalid/a%20b?q=%22%27&amp;token=one#frag\">Screenshot</a>".to_owned(),
        ),
    ] {
        assert_eq!(
            success(f.run(&args, &input)),
            serde_json::json!({"format":format,"text":text})
        );
    }
    assert_eq!(
        success(f.run(
            &["format", "markdown", "--label", "Bild 🦊"],
            br#"{"deletion":null,"thumbnail":null,"url":"https://example.invalid/"}"#,
        )),
        serde_json::json!({"format":"markdown","text":"[Bild 🦊](<https://example.invalid/>)"})
    );
    assert_eq!(
        success(f.run(
            &["format", "plain"],
            &object("https://EXAMPLE.invalid/ä?x=ö")
        )),
        serde_json::json!({"format":"plain","text":"https://example.invalid/%C3%A4?x=%C3%B6"})
    );
    let optional = serde_json::to_vec(&serde_json::json!({
        "url":URL,"thumbnail":"https://example.invalid/thumb","deletion":"http://example.invalid/delete"
    })).unwrap();
    assert_eq!(success(f.run(&["format", "plain"], &optional))["text"], URL);
    assert_eq!(
        success(f.run(
            &["format", "markdown"],
            &object("https://example.invalid/?x=&amp;&y=&#123;&z=&#x41;")
        ))["text"],
        "[Screenshot](<https://example.invalid/?x=&amp;amp;&amp;y=&amp;#123;&amp;z=&amp;#x41;>)"
    );
    assert_eq!(
        success(f.run(
            &["format", "markdown"],
            &object("https://example.invalid/?q=\\path&x=%3C#part")
        ))["text"],
        "[Screenshot](<https://example.invalid/?q=%5Cpath&amp;x=%3C#part>)"
    );
    f.no_state();
}

#[test]
fn strict_json_url_and_byte_limits_fail_without_echoing_payloads() {
    let f = Fixture::new();
    let input = object(URL);
    let mut exact = input.clone();
    exact.resize(8192, b' ');
    assert_eq!(success(f.run(&["format", "plain"], &exact))["text"], URL);
    exact.push(b' ');
    let mut invalid = vec![
        Vec::new(),
        exact,
        b"\xef\xbb\xbf{\"url\":\"https://example.invalid/\"}".to_vec(),
        vec![0xff],
        format!(r#"{{"url":"{URL}","url":"https://example.invalid/{SECRET}"}}"#).into_bytes(),
        format!(r#"{{"url":"{URL}","unexpected":"{SECRET}"}}"#).into_bytes(),
        format!(r#"{{"url":{{"nested":"{SECRET}"}}}}"#).into_bytes(),
        format!(r#"[{{"url":"https://example.invalid/{SECRET}"}}]"#).into_bytes(),
        format!(r#"["https://example.invalid/{SECRET}",null,null]"#).into_bytes(),
        format!(r#"{{"type":"uploaded","index":1,"total":1,"url":"{URL}"}}"#).into_bytes(),
        format!("{{\"url\":\"{URL}\"}}\n{{\"url\":\"{URL}\"}}\n").into_bytes(),
        format!(r#"{{"url":"{URL}"}} {SECRET}"#).into_bytes(),
        format!(r#"{{"url":"https://example.invalid/{SECRET}""#).into_bytes(),
        b"{\"url\":null}".to_vec(),
        b"{\"url\":\"\"}".to_vec(),
        format!(r#"{{"thumbnail":"https://example.invalid/{SECRET}"}}"#).into_bytes(),
    ];
    for url in [
        format!("javascript:{SECRET}"),
        format!("https://user:{SECRET}@example.invalid/"),
        format!("https://example.invalid/{SECRET}\n"),
        format!(" https://example.invalid/{SECRET}"),
        format!("https://example.invalid/{SECRET}\u{2028}"),
        format!("https://example.invalid/{}", "x".repeat(2048)),
        format!("https://example.invalid/{}", "ä".repeat(600)),
    ] {
        invalid.push(object(&url));
    }
    for field in ["thumbnail", "deletion"] {
        invalid.push(format!(r#"{{"url":"{URL}","{field}":""}}"#).into_bytes());
        invalid.push(format!(r#"{{"url":"{URL}","{field}":"file:///{SECRET}"}}"#).into_bytes());
        invalid.push(format!(r#"{{"url":"{URL}","{field}":42}}"#).into_bytes());
        invalid.push(format!(r#"{{"url":"{URL}","{field}":null,"{field}":null}}"#).into_bytes());
    }
    for bytes in invalid {
        rejected(f.run(&["format", "plain"], &bytes));
    }
    let label = "x".repeat(256);
    assert_eq!(
        success(f.run(&["format", "markdown", "--label", &label], &input))["text"],
        format!("[{label}](<{URL}>)")
    );
    for label in [
        "".to_owned(),
        "x".repeat(257),
        format!("{SECRET}\n"),
        format!("{SECRET}\u{2029}"),
    ] {
        rejected(f.run(&["format", "html", "--label", &label], &input));
    }
    f.no_state();
}

#[test]
fn arguments_and_output_collisions_fail_before_reading_open_stdin() {
    let f = Fixture::new();
    for args in [
        vec![],
        vec!["format"],
        vec!["format", "PLAIN"],
        vec!["format", "plain", "--label", SECRET],
        vec!["format", "markdown", "--label"],
        vec!["format", "markdown", "--label", "x", "--label", "y"],
        vec!["format", "html", "--unknown", SECRET],
    ] {
        rejected(
            f.spawn(&args.into_iter().map(OsString::from).collect::<Vec<_>>())
                .finish(),
        );
    }
    let existing = f.path("existing.png");
    std::fs::write(&existing, b"preserved").unwrap();
    let directory = f.path("directory.png");
    std::fs::create_dir(&directory).unwrap();
    for destination in [&existing, &directory, &f.path("bad.jpg")] {
        rejected(f.spawn(&["qr".into(), destination.into()]).finish());
    }
    assert_eq!(std::fs::read(&existing).unwrap(), b"preserved");
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        rejected(
            f.spawn(&[
                "format".into(),
                "html".into(),
                "--label".into(),
                OsString::from_vec(vec![0xff]),
            ])
            .finish(),
        );
        let target = f.path("absent.png");
        let link = f.path("broken.png");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        rejected(f.spawn(&["qr".into(), link.as_os_str().into()]).finish());
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert!(!target.exists());
    }
    f.no_state();
}

#[test]
fn complete_object_waits_for_eof_and_oversized_open_pipe_is_rejected() {
    let f = Fixture::new();
    let mut running = f.spawn(&["format".into(), "plain".into()]);
    running
        .child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(&object(URL))
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert!(running.child.try_wait().unwrap().is_none());
    assert_eq!(running.stdout_bytes.load(Ordering::Relaxed), 0);
    assert_eq!(running.stderr_bytes.load(Ordering::Relaxed), 0);
    // Deliberately terminate the fixture; production has no stdin timeout.
    running.child.kill().unwrap();
    let stopped = running.finish();
    assert!(stopped.stdout.is_empty());
    let mut running = f.spawn(&["format".into(), "plain".into()]);
    let mut overflow = object(URL);
    overflow.resize(sharex_result_capacity() + 1, b' ');
    running
        .child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(&overflow)
        .unwrap();
    rejected(running.finish());
    f.no_state();
}

#[test]
fn qr_contains_canonical_url_with_opaque_pixels_quiet_zone_and_no_state() {
    let f = Fixture::new();
    let input = object("HTTPS://EXAMPLE.INVALID:443/inert%20link?x=1&y=2#part");
    let canonical = "https://example.invalid/inert%20link?x=1&y=2#part";
    let source = f.path("upload-result.json");
    std::fs::write(&source, &input).unwrap();
    let output = f.path("share.PNG");
    let metadata = success(f.qr(&output, &input));
    let pixels = image::open(&output).unwrap().into_rgba8();
    assert_eq!(
        metadata,
        serde_json::json!({"format":"png","width":pixels.width(),"height":pixels.height(),"error_correction":"M"})
    );
    assert_eq!(pixels.width(), pixels.height());
    assert!(pixels.width() <= 740);
    for (x, y, pixel) in pixels.enumerate_pixels() {
        assert!(pixel.0 == [0, 0, 0, 255] || pixel.0 == [255; 4]);
        if x < 16 || y < 16 || x >= pixels.width() - 16 || y >= pixels.height() - 16 {
            assert_eq!(pixel.0, [255; 4]);
        }
    }
    let grayscale = pixels.pixels().map(|p| p[0]).collect::<Vec<_>>();
    let mut decoder = quircs::Quirc::default();
    let codes = decoder
        .identify(
            pixels.width() as usize,
            pixels.height() as usize,
            &grayscale,
        )
        .map(|code| code.unwrap().decode().unwrap().payload)
        .collect::<Vec<_>>();
    assert_eq!(codes, vec![canonical.as_bytes().to_vec()]);
    let original = std::fs::read(&output).unwrap();
    rejected(f.qr(&output, &input));
    assert_eq!(std::fs::read(output).unwrap(), original);
    assert_eq!(std::fs::read(source).unwrap(), input);
    f.no_state();
}

#[test]
fn maximum_length_canonical_url_survives_qr_without_truncation() {
    let f = Fixture::new();
    let prefix = "https://example.invalid/";
    let url = format!("{prefix}{}", "z".repeat(2048 - prefix.len()));
    assert_eq!(url.len(), 2048);
    let output = f.path("maximum.png");
    success(f.qr(&output, &object(&url)));
    let pixels = image::open(output).unwrap().into_luma8();
    let mut decoder = quircs::Quirc::default();
    let codes = decoder
        .identify(
            pixels.width() as usize,
            pixels.height() as usize,
            pixels.as_raw(),
        )
        .map(|code| code.unwrap().decode().unwrap().payload)
        .collect::<Vec<_>>();
    assert_eq!(codes, vec![url.into_bytes()]);
    f.no_state();
}

#[test]
fn qr_destination_created_while_waiting_for_input_is_preserved() {
    let f = Fixture::new();
    let destination = f.path("raced.png");
    let mut running = f.spawn(&["qr".into(), destination.as_os_str().into()]);
    running
        .child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b" ")
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert!(running.child.try_wait().unwrap().is_none());
    assert_eq!(running.stdout_bytes.load(Ordering::Relaxed), 0);
    assert_eq!(running.stderr_bytes.load(Ordering::Relaxed), 0);
    std::fs::write(&destination, b"created after initial admission").unwrap();
    let broken_pipe = running.input(&object(URL));
    let result = running.finish();
    assert!(!broken_pipe || !result.status.success());
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert_eq!(
        std::fs::read(destination).unwrap(),
        b"created after initial admission"
    );
    f.no_state();
}

#[cfg(target_os = "linux")]
#[test]
fn stdout_failure_distinguishes_no_file_from_already_saved_qr() {
    let f = Fixture::new();
    let full = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let mut formatting = Running::new(
        f.command()
            .args(["link", "format", "plain"])
            .stdout(full.try_clone().unwrap())
            .spawn()
            .unwrap(),
    );
    assert!(!formatting.input(&object(URL)));
    let failed = formatting.finish();
    assert!(!failed.status.success());
    assert!(failed.stdout.is_empty());
    let message = String::from_utf8_lossy(&failed.stderr);
    assert!(message.contains("Could not write link result"), "{message}");
    assert!(!message.contains("saved"));
    assert_eq!(f.0.read_dir().unwrap().count(), 0);

    let destination = f.path("saved.png");
    let mut qr = Running::new(
        f.command()
            .args(["link", "qr"])
            .arg(&destination)
            .stdout(full)
            .spawn()
            .unwrap(),
    );
    assert!(!qr.input(&object(URL)));
    let failed = qr.finish();
    assert!(!failed.status.success());
    assert!(failed.stdout.is_empty());
    let message = String::from_utf8_lossy(&failed.stderr);
    assert!(message.contains("QR PNG was saved"), "{message}");
    assert!(!message.contains(URL));
    assert!(image::open(destination).is_ok());
    f.no_state();
}

struct Server {
    url: String,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Vec<Vec<u8>>>>,
}
impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/file", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(15);
            let mut bodies = Vec::new();
            while !stopping.load(Ordering::Relaxed) && Instant::now() < deadline {
                let (mut socket, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut header = Vec::new();
                while !header.ends_with(b"\r\n\r\n") {
                    assert!(header.len() < 16384);
                    let mut byte = [0];
                    socket.read_exact(&mut byte).unwrap();
                    header.push(byte[0]);
                }
                let header = std::str::from_utf8(&header).unwrap();
                assert!(header.starts_with("PUT /file HTTP/1.1\r\n"));
                let length = header
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|length| length.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                assert!(length <= 1024);
                let mut body = vec![0; length];
                socket.read_exact(&mut body).unwrap();
                bodies.push(body);
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{URL}",
                    URL.len()
                )
                .unwrap();
            }
            bodies
        });
        Self {
            url,
            stop,
            thread: Some(thread),
        }
    }
    fn finish(mut self) -> Vec<Vec<u8>> {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[test]
fn one_local_upload_can_feed_html_and_qr_without_another_request() {
    let f = Fixture::new();
    let server = Server::new();
    let source = f.path("fixture.bin");
    let bytes = b"local upload fixture\0\xff";
    std::fs::write(&source, bytes).unwrap();
    let profile = f.path("profile.json");
    let profile_bytes = serde_json::to_vec(&serde_json::json!({
        "url":server.url,"method":"PUT","allow_http":true,"response":{"type":"text"}
    }))
    .unwrap();
    std::fs::write(&profile, &profile_bytes).unwrap();
    let mut child = Running::new(
        f.command()
            .arg("upload-file")
            .arg(&profile)
            .arg(&source)
            .spawn()
            .unwrap(),
    );
    child.child.stdin.take();
    let uploaded = child.finish();
    assert!(
        uploaded.status.success(),
        "{}",
        String::from_utf8_lossy(&uploaded.stderr)
    );
    assert!(uploaded.stderr.is_empty());
    assert_eq!(
        success(f.run(
            &["format", "html", "--label", "Local fixture"],
            &uploaded.stdout
        )),
        serde_json::json!({"format":"html","text":format!("<a href=\"{URL}\">Local fixture</a>")})
    );
    let qr = f.path("link.png");
    success(f.qr(&qr, &uploaded.stdout));
    let pixels = image::open(qr).unwrap().into_luma8();
    let mut decoder = quircs::Quirc::default();
    let codes = decoder
        .identify(
            pixels.width() as usize,
            pixels.height() as usize,
            pixels.as_raw(),
        )
        .map(|code| code.unwrap().decode().unwrap().payload)
        .collect::<Vec<_>>();
    assert_eq!(codes, vec![URL.as_bytes().to_vec()]);
    assert_eq!(server.finish(), vec![bytes.to_vec()]);
    assert_eq!(std::fs::read(source).unwrap(), bytes);
    assert_eq!(std::fs::read(profile).unwrap(), profile_bytes);
    f.no_state();
}
