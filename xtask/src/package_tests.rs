//! Portable packaging regression tests; never installs into the user's system.
//! Port of the former `scripts/test-package.py` (runtime-bundle cases were dropped
//! together with the separate SXCU runtime).
use super::*;
use std::{collections::BTreeMap, sync::Mutex};

/// Environment-variable tests must not interleave.
static ENV: Mutex<()> = Mutex::new(());

struct Fixture {
    dir: TempDir,
    binary: PathBuf,
    installer: PathBuf,
}
impl Fixture {
    fn new(binary: &[u8]) -> Self {
        let dir = TempDir::new("xtask-package-").unwrap();
        let (path, installer) = (dir.path().join("binary"), dir.path().join("installer"));
        fs::write(&path, binary).unwrap();
        fs::write(&installer, b"installer fixture").unwrap();
        Self { dir, binary: path, installer }
    }
    fn package(&self, output: &str, name: &str, target: &str, epoch: u64, info: Value, licenses: Vec<Value>) -> Result<PathBuf> {
        package(Inputs { binary: &self.binary, installer: &self.installer, output: &self.dir.path().join(output), name, target, epoch, info, licenses, abi: None })
    }
}
fn files(archive: &Path) -> BTreeMap<String, (u32, Vec<u8>)> {
    let mut out = BTreeMap::new();
    let mut visit = |member: archive::Member<'_>| -> Result<()> {
        let mut data = Vec::new();
        member.reader.read_to_end(&mut data)?;
        out.insert(member.name, (member.mode & 0o777, data));
        Ok(())
    };
    let file = fs::File::open(archive).unwrap();
    if archive.extension().is_some_and(|e| e == "zip") {
        archive::read_zip(file, &archive::ZipLimits { entries: 10000, member: 1 << 30 }, &mut visit).unwrap();
    } else {
        archive::read_tar_gz(file, &archive::TarLimits { entries: 10000, member: 1 << 30, total: 1 << 31 }, &mut visit).unwrap();
    }
    out
}

#[test]
fn vendored_accesskit_original_licenses_are_required_and_packaged() {
    let hashes = [
        ("LICENSE-MIT", "23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3"),
        ("LICENSE-APACHE", "62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a"),
    ];
    let mut licenses = Vec::new();
    for (name, version) in [("accesskit_unix", "0.22.1"), ("accesskit_atspi_common", "0.19.1")] {
        let item = json!({"name": name, "version": version, "license": "MIT OR Apache-2.0", "manifest_path": root().join("vendor").join(name).join("Cargo.toml")});
        let paths = license_files(&item).unwrap();
        let names: BTreeMap<String, String> = paths.iter().map(|p| (p.file_name().unwrap().to_string_lossy().into_owned(), sha(p).unwrap())).collect();
        assert_eq!(names, hashes.iter().map(|(n, h)| (n.to_string(), h.to_string())).collect());
        licenses.push(json!({"name": name, "version": version, "license": "MIT OR Apache-2.0", "license_files": paths}));
    }
    let fixture = Fixture::new(b"synthetic executable");
    for target in ["x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc"] {
        let archive = fixture.package(target, "accesskit-test", target, 0, json!({}), licenses.clone()).unwrap();
        let contents = files(&archive);
        for item in &licenses {
            for (name, digest) in hashes {
                let path = format!("accesskit-test/licenses/{}-{}/{name}", item["name"].as_str().unwrap(), item["version"].as_str().unwrap());
                assert_eq!(sha_bytes(&contents[&path].1), digest, "{path}");
            }
        }
    }
    for name in ["accesskit_unix", "accesskit_atspi_common"] {
        // A metadata entry alone must not silently ship without its notices.
        assert!(license_files(&json!({"name": name, "manifest_path": fixture.dir.path().join("Cargo.toml")})).is_err());
    }
}

#[test]
fn checksum_publication_failure_never_deletes_the_published_archive() {
    let dir = TempDir::new("xtask-publish-").unwrap();
    let (archive, checksum) = (dir.path().join("staged"), dir.path().join("staged.sha256"));
    fs::write(&archive, b"archive").unwrap();
    fs::write(&checksum, b"sum").unwrap();
    let out = dir.path().join("out");
    fs::create_dir(&out).unwrap();
    fs::write(out.join("a.sha256"), b"foreign").unwrap();
    assert!(publish(&archive, &checksum, &out.join("a"), &out.join("a.sha256")).is_err());
    assert_eq!(fs::read(out.join("a")).unwrap(), b"archive", "linked archive is kept for review");
    assert_eq!(fs::read(out.join("a.sha256")).unwrap(), b"foreign");
}

#[test]
fn linux_abi_uses_numeric_symbol_versions_and_requires_metadata() {
    assert_eq!(glibc_requirement("Name: GLIBC_2.9 Name: GLIBC_2.39 Name: GLIBC_2.3.4").unwrap(), json!({"glibc": "2.39"}));
    assert_eq!(glibc_requirement("GLIBC_2.40.1").unwrap(), json!({"glibc": "2.40.1"}));
    assert!(glibc_requirement("no versions").is_err());
}

#[test]
fn native_abi_is_measured_from_the_frozen_staged_binary() {
    let fixture = Fixture::new(b"synthetic frozen executable");
    let input = fixture.binary.clone();
    let probe = |path: &Path| -> Result<Value> {
        assert_ne!(path, input);
        assert_eq!(fs::read(path)?, b"synthetic frozen executable");
        fs::write(&input, b"later input replacement")?;
        Ok(json!({"glibc": "2.39"}))
    };
    let archive = package(Inputs { binary: &fixture.binary, installer: &fixture.installer, output: &fixture.dir.path().join("output"), name: "abi-test",
        target: "x86_64-unknown-linux-gnu", epoch: 0, info: json!({}), licenses: vec![], abi: Some(&probe) }).unwrap();
    let contents = files(&archive);
    let info: Value = serde_json::from_slice(&contents["abi-test/BUILD-INFO.json"].1).unwrap();
    assert_eq!(info["linux_abi"], json!({"glibc": "2.39"}));
    assert_eq!(contents["abi-test/screenshots"].1, b"synthetic frozen executable");
}

fn notices(root: &Path, triplet: &str, names: &[&str]) {
    for name in names {
        let folder = root.join(triplet).join("share").join(name);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("copyright"), format!("{name} native notice")).unwrap();
    }
}
fn with_vcpkg<T>(root: &Path, triplet: &str, run: impl FnOnce() -> T) -> T {
    // SAFETY: serialized by ENV; no other test reads these variables.
    unsafe {
        std::env::set_var("VCPKG_INSTALLED_ROOT", root);
        std::env::set_var("VCPKGRS_TRIPLET", triplet);
    }
    let result = run();
    unsafe {
        std::env::remove_var("VCPKG_INSTALLED_ROOT");
        std::env::remove_var("VCPKGRS_TRIPLET");
    }
    result
}
const NATIVE: [&str; 4] = ["curl", "libssh2", "openssl", "zlib"];

#[test]
fn static_mac_native_notices_use_the_matching_target_triplet() {
    let _guard = ENV.lock().unwrap_or_else(|p| p.into_inner());
    for (target, triplet, other) in [("aarch64-apple-darwin", "arm64-osx", "x64-osx"), ("x86_64-apple-darwin", "x64-osx", "arm64-osx")] {
        let dir = TempDir::new("xtask-vcpkg-").unwrap();
        notices(dir.path(), triplet, &NATIVE);
        assert_eq!(with_vcpkg(dir.path(), triplet, || native_licenses(target)).unwrap().len(), 4);
        assert!(with_vcpkg(dir.path(), other, || native_licenses(target)).is_err());
    }
}

#[test]
fn static_windows_native_notices_are_required_bounded_and_packaged() {
    let _guard = ENV.lock().unwrap_or_else(|p| p.into_inner());
    let target = "x86_64-pc-windows-msvc";
    let triplet = "x64-windows-static-md";
    let fixture = Fixture::new(b"synthetic executable");
    let root = fixture.dir.path().join("vcpkg");
    assert!(native_licenses("x86_64-unknown-linux-gnu").unwrap().is_empty());
    assert!(with_vcpkg(&root, triplet, || native_licenses(target)).is_err(), "missing notices are refused");
    notices(&root, triplet, &NATIVE);
    let found = with_vcpkg(&root, triplet, || native_licenses(target)).unwrap();
    assert_eq!(found.len(), 4);
    let copyright = root.join(triplet).join("share/curl/copyright");
    fs::write(&copyright, vec![b'x'; 1024 * 1024 + 1]).unwrap();
    assert!(with_vcpkg(&root, triplet, || native_licenses(target)).is_err(), "oversized notice is refused");
    fs::write(&copyright, b"curl native notice").unwrap();
    let rust_license = fixture.dir.path().join("LICENSE-fixture");
    fs::write(&rust_license, b"existing Rust notice").unwrap();
    let mut licenses = vec![json!({"name": "rust-fixture", "version": "1", "license_files": [rust_license]})];
    licenses.extend(found.clone());
    let archive = fixture.package("packages", "native-test", target, 0, json!({}), licenses).unwrap();
    let contents = files(&archive);
    let dependencies: Value = serde_json::from_slice(&contents["native-test/licenses/dependencies.json"].1).unwrap();
    let names: std::collections::BTreeSet<&str> = dependencies.as_array().unwrap().iter().filter_map(|i| i["name"].as_str()).collect();
    assert_eq!(names, ["native-curl", "native-libssh2", "native-openssl", "native-zlib", "rust-fixture"].into_iter().collect());
    for item in &found {
        let name = item["name"].as_str().unwrap();
        let path = format!("native-test/licenses/{name}-{}/copyright", item["version"].as_str().unwrap());
        assert_eq!(contents[&path].1, format!("{} native notice", &name[7..]).into_bytes());
    }
    assert_eq!(contents["native-test/licenses/rust-fixture-1/LICENSE-fixture"].1, b"existing Rust notice");
    let listing = String::from_utf8_lossy(&contents["native-test/licenses/dependencies.json"].1).into_owned();
    assert!(!listing.contains(&*fixture.dir.path().to_string_lossy()), "no local paths leak into the listing");
}

#[test]
fn bundled_webp_notices_are_required_and_packaged() {
    let fixture = Fixture::new(b"fixture");
    let root = fixture.dir.path();
    fs::write(root.join("Cargo.toml"), "").unwrap();
    fs::create_dir(root.join("vendor")).unwrap();
    let item = json!({"name": "libwebp-sys", "manifest_path": root.join("Cargo.toml")});
    assert!(license_files(&item).is_err());
    for name in ["COPYING", "PATENTS"] {
        fs::write(root.join("vendor").join(name), name).unwrap();
    }
    let paths = license_files(&item).unwrap();
    let names: Vec<String> = paths.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["COPYING", "PATENTS"]);
    let archive = fixture.package("packages", "test", "linux", 0, json!({}), vec![json!({"name": "libwebp-sys", "version": "0.14.4", "license_files": paths})]).unwrap();
    let contents = files(&archive);
    for name in ["COPYING", "PATENTS"] {
        assert_eq!(contents[&format!("test/licenses/libwebp-sys-0.14.4/{name}")].1, name.as_bytes());
    }
}

#[test]
fn archives_are_repeatable_and_never_replace_outputs() {
    for target in ["x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc", "aarch64-apple-darwin"] {
        let fixture = Fixture::new(b"synthetic executable fixture\x00");
        let license = fixture.dir.path().join("LICENSE-test");
        fs::write(&license, "Synthetic license fixture\n").unwrap();
        let licenses = vec![json!({"name": "fixture", "version": "1", "license": "MIT", "license_files": [license]})];
        let first = fixture.package("one", "screenshots-test", target, 1_700_000_000, json!({"fixture": true}), licenses.clone()).unwrap();
        let second = fixture.package("two", "screenshots-test", target, 1_700_000_000, json!({"fixture": true}), licenses).unwrap();
        assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap(), "{target} archive is repeatable");
        assert!(fixture.package("one", "screenshots-test", target, 1_700_000_000, json!({}), vec![]).is_err());
        let name = first.file_name().unwrap().to_string_lossy().into_owned();
        let checksum = fs::read_to_string(first.with_file_name(format!("{name}.sha256"))).unwrap();
        assert_eq!(checksum, format!("{}  {name}\n", sha(&first).unwrap()));
        let contents = files(&first);
        let windows = target.contains("windows");
        let executable = format!("screenshots-test/screenshots{}", if windows { ".exe" } else { "" });
        let installer = format!("screenshots-test/screenshots-install{}", if windows { ".exe" } else { "" });
        assert_eq!(contents[&executable], (0o755, b"synthetic executable fixture\x00".to_vec()));
        assert_eq!(contents[&installer], (0o755, b"installer fixture".to_vec()));
        assert!(contents.contains_key("screenshots-test/licenses/fixture-1/LICENSE-test"));
        let packaging = String::from_utf8_lossy(&contents["screenshots-test/PACKAGING.md"].1).into_owned();
        assert!(packaging.contains("](DESKTOP-INTEGRATION.md)") && packaging.contains("](NATIVE-SIGNING.md)"));
        for required in ["LICENSE", "THIRD_PARTY_NOTICES.md", "Cargo.lock", "PACKAGING.md", "DESKTOP-INTEGRATION.md", "NATIVE-SIGNING.md", "README.md"] {
            assert!(contents.contains_key(&format!("screenshots-test/{required}")), "{required}");
        }
        assert_eq!(contents["screenshots-test/LICENSE"].0, 0o644);
        let info: Value = serde_json::from_slice(&contents["screenshots-test/BUILD-INFO.json"].1).unwrap();
        assert_eq!(info["binary_sha256"], sha_bytes(b"synthetic executable fixture\x00"));
        assert_eq!(contents.contains_key("screenshots-test/screenshots.desktop"), target.contains("linux"));
    }
}

#[test]
fn missing_binary_creates_no_output() {
    let dir = TempDir::new("xtask-package-").unwrap();
    let installer = dir.path().join("installer");
    fs::write(&installer, b"x").unwrap();
    let output = dir.path().join("output");
    assert!(package(Inputs { binary: &dir.path().join("missing"), installer: &installer, output: &output, name: "test", target: "linux", epoch: 0, info: json!({}), licenses: vec![], abi: None }).is_err());
    assert!(!output.exists());
}

#[cfg(unix)]
#[test]
fn extracted_artifact_smoke_and_bad_checksum() {
    let target = screenshots_install::local_target().unwrap();
    let fixture = Fixture::new(b"#!/bin/sh\nprintf 'screenshots package fixture\\n'\n");
    let archive = fixture.package("packages", "screenshots-test", &target, 1_700_000_000, json!({"version": "0.1.0", "target": target}), vec![]).unwrap();
    let report = smoke(&archive).unwrap();
    assert_eq!(report["user_state"], "unchanged");
    let name = archive.file_name().unwrap().to_string_lossy().into_owned();
    fs::write(archive.with_file_name(format!("{name}.sha256")), "invalid checksum\n").unwrap();
    assert!(smoke(&archive).is_err());
}
