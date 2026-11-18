//! Explicit CLI printing. One isolated helper, no desktop startup or printer discovery.
use anyhow::{Context, Result, bail, ensure};
use app_core::PrintPaper;
#[cfg(unix)]
use std::process::Stdio;
use std::{
    ffi::OsString,
    io::{Read, Write},
    path::PathBuf,
    process::{Command, ExitStatus},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
#[cfg(target_os = "macos")]
#[path = "../../../crates/app-core/src/clipboard_input_budget.rs"]
mod image_budget;
#[path = "capture_worker_transport.rs"]
#[allow(dead_code)] // Reuse the bounded transport without unused-operation warnings.
mod transport;
#[cfg(unix)]
#[path = "capture_worker_unix.rs"]
#[allow(dead_code)]
mod unix;

const WORKER: &str = "__print-image-worker";
const HANDSHAKE: &[u8; 8] = b"PRINTv1\n";
const ACK: &[u8; 8] = b"ACCEPT1\n";
const DEADLINE: Duration = Duration::from_secs(60);
const CLEANUP: Duration = Duration::from_millis(250);
static BUSY: AtomicBool = AtomicBool::new(false);
static PENDING: Mutex<Option<Native>> = Mutex::new(None);
struct Admission;
impl Drop for Admission {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}

#[derive(Debug)]
struct Options {
    input: PathBuf,
    printer: String,
    paper: PrintPaper,
    #[cfg(unix)]
    server: Option<String>,
}
fn text(value: &OsString) -> Result<&str> {
    value.to_str().context("Print option must be UTF-8")
}
fn parse(args: &[OsString]) -> Result<Options> {
    ensure!(
        args.len() >= 5 && args.len() <= 7 && args.len() % 2 == 1,
        "Usage: screenshots print-image INPUT --printer NAME --paper a4|letter [--server HOST:PORT] (server required on Linux/macOS)"
    );
    ensure!(
        !args[0].is_empty() && args[0].as_encoded_bytes().len() <= 4096,
        "Invalid image path"
    );
    let mut printer = None;
    let mut paper = None;
    let mut server = None;
    for pair in args[1..].chunks_exact(2) {
        match text(&pair[0])? {
            "--printer" if printer.is_none() => {
                let value = text(&pair[1])?;
                ensure!(
                    !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control),
                    "Invalid printer name"
                );
                #[cfg(unix)]
                ensure!(
                    value.len() <= 127
                        && !value.starts_with('-')
                        && value
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte)),
                    "CUPS queue must use letters, digits, underscore, dot or hyphen; instances are unsupported"
                );
                printer = Some(value.to_owned());
            }
            "--paper" if paper.is_none() => {
                paper = Some(match text(&pair[1])? {
                    "a4" => PrintPaper::A4,
                    "letter" => PrintPaper::Letter,
                    _ => bail!("Paper must be a4 or letter"),
                });
            }
            "--server" if server.is_none() => {
                let value = text(&pair[1])?;
                validate_server(value)?;
                server = Some(value.to_owned());
            }
            _ => bail!("Unknown or duplicate print option"),
        }
    }
    #[cfg(unix)]
    ensure!(
        server.is_some(),
        "Printing requires an explicit CUPS --server HOST:PORT"
    );
    #[cfg(target_os = "windows")]
    ensure!(
        server.is_none(),
        "Windows uses the exact --printer queue; --server is unsupported"
    );
    Ok(Options {
        input: PathBuf::from(&args[0]),
        printer: printer.context("Printing requires --printer")?,
        paper: paper.context("Printing requires --paper")?,
        #[cfg(unix)]
        server,
    })
}
fn validate_server(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty() && value.len() <= 253,
        "Invalid CUPS server"
    );
    let (host, port) = value
        .rsplit_once(':')
        .context("CUPS server requires HOST:PORT")?;
    ensure!(
        host.parse::<std::net::Ipv4Addr>()
            .is_ok_and(|address| address.is_loopback())
            && port.parse::<u16>().is_ok_and(|port| port > 0),
        "CUPS server must be numeric loopback IPv4:PORT; configure network printers in that local spooler"
    );
    Ok(())
}
fn command(args: &[OsString]) -> Result<Command> {
    #[cfg(target_os = "linux")]
    let executable = PathBuf::from("/proc/self/exe");
    #[cfg(not(target_os = "linux"))]
    let executable = std::env::current_exe().context("Print worker executable is unavailable")?;
    let mut command = Command::new(executable);
    command.arg(WORKER).args(args).env_clear();
    #[cfg(target_os = "windows")]
    for key in ["SystemRoot", "WINDIR", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    #[cfg(unix)]
    command.env("LANG", "C").env("LC_ALL", "C");
    Ok(command)
}

#[cfg(unix)]
struct Native {
    child: std::process::Child,
    group: i32,
    reaped: bool,
    channel: Option<transport::Transport>,
    nonce: [u8; 16],
}
#[cfg(target_os = "windows")]
struct Native {
    child: app_core::windows_child::Child,
    job: Option<std::os::windows::io::OwnedHandle>,
    channel: Option<transport::Transport>,
    nonce: [u8; 16],
}

impl Native {
    fn spawn(mut command: Command, deadline: Instant) -> Result<Self> {
        let prepared = transport::Prepared::new(&mut command)?;
        #[cfg(unix)]
        {
            #[cfg(target_os = "linux")]
            use std::os::unix::process::CommandExt;
            command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            #[cfg(test)]
            if std::env::var_os("SCREENSHOTS_PRINT_TEST_TRACE").is_some() {
                command.stderr(Stdio::inherit());
            }
            #[cfg(target_os = "linux")]
            unsafe {
                command.pre_exec(move || {
                    let limit = libc::rlimit {
                        rlim_cur: 1024 * 1024 * 1024,
                        rlim_max: 1024 * 1024 * 1024,
                    };
                    if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            ensure!(Instant::now() < deadline, "Print startup deadline elapsed");
            let child = command
                .spawn()
                .context("Print worker could not start; nothing submitted")?;
            let group = child.id() as i32;
            let (channel, nonce) = prepared.parent();
            Ok(Self {
                child,
                group,
                reaped: false,
                channel: Some(channel),
                nonce,
            })
        }
        #[cfg(target_os = "windows")]
        {
            use app_core::windows_child::{self, Environment, Streams};
            use std::os::windows::io::AsHandle;
            let job = windows_child::job(Some(1), Some(1024 * 1024 * 1024))?;
            use std::os::windows::io::BorrowedHandle;
            let handles = prepared.child_handles();
            let inherited = handles
                .iter()
                .map(|handle| unsafe { BorrowedHandle::borrow_raw(*handle) })
                .collect::<Vec<_>>();
            let child = windows_child::spawn(
                &command,
                job.as_handle(),
                Streams::NULL,
                Environment::Explicit,
                &inherited,
            )?;
            let (channel, nonce) = prepared.parent();
            let mut native = Self {
                child,
                job: Some(job),
                channel: Some(channel),
                nonce,
            };
            native.child.resume(deadline)?;
            Ok(native)
        }
    }
    fn handshake(
        &mut self,
        check: &app_core::NativeCaptureCheck<'_>,
        deadline: Instant,
    ) -> Result<()> {
        let mut io = transport::ActiveIo {
            transport: self.channel.take().context("Print channel absent")?,
            deadline,
            check,
        };
        let result = io
            .write_all(HANDSHAKE)
            .and_then(|()| io.write_all(&self.nonce));
        self.channel = Some(io.transport);
        Ok(result?)
    }
    fn try_exit(&mut self) -> Result<Option<ExitStatus>> {
        #[cfg(unix)]
        {
            let exited = unix::exited_without_reaping(self.group, false)?;
            #[cfg(all(test, target_os = "macos"))]
            if std::env::var_os("SCREENSHOTS_PRINT_TEST_TRACE").is_some() {
                eprintln!(
                    "OWNED print mock: pid={}, exited={exited}, state={:?}",
                    self.group,
                    unix::process_state(self.group)
                );
            }
            if !exited {
                return Ok(None);
            }
            unix::signal_owned_group(self.group);
            let members = unix::live_members(self.group)?;
            #[cfg(all(test, target_os = "macos"))]
            if std::env::var_os("SCREENSHOTS_PRINT_TEST_TRACE").is_some() {
                eprintln!(
                    "OWNED print mock: pid={}, remaining={members:?}",
                    self.group
                );
            }
            if !members.is_empty() {
                return Ok(None);
            }
            let status = self.child.try_wait()?;
            self.reaped = status.is_some();
            Ok(status)
        }
        #[cfg(target_os = "windows")]
        Ok(self.child.try_wait()?)
    }
    fn stop(&mut self) {
        self.channel.take();
        #[cfg(unix)]
        if !self.reaped {
            unix::signal_owned_group(self.group);
        }
        #[cfg(target_os = "windows")]
        {
            self.job.take();
            let _ = self.child.kill();
        }
    }
    fn accepted(
        &mut self,
        check: &app_core::NativeCaptureCheck<'_>,
        deadline: Instant,
    ) -> Result<bool> {
        let mut io = transport::ActiveIo {
            transport: self.channel.take().context("Print channel absent")?,
            deadline,
            check,
        };
        let mut bytes = [0u8; 8];
        let result = io.read_exact(&mut bytes);
        self.channel = Some(io.transport);
        result?;
        Ok(&bytes == ACK)
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        self.stop();
    }
}

fn acquire() -> Result<Admission> {
    ensure!(
        BUSY.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok(),
        "Printing is busy"
    );
    let admission = Admission;
    let mut pending = PENDING.lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some(native) = pending.as_mut() {
        ensure!(
            native.try_exit()?.is_some(),
            "Previous print worker is still stopping; no new submission was started"
        );
        pending.take();
    }
    Ok(admission)
}
fn supervise(command: Command, cancelled: &impl Fn() -> bool, deadline: Instant) -> Result<()> {
    let _admission = acquire()?;
    ensure!(!cancelled(), "Print canceled before submission");
    let mut native = Native::spawn(command, deadline)?;
    let result = (|| -> Result<()> {
        let check = || {
            if cancelled() {
                Err(app_core::CaptureError::NativeCanceled)
            } else {
                Ok(())
            }
        };
        native
            .handshake(&check, deadline)
            .context("Print handoff failed before submission")?;
        loop {
            if let Some(status) = native.try_exit().context(
                "Print process status failed; outcome unknown; do not retry automatically",
            )? {
                if status.code() == Some(2) {
                    bail!(
                        "Print failed before submission: input or explicit printer settings unavailable"
                    );
                }
                ensure!(
                    status.success() && native.accepted(&check, deadline)
                        .context("Print acknowledgement failed; outcome unknown; do not retry automatically")?,
                    "Print outcome is unknown; do not retry automatically"
                );
                return Ok(());
            }
            ensure!(
                !cancelled(),
                "Print canceled; spool acceptance or physical output may already have occurred"
            );
            ensure!(
                Instant::now() < deadline,
                "Print timed out; spool acceptance or physical output may already have occurred"
            );
            // Operation-scoped wait only. No thread, timer or pending-cleanup loop survives completion.
            #[cfg(target_os = "windows")]
            native.child.wait_ready(Duration::from_millis(20));
            #[cfg(unix)]
            std::thread::sleep(Duration::from_millis(20));
        }
    })();
    native.stop();
    #[cfg(unix)]
    let reaped = native.reaped;
    #[cfg(target_os = "windows")]
    let reaped = native.child.try_wait().is_ok_and(|status| status.is_some());
    if !reaped {
        let until = Instant::now() + CLEANUP;
        loop {
            if native.try_exit().is_ok_and(|status| status.is_some()) {
                break;
            }
            if Instant::now() >= until {
                *PENDING.lock().unwrap_or_else(|poison| poison.into_inner()) = Some(native);
                return Err(result.err().unwrap_or_else(|| {
                    anyhow::anyhow!("Print worker cleanup remains pending; outcome unknown")
                }));
            }
            #[cfg(target_os = "windows")]
            native.child.wait_ready(Duration::from_millis(5));
            #[cfg(unix)]
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    result
}

pub fn run(args: &[OsString]) -> Result<()> {
    parse(args)?;
    let command = command(args)?;
    #[cfg(unix)]
    let cancellation = crate::record::unix::Cancellation::new()?;
    #[cfg(unix)]
    let flag = cancellation.flag();
    #[cfg(target_os = "windows")]
    let cancellation = WindowsCancellation::new()?;
    #[cfg(unix)]
    let cancelled = || flag.load(Ordering::Relaxed);
    #[cfg(target_os = "windows")]
    let cancelled = || CONSOLE_CANCEL.load(Ordering::Relaxed);
    let result = supervise(command, &cancelled, Instant::now() + DEADLINE);
    drop(cancellation);
    result?;
    let acknowledgement = serde_json::json!({"status":"accepted_by_spooler", "physical_delivery":"unverified", "copies":1, "pages":1});
    writeln!(std::io::stdout().lock(), "{acknowledgement}").context(
        "Spooler accepted the print job, but result output failed; do not retry automatically",
    )?;
    Ok(())
}

/// Runs before capture-worker installation or desktop initialization.
pub fn try_child(args: &[OsString]) -> Result<bool> {
    if args.first().is_none_or(|value| value != WORKER) {
        return Ok(false);
    }
    let code = worker(&args[1..]);
    std::process::exit(code);
}
fn worker(args: &[OsString]) -> i32 {
    let mut submission_started = false;
    let result = (|| -> Result<()> {
        let options = parse(args)?;
        let (channel, nonce, parent) = transport::Transport::inherited()?;
        #[cfg(target_os = "macos")]
        let _lifeline = transport::Lifeline::new(parent)?;
        #[cfg(not(target_os = "macos"))]
        let _ = parent;
        let check = || Ok(());
        let mut io = transport::ActiveIo {
            transport: channel,
            deadline: Instant::now() + DEADLINE,
            check: &check,
        };
        let mut handshake = [0; 8];
        let mut received_nonce = [0; 16];
        io.read_exact(&mut handshake)?;
        io.read_exact(&mut received_nonce)?;
        ensure!(
            &handshake == HANDSHAKE && nonce == received_nonce,
            "Private print handshake failed"
        );
        #[cfg(target_os = "macos")]
        {
            // Install against this fresh executable's VM, not the forked GUI's mappings.
            let mut budget = image_budget::Budget::enter()?;
            budget.decoding()?;
        }
        let pixels = app_core::load_editor_image(&options.input)?;
        #[cfg(target_os = "windows")]
        let mut pixels = pixels;
        #[cfg(unix)]
        unix_print(&pixels, &options, &mut submission_started)?;
        #[cfg(target_os = "windows")]
        app_core::print_windows_pixels(
            &mut pixels,
            &options.printer,
            options.paper,
            &mut submission_started,
        )?;
        io.write_all(ACK)?;
        io.flush()?;
        Ok(())
    })();
    match result {
        Ok(()) => 0,
        Err(_) if !submission_started => 2,
        Err(_) => 3,
    }
}

#[cfg(unix)]
const IPP_RESPONSE_LIMIT: usize = 64 * 1024;
#[cfg(unix)]
fn ipp_attribute(output: &mut Vec<u8>, tag: u8, name: &str, value: &[u8]) {
    // All callers supply fixed bounded names/values, or the admitted queue URI.
    output.push(tag);
    output.extend_from_slice(&(name.len() as u16).to_be_bytes());
    output.extend_from_slice(name.as_bytes());
    output.extend_from_slice(&(value.len() as u16).to_be_bytes());
    output.extend_from_slice(value);
}
#[cfg(unix)]
fn ipp_request(operation: u16, id: u32, uri: &str) -> Vec<u8> {
    let mut output = vec![1, 1];
    output.extend_from_slice(&operation.to_be_bytes());
    output.extend_from_slice(&id.to_be_bytes());
    output.push(1); // operation-attributes-tag
    ipp_attribute(&mut output, 0x47, "attributes-charset", b"utf-8");
    ipp_attribute(&mut output, 0x48, "attributes-natural-language", b"en");
    ipp_attribute(&mut output, 0x45, "printer-uri", uri.as_bytes());
    ipp_attribute(&mut output, 0x42, "requesting-user-name", b"screenshots");
    output
}
#[cfg(unix)]
struct IppAttribute<'a> {
    group: u8,
    tag: u8,
    name: &'a [u8],
    value: &'a [u8],
}
#[cfg(unix)]
fn ipp_response(bytes: &[u8], id: u32) -> Result<Vec<IppAttribute<'_>>> {
    ensure!(
        bytes.len() >= 9
            && bytes.len() <= IPP_RESPONSE_LIMIT
            && bytes[..4] == [1, 1, 0, 0]
            && bytes[4..8] == id.to_be_bytes(),
        "Spooler refused the exact IPP request or returned an invalid acknowledgement"
    );
    let mut position = 8;
    let mut group = 0;
    let mut previous = &bytes[0..0];
    let mut attributes = Vec::new();
    while position < bytes.len() {
        let tag = bytes[position];
        position += 1;
        if tag == 3 {
            ensure!(position == bytes.len(), "Unexpected IPP response data");
            return Ok(attributes);
        }
        if tag < 0x10 {
            ensure!(matches!(tag, 1 | 2 | 4 | 5), "Unsupported IPP group");
            group = tag;
            previous = &bytes[0..0];
            continue;
        }
        ensure!(
            group != 0 && tag < 0x7f && attributes.len() < 128,
            "Invalid or oversized IPP attributes"
        );
        let mut field = || -> Result<&[u8]> {
            let length = bytes
                .get(position..position + 2)
                .context("Truncated IPP attribute")?;
            let length = usize::from(u16::from_be_bytes([length[0], length[1]]));
            position += 2;
            let value = bytes
                .get(position..position + length)
                .context("Truncated IPP value")?;
            position += length;
            Ok(value)
        };
        let name = field()?;
        let value = field()?;
        if !name.is_empty() {
            ensure!(
                name.len() <= 255 && name.iter().all(u8::is_ascii),
                "Invalid IPP name"
            );
            previous = name;
        }
        ensure!(!previous.is_empty(), "IPP continuation lacks a name");
        attributes.push(IppAttribute {
            group,
            tag,
            name: previous,
            value,
        });
    }
    bail!("IPP response lacks its terminator")
}
#[cfg(unix)]
fn ipp_one<'a>(
    attributes: &'a [IppAttribute<'a>],
    group: u8,
    tag: u8,
    name: &[u8],
) -> Result<&'a [u8]> {
    let mut values = attributes
        .iter()
        .filter(|value| value.group == group && value.name == name);
    let value = values
        .next()
        .context("Required exact IPP attribute is absent")?;
    ensure!(
        values.next().is_none() && value.tag == tag,
        "Required exact IPP attribute is ambiguous"
    );
    Ok(value.value)
}
#[cfg(unix)]
fn ipp_existing_queue(bytes: &[u8], printer: &str) -> Result<()> {
    let attributes = ipp_response(bytes, 1)?;
    ensure!(
        ipp_one(&attributes, 4, 0x42, b"printer-name")? == printer.as_bytes(),
        "Spooler did not identify the exact existing queue"
    );
    ensure!(
        ipp_one(&attributes, 4, 0x22, b"printer-is-temporary")? == [0]
            && ipp_one(&attributes, 4, 0x22, b"printer-is-accepting-jobs")? == [1],
        "Queue is temporary or is not accepting jobs"
    );
    for (name, tag, value) in [
        (
            &b"document-format-supported"[..],
            0x49,
            &b"application/postscript"[..],
        ),
        (&b"operations-supported"[..], 0x23, &2i32.to_be_bytes()[..]),
    ] {
        ensure!(
            attributes.iter().any(|attribute| attribute.group == 4
                && attribute.tag == tag
                && attribute.name == name
                && attribute.value == value),
            "Queue lacks required print support"
        );
    }
    Ok(())
}
#[cfg(unix)]
fn ipp_print_request(uri: &str, paper: PrintPaper) -> Vec<u8> {
    let mut output = ipp_request(2, 2, uri); // Print-Job, never Create-Job or queue provisioning.
    ipp_attribute(&mut output, 0x42, "job-name", b"Screenshot");
    ipp_attribute(&mut output, 0x22, "ipp-attribute-fidelity", &[1]);
    ipp_attribute(
        &mut output,
        0x49,
        "document-format",
        b"application/postscript",
    );
    output.push(2); // job-attributes-tag
    for (name, value) in [
        ("copies", 1i32),
        ("number-up", 1),
        ("orientation-requested", 3),
    ] {
        let tag = if name == "orientation-requested" {
            0x23
        } else {
            0x21
        };
        ipp_attribute(&mut output, tag, name, &value.to_be_bytes());
    }
    let media = match paper {
        PrintPaper::A4 => "iso_a4_210x297mm",
        PrintPaper::Letter => "na_letter_8.5x11in",
    };
    for (name, value) in [
        ("media", media),
        ("sides", "one-sided"),
        ("print-scaling", "none"),
        ("job-sheets", "none"),
    ] {
        ipp_attribute(&mut output, 0x44, name, value.as_bytes());
    }
    ipp_attribute(&mut output, 0x33, "page-ranges", &[0, 0, 0, 1, 0, 0, 0, 1]);
    output.push(3);
    output
}
#[cfg(unix)]
fn ipp_connect(address: std::net::SocketAddrV4) -> Result<std::net::TcpStream> {
    let stream = std::net::TcpStream::connect_timeout(&address.into(), Duration::from_secs(5))?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    Ok(stream)
}
#[cfg(unix)]
fn ipp_http_response(stream: impl Read) -> Result<Vec<u8>> {
    let mut input = std::io::BufReader::with_capacity(8192, stream);
    let mut header = Vec::new();
    while header.len() < 8192 && !header.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        input.read_exact(&mut byte)?;
        header.push(byte[0]);
    }
    ensure!(
        header.ends_with(b"\r\n\r\n"),
        "Spool HTTP headers exceed limit"
    );
    let header = std::str::from_utf8(&header)?;
    let mut lines = header.split("\r\n");
    let mut status = lines.next().unwrap_or_default().split_ascii_whitespace();
    ensure!(
        matches!(status.next(), Some("HTTP/1.1" | "HTTP/1.0")) && status.next() == Some("200"),
        "Spool HTTP request was refused; authentication, TLS upgrades and redirects are unsupported"
    );
    let mut length = None;
    let mut content_type = false;
    for (index, line) in lines.filter(|line| !line.is_empty()).enumerate() {
        ensure!(index < 64, "Too many spool HTTP headers");
        let (name, value) = line
            .split_once(':')
            .context("Malformed spool HTTP header")?;
        let value = value.trim();
        ensure!(
            !name.eq_ignore_ascii_case("Transfer-Encoding"),
            "Chunked spool responses are unsupported"
        );
        if name.eq_ignore_ascii_case("Content-Length") {
            ensure!(
                length.is_none()
                    && !value.is_empty()
                    && value.bytes().all(|byte| byte.is_ascii_digit()),
                "Ambiguous spool HTTP length"
            );
            length = Some(value.parse::<usize>()?);
        }
        if name.eq_ignore_ascii_case("Content-Type") {
            ensure!(
                !content_type && value.eq_ignore_ascii_case("application/ipp"),
                "Invalid spool content type"
            );
            content_type = true;
        }
    }
    let length = length.context("Spool response requires Content-Length")?;
    ensure!(
        content_type && (9..=IPP_RESPONSE_LIMIT).contains(&length),
        "Invalid or oversized spool response"
    );
    let mut bytes = vec![0; length];
    input.read_exact(&mut bytes)?;
    Ok(bytes)
}
#[cfg(unix)]
struct IppChunks<'a>(&'a mut std::net::TcpStream);
#[cfg(unix)]
impl Write for IppChunks<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if !bytes.is_empty() {
            write!(self.0, "{:x}\r\n", bytes.len())?;
            self.0.write_all(bytes)?;
            self.0.write_all(b"\r\n")?;
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}
#[cfg(unix)]
fn unix_print(pixels: &image::RgbaImage, options: &Options, started: &mut bool) -> Result<()> {
    let server = options
        .server
        .as_deref()
        .context("Explicit CUPS server is missing")?;
    validate_server(server)?;
    let (host, port) = server.rsplit_once(':').context("Invalid spool address")?;
    let address = std::net::SocketAddrV4::new(host.parse()?, port.parse()?);
    let resource = format!("/printers/{}", options.printer);
    let uri = format!("ipp://{address}{resource}");
    let mut query = ipp_request(0x000b, 1, &uri);
    for (index, name) in [
        "printer-name",
        "printer-is-temporary",
        "printer-is-accepting-jobs",
        "document-format-supported",
        "operations-supported",
    ]
    .iter()
    .enumerate()
    {
        ipp_attribute(
            &mut query,
            0x44,
            if index == 0 {
                "requested-attributes"
            } else {
                ""
            },
            name.as_bytes(),
        );
    }
    query.push(3);
    let mut stream = ipp_connect(address)?;
    write!(
        stream,
        "POST {resource} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/ipp\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        query.len()
    )?;
    stream.write_all(&query)?;
    ipp_existing_queue(&ipp_http_response(stream)?, &options.printer)?;
    let request = ipp_print_request(&uri, options.paper);
    let mut stream = ipp_connect(address)?;
    // No response URI is ever used as an address. Once this request begins, failure is ambiguous.
    *started = true;
    write!(
        stream,
        "POST {resource} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/ipp\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
    )?;
    let mut output = IppChunks(&mut stream);
    output.write_all(&request)?;
    app_core::write_print_postscript(pixels, options.paper, &mut output, &|| false)?;
    stream.write_all(b"0\r\n\r\n")?;
    let response = ipp_http_response(stream)?;
    let attributes = ipp_response(&response, 2)?;
    let id = ipp_one(&attributes, 2, 0x21, b"job-id")?;
    ensure!(
        id.len() == 4 && i32::from_be_bytes(id.try_into()?) > 0,
        "Spool acknowledgement is invalid; outcome unknown"
    );
    Ok(())
}

#[cfg(target_os = "windows")]
static CONSOLE_CANCEL: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "windows")]
unsafe extern "system" fn console(event: u32) -> i32 {
    if matches!(event, 0 | 1 | 2 | 5 | 6) {
        CONSOLE_CANCEL.store(true, Ordering::Relaxed);
        1
    } else {
        0
    }
}
#[cfg(target_os = "windows")]
struct WindowsCancellation;
#[cfg(target_os = "windows")]
impl WindowsCancellation {
    fn new() -> Result<Self> {
        CONSOLE_CANCEL.store(false, Ordering::Relaxed);
        ensure!(
            unsafe { windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(console), 1) }
                != 0,
            "Print cancellation handler is unavailable"
        );
        Ok(Self)
    }
}
#[cfg(target_os = "windows")]
impl Drop for WindowsCancellation {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(console), 0);
        }
    }
}

#[cfg(test)]
#[path = "print_image_tests.rs"]
mod tests;
