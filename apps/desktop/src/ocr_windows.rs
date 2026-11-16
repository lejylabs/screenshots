//! Operation-scoped Windows process isolation for the optional OCR engine.

use crate::ocr::{MAX_DIAGNOSTIC_BYTES, MAX_TEXT_BYTES, read_pipe};
use anyhow::{Context, Result, ensure};
use app_core::windows_child::{self, Child, Environment, Streams};
use std::{
    io::Write,
    os::windows::io::{AsHandle, OwnedHandle},
    process::Command,
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread::JoinHandle,
    time::{Duration, Instant},
};

const DEADLINE: Duration = Duration::from_secs(30);
const CHECK_INTERVAL: Duration = Duration::from_millis(25);

struct Engine {
    child: Child,
    job: Option<OwnedHandle>,
    workers: Vec<JoinHandle<()>>,
    finished: bool,
}

impl Engine {
    fn stop(&mut self) {
        // Closing our sole job handle terminates descendants before joining their pipes.
        drop(self.job.take());
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

pub(crate) fn job_object() -> Result<OwnedHandle> {
    windows_child::job(None, None)
}

enum Message {
    Input(Result<(), &'static str>),
    Output(Result<Vec<u8>, &'static str>),
    Diagnostic(Result<Vec<u8>, &'static str>),
}

fn await_result(
    engine: &mut Engine,
    messages: Receiver<Message>,
    deadline: Instant,
) -> Result<String> {
    let mut completed = 0;
    let mut output = None;
    loop {
        ensure!(
            Instant::now() < deadline,
            "OCR exceeded its 30 second deadline"
        );
        let status = engine
            .child
            .try_wait()
            .map_err(|_| anyhow::anyhow!("Could not observe OCR engine completion"))?;
        if let Some(status) = status {
            ensure!(
                status.success(),
                "OCR engine failed; verify the requested language data is installed"
            );
            if completed == 3 {
                engine.stop();
                let bytes = output.context("OCR engine returned no output stream")?;
                return String::from_utf8(bytes)
                    .map_err(|_| anyhow::anyhow!("OCR engine output was not UTF-8"));
            }
        }
        let remaining = CHECK_INTERVAL.min(deadline.saturating_duration_since(Instant::now()));
        match messages.recv_timeout(remaining) {
            Ok(message) => {
                completed += 1;
                match message {
                    Message::Input(result) => result.map_err(anyhow::Error::msg)?,
                    Message::Output(result) => output = Some(result.map_err(anyhow::Error::msg)?),
                    Message::Diagnostic(result) => {
                        result.map_err(anyhow::Error::msg)?;
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                ensure!(completed == 3, "OCR pipe worker ended without completion");
                std::thread::sleep(remaining);
            }
        }
    }
}

fn run_command(command: Command, input: Vec<u8>, timeout: Duration) -> Result<String> {
    let deadline = Instant::now() + timeout;
    let job = job_object()?;
    let child = windows_child::spawn(
        &command, job.as_handle(), Streams::PIPED, Environment::Inherit, &[],
    ).map_err(|_| anyhow::anyhow!("OCR engine is unavailable; install tesseract and the requested language data separately"))?;
    let mut engine = Engine {
        child,
        job: Some(job),
        workers: Vec::with_capacity(3),
        finished: false,
    };
    let mut stdin = engine
        .child
        .stdin
        .take()
        .context("OCR input pipe is unavailable")?;
    let stdout = engine
        .child
        .stdout
        .take()
        .context("OCR output pipe is unavailable")?;
    let stderr = engine
        .child
        .stderr
        .take()
        .context("OCR diagnostic pipe is unavailable")?;
    let (sender, messages) = mpsc::sync_channel(3);
    let input_sender = sender.clone();
    engine.workers.push(
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
    engine.workers.push(
        std::thread::Builder::new()
            .name("ocr-output".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                let _ =
                    output_sender.send(Message::Output(read_pipe(stdout, MAX_TEXT_BYTES, true)));
            })
            .context("Could not start OCR output worker")?,
    );
    engine.workers.push(
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
    engine.child.resume(deadline)?;
    await_result(&mut engine, messages, deadline)
}

pub(crate) fn recognize(input: Vec<u8>, language: &str) -> Result<String> {
    let mut command = Command::new("tesseract.exe");
    command
        .args(["stdin", "stdout", "-l", language, "--psm", "6"])
        .env("OMP_THREAD_LIMIT", "2");
    run_command(command, input, DEADLINE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        os::windows::io::{AsRawHandle, FromRawHandle},
        process::Stdio,
    };
    use windows_sys::Win32::Foundation::GetLastError;

    fn fixture(mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "ocr_windows::tests::engine_fixture",
                "--ignored",
                "--nocapture",
                "--quiet",
            ])
            .env("SCREENSHOTS_OCR_FIXTURE", mode);
        command
    }

    #[test]
    #[ignore = "subprocess fixture invoked by Windows OCR tests"]
    fn engine_fixture() {
        let Ok(mode) = std::env::var("SCREENSHOTS_OCR_FIXTURE") else {
            return;
        };
        if mode == "hold" {
            std::thread::sleep(Duration::from_secs(60));
            std::process::exit(0);
        }
        std::io::stdin().read_to_end(&mut Vec::new()).unwrap();
        match mode.as_str() {
            "text" => {
                std::io::stdout().write_all(b"SYNTHETIC TEXT\n").unwrap();
                std::io::stderr().write_all(b"PRIVATE DIAGNOSTIC").unwrap();
            }
            "invalid-utf8" => std::io::stdout().write_all(&[0xff]).unwrap(),
            "stdout-overflow" => {
                std::io::stdout()
                    .write_all(&vec![b'x'; MAX_TEXT_BYTES + 1])
                    .unwrap();
            }
            "stderr-overflow" => {
                std::io::stderr()
                    .write_all(&vec![b'x'; MAX_DIAGNOSTIC_BYTES + 1])
                    .unwrap();
            }
            "failure" => std::process::exit(9),
            "descendant" => {
                let mut command = fixture("hold");
                let child = command.stdin(Stdio::null()).spawn().unwrap();
                std::fs::write(
                    std::env::var_os("SCREENSHOTS_OCR_PID").unwrap(),
                    child.id().to_string(),
                )
                .unwrap();
                std::thread::sleep(Duration::from_secs(60));
            }
            _ => panic!("unknown synthetic fixture mode"),
        }
        std::io::stdout().flush().unwrap();
        std::io::stderr().flush().unwrap();
        std::process::exit(0);
    }

    #[test]
    fn nonexistent_absolute_engine_path_reports_unavailable() {
        let missing = std::env::temp_dir()
            .join(format!(
                "screenshots-ocr-missing-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ))
            .join("tesseract.exe");
        let error = run_command(Command::new(missing), Vec::new(), DEADLINE).unwrap_err();
        assert!(error.to_string().contains("engine is unavailable"));
    }

    #[test]
    fn isolated_engine_returns_text_and_rejects_failures_and_stream_limits() {
        let text = run_command(fixture("text"), b"input\n".to_vec(), DEADLINE).unwrap();
        assert!(text.contains("SYNTHETIC TEXT\n"));
        assert!(!text.contains("PRIVATE DIAGNOSTIC"));
        for (mode, expected) in [
            ("invalid-utf8", "was not UTF-8"),
            ("stdout-overflow", "byte limit"),
            ("stderr-overflow", "byte limit"),
            ("failure", "engine failed"),
        ] {
            let error = run_command(fixture(mode), b"input\n".to_vec(), DEADLINE).unwrap_err();
            assert!(error.to_string().contains(expected), "{mode}: {error}");
        }
    }

    #[test]
    fn timeout_terminates_descendants_holding_inherited_pipes() {
        use windows_sys::Win32::{
            Foundation::{ERROR_INVALID_PARAMETER, WAIT_OBJECT_0},
            System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
        };
        let pid_file = std::env::temp_dir().join(format!(
            "screenshots-ocr-descendant-{}-{}.pid",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut command = fixture("descendant");
        command.env("SCREENSHOTS_OCR_PID", &pid_file);
        let start = Instant::now();
        let error = run_command(command, b"input\n".to_vec(), Duration::from_secs(3)).unwrap_err();
        assert!(error.to_string().contains("deadline"));
        assert!(start.elapsed() < Duration::from_secs(10));
        let pid = std::fs::read_to_string(&pid_file)
            .expect("descendant was started")
            .parse()
            .unwrap();
        std::fs::remove_file(pid_file).unwrap();
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if !handle.is_null() {
            let child = unsafe { OwnedHandle::from_raw_handle(handle) };
            assert_eq!(
                unsafe { WaitForSingleObject(child.as_raw_handle(), 5000) },
                WAIT_OBJECT_0
            );
        } else {
            assert_eq!(unsafe { GetLastError() }, ERROR_INVALID_PARAMETER);
        }
    }
}
