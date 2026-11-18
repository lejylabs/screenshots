use super::*;

/// Build a minimal valid portable archive for this machine with its checksum sidecar.
fn package(directory: &Path, version_text: &str, binary: &[u8], zip: bool) -> PathBuf {
    let target_name = local_target().unwrap();
    let root = format!("screenshots-{version_text}-{target_name}");
    let executable = executable_name(&target_name);
    let info = json!({"version": version_text, "target": target_name, "binary_sha256": sha_bytes(binary)}).to_string();
    let members: Vec<(String, u32, Vec<u8>)> = vec![
        (format!("{root}/{executable}"), 0o755, binary.to_vec()),
        (format!("{root}/BUILD-INFO.json"), 0o644, info.into_bytes()),
        (format!("{root}/LICENSE"), 0o644, b"MIT".to_vec()),
        (format!("{root}/README.md"), 0o644, b"readme".to_vec()),
        (format!("{root}/Cargo.lock"), 0o644, b"lock".to_vec()),
        (format!("{root}/THIRD_PARTY_NOTICES.md"), 0o644, b"notices".to_vec()),
        (format!("{root}/licenses/dependencies.json"), 0o644, b"[]".to_vec()),
    ];
    let name = format!("{root}.{}", if zip { "zip" } else { "tar.gz" });
    let path = directory.join(&name);
    let file = File::create(&path).unwrap();
    if zip {
        let mut writer = archive::ZipWriter::new(file);
        for (member, mode, body) in &members {
            writer.file(member, *mode, archive::utc_civil(0), &mut &body[..]).unwrap();
        }
        writer.finish().unwrap();
    } else {
        let mut writer = archive::TarWriter::new(archive::gzip_writer(file, 0));
        for (member, mode, body) in &members {
            writer.file(member, *mode, 0, &mut &body[..], body.len() as u64).unwrap();
        }
        writer.finish().unwrap().finish().unwrap();
    }
    let digest = sha(&path).unwrap();
    fs::write(directory.join(format!("{name}.sha256")), format!("{digest}  {name}\n")).unwrap();
    path
}
fn field(outcome: &Outcome, key: &str) -> Value {
    outcome.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone()).unwrap_or(Value::Null)
}

#[test]
fn install_upgrade_rollback_recover_and_uninstall() {
    let work = TempDir::new("screenshots-install-test-").unwrap();
    let packages = work.path().join("packages");
    fs::create_dir(&packages).unwrap();
    fs::create_dir(work.path().join("home")).unwrap();
    let root = work.path().join("home/Screenshots");
    let zip = cfg!(windows);
    let first = package(&packages, "1.0.0", b"first", zip);
    let second = package(&packages, "1.1.0", b"second", zip);

    assert_eq!(field(&status(&root).unwrap(), "status"), "not_installed");
    let outcome = install(&first, &root).unwrap();
    assert_eq!(field(&outcome, "status"), "installed");
    assert_eq!(outcome_json(&outcome), format!(
        r#"{{"status": "installed", "version": "1.0.0", "target": "{}", "previous_retained": false}}"#,
        local_target().unwrap()
    ));
    assert_eq!(field(&install(&first, &root).unwrap(), "status"), "unchanged");
    let launcher = root.join(executable_name(&local_target().unwrap()));
    assert_eq!(fs::read(&launcher).unwrap(), b"first");

    assert_eq!(field(&install(&second, &root).unwrap(), "status"), "upgraded");
    assert_eq!(fs::read(&launcher).unwrap(), b"second");
    let current = status(&root).unwrap();
    assert_eq!(field(&current, "version"), "1.1.0");
    assert_eq!(field(&current, "previous_retained"), true);
    assert!(install(&first, &root).is_err(), "downgrade must use rollback");

    assert_eq!(field(&rollback(&root).unwrap(), "version"), "1.0.0");
    assert_eq!(fs::read(&launcher).unwrap(), b"first");
    assert_eq!(field(&recover(&root, "previous").unwrap(), "version"), "1.1.0");
    assert_eq!(fs::read(&launcher).unwrap(), b"second");

    // An unowned file blocks removal and is preserved.
    let stray = root.join("stray.txt");
    fs::write(&stray, b"user").unwrap();
    assert!(uninstall(&root).is_err());
    fs::remove_file(&stray).unwrap();
    assert_eq!(field(&uninstall(&root).unwrap(), "status"), "uninstalled");
    assert!(!root.exists());
    assert!(root.parent().unwrap().exists(), "parent and lock are kept");
}

#[test]
fn checksum_tampering_and_modified_installs_are_refused() {
    let work = TempDir::new("screenshots-install-test-").unwrap();
    let packages = work.path().join("packages");
    fs::create_dir(&packages).unwrap();
    let root = work.path().join("Screenshots");
    let zip = cfg!(windows);
    let first = package(&packages, "2.0.0", b"payload", zip);
    let sidecar = PathBuf::from(format!("{}.sha256", first.display()));
    let original = fs::read_to_string(&sidecar).unwrap();
    fs::write(&sidecar, original.replacen(&original[..1], if &original[..1] == "0" { "1" } else { "0" }, 1)).unwrap();
    assert!(install(&first, &root).is_err());
    assert!(installation_absent(&root).unwrap(), "failed install leaves no owned state");
    fs::write(&sidecar, original).unwrap();
    install(&first, &root).unwrap();
    let identifier = format!("2.0.0-{}", local_target().unwrap());
    fs::write(root.join("versions").join(&identifier).join("LICENSE"), b"changed").unwrap();
    assert!(status(&root).is_err(), "modified installed file is detected");
}

#[test]
fn interrupted_install_intent_recovers_to_published_version() {
    let work = TempDir::new("screenshots-install-test-").unwrap();
    let packages = work.path().join("packages");
    fs::create_dir(&packages).unwrap();
    let root = work.path().join("Screenshots");
    let zip = cfg!(windows);
    install(&package(&packages, "3.0.0", b"three", zip), &root).unwrap();
    // A pin intent that never completed is reported and then completed on recovery.
    let value = required_state(&root).unwrap();
    let key = b"-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEAGb9ECWmEzf6FQbrBZ9w7lshQhqowtrbLDFw4rXAxZuE=\n-----END PUBLIC KEY-----\n";
    use base64::Engine;
    let intent = json!({"owner": OWNER, "kind": "pin", "prior": value,
                        "new": with(&value, &[("key_sha256", sha_bytes(key).into())]),
                        "public_key": base64::engine::general_purpose::STANDARD.encode(key)});
    atomic_json(&root.join(TRANSACTION), &intent, None).unwrap();
    assert_eq!(field(&status(&root).unwrap(), "status"), "recovery_required");
    assert_eq!(field(&recover(&root, "current").unwrap(), "update_key_pinned"), true);
    assert_eq!(field(&status(&root).unwrap(), "update_key_pinned"), true);
    assert_eq!(fs::read(root.join(PUBLIC_KEY)).unwrap(), key);
}

#[test]
fn names_versions_and_json_follow_the_former_contract() {
    assert!(safe_name("root/a/b").is_ok());
    for bad in ["root", "/root/a", "root/../a", "root/a\\b", "root/CON.txt", "root/a.", "root/c:d"] {
        assert!(safe_name(bad).is_err(), "{bad}");
    }
    assert!(version("1.2.3").is_ok());
    for bad in ["01.2.3", "1.2", "1.2.3.4", "1.x.3", "1234567890.0.0"] {
        assert!(version(bad).is_err(), "{bad}");
    }
    assert!(target("x86_64-pc-windows-msvc").is_ok() && target("bad").is_err());
    // Python's ensure_ascii escaping; \u{e9} is "e acute".
    assert_eq!(dumps(&json!({"b": 1, "a": "\u{e9}\n"}), true), "{\"a\":\"\\u00e9\\n\",\"b\":1}");
    assert!(parse_json(br#"{"a":1,"a":2}"#).is_err());
}
