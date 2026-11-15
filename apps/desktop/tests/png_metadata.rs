use image::{Rgba, RgbaImage};
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("screenshots-png-metadata-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn command(&self, input: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command
            .args(["image", "metadata"])
            .arg(input)
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

    fn run(&self, input: &Path) -> Output {
        self.command(input).output().unwrap()
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

fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
    bytes.extend_from_slice(kind);
    bytes.extend_from_slice(data);
    bytes.extend_from_slice(&crc32fast::hash(&bytes[4..]).to_be_bytes());
    bytes
}

fn png(extra: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend(chunk(b"IHDR", &[0, 0, 0, 2, 0, 0, 0, 1, 8, 6, 0, 0, 0]));
    for entry in extra {
        bytes.extend(entry);
    }
    // Inventory is deliberately independent of the compressed image stream.
    bytes.extend(chunk(b"IDAT", b"not decoded"));
    bytes.extend(chunk(b"IEND", b""));
    bytes
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

fn failure(output: Output) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

#[test]
fn metadata_inventory_never_prints_payload_values_or_changes_source() {
    let fixture = Fixture::new();
    let input = fixture.path("Größe 東京 $(ignored).data");
    let secret = b"private-key-do-not-print";
    let mut text = b"Comment\0".to_vec();
    text.extend(secret);
    let mut compressed_text = b"Comment\0\0".to_vec();
    compressed_text.extend(secret);
    let mut profile = b"ICC\0\0".to_vec();
    profile.extend(secret);
    let bytes = png(&[
        chunk(b"tEXt", &text),
        chunk(b"zTXt", &compressed_text),
        chunk(b"iCCP", &profile),
        chunk(b"eXIf", secret),
        chunk(b"vpAg", secret),
    ]);
    std::fs::write(&input, &bytes).unwrap();
    let output = fixture.run(&input);
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains(std::str::from_utf8(secret).unwrap())
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains(std::str::from_utf8(secret).unwrap())
    );
    let result = success(output);
    assert_eq!(result["width"], 2);
    assert_eq!(result["height"], 1);
    assert_eq!(result["bit_depth"], 8);
    assert_eq!(result["color_type"], 6);
    assert_eq!(result["metadata_present"], true);
    assert_eq!(result["animated"], false);
    assert_eq!(result["source_file_bytes"], bytes.len());
    assert_eq!(
        result["chunks"][1],
        serde_json::json!({"type":"tEXt","bytes":text.len()})
    );
    for entry in result["chunks"].as_array().unwrap() {
        assert_eq!(entry.as_object().unwrap().len(), 2);
    }
    assert_eq!(std::fs::read(&input).unwrap(), bytes);
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
    fixture.no_user_state();
}

#[test]
fn real_png_and_animation_declaration_are_reported_without_decoding() {
    let fixture = Fixture::new();
    let input = fixture.path("plain.png");
    RgbaImage::from_pixel(2, 1, Rgba([10, 20, 30, 40]))
        .save(&input)
        .unwrap();
    let result = success(fixture.run(&input));
    assert_eq!(result["metadata_present"], false);
    assert_eq!(result["animated"], false);
    let mut animation = Vec::new();
    animation.extend_from_slice(&1u32.to_be_bytes());
    animation.extend_from_slice(&0u32.to_be_bytes());
    let bytes = png(&[chunk(b"acTL", &animation)]);
    std::fs::write(&input, &bytes).unwrap();
    let result = success(fixture.run(&input));
    assert_eq!(result["animated"], true);
    assert_eq!(result["metadata_present"], true);
    assert_eq!(std::fs::read(&input).unwrap(), bytes);
    fixture.no_user_state();
}

#[test]
fn malformed_envelopes_crc_order_and_headers_fail_without_partial_output() {
    let fixture = Fixture::new();
    let input = fixture.path("bad.png");
    let baseline = png(&[]);
    let mut corrupt_crc = baseline.clone();
    corrupt_crc[29] ^= 1;
    let mut corrupt_length = baseline.clone();
    corrupt_length[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
    let mut trailing = baseline.clone();
    trailing.push(0);
    let mut nonconsecutive = baseline[..baseline.len() - 12].to_vec();
    nonconsecutive.extend(chunk(b"tEXt", b"Comment\0value"));
    nonconsecutive.extend(chunk(b"IDAT", b""));
    nonconsecutive.extend(chunk(b"IEND", b""));
    let mut ihdr_not_first = b"\x89PNG\r\n\x1a\n".to_vec();
    ihdr_not_first.extend(chunk(b"tEXt", b"Comment\0value"));
    ihdr_not_first.extend_from_slice(&baseline[8..]);
    let mut cases = vec![
        b"not PNG".to_vec(),
        baseline[..baseline.len() - 1].to_vec(),
        baseline[..baseline.len() - 12].to_vec(),
        corrupt_crc,
        corrupt_length,
        trailing,
        nonconsecutive,
        ihdr_not_first,
        png(&[chunk(b"ABCD", b"value")]),
        png(&[chunk(b"aaad", b"value")]),
        png(&[chunk(b"acTL", b"short")]),
    ];
    for header in [
        [0, 0, 0, 0, 0, 0, 0, 1, 8, 6, 0, 0, 0],
        [0, 0, 0, 1, 0, 0, 0, 1, 4, 6, 0, 0, 0],
        [0, 1, 0, 0, 0, 1, 0, 0, 8, 6, 0, 0, 0],
        [0, 0, 0, 1, 0, 0, 0, 1, 8, 3, 0, 0, 0],
        [0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 2],
    ] {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend(chunk(b"IHDR", &header));
        bytes.extend_from_slice(&baseline[33..]);
        cases.push(bytes);
    }
    for bytes in cases {
        std::fs::write(&input, &bytes).unwrap();
        failure(fixture.run(&input));
        assert_eq!(std::fs::read(&input).unwrap(), bytes);
    }
    fixture.no_user_state();
}

#[test]
fn chunk_count_file_size_missing_paths_and_argument_limits_are_enforced() {
    let fixture = Fixture::new();
    let input = fixture.path("many.png");
    std::fs::write(&input, png(&vec![chunk(b"vpAg", b""); 4093])).unwrap();
    assert_eq!(
        success(fixture.run(&input))["chunks"]
            .as_array()
            .unwrap()
            .len(),
        4096
    );
    std::fs::write(&input, png(&vec![chunk(b"vpAg", b""); 4094])).unwrap();
    failure(fixture.run(&input));
    std::fs::File::create(&input)
        .unwrap()
        .set_len(32 * 1024 * 1024 + 1)
        .unwrap();
    failure(fixture.run(&input));
    failure(fixture.run(&fixture.0));
    failure(fixture.run(&fixture.path("missing.png")));
    failure(fixture.command(&input).arg("extra").output().unwrap());
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
    fixture.no_user_state();
}

#[cfg(unix)]
#[test]
fn fifo_input_is_rejected_without_waiting_for_a_writer() {
    use std::{
        ffi::CString,
        os::unix::ffi::OsStrExt as _,
        process::Stdio,
        time::{Duration, Instant},
    };
    let fixture = Fixture::new();
    let input = fixture.path("fifo.png");
    let path = CString::new(input.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    let mut child = fixture
        .command(&input)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if started.elapsed() > Duration::from_secs(5) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("Metadata inspection blocked on a FIFO");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    failure(child.wait_with_output().unwrap());
    fixture.no_user_state();
}
