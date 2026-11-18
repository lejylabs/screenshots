//! Synthetic provenance controls; no Cargo, network, native API or account.
//! Port of the former `scripts/test-vendored-advisory-view.py`.
use super::*;
use screenshots_install::TempDir;

const ORIGINAL: &[u8] = b"let p: *mut libc::c_char = std::ptr::null_mut();\n                &p,\n";
const FIXED: &[u8] = b"let mut p: *mut libc::c_char = std::ptr::null_mut();\n                &mut p,\n";
const MANIFEST: &[u8] = b"[package]\nname=\"glib\"\nversion=\"0.18.5\"\n";
const POLICY: &str = r#"
[graph]
targets = ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "x86_64-pc-windows-msvc", "aarch64-apple-darwin", "x86_64-apple-darwin"]
all-features = false
no-default-features = false
features = []
exclude-dev = true
exclude-unpublished = false
exclude = []
[advisories]
db-path = "unused"
db-urls = ["https://github.com/RustSec/advisory-db"]
unmaintained = "all"
unsound = "all"
yanked = "deny"
ignore = []
unused-ignored-advisory = "deny"
"#;

struct Fixture {
    _owned: TempDir,
    root: PathBuf,
    folder: PathBuf,
    archive: Vec<u8>,
    metadata: Value,
    selection: Value,
    provenance: Value,
    policy: toml::Table,
    database: PathBuf,
    advisory: PathBuf,
}
fn tar_gz(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut writer = archive::TarWriter::new(archive::gzip_writer(&mut out, 0));
    for (name, body) in members {
        writer.file(name, 0o644, 0, &mut &body[..], body.len() as u64).unwrap();
    }
    writer.finish().unwrap().finish().unwrap();
    out
}
impl Fixture {
    fn new() -> Self {
        let owned = TempDir::new("screenshots-policy-test-").unwrap();
        let root = owned.path().to_path_buf();
        let folder = root.join("vendor/glib");
        fs::create_dir_all(folder.join("src")).unwrap();
        fs::write(folder.join("Cargo.toml"), MANIFEST).unwrap();
        fs::write(folder.join("src/variant_iter.rs"), FIXED).unwrap();
        let archive = tar_gz(&[("glib-0.18.5/Cargo.toml", MANIFEST), ("glib-0.18.5/src/variant_iter.rs", ORIGINAL)]);
        let url = |p: &Path| format!("path+file://{}", p.display());
        let app = format!("{}#app@0.1.0", url(&root.join("app")));
        let vendor = format!("{}#glib@0.18.5", url(&folder));
        let metadata = json!({"workspace_root": root, "workspace_members": [app],
            "packages": [
                {"id": app, "name": "app", "version": "0.1.0", "source": null, "manifest_path": root.join("app").join("Cargo.toml"), "features": {}},
                {"id": vendor, "name": "glib", "version": "0.18.5", "source": null, "manifest_path": folder.join("Cargo.toml"), "features": {"native": []}}],
            "resolve": {"nodes": [{"id": app, "deps": [{"pkg": vendor}], "features": []}, {"id": vendor, "deps": [], "features": ["native"]}]}});
        let mut selection = serde_json::Map::new();
        for p in metadata["packages"].as_array().unwrap() {
            let id = s(p, "id");
            selection.insert(format!("{} {} {}", s(p, "name"), s(p, "version"), id.rsplit_once('#').unwrap().0), json!({"licenses": ["MIT"]}));
        }
        let entry = json!({"name": "glib", "version": "0.18.5", "path": "vendor/glib",
            "published_archive_sha256": sha_bytes(&archive), "published_manifest_sha256": sha_bytes(MANIFEST),
            "published_checksum_api": "https://crates.io/api/v1/crates/glib/0.18.5", "local_source": source_tree(&folder).unwrap()});
        let provenance = json!({"version": 1, "scope": "synthetic", "first_party": [{"name": "app", "manifest": "app/Cargo.toml"}],
                                "packages": [entry], "mitigations": []});
        let database = root.join("databases");
        let advisory = database.join("owned/crates/glib/RUSTSEC-2024-0429.md");
        fs::create_dir_all(advisory.parent().unwrap()).unwrap();
        fs::write(&advisory, "synthetic reviewed advisory body").unwrap();
        Fixture { _owned: owned, root, folder, archive, metadata, selection: Value::Object(selection), provenance, policy: POLICY.parse().unwrap(), database, advisory }
    }
    fn prepare_limited(&self, unpacked: u64) -> Result<(Value, String, Value)> {
        let archive = self.archive.clone();
        prepare_bounded(&self.metadata, &self.selection, &self.provenance, &self.root, &self.policy, &self.database, &move |_| Ok(archive.clone()), unpacked)
    }
    fn prepare(&self) -> Result<(Value, String, Value)> {
        self.prepare_limited(MAX_UNPACKED)
    }
    fn mitigation(&mut self) {
        self.provenance["mitigations"] = json!([{"id": "RUSTSEC-2024-0429", "package": "glib", "version": "0.18.5",
            "kind": "glib-variant-out-argument-v1", "advisory_path": "crates/glib/RUSTSEC-2024-0429.md",
            "advisory_sha256": sha_bytes(&fs::read(&self.advisory).unwrap()), "source_file": "src/variant_iter.rs",
            "published_file_sha256": sha_bytes(ORIGINAL), "patched_file_sha256": sha_bytes(FIXED),
            "upstream_fix": {"url": "https://example.invalid/synthetic", "sha256": "f".repeat(64)},
            "evidence": {"scope": "synthetic", "logs": [{"sha256": "a".repeat(64)}]}}]);
    }
}
fn fails(result: Result<impl Sized>, text: &str) {
    match result {
        Ok(_) => panic!("expected failure containing {text:?}"),
        Err(error) => assert!(format!("{error:#}").contains(text), "{error:#} does not contain {text:?}"),
    }
}

#[test]
fn only_advisory_identity_changes() {
    let f = Fixture::new();
    let snapshot = f.metadata.clone();
    let (mut projected, policy, report) = f.prepare().unwrap();
    assert_eq!(f.metadata, snapshot);
    assert!(projected["packages"][0]["source"].is_null());
    assert_eq!(projected["packages"][1]["source"], SPARSE_IDENTITY);
    projected["packages"][1]["source"] = Value::Null;
    assert_eq!(projected, snapshot);
    assert_eq!(report["selected_packages"], 2);
    assert!(policy.contains("ignore = [\n]"));
}
#[test]
fn missing_mapping_is_failure() {
    let mut f = Fixture::new();
    f.provenance["packages"] = json!([]);
    fails(f.prepare(), "no verified published base");
}
#[test]
fn wrong_archive_fails_before_projection() {
    let mut f = Fixture::new();
    f.archive.extend_from_slice(b"changed");
    fails(f.prepare(), "archive checksum");
}
#[test]
fn unpacked_archive_extent_is_bounded() {
    fails(Fixture::new().prepare_limited(1024), "expanded-byte budget");
}
#[test]
fn changed_local_source_is_failure() {
    let f = Fixture::new();
    fs::write(f.folder.join("src/variant_iter.rs"), ORIGINAL).unwrap();
    fails(f.prepare(), "fingerprint changed");
}
#[test]
fn unfixed_source_cannot_receive_mitigation() {
    let mut f = Fixture::new();
    f.mitigation();
    fs::write(f.folder.join("src/variant_iter.rs"), ORIGINAL).unwrap();
    f.provenance["packages"][0]["local_source"] = source_tree(&f.folder).unwrap();
    fails(f.prepare(), "security fix is absent");
}
#[test]
fn exact_source_mitigation_is_explicit() {
    let mut f = Fixture::new();
    f.mitigation();
    let (_, policy, report) = f.prepare().unwrap();
    assert!(policy.contains("id = \"RUSTSEC-2024-0429\""));
    assert_eq!(report["conditional_source_mitigations"].as_array().unwrap().len(), 1);
}
#[test]
fn changed_advisory_reopens_mitigation() {
    let mut f = Fixture::new();
    f.mitigation();
    fs::write(&f.advisory, "new advisory scope").unwrap();
    fails(f.prepare(), "advisory changed");
}
#[test]
fn other_same_name_package_prevents_id_mitigation() {
    let mut f = Fixture::new();
    f.mitigation();
    let mut duplicate = f.metadata["packages"][1].clone();
    duplicate["id"] = "registry+https://example.invalid#glib@0.18.4".into();
    duplicate["version"] = "0.18.4".into();
    duplicate["source"] = "registry+https://example.invalid".into();
    f.metadata["packages"].as_array_mut().unwrap().push(duplicate);
    fails(f.prepare(), "not unique");
}
#[test]
fn unknown_advisory_cannot_be_added_to_mitigation() {
    let mut f = Fixture::new();
    f.mitigation();
    f.provenance["mitigations"][0]["id"] = "RUSTSEC-2099-9999".into();
    fails(f.prepare(), "Unreviewed mitigation");
}
#[test]
fn selection_cannot_drop_first_party() {
    let mut f = Fixture::new();
    let key = f.selection.as_object().unwrap().keys().find(|k| k.starts_with("app ")).unwrap().clone();
    f.selection.as_object_mut().unwrap().remove(&key);
    fails(f.prepare(), "first-party roots");
}
#[test]
fn changed_selected_listing_is_failure() {
    let f = Fixture::new();
    let mut after = f.selection.clone();
    let first = after.as_object().unwrap().keys().next().unwrap().clone();
    after.as_object_mut().unwrap().remove(&first);
    fails(verify_selection(&f.selection, &after), "listing differs");
    verify_selection(&f.selection, &f.selection).unwrap();
}
#[test]
fn unknown_provenance_fields_fail() {
    let mut f = Fixture::new();
    f.provenance["skip"] = true.into();
    fails(f.prepare(), "Unknown provenance");
}
#[cfg(unix)]
#[test]
fn symlink_source_is_failure() {
    let f = Fixture::new();
    std::os::unix::fs::symlink(f.folder.join("Cargo.toml"), f.folder.join("link")).unwrap();
    fails(f.prepare(), "symlink");
}
#[test]
fn duplicate_json_is_failure() {
    fails(parse_json(br#"{"version":1,"version":2}"#), "duplicate");
}
