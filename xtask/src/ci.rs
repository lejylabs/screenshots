//! Small CI helpers that replace former inline Python workflow steps.
use anyhow::{Context, Result, bail, ensure};
use screenshots_install::{archive, sha_bytes};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn env(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("{name} is not set"))
}
fn sha_file(path: &Path) -> Result<String> {
    Ok(sha_bytes(&fs::read(path)?))
}
fn output(command: &mut Command) -> Result<String> {
    let result = command.output()?;
    ensure!(result.status.success(), "Command failed: {command:?}");
    Ok(String::from_utf8(result.stdout)?.trim().to_owned())
}

/// Download `<name>-<version>.crate`, verify its checksum and manifest identity and
/// extract it below `dest` (regular files only, no traversal).
fn fetch_crate_source(name: &str, version: &str, expected: &str, dest: &Path) -> Result<()> {
    let url = format!("https://static.crates.io/crates/{name}/{name}-{version}.crate");
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let body = runtime.block_on(async {
        let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(60)).build()?;
        let mut response = client.get(url).send().await?.error_for_status()?;
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            body.extend_from_slice(&chunk);
            ensure!(body.len() <= 4 * 1024 * 1024, "{name} source archive is oversized");
        }
        anyhow::Ok(body)
    })?;
    ensure!(sha_bytes(&body) == expected, "{name} source checksum mismatch");
    fs::create_dir(dest)?;
    let limits = archive::TarLimits { entries: 8192, member: 64 << 20, total: 256 << 20 };
    archive::read_tar_gz(&body[..], &limits, &mut |member| {
        let parts: Vec<&str> = member.name.split('/').collect();
        ensure!(parts.iter().all(|p| !matches!(*p, "" | "." | "..") && !p.contains('\\') && !p.contains(':')), "Unsafe archive path");
        let path = parts.iter().fold(dest.to_path_buf(), |p, part| p.join(part));
        fs::create_dir_all(path.parent().context("parent")?)?;
        let mut data = Vec::new();
        member.reader.read_to_end(&mut data)?;
        fs::write(path, data)?;
        Ok(())
    })?;
    let manifest: toml::Table = fs::read_to_string(dest.join(format!("{name}-{version}/Cargo.toml")))?.parse()?;
    let package = manifest.get("package").context("missing [package]")?;
    ensure!(
        package.get("name").and_then(|v| v.as_str()) == Some(name) && package.get("version").and_then(|v| v.as_str()) == Some(version),
        "Fetched crate identity mismatch"
    );
    Ok(())
}

fn deny_provenance() -> Result<()> {
    let evidence = PathBuf::from(env("SCREENSHOTS_POLICY_EVIDENCE")?);
    let toolchain = env("SCREENSHOTS_RUST_TOOLCHAIN")?;
    let pinned: toml::Table = fs::read_to_string("rust-toolchain.toml")?.parse()?;
    ensure!(pinned.get("toolchain").and_then(|t| t.get("channel")).and_then(|c| c.as_str()) == Some(toolchain.as_str()), "Toolchain differs from rust-toolchain.toml");
    let version = env("SCREENSHOTS_DENY_VERSION")?;
    let expected = env("SCREENSHOTS_DENY_SOURCE_SHA256")?;
    let source = PathBuf::from(env("SCREENSHOTS_DENY_SOURCE")?).join(format!("cargo-deny-{version}"));
    let binary = PathBuf::from(env("SCREENSHOTS_DENY_ROOT")?).join("bin/cargo-deny");
    let actual = output(Command::new(&binary).arg("--version"))?;
    ensure!(actual == format!("cargo-deny {version}"), "{actual}");
    let inputs = ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "deny.toml", ".github/workflows/dependencies.yml",
        "config/vendored-upstream-advisories.json", "xtask/src/advisory.rs", "xtask/src/advisory_tests.rs", "xtask/src/policy_controls.rs"];
    let mut hashes = serde_json::Map::new();
    for path in inputs {
        hashes.insert(path.into(), sha_file(Path::new(path))?.into());
    }
    let provenance = json!({"source_commit": env("GITHUB_SHA")?, "rust_toolchain": toolchain, "cargo_deny": actual,
        "tool_source_sha256": expected, "tool_binary_sha256": sha_file(&binary)?, "tool_lock_sha256": sha_file(&source.join("Cargo.lock"))?,
        "tool_cache": "none; compiled after exact registry archive checksum verification",
        "advisory_cache": "none; fresh job checkout/fetch, commit recorded after check", "inputs": hashes});
    fs::write(evidence.join("provenance.json"), serde_json::to_string_pretty(&provenance)? + "\n")?;
    Ok(())
}

fn advisory_snapshots() -> Result<()> {
    let evidence = PathBuf::from(env("SCREENSHOTS_POLICY_EVIDENCE")?);
    let mut snapshots = Vec::new();
    let root = Path::new("target/dependency-audit/advisory-dbs");
    let mut paths: Vec<PathBuf> = match fs::read_dir(root) {
        Ok(entries) => entries.filter_map(|e| e.ok()).map(|e| e.path()).collect(),
        Err(_) => Vec::new(),
    };
    paths.sort();
    for path in paths {
        if !path.is_dir() || !(path.join(".git").exists() || path.join("HEAD").is_file()) {
            continue;
        }
        let git = |args: &[&str]| output(Command::new("git").arg("-C").arg(&path).args(args));
        snapshots.push(json!({"directory": path.file_name().map(|n| n.to_string_lossy().into_owned()),
            "origin": git(&["remote", "get-url", "origin"])?, "commit": git(&["rev-parse", "HEAD"])?}));
    }
    fs::write(evidence.join("advisory-snapshots.json"), serde_json::to_string_pretty(&snapshots)? + "\n")?;
    if evidence.join("check-succeeded").exists() {
        ensure!(snapshots.len() == 1, "Successful check must identify its exact advisory database");
        let origin = snapshots[0]["origin"].as_str().unwrap_or_default().trim_end_matches('/').to_lowercase();
        ensure!(origin == "https://github.com/rustsec/advisory-db", "Unexpected advisory database origin");
    }
    let before = evidence.join("provenance.json");
    if before.exists() {
        let recorded: Value = serde_json::from_slice(&fs::read(&before)?)?;
        ensure!(recorded["inputs"]["Cargo.lock"] == sha_file(Path::new("Cargo.lock"))?, "Dependency check changed Cargo.lock");
    }
    Ok(())
}

/// Standalone crate that enters the clipboard decode VM budget in a fresh process.
fn vm_budget_fixture(fixture: &Path) -> Result<()> {
    let root = std::env::current_dir()?;
    let lock: toml::Table = fs::read_to_string(root.join("Cargo.lock"))?.parse()?;
    let packages = lock.get("package").and_then(|p| p.as_array()).context("Cargo.lock packages")?;
    let version = |name: &str| -> Result<String> {
        packages.iter().find(|p| p.get("name").and_then(|n| n.as_str()) == Some(name))
            .and_then(|p| p.get("version")).and_then(|v| v.as_str()).map(str::to_owned).with_context(|| format!("{name} not locked"))
    };
    fs::create_dir_all(fixture.join("src"))?;
    fs::write(fixture.join("Cargo.toml"), format!(
        "[package]\nname=\"owned-native-budget-probe\"\nversion=\"0.0.0\"\nedition=\"2024\"\n[workspace]\n[dependencies]\nanyhow=\"={}\"\nlibc=\"={}\"\n",
        version("anyhow")?, version("libc")?))?;
    let budget = root.join("crates/app-core/src/clipboard_input_budget.rs");
    fs::write(fixture.join("src/main.rs"), format!(
        "#[allow(dead_code)]\n#[path={}]\nmod budget;\nfn main()->anyhow::Result<()> {{ let mut budget=budget::Budget::enter()?; budget.decoding()?; println!(\"post-exec VM growth bound enforced\"); Ok(()) }}\n",
        Value::from(budget.to_string_lossy().into_owned())))?;
    Ok(())
}

fn sdk_contract() -> Result<()> {
    let sdk = PathBuf::from(output(Command::new("xcrun").arg("--show-sdk-path"))?);
    let source = fs::read_to_string(sdk.join("System/Library/Frameworks/AVFoundation.framework/Headers/AVCaptureInput.h"))?;
    let interface = source.find("@interface AVCaptureScreenInput").unwrap_or(0);
    let start = source[interface..].find("@property").map(|i| i + interface).unwrap_or(interface);
    for name in ["cropRect", "scaleFactor", "removesDuplicateFrames"] {
        let found = source[start..].find(name).map(|i| i + start).unwrap_or(start);
        let from = start.max(found.saturating_sub(2300));
        let to = (found + 700).min(source.len());
        let (from, to) = (floor(&source, from), floor(&source, to));
        println!("SDK contract {name} {}", &source[from..to]);
    }
    Ok(())
}
fn floor(text: &str, mut index: usize) -> usize {
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

pub fn main(args: &[String]) -> Result<()> {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["fetch-crate-source", name, version, sha256, dest] => fetch_crate_source(name, version, sha256, Path::new(dest)),
        ["deny-provenance"] => deny_provenance(),
        ["advisory-snapshots"] => advisory_snapshots(),
        ["vm-budget-fixture", dir] => vm_budget_fixture(Path::new(dir)),
        ["sdk-contract"] => sdk_contract(),
        _ => bail!("usage: cargo xtask ci fetch-crate-source <name> <version> <sha256> <dest> | deny-provenance | advisory-snapshots | vm-budget-fixture <dir> | sdk-contract"),
    }
}
