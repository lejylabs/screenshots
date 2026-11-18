//! Build a native portable archive without installing or publishing it.
use anyhow::{Context, Result, bail, ensure};
use screenshots_install::{TempDir, archive, open_regular, regular_bytes, set_mode, sha, sha_bytes};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
};

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("workspace root").to_path_buf()
}
pub fn command(args: &[&str]) -> Result<String> {
    let output = Command::new(args[0]).args(&args[1..]).current_dir(root()).output()?;
    ensure!(output.status.success(), "{} failed", args[0]);
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn license_files(item: &Value) -> Result<Vec<PathBuf>> {
    let source = Path::new(item["manifest_path"].as_str().context("manifest path")?).parent().context("manifest dir")?.to_path_buf();
    let mut files = Vec::new();
    for entry in fs::read_dir(&source)? {
        let path = entry?.path();
        let upper = path.file_name().unwrap_or_default().to_string_lossy().to_uppercase();
        if path.is_file() && ["LICENSE", "LICENCE", "COPYING", "NOTICE", "PATENTS"].iter().any(|p| upper.starts_with(p)) {
            files.push(path);
        }
    }
    if let Some(file) = item["license_file"].as_str() {
        files.push(source.join(file));
    }
    let name = item["name"].as_str().unwrap_or_default();
    let extra: &[(&str, &str)] = match name {
        // Their published crates omit the workspace licenses restored in vendor/.
        "accesskit_unix" | "accesskit_atspi_common" => &[("", "LICENSE-MIT"), ("", "LICENSE-APACHE")],
        // The statically compiled C library's notices live below the Rust wrapper.
        "libwebp-sys" => &[("vendor", "COPYING"), ("vendor", "PATENTS")],
        _ => &[],
    };
    for (directory, file) in extra {
        let path = source.join(directory).join(file);
        ensure!(fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()), "Vendored license or patent notice is missing");
        files.push(path);
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn bounded_license(path: &Path) -> Result<Vec<u8>> {
    let bytes = regular_bytes(path, 1024 * 1024)?;
    ensure!(!bytes.is_empty(), "Native dependency notice must be a regular file of at most 1 MiB");
    Ok(bytes)
}

fn native_licenses(target: &str) -> Result<Vec<Value>> {
    let expected = if target.contains("windows") {
        "x64-windows-static-md"
    } else if target == "aarch64-apple-darwin" {
        "arm64-osx"
    } else if target == "x86_64-apple-darwin" {
        "x64-osx"
    } else {
        return Ok(Vec::new());
    };
    let installed = std::env::var("VCPKG_INSTALLED_ROOT").ok().filter(|p| Path::new(p).is_absolute());
    let triplet = std::env::var("VCPKGRS_TRIPLET").ok();
    let (Some(installed), true) = (installed, triplet.as_deref() == Some(expected)) else {
        bail!("Native packaging requires the pinned static vcpkg installation and target triplet");
    };
    let manifest = screenshots_install::read_json(&root().join("packaging/windows/vcpkg.json"), 1024 * 1024)?;
    let baseline = manifest["builtin-baseline"].as_str().context("vcpkg baseline")?;
    let mut result = Vec::new();
    for (name, license) in [("curl", "curl"), ("libssh2", "BSD-3-Clause"), ("openssl", "Apache-2.0"), ("zlib", "Zlib")] {
        let path = Path::new(&installed).join(expected).join("share").join(name).join("copyright");
        bounded_license(&path)?;
        result.push(json!({"name": format!("native-{name}"), "version": format!("vcpkg-{baseline}"), "license": license,
                           "source": format!("vcpkg baseline {baseline}"), "license_files": [path], "bounded_native_notice": true}));
    }
    Ok(result)
}

/// Record the executable's linked glibc requirement rather than guessing its build host.
fn linux_abi(binary: &Path) -> Result<Value> {
    let output = Command::new("readelf").args(["--version-info", "--wide"]).arg(binary).output()?;
    ensure!(output.status.success() && output.stdout.len() <= 8 * 1024 * 1024, "ELF version metadata is unavailable or exceeds 8 MiB");
    glibc_requirement(&String::from_utf8_lossy(&output.stdout))
}
/// Highest `GLIBC_x.y[.z]` symbol version in readelf output.
fn glibc_requirement(text: &str) -> Result<Value> {
    let mut minimum: Option<(u32, u32, u32)> = None;
    for (index, _) in text.match_indices("GLIBC_") {
        let rest = &text[index + 6..];
        let version: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        let parts: Vec<u32> = version.split('.').filter_map(|p| p.parse().ok()).collect();
        if (2..=3).contains(&parts.len()) {
            let value = (parts[0], parts[1], parts.get(2).copied().unwrap_or(0));
            minimum = minimum.max(Some(value));
        }
    }
    let (major, minor, patch) = minimum.context("Native Linux executable has no declared glibc ABI")?;
    let glibc = if patch > 0 { format!("{major}.{minor}.{patch}") } else { format!("{major}.{minor}") };
    Ok(json!({"glibc": glibc}))
}

/// Copy a bounded regular file, failing if it changes while copying; returns its SHA-256.
fn frozen_copy(source: &Path, destination: &Path) -> Result<String> {
    let (mut input, admitted) = open_regular(source)?;
    ensure!(admitted.len() > 0 && admitted.len() <= 512 * 1024 * 1024, "Package executable must be a regular file of at most 512 MiB");
    let mut output = fs::OpenOptions::new().write(true).create_new(true).open(destination)?;
    let mut data = Vec::with_capacity(admitted.len() as usize);
    (&mut input).take(admitted.len() + 1).read_to_end(&mut data)?;
    let current = input.metadata()?;
    ensure!(data.len() as u64 == admitted.len() && current.len() == admitted.len() && current.modified().ok() == admitted.modified().ok(), "Package executable changed during copying");
    output.write_all(&data)?;
    output.sync_all()?;
    set_mode(destination, 0o755)?;
    Ok(sha_bytes(&data))
}

fn files_sorted(folder: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![folder.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            if fs::symlink_metadata(&path)?.is_dir() { stack.push(path) } else { out.push(path) }
        }
    }
    out.sort();
    Ok(out)
}
fn relative_name(path: &Path, base: &Path) -> String {
    path.strip_prefix(base).unwrap_or(path).components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

/// Write `folder` (named relative to `base`) as tar.gz or zip into `archive_path`.
pub fn write_archive(folder: &Path, base: &Path, archive_path: &Path, zip: bool, epoch: u64, executables: &[PathBuf]) -> Result<()> {
    let file = fs::OpenOptions::new().write(true).create_new(true).open(archive_path)?;
    let mode = |path: &Path| if executables.iter().any(|e| e == path) { 0o755 } else { 0o644 };
    if zip {
        // ZIP timestamps cannot represent dates before 1980.
        let date = archive::utc_civil(epoch.max(315_532_800) as i64);
        let mut writer = archive::ZipWriter::new(file);
        for path in files_sorted(folder)? {
            writer.file(&relative_name(&path, base), mode(&path), date, &mut fs::File::open(&path)?)?;
        }
        writer.finish()?.sync_all()?;
    } else {
        let mut writer = archive::TarWriter::new(archive::gzip_writer(file, epoch as u32));
        for path in files_sorted(folder)? {
            let size = fs::metadata(&path)?.len();
            writer.file(&relative_name(&path, base), mode(&path), epoch, &mut fs::File::open(&path)?, size)?;
        }
        writer.finish()?.finish()?.sync_all()?;
    }
    Ok(())
}

/// Hard links publish without replacing an existing name on the destination filesystem.
pub fn publish(staged: &Path, staged_checksum: &Path, destination: &Path, checksum: &Path) -> Result<()> {
    fs::hard_link(staged, destination)?;
    // A shared-name unlink could delete a concurrent replacement; keep any linked
    // archive for review instead of a destructive rollback.
    fs::hard_link(staged_checksum, checksum)
        .context("Archive publication may be incomplete; the output directory was preserved. Use a new directory before retrying.")
}

pub struct Inputs<'a> {
    pub binary: &'a Path,
    pub installer: &'a Path,
    pub output: &'a Path,
    pub name: &'a str,
    pub target: &'a str,
    pub epoch: u64,
    pub info: Value,
    pub licenses: Vec<Value>,
    /// Measures the frozen staged Linux executable; `None` skips ABI recording.
    pub abi: Option<&'a dyn Fn(&Path) -> Result<Value>>,
}

/// Stage complete files, then publish without replacing an existing archive.
pub fn package(inputs: Inputs<'_>) -> Result<PathBuf> {
    let Inputs { binary, installer, output, name, target, epoch, mut info, licenses, abi } = inputs;
    // Refuse a missing or irregular input before creating any output.
    open_regular(binary)?;
    open_regular(installer)?;
    fs::create_dir_all(output)?;
    let windows = target.contains("windows");
    let filename = format!("{name}{}", if windows { ".zip" } else { ".tar.gz" });
    let destination = output.join(&filename);
    let checksum = output.join(format!("{filename}.sha256"));
    ensure!(!destination.exists() && !checksum.exists(), "Package or checksum already exists");
    let staging = TempDir::new_in(output, ".package-")?;
    let folder = staging.path().join(name);
    fs::create_dir(&folder)?;
    let suffix = if windows { ".exe" } else { "" };
    let executable = folder.join(format!("screenshots{suffix}"));
    let installer_copy = folder.join(format!("screenshots-install{suffix}"));
    let digest = frozen_copy(binary, &executable)?;
    frozen_copy(installer, &installer_copy)?;
    if let Some(abi) = abi.filter(|_| target.contains("linux")) {
        // Inspect the frozen staged executable, not a concurrently rebuilt input.
        info["linux_abi"] = abi(&executable)?;
    }
    info["binary_sha256"] = digest.into();
    fs::write(folder.join("BUILD-INFO.json"), serde_json::to_string_pretty(&info)? + "\n")?;
    let root = root();
    for source in ["LICENSE", "THIRD_PARTY_NOTICES.md", "README.md", "Cargo.lock"] {
        fs::copy(root.join(source), folder.join(source))?;
    }
    let packaging = fs::read_to_string(root.join("docs/packaging.md"))?
        .replace("](desktop-integration.md)", "](DESKTOP-INTEGRATION.md)")
        .replace("](native-signing.md)", "](NATIVE-SIGNING.md)");
    fs::write(folder.join("PACKAGING.md"), packaging)?;
    fs::copy(root.join("docs/native-signing.md"), folder.join("NATIVE-SIGNING.md"))?;
    fs::copy(root.join("docs/desktop-integration.md"), folder.join("DESKTOP-INTEGRATION.md"))?;
    if target.contains("linux") {
        fs::copy(root.join("packaging/screenshots.desktop"), folder.join("screenshots.desktop"))?;
    }
    let license_dir = folder.join("licenses");
    fs::create_dir(&license_dir)?;
    let public: Vec<Value> = licenses
        .iter()
        .map(|item| {
            let mut item = item.clone();
            if let Some(map) = item.as_object_mut() {
                map.remove("license_files");
                map.remove("bounded_native_notice");
            }
            item
        })
        .collect();
    fs::write(license_dir.join("dependencies.json"), serde_json::to_string_pretty(&public)? + "\n")?;
    for item in &licenses {
        for source in item["license_files"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            // Package roots are Cargo metadata, never user image or config paths.
            let source = Path::new(source);
            let directory = license_dir.join(format!("{}-{}", item["name"].as_str().unwrap_or_default(), item["version"].as_str().unwrap_or_default()));
            fs::create_dir_all(&directory)?;
            let target_file = directory.join(source.file_name().context("license name")?);
            if item["bounded_native_notice"] == true {
                fs::write(target_file, bounded_license(source)?)?;
            } else {
                fs::copy(source, target_file)?;
            }
        }
    }
    let archive_path = staging.path().join(&filename);
    write_archive(&folder, staging.path(), &archive_path, windows, epoch, &[executable, installer_copy])?;
    let staged_checksum = staging.path().join(format!("{filename}.sha256"));
    fs::write(&staged_checksum, format!("{}  {filename}\n", sha(&archive_path)?))?;
    publish(&archive_path, &staged_checksum, &destination, &checksum)?;
    Ok(destination)
}

pub fn main(args: &[String]) -> Result<()> {
    let mut target = None;
    let mut binary = None;
    let mut installer = None;
    let mut output = root().join("target/packages");
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        let value = iter.next().with_context(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--target" => target = Some(value.clone()),
            "--binary" => binary = Some(PathBuf::from(value)),
            "--installer" => installer = Some(PathBuf::from(value)),
            "--output" => output = PathBuf::from(value),
            _ => bail!("usage: cargo xtask package --target <triple> [--binary <path> --installer <path>] [--output <dir>]"),
        }
    }
    let target = target.context("--target is required")?;
    let verbose = command(&["rustc", "-vV"])?;
    let host = verbose.lines().find_map(|l| l.strip_prefix("host: ")).context("rustc host")?.to_owned();
    ensure!(target == host, "Build and package on the native target; cross-built archives need a separate runtime check");
    let prebuilt = binary.is_some();
    if !prebuilt {
        let status = Command::new("cargo").args(["build", "--locked", "--release", "-p", "screenshots", "-p", "screenshots-install"]).current_dir(root()).status()?;
        ensure!(status.success(), "cargo build failed");
    }
    let suffix = if host.contains("windows") { ".exe" } else { "" };
    let binary = binary.unwrap_or_else(|| root().join(format!("target/release/screenshots{suffix}")));
    let installer = installer.unwrap_or_else(|| binary.with_file_name(format!("screenshots-install{suffix}")));
    let metadata: Value = serde_json::from_str(&command(&["cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", &host])?)?;
    let packages = metadata["packages"].as_array().context("metadata packages")?;
    let version = packages.iter().find(|p| p["name"] == "screenshots").and_then(|p| p["version"].as_str()).context("screenshots version")?.to_owned();
    let mut sorted: Vec<&Value> = packages.iter().collect();
    sorted.sort_by_key(|p| (p["name"].as_str().unwrap_or_default().to_owned(), p["version"].as_str().unwrap_or_default().to_owned()));
    let mut licenses = Vec::new();
    for item in sorted {
        let files: Vec<String> = license_files(item)?.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        licenses.push(json!({"name": item["name"], "version": item["version"], "license": item["license"], "source": item["source"], "license_files": files}));
    }
    licenses.extend(native_licenses(&host)?);
    let epoch: u64 = match std::env::var("SOURCE_DATE_EPOCH") {
        Ok(value) => value.parse()?,
        Err(_) => command(&["git", "show", "-s", "--format=%ct", "HEAD"])?.parse()?,
    };
    let info = json!({"version": version, "target": host, "commit": command(&["git", "rev-parse", "HEAD"])?,
        "dirty": !command(&["git", "status", "--porcelain"])?.is_empty(), "rustc": verbose, "source_date_epoch": epoch,
        "binary_input": if prebuilt { "explicit prebuilt binary" } else { "cargo build --locked --release" },
        "signing": "unsigned; native runtime acceptance is separate"});
    let name = format!("screenshots-{version}-{host}");
    let path = package(Inputs { binary: &binary, installer: &installer, output: &output, name: &name, target: &host, epoch, info, licenses, abi: Some(&linux_abi) })?;
    println!("{}", path.display());
    Ok(())
}

/// Verify an archive's checksum, extract it and run headless `--version`/`--help`
/// with isolated user directories, failing if either writes user state.
pub fn smoke(archive: &Path) -> Result<Value> {
    let name = archive.file_name().context("archive name")?.to_string_lossy().into_owned();
    let checksum = String::from_utf8(regular_bytes(&archive.with_file_name(format!("{name}.sha256")), 1024)?)?;
    ensure!(checksum == format!("{}  {name}\n", sha(archive)?), "Archive checksum does not match");
    let root = TempDir::new("screenshots-package-smoke-")?;
    let folder = root.path().join("extract");
    fs::create_dir(&folder)?;
    let extracted = screenshots_install::extract(archive, &folder)?;
    let binary = folder.join(extracted.executable);
    let isolated = ["HOME", "USERPROFILE", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "APPDATA", "LOCALAPPDATA"];
    let mut command_env: Vec<(String, PathBuf)> = Vec::new();
    for variable in isolated {
        let path = root.path().join(variable.to_lowercase());
        fs::create_dir(&path)?;
        command_env.push((variable.into(), path));
    }
    for argument in ["--version", "--help"] {
        let mut command = Command::new(&binary);
        command.arg(argument).current_dir(root.path());
        for variable in ["DISPLAY", "WAYLAND_DISPLAY", "RUST_LOG"] {
            command.env_remove(variable);
        }
        command.envs(command_env.iter().map(|(k, v)| (k, v)));
        let output = run_bounded(command, std::time::Duration::from_secs(15))?;
        ensure!(String::from_utf8_lossy(&output).to_lowercase().contains("screenshots"), "Headless command did not identify screenshots");
    }
    for (_, path) in &command_env {
        ensure!(fs::read_dir(path)?.next().is_none(), "Headless help/version unexpectedly wrote user state");
    }
    Ok(json!({"archive": name, "binary_sha256": extracted.info["binary_sha256"], "headless_help_and_version": "passed", "user_state": "unchanged"}))
}
fn run_bounded(mut command: Command, timeout: std::time::Duration) -> Result<Vec<u8>> {
    command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
    let mut child = command.spawn()?;
    let mut stdout = child.stdout.take().context("stdout")?;
    let reader = std::thread::spawn(move || {
        let mut data = Vec::new();
        let _ = (&mut stdout).take(1 << 20).read_to_end(&mut data);
        data
    });
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Headless command exceeded its deadline");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let output = reader.join().map_err(|_| anyhow::anyhow!("reader failed"))?;
    ensure!(status.success(), "Headless command failed");
    Ok(output)
}

pub fn smoke_main(args: &[String]) -> Result<()> {
    let [archive] = args else { bail!("usage: cargo xtask package-smoke <archive>") };
    println!("{}", smoke(&Path::new(archive).canonicalize()?)?);
    Ok(())
}

#[cfg(test)]
#[path = "package_tests.rs"]
mod tests;
