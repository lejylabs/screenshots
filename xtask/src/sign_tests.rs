//! Synthetic signing-flow tests: no real certificate, keychain, printer or account calls.
//! Port of the former `scripts/test-native-signing.py` (runtime-manifest cases were
//! dropped together with the separate SXCU runtime).
use super::*;
use serde_json::json;
use std::{cell::RefCell, fs, time::Instant};

fn mac() -> Value {
    json!({"version": 1, "platform": "macos", "application_identity": "Developer ID Application: Test (ABCDEFGHIJ)",
           "installer_identity": "Developer ID Installer: Test (ABCDEFGHIJ)", "team_id": "ABCDEFGHIJ"})
}
fn windows() -> Value {
    json!({"version": 1, "platform": "windows", "thumbprint": "a".repeat(40), "publisher": "CN=Test \"quoted\" & Company",
           "timestamp_url": "https://timestamp.example.invalid/"})
}

/// Records calls and imitates signer side effects on the named files.
struct Fake {
    calls: RefCell<Vec<Vec<String>>>,
    reject: bool,
    status: String,
    leaf: String,
}
impl Fake {
    fn new() -> Self {
        Self { calls: RefCell::default(), reject: false, status: "signed by a certificate trusted by macOS".into(), leaf: mac()["installer_identity"].as_str().unwrap().into() }
    }
    fn calls(&self) -> Vec<Vec<String>> {
        self.calls.borrow().clone()
    }
    fn any(&self, prefix: &[&str]) -> bool {
        self.calls().iter().any(|c| c.len() >= prefix.len() && c.iter().zip(prefix).all(|(a, b)| a == b))
    }
}
impl Tools for Fake {
    fn run(&self, args: &[OsString], _: Duration, _: bool) -> Result<Vec<u8>> {
        let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        self.calls.borrow_mut().push(args.clone());
        let last = PathBuf::from(args.last().cloned().unwrap_or_default());
        let append = |path: &Path| fs::write(path, [fs::read(path).unwrap(), b"SYNTHETIC-SIGNATURE".to_vec()].concat()).unwrap();
        match args[0].as_str() {
            "codesign" if args.contains(&"--force".into()) => append(&if last.is_dir() { last.join("Contents/MacOS/screenshots") } else { last }),
            "signtool.exe" if args[1] == "sign" => append(&last),
            "productsign" => {
                fs::copy(&args[args.len() - 2], &last)?;
            }
            "xcrun" if args[1] == "notarytool" => return Ok(json!({"status": if self.reject { "Rejected" } else { "Accepted" }}).to_string().into_bytes()),
            "pkgutil" => {
                return Ok(format!("Status: {}\nCertificate Chain:\n 1. {}\n 2. Developer ID Certification Authority\n", self.status, self.leaf).into_bytes())
            }
            _ => {}
        }
        Ok(Vec::new())
    }
}

#[test]
fn exact_public_profiles_reject_secrets_and_wrong_platform_before_tools() {
    let dir = TempDir::new("xtask-sign-").unwrap();
    let path = dir.path().join("profile");
    for (expected, platform) in [(mac(), "macos"), (windows(), "windows")] {
        fs::write(&path, expected.to_string()).unwrap();
        assert_eq!(profile(&path, platform).unwrap(), expected);
    }
    let changed = |base: Value, key: &str, value: Value| {
        let mut v = base;
        v[key] = value;
        v
    };
    for bad in [
        changed(mac(), "password", "must-not-use".into()),
        changed(mac(), "version", true.into()),
        changed(mac(), "team_id", "wrong".into()),
        changed(mac(), "application_identity", "-".into()),
        changed(windows(), "timestamp_url", "http://unsafe.invalid".into()),
        changed(windows(), "thumbprint", "not-a-certificate".into()),
    ] {
        fs::write(&path, bad.to_string()).unwrap();
        assert!(profile(&path, bad["platform"].as_str().unwrap()).is_err(), "{bad}");
    }
    fs::write(&path, mac().to_string()).unwrap();
    assert!(profile(&path, "linux").is_err());
}

#[test]
fn publisher_is_exact_and_xml_escaped() {
    let manifest = crate::package_app::appx_manifest("0.1.0", "x64", windows()["publisher"].as_str().unwrap()).unwrap();
    assert!(manifest.contains("Publisher=\"CN=Test &quot;quoted&quot; &amp; Company\""));
}

#[test]
fn mac_bundle_is_signed_last_and_verified_against_the_team() {
    let dir = TempDir::new("xtask-sign-").unwrap();
    let app = dir.path().join("Screenshots.app");
    let binary = app.join("Contents/MacOS/screenshots");
    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    fs::create_dir_all(app.join("Contents/Resources")).unwrap();
    fs::write(&binary, b"unsigned main").unwrap();
    let info = json!({"version": "0.1.0", "target": "aarch64-apple-darwin", "binary_sha256": sha(&binary).unwrap()});
    let tools = Fake::new();
    let result = mac_app(&tools, &app, &info, &mac()).unwrap();
    assert_ne!(result["binary_sha256"], sha(&binary).unwrap(), "recorded hash is the pre-signature input");
    assert_eq!(result["binary_sha256_scope"], "admitted executable before enclosing bundle signature");
    let signed: Vec<_> = tools.calls().into_iter().filter(|c| c.contains(&"--force".into())).collect();
    assert_eq!(PathBuf::from(signed.last().unwrap().last().unwrap()).canonicalize().unwrap(), app.canonicalize().unwrap());
    assert!(!signed.iter().any(|c| c.contains(&"--entitlements".into())), "no entitlements are requested");
    assert!(tools.calls().iter().any(|c| c.contains(&"--verify".into()) && c.join(" ").contains("ABCDEFGHIJ")));
    let written: Value = serde_json::from_slice(&fs::read(app.join("Contents/Resources/BUILD-INFO.json")).unwrap()).unwrap();
    assert_eq!(written, result);
}

#[test]
fn windows_payload_tracks_signed_bytes_and_timestamp_algorithm() {
    let dir = TempDir::new("xtask-sign-").unwrap();
    fs::write(dir.path().join("screenshots.exe"), b"native exe").unwrap();
    fs::write(dir.path().join("screenshots-install.exe"), b"installer exe").unwrap();
    let tools = Fake::new();
    let result = windows_payload(&tools, dir.path(), &json!({"target": "x86_64-pc-windows-msvc"}), &windows()).unwrap();
    assert_eq!(result["binary_sha256"], sha(&dir.path().join("screenshots.exe")).unwrap());
    let signs: Vec<_> = tools.calls().into_iter().filter(|c| c[1] == "sign").collect();
    assert_eq!(signs.len(), 2);
    for call in signs {
        let at = |flag: &str| call.iter().position(|a| a == flag).unwrap();
        assert!(at("/tr") < at("/td"));
        assert!(call.contains(&"/sha1".into()) && !call.contains(&"/p".into()));
    }
}

#[test]
fn accepted_notary_staples_only_the_signed_package() {
    let dir = TempDir::new("xtask-sign-").unwrap();
    let (source, destination) = (dir.path().join("unsigned.pkg"), dir.path().join("signed.pkg"));
    fs::write(&source, b"unsigned").unwrap();
    let tools = Fake::new();
    mac_package(&tools, &source, &destination, &mac(), Some("existing-profile")).unwrap();
    assert!(tools.calls().iter().any(|c| c[..3] == ["xcrun", "stapler", "staple"] && Path::new(c.last().unwrap()) == destination));
    assert!(tools.any(&["xcrun", "stapler", "validate"]));
}

#[test]
fn notary_rejection_does_not_staple_or_publish() {
    let dir = TempDir::new("xtask-sign-").unwrap();
    let source = dir.path().join("unsigned.pkg");
    fs::write(&source, b"unsigned").unwrap();
    let tools = Fake { reject: true, ..Fake::new() };
    assert!(mac_package(&tools, &source, &dir.path().join("signed.pkg"), &mac(), Some("already-existing-profile")).is_err());
    assert!(!tools.any(&["xcrun", "stapler"]));
    assert_eq!(fs::read(&source).unwrap(), b"unsigned");
}

#[test]
fn installer_status_allows_exact_trusted_variants_and_refuses_other_statuses() {
    let trusted = ["signed by a certificate trusted by macOS", "signed by a certificate trusted by Mac OS X", "signed by a developer certificate issued by Apple for distribution"];
    let refused = ["no signature", "signed by an untrusted certificate", "signed by a developer certificate issued by Apple for distribution (expired)", "unsigned\nNotarization: trusted by the Apple notary service"];
    let dir = TempDir::new("xtask-sign-").unwrap();
    let source = dir.path().join("unsigned");
    fs::write(&source, b"input").unwrap();
    for (index, status) in trusted.iter().chain(refused.iter()).enumerate() {
        let tools = Fake { status: (*status).into(), ..Fake::new() };
        let destination = dir.path().join(index.to_string());
        if trusted.contains(status) {
            mac_package(&tools, &source, &destination, &mac(), None).unwrap();
        } else {
            assert!(mac_package(&tools, &source, &destination, &mac(), Some("existing-profile")).is_err(), "{status}");
            assert!(!tools.any(&["xcrun", "notarytool"]));
        }
    }
}

#[test]
fn mismatched_installer_certificate_is_refused_before_notary() {
    let dir = TempDir::new("xtask-sign-").unwrap();
    let source = dir.path().join("unsigned");
    fs::write(&source, b"input").unwrap();
    let tools = Fake { leaf: "Developer ID Installer: Other (ZZZZZZZZZZ)".into(), ..Fake::new() };
    assert!(mac_package(&tools, &source, &dir.path().join("signed"), &mac(), Some("existing-profile")).is_err());
    assert!(!tools.any(&["xcrun", "notarytool"]));
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn bare_path_lookalike_cannot_be_native_verifier() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new("xtask-sign-").unwrap();
    let marker = dir.path().join("marker");
    let fake = dir.path().join("codesign");
    fs::write(&fake, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    // The protected macOS path does not exist here, so the tool must be refused.
    assert!(Native.run(&args(&[&"codesign", &"--verify", &"file"]), Duration::from_secs(5), false).is_err());
    assert!(!marker.exists());
    assert!(!path.is_empty());
}

fn shell(script: &str) -> (PathBuf, Vec<OsString>) {
    if cfg!(windows) {
        (PathBuf::from(std::env::var("ComSpec").unwrap_or_else(|_| r"C:\Windows\System32\cmd.exe".into())), vec!["/d".into(), "/c".into(), script.into()])
    } else {
        (PathBuf::from("/bin/sh"), vec!["-c".into(), script.into()])
    }
}

#[test]
fn native_runner_uses_protected_path_private_cwd_and_clean_environment() {
    let script = if cfg!(windows) { "cd & echo %PATH% & echo [%DEVELOPER_DIR%]" } else { "pwd; echo \"$PATH\"; echo \"[$DEVELOPER_DIR]\"" };
    let (program, arguments) = shell(script);
    // SAFETY: only this test reads or writes DEVELOPER_DIR.
    unsafe { std::env::set_var("DEVELOPER_DIR", "untrusted-developer-dir") };
    let output = String::from_utf8(run_program(&program, &arguments, true, Duration::from_secs(20), true).unwrap()).unwrap();
    unsafe { std::env::remove_var("DEVELOPER_DIR") };
    let lines: Vec<&str> = output.lines().map(str::trim).collect();
    let workspace = std::env::current_dir().unwrap();
    assert_ne!(Path::new(lines[0]), workspace, "tool runs in a private working directory");
    assert_eq!(lines[1], protected_path().unwrap());
    assert!(!output.contains("untrusted-developer-dir"));
}

#[test]
fn ended_tool_cannot_leave_a_pipe_holding_descendant() {
    let script = if cfg!(windows) { "start /b ping -n 30 127.0.0.1 >nul & echo public" } else { "sleep 30 & echo public" };
    let (program, arguments) = shell(script);
    let started = Instant::now();
    let output = run_program(&program, &arguments, false, Duration::from_secs(10), true).unwrap();
    assert_eq!(String::from_utf8(output).unwrap().trim(), "public");
    assert!(started.elapsed() < Duration::from_secs(8), "descendant kept the result pipe open");
}

#[test]
fn bounded_native_result_and_timeout() {
    let (program, arguments) = shell("echo public");
    assert_eq!(String::from_utf8(run_program(&program, &arguments, false, Duration::from_secs(10), true).unwrap()).unwrap().trim(), "public");
    let large = if cfg!(windows) {
        "for /l %i in (1,1,20000) do @echo xxxxxxxxxxxxxxxxxxxx"
    } else {
        "i=0; while [ $i -lt 20000 ]; do echo xxxxxxxxxxxxxxxxxxxx; i=$((i+1)); done"
    };
    let (program, arguments) = shell(large);
    assert!(run_program(&program, &arguments, false, Duration::from_secs(30), true).is_err(), "result above 128 KiB is refused");
    let (program, arguments) = shell(if cfg!(windows) { "ping -n 10 127.0.0.1 >nul" } else { "sleep 10" });
    let started = Instant::now();
    assert!(run_program(&program, &arguments, false, Duration::from_millis(200), false).is_err());
    assert!(started.elapsed() < Duration::from_secs(5));
}
