#[cfg(unix)]
use image::{Rgba, RgbaImage};
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "screenshots-ocr-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command.arg("ocr");
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
        command.env_remove("DISPLAY").env_remove("WAYLAND_DISPLAY");
        command
    }
    fn run(&self, source: &Path, arguments: &[&str]) -> Output {
        self.command().arg(source).args(arguments).output().unwrap()
    }
    fn assert_no_user_state(&self) {
        for name in [
            "home",
            "userprofile",
            "config",
            "data",
            "cache",
            "appdata",
            "localappdata",
        ] {
            assert!(!self.path(name).exists());
        }
    }
    #[cfg(unix)]
    fn fake_engine(&self, script: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let directory = self.path("engine");
        std::fs::create_dir(&directory).unwrap();
        let engine = directory.join("tesseract");
        std::fs::write(&engine, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(engine, std::fs::Permissions::from_mode(0o700)).unwrap();
        directory
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn invalid_arguments_fail_without_output_or_engine_access() {
    let f = Fixture::new();
    for arguments in [
        vec!["--unknown", "eng"],
        vec!["--language", ""],
        vec!["--language", "eng++deu"],
        vec!["--language", "../../eng"],
        vec!["--language", "eng", "extra"],
    ] {
        let output = f.run(&f.path("missing.png"), &arguments);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
    f.assert_no_user_state();
}

#[cfg(unix)]
#[test]
fn missing_engine_and_oversized_input_fail_without_changing_sources() {
    let f = Fixture::new();
    let source = f.path("synthetic.png");
    RgbaImage::from_pixel(2, 2, Rgba([255, 255, 255, 255]))
        .save(&source)
        .unwrap();
    let bytes = std::fs::read(&source).unwrap();
    let output = f
        .command()
        .arg(&source)
        .env("PATH", f.path("absent"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("engine is unavailable"));
    assert_eq!(std::fs::read(&source).unwrap(), bytes);
    let large = f.path("oversized.png");
    RgbaImage::from_pixel(2049, 2048, Rgba([255, 255, 255, 255]))
        .save(&large)
        .unwrap();
    let output = f
        .command()
        .arg(&large)
        .env("PATH", f.path("absent"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("memory limit"));
    f.assert_no_user_state();
}

#[cfg(unix)]
#[test]
fn fake_engine_output_is_json_and_diagnostics_are_never_exposed() {
    let f = Fixture::new();
    let source = f.path("synthetic.png");
    RgbaImage::from_pixel(2, 2, Rgba([255, 255, 255, 255]))
        .save(&source)
        .unwrap();
    let bytes = std::fs::read(&source).unwrap();
    let engine = f.fake_engine("while IFS= read -r line; do :; done\nprintf 'SYNTHETIC TEXT\\n'\nprintf 'PRIVATE ENGINE DIAGNOSTIC' >&2");
    let output = f
        .command()
        .arg(&source)
        .env("PATH", engine)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["text"], "SYNTHETIC TEXT\n");
    assert_eq!(result["engine"], "tesseract");
    assert!(output.stderr.is_empty());
    assert_eq!(std::fs::read(source).unwrap(), bytes);
    f.assert_no_user_state();
}

#[cfg(unix)]
#[test]
fn overflowing_engine_streams_fail_and_are_not_logged() {
    for stream in ["", " >&2"] {
        let f = Fixture::new();
        let source = f.path("synthetic.png");
        RgbaImage::from_pixel(2, 2, Rgba([255, 255, 255, 255]))
            .save(&source)
            .unwrap();
        let engine = f.fake_engine(&format!("while IFS= read -r line; do :; done\nwhile :; do printf 'PRIVATE CONTENT PRIVATE CONTENT PRIVATE CONTENT PRIVATE CONTENT'{stream}; done"));
        let output = f
            .command()
            .arg(source)
            .env("PATH", engine)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("PRIVATE CONTENT"));
        f.assert_no_user_state();
    }
}

#[cfg(unix)]
fn printed_fixture() -> RgbaImage {
    let glyphs = [
        [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        [
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
    ];
    let x_scale = 8;
    let y_scale = 10;
    let mut image = RgbaImage::from_pixel(640, 180, Rgba([255, 255, 255, 255]));
    for (index, glyph) in glyphs.iter().enumerate() {
        for (row, bits) in glyph.iter().enumerate() {
            for column in 0..5 {
                if bits & (1 << (4 - column)) != 0 {
                    for y in 0..y_scale {
                        for x in 0..x_scale {
                            image.put_pixel(
                                30 + index as u32 * 50
                                    + if index >= 5 { 40 } else { 0 }
                                    + column * x_scale
                                    + x,
                                40 + row as u32 * y_scale + y,
                                Rgba([0, 0, 0, 255]),
                            );
                        }
                    }
                }
            }
        }
    }
    image
}

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}

#[cfg(unix)]
struct GroupCleanup(i32);

#[cfg(unix)]
impl Drop for GroupCleanup {
    fn drop(&mut self) {
        unsafe {
            kill(-self.0, 9);
        }
    }
}

#[cfg(unix)]
fn terminated(pid: i32) -> bool {
    if unsafe { kill(pid, 0) } == -1 {
        return true;
    }
    #[cfg(target_os = "linux")]
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        // A killed orphan may briefly await init's reap; it cannot retain pipe handles.
        return stat
            .rsplit_once(") ")
            .is_some_and(|(_, fields)| fields.starts_with("Z "));
    }
    false
}

#[cfg(unix)]
fn assert_engine_group_stopped(group: i32, descendant: i32) {
    use std::time::{Duration, Instant};
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let group_exists = unsafe { kill(-group, 0) } == 0;
        let direct_stopped = terminated(group);
        let descendant_stopped = terminated(descendant);
        // On Linux an orphan zombie keeps its PGID until init reaps it.
        if !group_exists || (cfg!(target_os = "linux") && direct_stopped && descendant_stopped) {
            assert!(direct_stopped, "Direct engine is still running");
            assert!(descendant_stopped, "Engine descendant is still running");
            return;
        }
        if Instant::now() >= deadline {
            unsafe {
                kill(-group, 9);
            }
            panic!("OCR engine process group still has a running descendant");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
fn engine_with_descendant(parent_exits: bool) -> (Fixture, PathBuf, std::process::Child, i32, i32) {
    use std::{
        process::Stdio,
        time::{Duration, Instant},
    };
    let f = Fixture::new();
    let source = f.path("synthetic.png");
    RgbaImage::from_pixel(2, 2, Rgba([255, 255, 255, 255]))
        .save(&source)
        .unwrap();
    let ending = if parent_exits { "exit 0" } else { "wait" };
    let engine = f.fake_engine(&format!("while IFS= read -r line; do :; done\n/bin/sleep 60 &\nprintf '%s %s' \"$$\" \"$!\" > \"$OCR_TEST_MARKER\"\n{ending}"));
    let marker = f.path("engine.pid");
    let mut child = f
        .command()
        .arg(&source)
        .env("PATH", engine)
        .env("OCR_TEST_MARKER", &marker)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let (group, descendant) = loop {
        if let Ok(text) = std::fs::read_to_string(&marker) {
            let pids = text
                .split_whitespace()
                .map(str::parse::<i32>)
                .collect::<Result<Vec<_>, _>>();
            if let Ok(pids) = pids {
                if pids.len() == 2 && pids.iter().all(|pid| *pid > 0) {
                    break (pids[0], pids[1]);
                }
            }
        }
        if child.try_wait().unwrap().is_some() || Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            panic!(
                "OCR fixture did not start: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    (f, source, child, group, descendant)
}

#[cfg(unix)]
fn wait_for_cli(
    mut child: std::process::Child,
    group: i32,
    timeout: std::time::Duration,
) -> Output {
    use std::time::{Duration, Instant};
    let deadline = Instant::now() + timeout;
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            unsafe {
                kill(-group, 9);
            }
            let _ = child.kill();
            let _ = child.wait();
            panic!("OCR subprocess did not stop within its test deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}

#[cfg(unix)]
#[test]
fn interrupt_and_terminate_reap_engine_descendants_without_recognized_output() {
    for signal in [2, 15] {
        let (f, source, child, group, descendant) = engine_with_descendant(false);
        let _cleanup = GroupCleanup(group);
        let original = std::fs::read(&source).unwrap();
        unsafe {
            assert_eq!(kill(child.id() as i32, signal), 0);
        }
        let output = wait_for_cli(child, group, std::time::Duration::from_secs(5));
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("OCR canceled"));
        assert_engine_group_stopped(group, descendant);
        assert_eq!(std::fs::read(source).unwrap(), original);
        f.assert_no_user_state();
    }
}

#[cfg(unix)]
#[test]
fn deadline_kills_descendant_held_pipes_after_the_engine_parent_exits() {
    use std::time::{Duration, Instant};
    let (f, source, child, group, descendant) = engine_with_descendant(true);
    let _cleanup = GroupCleanup(group);
    let original = std::fs::read(&source).unwrap();
    let started = Instant::now();
    let output = wait_for_cli(child, group, Duration::from_secs(40));
    assert!(
        started.elapsed() >= Duration::from_secs(27),
        "Fixture did not exercise the engine deadline"
    );
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("30 second deadline"));
    assert_engine_group_stopped(group, descendant);
    assert_eq!(std::fs::read(source).unwrap(), original);
    f.assert_no_user_state();
}

#[cfg(unix)]
#[test]
fn terminate_stops_blocked_final_stdout_after_engine_cleanup() {
    use std::{
        io::Read,
        process::Stdio,
        time::{Duration, Instant},
    };
    let f = Fixture::new();
    let source = f.path("synthetic.png");
    RgbaImage::from_pixel(2, 2, Rgba([255, 255, 255, 255]))
        .save(&source)
        .unwrap();
    let original = std::fs::read(&source).unwrap();
    let engine = f.fake_engine("while IFS= read -r line; do :; done\ni=0\nwhile [ \"$i\" -lt 3200 ]; do\n  printf 'XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX'\n  i=$((i+1))\ndone");
    let mut child = f
        .command()
        .arg(&source)
        .env("PATH", engine)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut prefix = [0];
    stdout.read_exact(&mut prefix).unwrap();
    assert_eq!(prefix[0], b'{');
    // JSON output starts only after recognition and its cancellation guard have returned.
    // Keep the remaining 200 KiB undrained so the final stdout write blocks.
    std::thread::sleep(Duration::from_millis(50));
    assert!(child.try_wait().unwrap().is_none());
    unsafe {
        assert_eq!(kill(child.id() as i32, 15), 0);
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("OCR blocked stdout ignored SIGTERM after engine cleanup");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert_eq!(std::fs::read(source).unwrap(), original);
    f.assert_no_user_state();
}

#[cfg(unix)]
#[test]
#[ignore = "runs the explicitly installed tesseract engine on synthetic printed text; no desktop, uploads or clipboard"]
fn real_tesseract_recognizes_synthetic_printed_text_without_user_state() {
    let f = Fixture::new();
    let source = f.path("synthetic printed text.png");
    printed_fixture().save(&source).unwrap();
    let bytes = std::fs::read(&source).unwrap();
    let output = f.run(&source, &["--language", "deu"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let normalized: String = result["text"]
        .as_str()
        .unwrap()
        .chars()
        .filter(|value| value.is_ascii_alphanumeric())
        .collect();
    assert_eq!(normalized, "HELLO123");
    assert_eq!(std::fs::read(source).unwrap(), bytes);
    f.assert_no_user_state();
}
