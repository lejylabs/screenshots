//! Explicit native identity selectors and owned-staging signing; never configures trust.
use crate::run::contained;
use anyhow::{Context, Result, bail, ensure};
use screenshots_install::{TempDir, read_json, sha};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

/// Public selector text: bounded, no leading dash (never an option), no controls.
pub fn text(value: &str, limit: usize) -> Result<&str> {
    ensure!(
        !value.is_empty() && value.len() <= limit && !value.starts_with('-') && !value.chars().any(|c| (c as u32) < 32 || c == '\u{7f}'),
        "Invalid public signing selector"
    );
    Ok(value)
}

pub fn profile(path: &Path, platform: &str) -> Result<Value> {
    let value = read_json(path, 16384)?;
    let fields: &[&str] = match platform {
        "macos" => &["application_identity", "installer_identity", "team_id"],
        "windows" => &["thumbprint", "publisher", "timestamp_url"],
        _ => bail!("Signing profile must match the native platform and exact version1 schema"),
    };
    let object = value.as_object().context("Signing profile must be an object")?;
    ensure!(
        object.len() == fields.len() + 2
            && fields.iter().all(|f| object.contains_key(*f))
            && value["version"].as_u64() == Some(1)
            && value["platform"] == platform,
        "Signing profile must match the native platform and exact version1 schema"
    );
    for key in fields {
        let field = value[*key].as_str().context("Invalid public signing selector")?;
        text(field, if *key == "timestamp_url" { 4096 } else { 1024 })?;
    }
    let get = |key: &str| value[key].as_str().unwrap_or_default();
    if platform == "macos" {
        let team = get("team_id");
        ensure!(team.len() == 10 && team.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()), "Invalid application team");
        for (field, role) in [("application_identity", "Application"), ("installer_identity", "Installer")] {
            ensure!(
                get(field).starts_with(&format!("Developer ID {role}: ")) && get(field).ends_with(&format!(" ({team})")),
                "Exact Developer ID identity names must match the configured team"
            );
        }
    } else {
        let thumbprint = get("thumbprint");
        ensure!(thumbprint.len() == 40 && thumbprint.bytes().all(|b| b.is_ascii_hexdigit()), "Certificate thumbprint must contain 40 hexadecimal digits");
        let url = reqwest::Url::parse(get("timestamp_url")).context("Invalid timestamp endpoint")?;
        ensure!(
            url.scheme() == "https" && url.host_str().is_some() && url.username().is_empty() && url.password().is_none() && url.fragment().is_none(),
            "Timestamp endpoint must be HTTPS without credentials/fragment"
        );
    }
    Ok(value)
}

const MAC_TOOLS: [(&str, &str); 5] = [
    ("codesign", "/usr/bin/codesign"),
    ("productsign", "/usr/bin/productsign"),
    ("pkgbuild", "/usr/bin/pkgbuild"),
    ("pkgutil", "/usr/sbin/pkgutil"),
    ("xcrun", "/usr/bin/xcrun"),
];

/// Resolve a protected native tool: fixed macOS system paths or the newest
/// registered Windows SDK binary.
pub fn native_tool(name: &str) -> Result<PathBuf> {
    if let Some((_, path)) = MAC_TOOLS.iter().find(|(tool, _)| *tool == name) {
        let path = PathBuf::from(path);
        ensure!(std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()), "Required protected macOS tool is unavailable");
        return Ok(path);
    }
    ensure!(matches!(name, "signtool.exe" | "makeappx.exe") && cfg!(windows), "Unsupported native signing tool");
    windows_sdk_tool(name)
}
#[cfg(windows)]
fn windows_sdk_tool(name: &str) -> Result<PathBuf> {
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6432KEY, RegGetValueW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (key, value) = (wide(r"SOFTWARE\Microsoft\Windows Kits\Installed Roots"), wide("KitsRoot10"));
    let mut buffer = vec![0u16; 1024];
    let mut size = (buffer.len() * 2) as u32;
    // SAFETY: valid NUL-terminated names and a writable buffer of `size` bytes.
    let status = unsafe {
        RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ | RRF_SUBKEY_WOW6432KEY, std::ptr::null_mut(), buffer.as_mut_ptr().cast(), &mut size)
    };
    ensure!(status == 0, "Invalid protected SDK registry root");
    let length = buffer.iter().position(|u| *u == 0).unwrap_or(buffer.len());
    let root = PathBuf::from(String::from_utf16(&buffer[..length])?);
    ensure!(root.is_absolute(), "Invalid protected SDK registry root");
    let mut best: Option<(Vec<u64>, PathBuf)> = None;
    for entry in std::fs::read_dir(root.join("bin"))? {
        let entry = entry?;
        let version_name = entry.file_name().to_string_lossy().into_owned();
        let parts: Option<Vec<u64>> = version_name.split('.').map(|p| p.parse().ok()).collect();
        let Some(parts) = parts.filter(|p| p.len() >= 2) else { continue };
        let candidate = entry.path().join("x64").join(name);
        if std::fs::symlink_metadata(&candidate).is_ok_and(|m| m.is_file()) && best.as_ref().is_none_or(|(v, _)| parts > *v) {
            best = Some((parts, candidate));
        }
    }
    let (_, path) = best.context("Windows SDK signing tool is unavailable")?;
    Ok(path.canonicalize()?)
}
#[cfg(not(windows))]
fn windows_sdk_tool(_: &str) -> Result<PathBuf> {
    bail!("Unsupported native signing tool")
}

/// Inherited environment without loader/toolchain overrides, a protected PATH and C locale.
fn native_environment() -> Result<Vec<(String, String)>> {
    let mut environment: Vec<(String, String)> = std::env::vars()
        .filter(|(name, _)| {
            let upper = name.to_uppercase();
            !(upper.starts_with("DYLD_")
                || upper.starts_with("LD_")
                || upper.starts_with("XCRUN_")
                || matches!(upper.as_str(), "DEVELOPER_DIR" | "SDKROOT" | "TOOLCHAINS" | "CODESIGN_ALLOCATE" | "PATH"))
        })
        .filter(|(name, _)| !matches!(name.to_uppercase().as_str(), "LC_ALL" | "LANG"))
        .collect();
    environment.push(("PATH".into(), protected_path()?));
    environment.push(("LC_ALL".into(), "C".into()));
    environment.push(("LANG".into(), "C".into()));
    Ok(environment)
}
#[cfg(windows)]
fn protected_path() -> Result<String> {
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
    let mut buffer = vec![0u16; 32768];
    // SAFETY: writable buffer of the given length.
    let length = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    ensure!(length > 0 && length < buffer.len(), "Could not resolve protected system directory");
    Ok(String::from_utf16(&buffer[..length])?)
}
#[cfg(not(windows))]
fn protected_path() -> Result<String> {
    Ok("/usr/bin:/bin:/usr/sbin:/sbin".into())
}

/// Run a release tool. Known native tools are resolved to protected paths and run
/// with the protected environment.
pub fn tool(args: &[&dyn AsRef<std::ffi::OsStr>], timeout: Duration, capture: bool) -> Result<Vec<u8>> {
    let first = args.first().context("Missing release tool")?.as_ref().to_string_lossy().into_owned();
    let native = MAC_TOOLS.iter().any(|(name, _)| *name == first) || matches!(first.as_str(), "signtool.exe" | "makeappx.exe");
    let program = if native { native_tool(&first)? } else { PathBuf::from(&first) };
    let mut command = Command::new(program);
    command.args(args[1..].iter().map(|a| a.as_ref()));
    if native {
        command.env_clear().envs(native_environment()?);
    }
    let working = TempDir::new("screenshots-sign-tool-")?;
    contained(command, working.path(), timeout, capture)
}
fn run(args: &[&dyn AsRef<std::ffi::OsStr>]) -> Result<()> {
    tool(args, Duration::from_secs(120), false).map(drop)
}

fn verify_mac(path: &Path, signing: &Value) -> Result<()> {
    let requirement = format!("anchor apple generic and certificate leaf[subject.OU] = \"{}\"", signing["team_id"].as_str().unwrap_or_default());
    run(&[&"codesign", &"--verify", &"--strict", &"-R", &requirement, &path])
}
fn write_info(path: &Path, info: &Value) -> Result<()> {
    std::fs::write(path, serde_json::to_string_pretty(info)? + "\n")?;
    Ok(())
}

/// Sign and verify the application bundle; returns the updated build information.
pub fn mac_app(app: &Path, info: &Value, signing: &Value) -> Result<Value> {
    let app = app.canonicalize()?;
    let identity = signing["application_identity"].as_str().unwrap_or_default();
    let mut updated = info.clone();
    updated["binary_sha256_scope"] = "admitted executable before enclosing bundle signature".into();
    updated["signing"] = format!("Developer ID team {}; native signature verified before publication", signing["team_id"].as_str().unwrap_or_default()).into();
    write_info(&app.join("Contents/Resources/BUILD-INFO.json"), &updated)?;
    run(&[&"codesign", &"--force", &"--sign", &identity, &"--identifier", &"me.lejy.screenshots", &"--options", &"runtime", &"--timestamp", &app])?;
    verify_mac(&app, signing)?;
    run(&[&"codesign", &"--verify", &"--deep", &"--strict", &app])?;
    Ok(updated)
}

/// Authenticode-sign the portable executables before MSIX packing.
pub fn windows_payload(portable: &Path, info: &Value, signing: &Value) -> Result<Value> {
    let portable = portable.canonicalize()?;
    for name in ["screenshots.exe", "screenshots-install.exe"] {
        windows_package(&portable.join(name), signing)?;
    }
    let mut updated = info.clone();
    updated["binary_sha256"] = sha(&portable.join("screenshots.exe"))?.into();
    updated["signing"] = "Authenticode verified before packaging".into();
    write_info(&portable.join("BUILD-INFO.json"), &updated)?;
    Ok(updated)
}
pub fn windows_package(path: &Path, signing: &Value) -> Result<()> {
    let path = path.canonicalize()?;
    let thumbprint = signing["thumbprint"].as_str().unwrap_or_default();
    let timestamp = signing["timestamp_url"].as_str().unwrap_or_default();
    run(&[&"signtool.exe", &"sign", &"/fd", &"SHA256", &"/sha1", &thumbprint, &"/tr", &timestamp, &"/td", &"SHA256", &path])?;
    run(&[&"signtool.exe", &"verify", &"/pa", &"/all", &path])
}

pub fn mac_package(unsigned: &Path, signed: &Path, signing: &Value, notary_profile: Option<&str>) -> Result<()> {
    let installer = signing["installer_identity"].as_str().unwrap_or_default();
    run(&[&"productsign", &"--sign", &installer, &unsigned, &signed])?;
    let certificate = String::from_utf8(tool(&[&"pkgutil", &"--check-signature", &signed], Duration::from_secs(120), true)?)?;
    let leaf = certificate.lines().find_map(|line| {
        let rest = line.trim_start().strip_prefix("1.")?;
        rest.starts_with(char::is_whitespace).then(|| rest.trim())
    });
    let trusted = certificate.lines().any(|line| {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        line == "Status: signed by a certificate trusted by macOS"
            || line == "Status: signed by a certificate trusted by Mac OS X"
            || line == "Status: signed by a developer certificate issued by Apple for distribution"
    });
    ensure!(leaf == Some(installer) && trusted, "Signed installer certificate/team does not match the requested trusted identity");
    if let Some(profile) = notary_profile {
        text(profile, 256)?;
        let response = tool(
            &[&"xcrun", &"notarytool", &"submit", &signed, &"--keychain-profile", &profile, &"--wait", &"--timeout", &"15m", &"--output-format", &"json"],
            Duration::from_secs(930),
            true,
        )?;
        let result: Value = serde_json::from_slice(&response).context("Notarization did not accept the package")?;
        ensure!(result["status"] == "Accepted", "Notarization did not accept the package");
        run(&[&"xcrun", &"stapler", &"staple", &signed])?;
        run(&[&"xcrun", &"stapler", &"validate", &signed])?;
    }
    Ok(())
}
