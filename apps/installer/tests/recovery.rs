//! Isolated user installation/update verification; no user data or desktop writes.
//! Port of the former `scripts/test-install-user.py`. Crashes are injected into the
//! real `screenshots-install` binary through its debug-only failpoints.
use screenshots_install::{self as install, InstallationLock, Outcome, TempDir, archive, executable_name, local_target, sha, sha_bytes};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const STATE: &str = ".screenshots-install.json";
const TRANSACTION: &str = ".screenshots-transaction.json";
const KEY: &str = "update-public-key.pem";

enum Extra {
    None,
    File(&'static str),
    Symlink(&'static str, &'static str),
}

fn package(directory: &Path, version: &str, extra: Extra) -> PathBuf {
    let target = local_target().unwrap();
    let name = format!("screenshots-{version}");
    let destination = directory.join(format!("{name}.tar.gz"));
    let binary = format!("synthetic executable {version}").into_bytes();
    let executable = executable_name(&target);
    let info = json!({"version": version, "target": target, "binary_sha256": sha_bytes(&binary)}).to_string();
    let mut members = vec![(executable.to_owned(), 0o755, binary), ("BUILD-INFO.json".to_owned(), 0o644, info.into_bytes())];
    for item in ["LICENSE", "README.md", "Cargo.lock", "THIRD_PARTY_NOTICES.md"] {
        members.push((item.to_owned(), 0o644, format!("{item} fixture").into_bytes()));
    }
    let file = fs::File::create(&destination).unwrap();
    let mut writer = archive::TarWriter::new(archive::gzip_writer(file, 0));
    for (path, mode, data) in &members {
        writer.file(&format!("{name}/{path}"), *mode, 0, &mut &data[..], data.len() as u64).unwrap();
    }
    match extra {
        Extra::None => {}
        Extra::File(raw) => writer.file(raw, 0o644, 0, &mut &b""[..], 0).unwrap(),
        Extra::Symlink(raw, target) => writer.symlink(raw, target, 0).unwrap(),
    }
    writer.finish().unwrap().finish().unwrap();
    let digest = sha(&destination).unwrap();
    fs::write(directory.join(format!("{name}.tar.gz.sha256")), format!("{digest}  {name}.tar.gz\n")).unwrap();
    destination
}

struct Env {
    _dir: TempDir,
    directory: PathBuf,
    root: PathBuf,
}
impl Env {
    fn new() -> Self {
        let dir = TempDir::new("screenshots-install-test-").unwrap();
        let directory = dir.path().to_path_buf();
        let root = directory.join("owned install");
        Self { _dir: dir, directory, root }
    }
    fn package(&self, version: &str) -> PathBuf {
        package(&self.directory, version, Extra::None)
    }
}
fn field(outcome: &Outcome, key: &str) -> Value {
    outcome.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone()).unwrap_or(Value::Null)
}
fn status(root: &Path) -> Outcome {
    install::status(root).unwrap()
}
fn version(root: &Path) -> Value {
    field(&status(root), "version")
}
fn state(root: &Path) -> Value {
    serde_json::from_slice(&fs::read(root.join(STATE)).unwrap()).unwrap()
}
fn transaction(root: &Path) -> Value {
    serde_json::from_slice(&fs::read(root.join(TRANSACTION)).unwrap()).unwrap()
}
fn entries(path: &Path) -> Vec<String> {
    fs::read_dir(path).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect()
}
/// Run the real binary with an injected failpoint; returns its exit code.
fn child(failpoint: &str, args: &[&dyn AsRef<std::ffi::OsStr>]) -> i32 {
    Command::new(env!("CARGO_BIN_EXE_screenshots-install"))
        .args(args.iter().map(|a| a.as_ref()))
        .env("SCREENSHOTS_INSTALL_FAILPOINT", failpoint)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .code()
        .unwrap_or(-1)
}
fn ed25519_pem(byte: u8) -> Vec<u8> {
    use base64::Engine;
    let mut der = vec![0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];
    der.extend([byte; 32]);
    format!("-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n", base64::engine::general_purpose::STANDARD.encode(der)).into_bytes()
}

#[test]
fn install_status_idempotence_upgrade_rollback_and_uninstall() {
    let env = Env::new();
    let (first, second) = (env.package("0.1.0"), env.package("0.2.0"));
    assert_eq!(field(&install::install(&first, &env.root).unwrap(), "status"), "installed");
    assert_eq!(version(&env.root), "0.1.0");
    assert_eq!(field(&install::install(&first, &env.root).unwrap(), "status"), "unchanged");
    assert_eq!(field(&install::install(&second, &env.root).unwrap(), "status"), "upgraded");
    assert_eq!(version(&env.root), "0.2.0");
    assert_eq!(field(&status(&env.root), "previous_retained"), true);
    assert_eq!(field(&install::rollback(&env.root).unwrap(), "version"), "0.1.0");
    assert_eq!(version(&env.root), "0.1.0");
    let outside = env.directory.join("history.sqlite3");
    fs::write(&outside, b"user database preserved").unwrap();
    assert_eq!(field(&install::uninstall(&env.root).unwrap(), "status"), "uninstalled");
    assert!(!env.root.exists());
    assert_eq!(fs::read(&outside).unwrap(), b"user database preserved");
}

#[test]
fn modified_or_unowned_files_are_preserved() {
    let env = Env::new();
    let first = env.package("0.1.0");
    install::install(&first, &env.root).unwrap();
    let readme = env.root.join("versions").join(state(&env.root)["current"].as_str().unwrap()).join("README.md");
    fs::write(&readme, b"user edit").unwrap();
    assert!(install::uninstall(&env.root).is_err());
    assert_eq!(fs::read(&readme).unwrap(), b"user edit");
    assert!(install::install(&env.package("0.2.0"), &env.root).is_err());
    let other = env.directory.join("foreign");
    fs::create_dir(&other).unwrap();
    fs::write(other.join("private.png"), b"unrelated").unwrap();
    assert!(install::install(&first, &other).is_err());
    assert_eq!(fs::read(other.join("private.png")).unwrap(), b"unrelated");
}

#[test]
fn hostile_archive_and_checksum_are_rejected() {
    let env = Env::new();
    let bad = package(&env.directory, "0.1.0", Extra::File("../escape"));
    assert!(install::install(&bad, &env.root).is_err());
    assert!(!env.directory.join("escape").exists());
    let _ = fs::remove_dir_all(&env.root);
    let bad = env.package("0.2.0");
    let name = bad.file_name().unwrap().to_string_lossy().into_owned();
    fs::write(bad.with_file_name(format!("{name}.sha256")), format!("{}  {name}", "0".repeat(64))).unwrap();
    assert!(install::install(&bad, &env.root).is_err());
    assert!(!env.root.join(STATE).exists());
}

#[test]
fn symlink_member_and_root_are_rejected() {
    let env = Env::new();
    let bad = package(&env.directory, "0.1.0", Extra::Symlink("package/linked", "/unrelated"));
    assert!(install::install(&bad, &env.root).is_err());
    #[cfg(unix)]
    {
        let foreign = env.directory.join("foreign");
        fs::create_dir(&foreign).unwrap();
        let link = env.directory.join("linked-root");
        std::os::unix::fs::symlink(&foreign, &link).unwrap();
        assert!(install::install(&env.package("0.2.0"), &link).is_err());
    }
}

#[test]
fn archive_metadata_budgets_and_reserved_paths_are_rejected() {
    for name in ["root/NUL", "root/COM1.txt", "root/trailing.", "root/path:stream", "root/../outside"] {
        assert!(install::safe_name(name).is_err(), "{name}");
    }
    let env = Env::new();
    let unpack = env.directory.join("unpack");
    fs::create_dir(&unpack).unwrap();
    // Oversized pax metadata.
    let pax = env.directory.join("metadata.tar.gz");
    {
        let mut header = [0u8; 512];
        header[..3].copy_from_slice(b"pax");
        header[124..135].copy_from_slice(format!("{:011o}", 65537).as_bytes());
        header[156] = b'x';
        header[257..263].copy_from_slice(b"ustar\0");
        header[148..156].fill(b' ');
        let sum: u32 = header.iter().map(|b| u32::from(*b)).sum();
        header[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        let mut out = archive::gzip_writer(fs::File::create(&pax).unwrap(), 0);
        std::io::Write::write_all(&mut out, &header).unwrap();
        out.finish().unwrap();
    }
    assert!(install::extract(&pax, &unpack).is_err());
    // A ZIP end directory claiming more than 10000 entries.
    let zip = env.directory.join("directory.zip");
    let mut eocd = b"PK\x05\x06".to_vec();
    eocd.extend([0, 0, 0, 0]);
    eocd.extend(10001u16.to_le_bytes());
    eocd.extend(10001u16.to_le_bytes());
    eocd.extend([0; 10]);
    fs::write(&zip, eocd).unwrap();
    assert!(install::extract(&zip, &unpack).is_err());
    // Larger than the archive bound (sparse).
    let oversized = env.directory.join("oversized.tar.gz");
    fs::File::create(&oversized).unwrap().set_len(install::MAX_ARCHIVE + 1).unwrap();
    assert!(install::install(&oversized, &env.root).is_err());
}

#[test]
fn interrupted_launcher_publication_is_recoverable() {
    let env = Env::new();
    install::install(&env.package("0.1.0"), &env.root).unwrap();
    assert_eq!(child("publish_entry=error", &[&"install", &env.package("0.2.0"), &env.root]), 1);
    assert_eq!(field(&status(&env.root), "status"), "recovery_required");
    assert_eq!(field(&install::recover(&env.root, "previous").unwrap(), "version"), "0.1.0");
    assert_eq!(version(&env.root), "0.1.0");
    assert_eq!(field(&install::recover(&env.root, "previous").unwrap(), "version"), "0.2.0");
    assert_eq!(version(&env.root), "0.2.0");
}

#[test]
fn interrupted_previous_recovery_can_resume_either_choice() {
    let env = Env::new();
    let (first, second) = (env.package("0.1.0"), env.package("0.2.0"));
    for (choice, expected) in [("previous", "0.1.0"), ("current", "0.2.0")] {
        let root = env.directory.join(format!("recover-{choice}"));
        install::install(&first, &root).unwrap();
        assert_eq!(child("publish_entry=error", &[&"install", &second, &root]), 1);
        assert_eq!(child("publish_entry=exit:97", &[&"recover", &root, &"previous"]), 97);
        assert_eq!(field(&status(&root), "status"), "recovery_required");
        assert_eq!(field(&install::recover(&root, choice).unwrap(), "version"), expected);
        assert_eq!(version(&root), expected);
    }
}

#[test]
fn terminal_uninstall_interruption_is_idempotent_and_preserves_empty_root() {
    let env = Env::new();
    install::install(&env.package("0.1.0"), &env.root).unwrap();
    assert_eq!(child("finish_transaction.after=exit:98", &[&"uninstall", &env.root]), 98);
    assert!(env.root.is_dir());
    assert!(entries(&env.root).is_empty());
    assert_eq!(field(&status(&env.root), "status"), "not_installed");
    assert_eq!(field(&install::recover(&env.root, "current").unwrap(), "status"), "uninstalled");
    assert_eq!(field(&install::uninstall(&env.root).unwrap(), "status"), "uninstalled");
    assert!(env.root.is_dir());
    fs::write(env.root.join("user-file"), b"preserved").unwrap();
    assert!(install::uninstall(&env.root).is_err());
    assert_eq!(fs::read(env.root.join("user-file")).unwrap(), b"preserved");
}

#[cfg(unix)]
#[test]
fn terminal_uninstall_conflict_preserves_recovery_intent() {
    let env = Env::new();
    install::install(&env.package("0.1.0"), &env.root).unwrap();
    assert_eq!(child("delete_launcher=exit:99", &[&"uninstall", &env.root]), 99);
    let versions = env.root.join("versions");
    assert!(!versions.exists());
    std::os::unix::fs::symlink(env.directory.join("absent-user-directory"), &versions).unwrap();
    assert!(install::uninstall(&env.root).is_err());
    assert!(fs::symlink_metadata(&versions).unwrap().file_type().is_symlink());
    assert!(env.root.join(TRANSACTION).is_file() && env.root.join(STATE).is_file());
    assert!(env.root.join(executable_name(&local_target().unwrap())).is_file());
    fs::remove_file(&versions).unwrap();
    assert_eq!(field(&install::uninstall(&env.root).unwrap(), "status"), "uninstalled");
}

#[test]
fn first_install_crash_after_version_publication_recovers_without_state() {
    let env = Env::new();
    let archive = env.package("0.1.0");
    assert_eq!(child("write:.screenshots-install.json=exit:91", &[&"install", &archive, &env.root]), 91);
    assert!(!env.root.join(STATE).exists());
    assert!(env.root.join(TRANSACTION).is_file());
    let pending = status(&env.root);
    assert_eq!(field(&pending, "status"), "recovery_required");
    assert_eq!(field(&pending, "state_published"), false);
    assert_eq!(field(&install::recover(&env.root, "current").unwrap(), "version"), "0.1.0");
    assert_eq!(version(&env.root), "0.1.0");
    assert!(!env.root.join(TRANSACTION).exists());
    assert_eq!(field(&install::uninstall(&env.root).unwrap(), "status"), "uninstalled");
}

#[test]
fn first_install_preparation_crashes_leave_root_retryable() {
    let env = Env::new();
    let archive = env.package("0.1.0");
    for (index, failpoint) in ["extract.after=exit:93", "partial:.screenshots-transaction.json=exit:93"].iter().enumerate() {
        let root = env.directory.join(format!("preparation-{index}"));
        assert_eq!(child(failpoint, &[&"install", &archive, &root]), 93, "{failpoint}");
        assert!(entries(&root).is_empty(), "{failpoint}");
        assert_eq!(field(&install::install(&archive, &root).unwrap(), "status"), "installed");
        assert_eq!(field(&install::uninstall(&root).unwrap(), "status"), "uninstalled");
    }
}

#[test]
fn first_install_intent_precedes_version_directory_and_recovers() {
    let env = Env::new();
    assert_eq!(child("versions.mkdir=exit:94", &[&"install", &env.package("0.1.0"), &env.root]), 94);
    assert!(!env.root.join("versions").exists());
    assert_eq!(field(&status(&env.root), "status"), "recovery_required");
    assert_eq!(field(&install::recover(&env.root, "current").unwrap(), "version"), "0.1.0");
    assert_eq!(field(&install::uninstall(&env.root).unwrap(), "status"), "uninstalled");
}

#[test]
fn failed_intent_write_leaves_first_install_root_empty() {
    let env = Env::new();
    let archive = env.package("0.1.0");
    assert_eq!(child("write:.screenshots-transaction.json=error", &[&"install", &archive, &env.root]), 1);
    assert!(entries(&env.root).is_empty());
    assert!(!entries(&env.directory).iter().any(|n| n.starts_with(".screenshots-stage-")));
    assert_eq!(field(&install::install(&archive, &env.root).unwrap(), "status"), "installed");
}

#[test]
fn public_key_pin_crash_can_resume_or_cancel_without_unowned_key() {
    let env = Env::new();
    install::install(&env.package("0.1.0"), &env.root).unwrap();
    let public = env.directory.join("public.pem");
    fs::write(&public, ed25519_pem(1)).unwrap();
    assert_eq!(child("write:.screenshots-install.json=exit:92", &[&"pin-key", &env.root, &public]), 92);
    assert!(env.root.join(KEY).exists());
    assert!(state(&env.root)["key_sha256"].is_null());
    assert_eq!(field(&status(&env.root), "operation"), "pin");
    assert_eq!(field(&install::recover(&env.root, "previous").unwrap(), "update_key_pinned"), false);
    assert!(!env.root.join(KEY).exists());
    assert_eq!(child("write:.screenshots-install.json=exit:92", &[&"pin-key", &env.root, &public]), 92);
    assert_eq!(field(&install::pin_key(&env.root, &public).unwrap(), "recovered"), true);
    assert_eq!(field(&status(&env.root), "update_key_pinned"), true);
    assert_eq!(field(&install::uninstall(&env.root).unwrap(), "status"), "uninstalled");
}

#[test]
fn partial_key_publication_never_exposes_an_incomplete_final_key() {
    let env = Env::new();
    install::install(&env.package("0.1.0"), &env.root).unwrap();
    let public = env.directory.join("public.pem");
    fs::write(&public, ed25519_pem(2)).unwrap();
    assert_eq!(child("partial:update-public-key.pem=exit:95", &[&"pin-key", &env.root, &public]), 95);
    assert!(!env.root.join(KEY).exists());
    assert_eq!(field(&status(&env.root), "operation"), "pin");
    assert_eq!(field(&install::recover(&env.root, "current").unwrap(), "update_key_pinned"), true);
    assert_eq!(fs::read(env.root.join(KEY)).unwrap(), fs::read(&public).unwrap());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(env.root.join(KEY)).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert_eq!(field(&install::uninstall(&env.root).unwrap(), "status"), "uninstalled");
}

#[test]
fn os_lock_serializes_concurrent_upgrades_and_rechecks_version() {
    let env = Env::new();
    install::install(&env.package("0.1.0"), &env.root).unwrap();
    let archives = [env.package("0.2.0"), env.package("0.3.0")];
    let mut children = Vec::new();
    {
        let _lock = InstallationLock::acquire(&env.root, false).unwrap();
        for archive in &archives {
            children.push(
                Command::new(env!("CARGO_BIN_EXE_screenshots-install"))
                    .arg("install")
                    .arg(archive)
                    .arg(&env.root)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap(),
            );
            // Start in order so the newer package waits behind the older one.
            std::thread::sleep(Duration::from_millis(300));
        }
        std::thread::sleep(Duration::from_millis(500));
        assert!(children.iter_mut().all(|c| c.try_wait().unwrap().is_none()), "both installs wait for the lock");
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    let codes: Vec<i32> = children
        .iter_mut()
        .map(|c| loop {
            if let Some(status) = c.try_wait().unwrap() {
                break status.code().unwrap_or(-1);
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        })
        .collect();
    assert!(codes.contains(&0));
    assert!(codes.iter().all(|c| matches!(c, 0 | 1)));
    assert_eq!(version(&env.root), "0.3.0");
    let current = state(&env.root)["current"].as_str().unwrap().to_owned();
    let executable = executable_name(&local_target().unwrap());
    assert_eq!(sha(&env.root.join(executable)).unwrap(), sha(&env.root.join("versions").join(current).join(executable)).unwrap());
}

#[test]
fn uninstall_preserves_unexpected_empty_directory_and_special_files() {
    let env = Env::new();
    install::install(&env.package("0.1.0"), &env.root).unwrap();
    let folder = env.root.join("versions").join(state(&env.root)["current"].as_str().unwrap());
    let extra = folder.join("user-empty-directory");
    fs::create_dir(&extra).unwrap();
    assert!(install::uninstall(&env.root).is_err());
    assert!(extra.is_dir());
    fs::remove_dir(&extra).unwrap();
    #[cfg(unix)]
    {
        let fifo = folder.join("user-fifo");
        let name = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
        // SAFETY: valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(install::uninstall(&env.root).is_err());
        assert!(fs::symlink_metadata(&fifo).is_ok());
        fs::remove_file(&fifo).unwrap();
    }
    assert_eq!(field(&install::uninstall(&env.root).unwrap(), "status"), "uninstalled");
}

#[test]
fn uninstall_crash_after_first_owned_deletion_can_resume() {
    let env = Env::new();
    install::install(&env.package("0.1.0"), &env.root).unwrap();
    let outside = env.directory.join("private-user-image.png");
    fs::write(&outside, b"never touched").unwrap();
    assert_eq!(child("delete_owned_file.after=exit:93", &[&"uninstall", &env.root]), 93);
    assert_eq!(field(&status(&env.root), "operation"), "uninstall");
    let intent = transaction(&env.root);
    assert!(!intent["active"].is_null());
    let folder = env.root.join("versions").join(intent["active"]["identifier"].as_str().unwrap());
    assert!(intent["active"]["manifest"]["files"].as_array().unwrap().iter().any(|i| !folder.join(i["path"].as_str().unwrap()).exists()));
    assert_eq!(field(&install::recover(&env.root, "current").unwrap(), "status"), "uninstalled");
    assert!(!env.root.exists());
    assert_eq!(fs::read(&outside).unwrap(), b"never touched");
}

#[test]
fn interrupted_uninstall_preserves_new_foreign_entries_before_retry() {
    let env = Env::new();
    install::install(&env.package("0.1.0"), &env.root).unwrap();
    assert_eq!(child("delete_owned_file.after=exit:94", &[&"uninstall", &env.root]), 94);
    let intent = transaction(&env.root);
    let folder = env.root.join("versions").join(intent["active"]["identifier"].as_str().unwrap());
    let added = folder.join("user-added-empty-directory");
    fs::create_dir(&added).unwrap();
    assert!(install::uninstall(&env.root).is_err());
    assert!(added.is_dir());
    fs::remove_dir(&added).unwrap();
    assert_eq!(field(&install::uninstall(&env.root).unwrap(), "status"), "uninstalled");
}

fn openssl(args: &[&dyn AsRef<std::ffi::OsStr>]) -> bool {
    Command::new("openssl").args(args.iter().map(|a| a.as_ref())).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}
fn signing_keys(directory: &Path) -> Option<(PathBuf, PathBuf)> {
    let (private, public) = (directory.join("signing-private.pem"), directory.join("signing-public.pem"));
    (openssl(&[&"genpkey", &"-algorithm", &"ED25519", &"-out", &private]) && openssl(&[&"pkey", &"-in", &private, &"-pubout", &"-out", &public]))
        .then_some((private, public))
}
fn signed_manifest(directory: &Path, private: &Path, artifact: &Path, url: &str) -> (PathBuf, PathBuf) {
    let (manifest, signature) = (directory.join("release.json"), directory.join("release.json.sig"));
    let data = json!({"version": "0.2.0", "target": local_target().unwrap(), "url": url, "sha256": sha(artifact).unwrap(), "size": fs::metadata(artifact).unwrap().len()});
    fs::write(&manifest, install::dumps(&data, true) + "\n").unwrap();
    assert!(openssl(&[&"pkeyutl", &"-sign", &"-inkey", &private, &"-rawin", &"-in", &manifest, &"-out", &signature]));
    (manifest, signature)
}

#[test]
fn signed_update_requires_pinned_key_digest_target_and_new_version() {
    let env = Env::new();
    let Some((private, public)) = signing_keys(&env.directory) else {
        eprintln!("skipped: OpenSSL with Ed25519 is unavailable");
        return;
    };
    let second = env.package("0.2.0");
    install::install(&env.package("0.1.0"), &env.root).unwrap();
    assert_eq!(field(&install::pin_key(&env.root, &public).unwrap(), "status"), "update_key_pinned");
    assert!(install::pin_key(&env.root, &public).is_err());
    let (manifest, signature) = signed_manifest(&env.directory, &private, &second, "https://example.invalid/release.tar.gz");
    let original = fs::read(&signature).unwrap();
    fs::write(&signature, [0u8; 64]).unwrap();
    assert!(install::apply_signed(&env.root, &manifest, &signature, &second).is_err());
    assert_eq!(version(&env.root), "0.1.0");
    fs::write(&signature, original).unwrap();
    assert_eq!(field(&install::apply_signed(&env.root, &manifest, &signature, &second).unwrap(), "status"), "upgraded");
    assert!(install::apply_signed(&env.root, &manifest, &signature, &second).is_err());
    assert_eq!(field(&status(&env.root), "update_key_pinned"), true);
}


/// Minimal HTTPS file server for the update test; records every requested path.
struct Server {
    port: u16,
    requests: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    files: std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<String, Vec<u8>>>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Server {
    fn start(certificate: &Path, key: &Path) -> Self {
        use rustls_pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
        use std::io::{Read, Write};
        use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
        let certs = CertificateDer::pem_file_iter(certificate).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
        let key = PrivateKeyDer::from_pem_file(key).unwrap();
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let config = Arc::new(
            rustls::ServerConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(certs, key)
                .unwrap(),
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::<String>::new()));
        let files = Arc::new(Mutex::new(std::collections::BTreeMap::<String, Vec<u8>>::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (requests, files, stop) = (requests.clone(), files.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    let Ok((socket, _)) = listener.accept() else {
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    };
                    socket.set_nonblocking(false).unwrap();
                    socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                    let connection = rustls::ServerConnection::new(config.clone()).unwrap();
                    let mut stream = rustls::StreamOwned::new(connection, socket);
                    let mut request = Vec::new();
                    let mut byte = [0u8; 1];
                    while !request.ends_with(b"\r\n\r\n") && request.len() < 8192 {
                        match stream.read(&mut byte) {
                            Ok(1) => request.push(byte[0]),
                            _ => break,
                        }
                    }
                    let text = String::from_utf8_lossy(&request);
                    let Some(path) = text.split_whitespace().nth(1).map(str::to_owned) else { continue };
                    requests.lock().unwrap().push(path.clone());
                    let body = files.lock().unwrap().get(&path).cloned();
                    let response = match body {
                        Some(body) => [format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes(), body].concat(),
                        None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                    };
                    let _ = stream.write_all(&response);
                    stream.conn.send_close_notify();
                    let _ = stream.flush();
                }
            })
        };
        Self { port, requests, files, stop, thread: Some(thread) }
    }
    fn serve(&self, path: &str, body: Vec<u8>) {
        self.files.lock().unwrap().insert(path.into(), body);
    }
    fn take_requests(&self) -> Vec<String> {
        std::mem::take(&mut *self.requests.lock().unwrap())
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[test]
fn online_https_update_authenticates_before_artifact_download() {
    let env = Env::new();
    let Some((private, public)) = signing_keys(&env.directory) else {
        eprintln!("skipped: OpenSSL with Ed25519 is unavailable");
        return;
    };
    let second = env.package("0.2.0");
    install::install(&env.package("0.1.0"), &env.root).unwrap();
    install::pin_key(&env.root, &public).unwrap();
    let (certificate, key) = (env.directory.join("localhost.pem"), env.directory.join("localhost-key.pem"));
    assert!(openssl(&[&"req", &"-x509", &"-newkey", &"rsa:2048", &"-nodes", &"-keyout", &key, &"-out", &certificate, &"-days", &"1",
        &"-subj", &"/CN=127.0.0.1", &"-addext", &"subjectAltName=IP:127.0.0.1"]));
    let server = Server::start(&certificate, &key);
    let address = format!("https://127.0.0.1:{}", server.port);
    let (manifest, signature) = signed_manifest(&env.directory, &private, &second, &format!("{address}/artifact.tar.gz"));
    server.serve("/release.json", fs::read(&manifest).unwrap());
    server.serve("/release.json.sig", vec![0u8; 64]);
    server.serve("/artifact.tar.gz", fs::read(&second).unwrap());
    let run = |args: &[&dyn AsRef<std::ffi::OsStr>]| {
        Command::new(env!("CARGO_BIN_EXE_screenshots-install"))
            .args(args.iter().map(|a| a.as_ref()))
            .env("SCREENSHOTS_INSTALL_TEST_ROOT_CERT", &certificate)
            .env("SCREENSHOTS_INSTALL_DEBUG_ERRORS", "1")
            .output()
            .unwrap()
    };
    let url = format!("{address}/release.json");
    assert!(!run(&[&"update", &env.root, &url]).status.success());
    assert_eq!(server.take_requests(), ["/release.json", "/release.json.sig"], "artifact is never requested before authentication");
    assert_eq!(version(&env.root), "0.1.0");
    server.serve("/release.json.sig", fs::read(&signature).unwrap());
    let accepted = run(&[&"update", &env.root, &url]);
    assert!(accepted.status.success(), "{}", String::from_utf8_lossy(&accepted.stderr));
    assert_eq!(server.take_requests(), ["/release.json", "/release.json.sig", "/artifact.tar.gz"]);
    assert!(String::from_utf8_lossy(&accepted.stdout).contains("\"status\": \"upgraded\""));
    assert_eq!(version(&env.root), "0.2.0");
}
