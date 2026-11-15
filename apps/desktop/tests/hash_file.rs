use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("screenshots-hash-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command
            .arg("hash-file")
            .env("HOME", self.path("home"))
            .env("USERPROFILE", self.path("userprofile"))
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env("XDG_CACHE_HOME", self.path("cache"))
            .env("APPDATA", self.path("appdata"))
            .env("LOCALAPPDATA", self.path("localappdata"))
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY");
        command
    }

    fn run(&self, input: &Path, options: &[&str]) -> Output {
        self.command().arg(input).args(options).output().unwrap()
    }

    fn no_user_state(&self) {
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
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn success(output: Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).unwrap()
}

fn failure(output: Output) -> String {
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
    String::from_utf8(output.stderr).unwrap()
}

#[test]
fn known_vectors_preserve_files_and_do_not_initialize_user_state() {
    let fixture = Fixture::new();
    for (name, contents, digest) in [
        ("empty", b"".as_slice(), EMPTY),
        ("Größe 東京 $(ignored).txt", b"abc".as_slice(), ABC),
    ] {
        let input = fixture.path(name);
        std::fs::write(&input, contents).unwrap();
        assert_eq!(
            success(fixture.run(&input, &[])),
            serde_json::json!({
                "algorithm":"sha256", "digest":digest, "bytes":contents.len()
            })
        );
        assert_eq!(std::fs::read(&input).unwrap(), contents);
    }
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 2);
    fixture.no_user_state();
}

#[test]
fn binary_input_spanning_many_read_buffers_has_the_expected_digest() {
    let fixture = Fixture::new();
    let input = fixture.path("binary");
    let bytes: Vec<u8> = (0..=255).cycle().take(1024 * 1024).collect();
    std::fs::write(&input, &bytes).unwrap();
    assert_eq!(
        success(fixture.run(&input, &[])),
        serde_json::json!({
            "algorithm":"sha256",
            "digest":"fbbab289f7f94b25736c58be46a994c441fd02552cc6022352e3d86d2fab7c83",
            "bytes":1024 * 1024
        })
    );
    assert_eq!(std::fs::read(input).unwrap(), bytes);
    fixture.no_user_state();
}

#[test]
fn expectations_are_case_insensitive_and_mismatch_still_reports_the_digest() {
    let fixture = Fixture::new();
    let input = fixture.path("abc");
    std::fs::write(&input, b"abc").unwrap();
    let matched = success(fixture.run(&input, &["--expect", &ABC.to_ascii_uppercase()]));
    assert_eq!(
        matched,
        serde_json::json!({
            "algorithm":"sha256", "digest":ABC, "bytes":3, "matches":true
        })
    );
    let mismatch = fixture.run(&input, &["--expect", EMPTY]);
    assert_eq!(mismatch.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&mismatch.stdout).unwrap(),
        serde_json::json!({
            "algorithm":"sha256", "digest":ABC, "bytes":3, "matches":false
        })
    );
    assert!(
        String::from_utf8(mismatch.stderr)
            .unwrap()
            .contains("SHA-256 digest does not match expected value")
    );
    assert_eq!(std::fs::read(input).unwrap(), b"abc");
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
    fixture.no_user_state();
}

#[test]
fn malformed_arguments_are_rejected_before_opening_input() {
    let fixture = Fixture::new();
    let missing = fixture.path("missing");
    for invalid in [
        "".to_owned(),
        "a".repeat(63),
        "a".repeat(65),
        "g".repeat(64),
        "é".repeat(32),
        format!(" {ABC}"),
    ] {
        let error = failure(fixture.run(&missing, &["--expect", &invalid]));
        assert!(error.contains("Expected SHA-256 must be exactly 64 ASCII hexadecimal digits"));
        assert!(!error.contains("Could not inspect hash input"));
    }
    for options in [
        &["--expect"][..],
        &["--other", ABC],
        &["--expect", ABC, "extra"],
    ] {
        assert!(failure(fixture.run(&missing, options)).contains("Usage:"));
    }
    assert!(failure(fixture.command().output().unwrap()).contains("Usage:"));
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 0);
    fixture.no_user_state();
}

#[test]
fn missing_directory_and_oversized_sparse_inputs_fail_without_outputs() {
    let fixture = Fixture::new();
    failure(fixture.run(&fixture.path("missing"), &[]));
    assert!(failure(fixture.run(&fixture.0, &[])).contains("regular file"));
    let input = fixture.path("oversized");
    std::fs::File::create(&input)
        .unwrap()
        .set_len(1024 * 1024 * 1024 + 1)
        .unwrap();
    assert!(failure(fixture.run(&input, &[])).contains("1 GiB limit"));
    assert_eq!(
        std::fs::metadata(input).unwrap().len(),
        1024 * 1024 * 1024 + 1
    );
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
    fixture.no_user_state();
}

#[cfg(unix)]
#[test]
fn non_utf8_paths_work_and_non_utf8_expectations_fail() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt as _};
    let fixture = Fixture::new();
    let input = fixture.0.join(OsString::from_vec(b"hash-\xff".to_vec()));
    std::fs::write(&input, b"abc").unwrap();
    assert_eq!(success(fixture.run(&input, &[]))["digest"], ABC);
    let output = fixture
        .command()
        .arg(&input)
        .arg("--expect")
        .arg(OsString::from_vec(vec![0xff; 64]))
        .output()
        .unwrap();
    assert!(failure(output).contains("64 ASCII hexadecimal digits"));
    assert_eq!(std::fs::read(input).unwrap(), b"abc");
    fixture.no_user_state();
}

#[cfg(unix)]
#[test]
fn fifo_is_rejected_without_waiting_for_a_writer() {
    use std::{
        ffi::CString,
        os::unix::ffi::OsStrExt as _,
        process::Stdio,
        time::{Duration, Instant},
    };
    let fixture = Fixture::new();
    let input = fixture.path("fifo");
    let path = CString::new(input.as_os_str().as_bytes()).unwrap();
    // SAFETY: path is a live NUL-terminated CString and the mode has no pointers.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    let mut child = fixture
        .command()
        .arg(&input)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > Duration::from_secs(5) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("Hashing a FIFO must not wait for a writer");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(failure(child.wait_with_output().unwrap()).contains("regular file"));
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
    fixture.no_user_state();
}
