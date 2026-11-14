//! Explicit OCR through an optional installed engine; no resident service or image files.

#[cfg(not(unix))]
use anyhow::bail;
use anyhow::{Context, Result, ensure};
use std::ffi::OsString;
#[cfg(unix)]
use std::path::Path;

#[cfg(unix)]
const MAX_OCR_IMAGE_BYTES: u64 = 16 * 1024 * 1024;
#[cfg(unix)]
const MAX_OCR_PNG_BYTES: usize = 32 * 1024 * 1024;
#[cfg(unix)]
const MAX_TEXT_BYTES: usize = 256 * 1024;
#[cfg(unix)]
const MAX_DIAGNOSTIC_BYTES: usize = 16 * 1024;

fn language(args: &[OsString]) -> Result<&str> {
    ensure!(
        args.len() == 1 || (args.len() == 3 && args[1] == "--language"),
        "Usage: screenshots ocr INPUT [--language LANG]"
    );
    let language = if args.len() == 3 {
        args[2].to_str().context("OCR language must be UTF-8")?
    } else {
        "eng"
    };
    ensure!(
        !language.is_empty()
            && language.len() <= 64
            && language.split('+').all(|part| {
                !part.is_empty()
                    && part
                        .bytes()
                        .all(|value| value.is_ascii_alphanumeric() || value == b'_')
            }),
        "OCR language must contain at most 64 ASCII letters, digits, underscores or language separators (+)"
    );
    Ok(language)
}

pub fn run(args: &[OsString]) -> Result<()> {
    let language = language(args)?;
    #[cfg(not(unix))]
    {
        let _ = language;
        bail!(
            "Headless OCR is currently supported on Unix only; Windows process isolation is not implemented"
        );
    }
    #[cfg(unix)]
    {
        let pixels =
            app_core::load_editor_image_with_limit(Path::new(&args[0]), MAX_OCR_IMAGE_BYTES)?;
        let input = crate::cli::frame(pixels).png_bytes()?;
        ensure!(
            input.len() <= MAX_OCR_PNG_BYTES,
            "OCR PNG input exceeds the 32 MiB pipe limit"
        );
        let text = engine::recognize(input, language)?;
        println!(
            "{}",
            serde_json::json!({"text": text, "engine": "tesseract"})
        );
        Ok(())
    }
}

#[cfg(any(unix, test))]
fn read_pipe(
    mut pipe: impl std::io::Read,
    limit: usize,
    keep: bool,
) -> Result<Vec<u8>, &'static str> {
    let mut output = Vec::new();
    let mut total = 0;
    let mut buffer = [0; 8192];
    loop {
        let length = pipe.read(&mut buffer).map_err(|_| "OCR pipe read failed")?;
        if length == 0 {
            return Ok(output);
        }
        total += length;
        if total > limit {
            return Err("OCR engine output exceeded its byte limit");
        }
        if keep {
            output.extend_from_slice(&buffer[..length]);
        }
    }
}

#[cfg(unix)]
mod engine {
    use super::*;
    use std::{
        io::Write,
        os::unix::process::CommandExt,
        process::{Child, Command, Stdio},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc::{self, Receiver, RecvTimeoutError},
        },
        thread::JoinHandle,
        time::{Duration, Instant},
    };

    const DEADLINE: Duration = Duration::from_secs(30);
    const CHECK_INTERVAL: Duration = Duration::from_millis(25);

    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }

    struct Cancellation {
        flag: Arc<AtomicBool>,
        finished: Arc<AtomicBool>,
        handlers: Vec<signal_hook::SigId>,
    }

    impl Cancellation {
        fn new() -> Result<Self> {
            let mut cancellation = Self {
                flag: Arc::new(AtomicBool::new(false)),
                finished: Arc::new(AtomicBool::new(false)),
                handlers: Vec::with_capacity(2),
            };
            for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
                // Headless OCR owns these two actions until process exit, including blocked stdout.
                signal_hook::flag::register_conditional_shutdown(
                    signal,
                    1,
                    cancellation.finished.clone(),
                )
                .context("Could not install OCR completion handler")?;
                cancellation.handlers.push(
                    signal_hook::flag::register(signal, cancellation.flag.clone())
                        .context("Could not install OCR cancellation handler")?,
                );
            }
            Ok(cancellation)
        }
    }

    impl Drop for Cancellation {
        fn drop(&mut self) {
            self.finished.store(true, Ordering::SeqCst);
            for handler in self.handlers.drain(..) {
                signal_hook::low_level::unregister(handler);
            }
        }
    }

    struct Engine {
        child: Child,
        group: i32,
        workers: Vec<JoinHandle<()>>,
        finished: bool,
    }

    impl Engine {
        fn stop(&mut self) {
            // The child starts its own process group; kill descendants before joining pipes.
            unsafe { kill(-self.group, 9) };
            let _ = self.child.kill();
            let _ = self.child.wait();
            for worker in self.workers.drain(..) {
                let _ = worker.join();
            }
            self.finished = true;
        }
    }

    impl Drop for Engine {
        fn drop(&mut self) {
            if !self.finished {
                self.stop();
            }
        }
    }

    enum Message {
        Input(Result<(), &'static str>),
        Output(Result<Vec<u8>, &'static str>),
        Diagnostic(Result<Vec<u8>, &'static str>),
    }

    fn await_result(
        job: &mut Engine,
        messages: Receiver<Message>,
        deadline: Instant,
        cancellation: &AtomicBool,
    ) -> Result<String> {
        let mut completed = 0;
        let mut output = None;
        loop {
            ensure!(!cancellation.load(Ordering::Relaxed), "OCR canceled");
            ensure!(
                Instant::now() < deadline,
                "OCR exceeded its 30 second deadline"
            );
            let status = job
                .child
                .try_wait()
                .map_err(|_| anyhow::anyhow!("Could not observe OCR engine completion"))?;
            if let Some(status) = status {
                ensure!(
                    status.success(),
                    "OCR engine failed; verify the requested language data is installed"
                );
                if completed == 3 {
                    job.stop();
                    let bytes = output.context("OCR engine returned no output stream")?;
                    return String::from_utf8(bytes)
                        .map_err(|_| anyhow::anyhow!("OCR engine output was not UTF-8"));
                }
            }
            match messages.recv_timeout(
                CHECK_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
            ) {
                Ok(message) => {
                    completed += 1;
                    match message {
                        Message::Input(result) => result.map_err(anyhow::Error::msg)?,
                        Message::Output(result) => {
                            output = Some(result.map_err(anyhow::Error::msg)?)
                        }
                        Message::Diagnostic(result) => {
                            result.map_err(anyhow::Error::msg)?;
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    ensure!(completed == 3, "OCR pipe worker ended without completion");
                    // Streams can close just before the engine exits; preserve the bounded wait.
                    std::thread::sleep(
                        CHECK_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
                    );
                }
            }
        }
    }

    pub(super) fn recognize(input: Vec<u8>, language: &str) -> Result<String> {
        let cancellation = Cancellation::new()?;
        let deadline = Instant::now() + DEADLINE;
        let mut child = Command::new("tesseract")
            .args(["stdin", "stdout", "-l", language, "--psm", "6"])
            .env("OMP_THREAD_LIMIT", "2")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .map_err(|_| anyhow::anyhow!("OCR engine is unavailable; install tesseract and the requested language data separately"))?;
        let group = match i32::try_from(child.id()) {
            Ok(group) => group,
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("OCR engine process ID is invalid");
            }
        };
        let mut job = Engine {
            child,
            group,
            workers: Vec::with_capacity(3),
            finished: false,
        };
        let mut stdin = job
            .child
            .stdin
            .take()
            .context("OCR input pipe is unavailable")?;
        let stdout = job
            .child
            .stdout
            .take()
            .context("OCR output pipe is unavailable")?;
        let stderr = job
            .child
            .stderr
            .take()
            .context("OCR diagnostic pipe is unavailable")?;
        let (sender, messages) = mpsc::sync_channel(3);
        let input_sender = sender.clone();
        job.workers.push(
            std::thread::Builder::new()
                .name("ocr-input".into())
                .stack_size(256 * 1024)
                .spawn(move || {
                    let result = stdin.write_all(&input).map_err(|_| "OCR input pipe failed");
                    drop(stdin);
                    let _ = input_sender.send(Message::Input(result));
                })
                .context("Could not start OCR input worker")?,
        );
        let output_sender = sender.clone();
        job.workers.push(
            std::thread::Builder::new()
                .name("ocr-output".into())
                .stack_size(256 * 1024)
                .spawn(move || {
                    let _ = output_sender.send(Message::Output(read_pipe(
                        stdout,
                        MAX_TEXT_BYTES,
                        true,
                    )));
                })
                .context("Could not start OCR output worker")?,
        );
        job.workers.push(
            std::thread::Builder::new()
                .name("ocr-errors".into())
                .stack_size(256 * 1024)
                .spawn(move || {
                    let _ = sender.send(Message::Diagnostic(read_pipe(
                        stderr,
                        MAX_DIAGNOSTIC_BYTES,
                        false,
                    )));
                })
                .context("Could not start OCR diagnostic worker")?,
        );
        await_result(&mut job, messages, deadline, &cancellation.flag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_options_are_bounded_and_unambiguous() {
        for value in ["eng", "deu+eng", "chi_sim"] {
            assert_eq!(
                language(&["x.png".into(), "--language".into(), value.into()]).unwrap(),
                value
            );
        }
        for value in [
            "",
            "-l",
            "+eng",
            "eng+",
            "eng++deu",
            "../../eng",
            "eng\n",
            "ä",
        ] {
            assert!(language(&["x.png".into(), "--language".into(), value.into()]).is_err());
        }
        assert!(language(&["x.png".into(), "--language".into(), "x".repeat(65).into()]).is_err());
        assert!(language(&[]).is_err());
        assert!(language(&["x.png".into(), "--unknown".into(), "eng".into()]).is_err());
    }

    #[test]
    fn pipe_reader_keeps_only_bounded_recognized_output_and_discards_diagnostics() {
        assert_eq!(read_pipe(&b"text"[..], 4, true).unwrap(), b"text");
        assert_eq!(
            read_pipe(&b"private diagnostic"[..], 18, false).unwrap(),
            b""
        );
        assert!(read_pipe(&b"text!"[..], 4, true).is_err());
        assert!(read_pipe(&b"diagnostic!"[..], 10, false).is_err());
    }
}
