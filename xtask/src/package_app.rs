//! Build native application bundles/installers from a verified portable archive.
//! macOS: .app plus .pkg when pkgbuild is available. Windows: .msix via the Windows
//! SDK MakeAppx. Linux: .deb. Signing/notarization requires a real identity.
use crate::{package, sign};
use anyhow::{Context, Result, bail, ensure};
use screenshots_install::{MAX_ARCHIVE, TempDir, archive, extract, open_regular, regular_bytes, set_mode, sha, version};
use serde_json::Value;
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

const ID: &str = "me.lejy.screenshots";

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn info_plist(version_text: &str) -> Result<String> {
    version(version_text)?;
    let types = ["public.png", "public.jpeg", "org.webmproject.webp", "com.microsoft.bmp", "public.tiff"]
        .map(|t| format!("\t\t\t\t<string>{t}</string>\n"))
        .concat();
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDisplayName</key>
	<string>Screenshots</string>
	<key>CFBundleDocumentTypes</key>
	<array>
		<dict>
			<key>CFBundleTypeName</key>
			<string>Image</string>
			<key>CFBundleTypeRole</key>
			<string>Viewer</string>
			<key>LSHandlerRank</key>
			<string>Alternate</string>
			<key>LSItemContentTypes</key>
			<array>
{types}			</array>
		</dict>
	</array>
	<key>CFBundleExecutable</key>
	<string>screenshots</string>
	<key>CFBundleIdentifier</key>
	<string>{ID}</string>
	<key>CFBundleName</key>
	<string>Screenshots</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>{version_text}</string>
	<key>CFBundleVersion</key>
	<string>{version_text}</string>
	<key>LSMinimumSystemVersion</key>
	<string>12.0</string>
	<key>NSHighResolutionCapable</key>
	<true/>
</dict>
</plist>
"#
    ))
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let path = entry?.path();
        let metadata = fs::symlink_metadata(&path)?;
        ensure!(!metadata.file_type().is_symlink(), "Portable bundle contains a symlink");
        let target = destination.join(path.file_name().context("name")?);
        if metadata.is_dir() { copy_tree(&path, &target)? } else { fs::copy(&path, &target).map(drop)? }
    }
    Ok(())
}

fn app_bundle(portable: &Path, app: &Path, info: &Value) -> Result<()> {
    ensure!(!app.exists(), "Application bundle destination exists");
    let executable = portable.join("screenshots");
    ensure!(executable.is_file(), "Portable macOS executable is missing");
    let binary = app.join("Contents/MacOS");
    let resources = app.join("Contents/Resources");
    fs::create_dir_all(&binary)?;
    fs::create_dir(&resources)?;
    fs::copy(&executable, binary.join("screenshots"))?;
    set_mode(&binary.join("screenshots"), 0o755)?;
    for entry in fs::read_dir(portable)? {
        let path = entry?.path();
        if path.file_name().is_some_and(|n| n == "screenshots") {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)?;
        ensure!(!metadata.file_type().is_symlink(), "Portable bundle contains a symlink");
        let target = resources.join(path.file_name().context("name")?);
        if metadata.is_dir() { copy_tree(&path, &target)? } else { fs::copy(&path, &target).map(drop)? }
    }
    fs::write(app.join("Contents/Info.plist"), info_plist(info["version"].as_str().unwrap_or_default())?)?;
    fs::write(app.join("Contents/PkgInfo"), b"APPL????")?;
    Ok(())
}

/// Small deterministic installer asset, without an image dependency.
fn png(path: &Path, side: u32) -> Result<()> {
    let mut raw = Vec::new();
    for y in 0..side {
        raw.push(0);
        for x in 0..side {
            let border = side / 6 <= x && x < 5 * side / 6 && side / 4 <= y && y < 3 * side / 4;
            let (dx, dy) = (x as i64 - (side / 2) as i64, y as i64 - (side / 2) as i64);
            let lens = dx * dx + dy * dy < ((side / 7) as i64).pow(2);
            raw.extend_from_slice(if border && !lens { &[235, 239, 245, 255] } else { &[35, 45, 65, 255] });
        }
    }
    let chunk = |kind: &[u8], data: &[u8]| {
        let mut out = (data.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(kind);
        hasher.update(data);
        out.extend_from_slice(&hasher.finalize().to_be_bytes());
        out
    };
    let mut header = side.to_be_bytes().to_vec();
    header.extend_from_slice(&side.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(&raw)?;
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend(chunk(b"IHDR", &header));
    bytes.extend(chunk(b"IDAT", &encoder.finish()?));
    bytes.extend(chunk(b"IEND", b""));
    fs::write(path, bytes)?;
    Ok(())
}

pub fn appx_manifest(version_text: &str, architecture: &str, publisher: &str) -> Result<String> {
    version(version_text)?;
    ensure!(matches!(architecture, "x64" | "arm64"), "Unsupported MSIX architecture");
    sign::text(publisher, 1024)?;
    let publisher = xml_escape(publisher);
    Ok(format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
 xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10"
 xmlns:uap3="http://schemas.microsoft.com/appx/manifest/uap/windows10/3"
 xmlns:rescap="http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities"
 IgnorableNamespaces="uap uap3 rescap">
 <Identity Name="me.lejy.screenshots" Publisher="{publisher}" Version="{version_text}.0" ProcessorArchitecture="{architecture}"/>
 <Properties><DisplayName>Screenshots</DisplayName><PublisherDisplayName>Screenshots</PublisherDisplayName><Logo>Assets\StoreLogo.png</Logo></Properties>
 <Resources><Resource Language="en-us"/></Resources>
 <Dependencies><TargetDeviceFamily Name="Windows.Desktop" MinVersion="10.0.19041.0" MaxVersionTested="10.0.26100.0"/></Dependencies>
 <Applications><Application Id="Screenshots" Executable="screenshots.exe" EntryPoint="Windows.FullTrustApplication">
  <uap:VisualElements DisplayName="Screenshots" Description="Capture and annotate screenshots" BackgroundColor="transparent" Square150x150Logo="Assets\Square150.png" Square44x44Logo="Assets\Square44.png"/>
  <Extensions><uap:Extension Category="windows.fileTypeAssociation"><uap3:FileTypeAssociation Name="screenshotsimage" Parameters="--open &quot;%1&quot;">
   <uap:SupportedFileTypes><uap:FileType>.png</uap:FileType><uap:FileType>.jpg</uap:FileType><uap:FileType>.jpeg</uap:FileType><uap:FileType>.webp</uap:FileType><uap:FileType>.bmp</uap:FileType><uap:FileType>.tif</uap:FileType><uap:FileType>.tiff</uap:FileType></uap:SupportedFileTypes>
  </uap3:FileTypeAssociation></uap:Extension></Extensions>
 </Application></Applications>
 <Capabilities><rescap:Capability Name="runFullTrust"/></Capabilities>
</Package>
"#
    ))
}

/// Unsigned builds use the newest installed SDK MakeAppx (PATH first).
fn sdk_makeappx() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("PATH").into_iter().flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).map(|d| d.join("makeappx.exe")).find(|p| p.is_file()) {
        return Ok(path);
    }
    let base = PathBuf::from(std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".into())).join("Windows Kits/10/bin");
    let mut paths: Vec<PathBuf> = fs::read_dir(&base)?.filter_map(|e| e.ok()).map(|e| e.path().join("x64/makeappx.exe")).filter(|p| p.is_file()).collect();
    paths.sort();
    paths.pop().context("Windows SDK MakeAppx is unavailable")
}

/// tar.gz of a tree, keeping symlinks as links (Debian control/data members).
fn tar_gzip(directory: &Path, output: &Path, epoch: u64) -> Result<()> {
    let file = fs::OpenOptions::new().write(true).create_new(true).open(output)?;
    let mut writer = archive::TarWriter::new(archive::gzip_writer(file, epoch as u32));
    let mut entries = Vec::new();
    let mut stack = vec![directory.to_path_buf()];
    while let Some(folder) = stack.pop() {
        for entry in fs::read_dir(folder)? {
            let path = entry?.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.is_dir() { stack.push(path) } else { entries.push((path, metadata)) }
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    for (path, metadata) in entries {
        let name = path.strip_prefix(directory)?.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
        if metadata.file_type().is_symlink() {
            writer.symlink(&name, &fs::read_link(&path)?.to_string_lossy().replace('\\', "/"), epoch)?;
        } else {
            let mode = if is_executable(&metadata) { 0o755 } else { 0o644 };
            writer.file(&name, mode, epoch, &mut fs::File::open(&path)?, metadata.len())?;
        }
    }
    writer.finish()?.finish()?;
    Ok(())
}
fn is_executable(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}

fn deb_package(portable: &Path, output: &Path, info: &Value) -> Result<()> {
    let target = info["target"].as_str().unwrap_or_default();
    let architecture = match target.split('-').next() {
        Some("x86_64") => "amd64",
        Some("aarch64") => "arm64",
        _ => bail!("Unsupported Debian architecture"),
    };
    let epoch = info["source_date_epoch"].as_u64().unwrap_or(0);
    let abi = info["linux_abi"]["glibc"].as_str().filter(|a| {
        let parts: Vec<&str> = a.split('.').collect();
        (2..=3).contains(&parts.len()) && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    });
    let abi = abi.context("Debian packaging requires the recorded native glibc ABI")?;
    let temporary = TempDir::new_in(output.parent().context("output dir")?, ".deb-")?;
    let (data, control) = (temporary.path().join("data"), temporary.path().join("control"));
    fs::create_dir_all(data.join("usr/bin"))?;
    let private = data.join("usr/lib/screenshots");
    fs::create_dir_all(&private)?;
    let docs = data.join("usr/share/doc/screenshots");
    fs::create_dir_all(&docs)?;
    fs::create_dir_all(data.join("usr/share/applications"))?;
    fs::copy(portable.join("screenshots"), private.join("screenshots"))?;
    set_mode(&private.join("screenshots"), 0o755)?;
    #[cfg(unix)]
    std::os::unix::fs::symlink("../lib/screenshots/screenshots", data.join("usr/bin/screenshots"))?;
    for entry in fs::read_dir(portable)? {
        let source = entry?.path();
        let name = source.file_name().context("name")?.to_string_lossy().into_owned();
        match name.as_str() {
            "screenshots" => {}
            "BUILD-INFO.json" | "screenshots-install" => {
                fs::copy(&source, private.join(&name))?;
                if name == "screenshots-install" {
                    set_mode(&private.join(&name), 0o755)?;
                }
            }
            _ if source.is_dir() => copy_tree(&source, &docs.join(&name))?,
            _ => fs::copy(&source, docs.join(&name)).map(drop)?,
        }
    }
    fs::write(
        data.join("usr/share/applications/me.lejy.screenshots.desktop"),
        "[Desktop Entry]\nType=Application\nVersion=1.0\nName=Screenshots\nExec=/usr/bin/screenshots --open %f\nTerminal=false\nCategories=Graphics;Utility;\nMimeType=image/png;image/jpeg;image/webp;image/bmp;image/tiff;\n",
    )?;
    fs::create_dir(&control)?;
    fs::write(control.join("control"), format!(
        "Package: screenshots\nVersion: {}\nArchitecture: {architecture}\nMaintainer: Screenshots contributors <noreply@github.com>\nSection: graphics\nPriority: optional\nDepends: libc6 (>= {abi}), libgcc-s1, libstdc++6, libcurl4t64 | libcurl4, libxkbcommon0, libxkbcommon-x11-0, libxcb1, libdbus-1-3, libgbm1, libfontconfig1, libfreetype6, ca-certificates, libssl3t64 | libssl3, zlib1g, libgssapi-krb5-2, libicu72 | libicu74 | libicu76 | libicu78\nHomepage: https://github.com/lejylabs/screenshots\nDescription: Native screenshot capture and sharing application\n",
        info["version"].as_str().unwrap_or_default()
    ))?;
    fs::write(temporary.path().join("debian-binary"), b"2.0\n")?;
    tar_gzip(&control, &temporary.path().join("control.tar.gz"), epoch)?;
    tar_gzip(&data, &temporary.path().join("data.tar.gz"), epoch)?;
    let mut archive = fs::OpenOptions::new().write(true).create_new(true).open(output)?;
    archive.write_all(b"!<arch>\n")?;
    for name in ["debian-binary", "control.tar.gz", "data.tar.gz"] {
        let source = temporary.path().join(name);
        let size = fs::metadata(&source)?.len();
        let header = format!("{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n", format!("{name}/"), epoch, 0, 0, "100644", size);
        ensure!(header.len() == 60, "Debian ar header exceeds its field widths");
        archive.write_all(header.as_bytes())?;
        std::io::copy(&mut fs::File::open(&source)?, &mut archive)?;
        if size % 2 == 1 {
            archive.write_all(b"\n")?;
        }
    }
    archive.sync_all()?;
    Ok(())
}

fn admit_destinations(sources: &[PathBuf], output: &Path) -> Result<()> {
    for source in sources {
        let name = source.file_name().context("name")?.to_string_lossy().into_owned();
        let destination = output.join(&name);
        let checksum = output.join(format!("{name}.sha256"));
        ensure!(fs::symlink_metadata(&destination).is_err() && fs::symlink_metadata(&checksum).is_err(), "Package destination exists");
    }
    Ok(())
}
/// Publish one artifact and its checksum by hard links. On failure nothing that was
/// already linked is removed: a shared-name unlink could delete a foreign replacement.
fn publish_file(source: &Path, destination: &Path) -> Result<()> {
    let name = destination.file_name().context("name")?.to_string_lossy().into_owned();
    // Open handles keep the verified source's identity alive until publication settles.
    let _artifact = fs::File::open(source)?;
    let staging = TempDir::new_in(source.parent().context("source dir")?, ".publish-checksum-")?;
    let staged = staging.path().join("checksum");
    fs::write(&staged, format!("{}  {name}\n", sha(source)?))?;
    let _checksum = fs::File::open(&staged)?;
    package::publish(source, &staged, destination, &destination.with_file_name(format!("{name}.sha256")))
        .context("Publication may be incomplete; output files were preserved for review. Use a new output directory before retrying.")
}
fn publish_all(sources: &[PathBuf], output: &Path) -> Result<()> {
    publish_all_with(sources, output, &publish_file)
}
fn publish_all_with(sources: &[PathBuf], output: &Path, publish: &dyn Fn(&Path, &Path) -> Result<()>) -> Result<()> {
    admit_destinations(sources, output)?;
    for source in sources {
        let destination = output.join(source.file_name().context("name")?);
        publish(source, &destination)
            .context("Artifact set publication failed; already created files remain in the selected output directory. No output was removed.")?;
    }
    Ok(())
}

fn which(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
}

pub fn main(args: &[String]) -> Result<()> {
    let (mut package_path, mut output, mut sign_profile, mut notary) = (None, None, None, None);
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        let value = iter.next().with_context(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--package" => package_path = Some(PathBuf::from(value)),
            "--output" => output = Some(PathBuf::from(value)),
            "--sign-profile" => sign_profile = Some(PathBuf::from(value)),
            "--notary-profile" => notary = Some(value.clone()),
            _ => bail!("usage: cargo xtask package-app --package <archive> --output <dir> [--sign-profile <json>] [--notary-profile <name>]"),
        }
    }
    let (package_path, output) = (package_path.context("--package is required")?, output.context("--output is required")?);
    let platform = if cfg!(target_os = "macos") { "macos" } else if cfg!(windows) { "windows" } else { "linux" };
    ensure!(notary.is_none() || platform == "macos" && sign_profile.is_some(), "Notarization requires an explicit macOS signing profile");
    let signing = sign_profile.as_deref().map(|p| sign::profile(p, platform)).transpose()?;
    if let Some(profile) = &notary {
        sign::text(profile, 256)?;
    }
    fs::create_dir_all(&output)?;
    let (mut source, metadata) = open_regular(&package_path)?;
    ensure!(metadata.len() <= MAX_ARCHIVE, "Portable package must be a bounded regular archive");
    let digest = sha(&package_path)?;
    let name = package_path.file_name().context("name")?.to_string_lossy().into_owned();
    let sidecar = String::from_utf8(regular_bytes(&package_path.with_file_name(format!("{name}.sha256")), 1024)?)?;
    ensure!(sidecar.trim() == format!("{digest}  {name}"), "Portable archive checksum mismatch");
    let temporary = TempDir::new_in(&output, ".native-package-")?;
    let work = temporary.path().canonicalize()?;
    let zip = package_path.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    let snapshot = work.join(if zip { "input.zip" } else { "input.tar.gz" });
    {
        let mut copy = fs::OpenOptions::new().write(true).create_new(true).open(&snapshot)?;
        let copied = std::io::copy(&mut (&mut source).take(metadata.len()), &mut copy)?;
        ensure!(copied == metadata.len(), "Portable package was truncated");
    }
    ensure!(sha(&snapshot)? == digest, "Portable package changed before authenticated parsing");
    let portable = work.join("portable");
    fs::create_dir(&portable)?;
    let mut info = extract(&snapshot, &portable)?.info;
    let base = format!("screenshots-{}-{}", info["version"].as_str().unwrap_or_default(), info["target"].as_str().unwrap_or_default());
    let epoch = info["source_date_epoch"].as_u64().unwrap_or(0);
    let result = match platform {
        "macos" => {
            let pkgbuild = signing.is_some() || which("pkgbuild");
            let mut planned = vec![work.join(format!("{base}.app.tar.gz"))];
            if pkgbuild {
                planned.push(work.join(format!("{base}{}", if signing.is_some() { ".signed.pkg" } else { ".pkg" })));
            }
            admit_destinations(&planned, &output)?;
            if signing.is_some() {
                sign::native_tool("pkgbuild")?;
            }
            let app = work.join("Screenshots.app");
            app_bundle(&portable, &app, &info)?;
            if let Some(signing) = &signing {
                info = sign::mac_app(&sign::Native, &app, &info, signing)?;
            }
            let archive_path = work.join(format!("{base}.app.tar.gz"));
            let executables = vec![app.join("Contents/MacOS/screenshots"), app.join("Contents/Resources/screenshots-install")];
            package::write_archive(&app, &work, &archive_path, false, epoch, &executables)?;
            let mut outputs = vec![archive_path];
            if pkgbuild {
                let mut pkg = work.join(format!("{base}.pkg"));
                let version_text = info["version"].as_str().unwrap_or_default().to_owned();
                let build: [&dyn AsRef<std::ffi::OsStr>; 9] =
                    [&"pkgbuild", &"--component", &app, &"--install-location", &"/Applications", &"--identifier", &ID, &"--version", &version_text];
                let mut full: Vec<&dyn AsRef<std::ffi::OsStr>> = build.to_vec();
                full.push(&pkg);
                sign::Tools::run(&sign::Native, &sign::args(&full), Duration::from_secs(120), false)?;
                if let Some(signing) = &signing {
                    let signed = work.join(format!("{base}.signed.pkg"));
                    sign::mac_package(&sign::Native, &pkg, &signed, signing, notary.as_deref())?;
                    pkg = signed;
                }
                outputs.push(pkg);
            }
            publish_all(&outputs, &output)?;
            serde_json::json!({"status": "packaged", "kind": "macOS app/pkg",
                "signing": if signing.is_some() { "native verified" } else { "unsigned" },
                "notarization": if notary.is_some() { "accepted and stapled" } else { "not requested" }})
        }
        "windows" => {
            let msix = work.join(format!("{base}.msix"));
            admit_destinations(std::slice::from_ref(&msix), &output)?;
            if let Some(signing) = &signing {
                info = sign::windows_payload(&sign::Native, &portable, &info, signing)?;
            }
            let assets = portable.join("Assets");
            fs::create_dir(&assets)?;
            for (file, side) in [("StoreLogo.png", 50), ("Square150.png", 150), ("Square44.png", 44)] {
                png(&assets.join(file), side)?;
            }
            let architecture = if info["target"].as_str().unwrap_or_default().starts_with("aarch64") { "arm64" } else { "x64" };
            let publisher = signing.as_ref().and_then(|s| s["publisher"].as_str()).unwrap_or("CN=Screenshots");
            fs::write(portable.join("AppxManifest.xml"), appx_manifest(info["version"].as_str().unwrap_or_default(), architecture, publisher)?)?;
            let tool = if signing.is_some() { PathBuf::from("makeappx.exe") } else { sdk_makeappx()? };
            sign::Tools::run(&sign::Native, &sign::args(&[&tool, &"pack", &"/d", &portable, &"/p", &msix, &"/o"]), Duration::from_secs(120), false)?;
            if let Some(signing) = &signing {
                sign::windows_package(&sign::Native, &msix, signing)?;
            }
            publish_all(std::slice::from_ref(&msix), &output)?;
            serde_json::json!({"status": "packaged", "kind": "MSIX",
                "signing": if signing.is_some() { "native verified" } else { "unsigned; production installation requires trusted signature" }})
        }
        _ => {
            let deb = work.join(format!("{base}.deb"));
            admit_destinations(std::slice::from_ref(&deb), &output)?;
            deb_package(&portable, &deb, &info)?;
            publish_all(std::slice::from_ref(&deb), &output)?;
            serde_json::json!({"status": "packaged", "kind": "Debian", "signing": "unsigned development package"})
        }
    };
    println!("{result}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manifests_and_assets_are_well_formed() {
        assert!(appx_manifest("1.2.3", "x64", "CN=A & B").unwrap().contains("Publisher=\"CN=A &amp; B\""));
        assert!(appx_manifest("1.2", "x64", "CN=A").is_err());
        assert!(info_plist("1.2.3").unwrap().contains("<string>1.2.3</string>"));
        let dir = TempDir::new("xtask-png-").unwrap();
        png(&dir.path().join("a.png"), 44).unwrap();
        assert!(fs::read(dir.path().join("a.png")).unwrap().starts_with(b"\x89PNG"));
    }

    fn sources(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let out = dir.join("out");
        fs::create_dir(&out).unwrap();
        let (a, b) = (dir.join("a"), dir.join("b"));
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        (out, a, b)
    }

    #[test]
    fn group_failure_retains_complete_published_members_and_refuses_foreign_names() {
        let dir = TempDir::new("xtask-publish-").unwrap();
        let (out, a, b) = sources(dir.path());
        let fail_second = |source: &Path, destination: &Path| -> Result<()> {
            if source == b { bail!("injected publish failure") } else { publish_file(source, destination) }
        };
        assert!(publish_all_with(&[a.clone(), b.clone()], &out, &fail_second).is_err());
        assert_eq!(fs::read(out.join("a")).unwrap(), b"a");
        assert!(out.join("a.sha256").is_file());
        // A later run never overwrites or removes what is already there.
        fs::remove_file(out.join("a")).unwrap();
        fs::write(out.join("a"), b"foreign").unwrap();
        assert!(publish_all(&[a, b], &out).is_err());
        assert_eq!(fs::read(out.join("a")).unwrap(), b"foreign");
    }

    #[test]
    fn rollback_preserves_foreign_replacements_of_published_names() {
        let dir = TempDir::new("xtask-publish-").unwrap();
        let (out, a, b) = sources(dir.path());
        let replace_then_fail = |source: &Path, destination: &Path| -> Result<()> {
            if source == b {
                for target in [out.join("a"), out.join("a.sha256")] {
                    fs::remove_file(&target)?;
                    fs::write(&target, b"foreign replacement")?;
                }
                bail!("injected");
            }
            publish_file(source, destination)
        };
        assert!(publish_all_with(&[a, b.clone()], &out, &replace_then_fail).is_err());
        assert_eq!(fs::read(out.join("a")).unwrap(), b"foreign replacement");
        assert_eq!(fs::read(out.join("a.sha256")).unwrap(), b"foreign replacement");
    }

    #[test]
    fn msix_manifest_is_utf8_lf_and_routes_file_activation() {
        let publisher = "CN=M\u{fc}ller & \u{6771}\u{4eac} \"Publisher\"";
        let manifest = appx_manifest("0.1.0", "x64", publisher).unwrap();
        assert!(!manifest.contains('\r'));
        assert!(manifest.contains("Publisher=\"CN=M\u{fc}ller &amp; \u{6771}\u{4eac} &quot;Publisher&quot;\""));
        assert!(manifest.contains("Executable=\"screenshots.exe\""));
        assert!(manifest.contains("Parameters=\"--open &quot;%1&quot;\""));
        assert_eq!(manifest.matches("<uap:FileType>").count(), 7);
        assert!(appx_manifest("bad", "x64", "CN=A").is_err());
        assert!(appx_manifest("0.1.0", "unsupported", "CN=A").is_err());
    }

    fn portable(root: &Path, binary: &[u8]) -> PathBuf {
        let portable = root.join("portable");
        fs::create_dir(&portable).unwrap();
        fs::write(portable.join("screenshots"), binary).unwrap();
        fs::write(portable.join("screenshots-install"), b"installer").unwrap();
        set_mode(&portable.join("screenshots-install"), 0o755).unwrap();
        fs::write(portable.join("LICENSE"), b"license retained").unwrap();
        fs::write(portable.join("BUILD-INFO.json"), b"build info").unwrap();
        portable
    }

    #[test]
    fn mac_bundle_has_document_types_and_exact_binary() {
        let dir = TempDir::new("xtask-bundle-").unwrap();
        let portable = portable(dir.path(), b"synthetic Mach-O");
        let app = dir.path().join("Screenshots.app");
        app_bundle(&portable, &app, &serde_json::json!({"version": "0.1.0"})).unwrap();
        assert_eq!(fs::read(app.join("Contents/MacOS/screenshots")).unwrap(), b"synthetic Mach-O");
        assert_eq!(fs::read(app.join("Contents/Resources/LICENSE")).unwrap(), b"license retained");
        assert_eq!(fs::read(app.join("Contents/Resources/screenshots-install")).unwrap(), b"installer");
        let plist = fs::read_to_string(app.join("Contents/Info.plist")).unwrap();
        for expected in ["<string>me.lejy.screenshots</string>", "<key>CFBundleExecutable</key>\n\t<string>screenshots</string>", "<string>Viewer</string>", "<string>public.png</string>"] {
            assert!(plist.contains(expected), "{expected}");
        }
        assert!(app_bundle(&portable, &app, &serde_json::json!({"version": "0.1.0"})).is_err());
    }

    /// Minimal tar walker for assertions: (name, mode, kind, link, body).
    #[cfg(unix)]
    fn tar_entries(gz: &[u8]) -> Vec<(String, u32, u8, String, Vec<u8>)> {
        let mut raw = Vec::new();
        flate2::read::GzDecoder::new(gz).read_to_end(&mut raw).unwrap();
        let (mut out, mut at, mut pax_path) = (Vec::new(), 0, None);
        let text = |b: &[u8]| String::from_utf8(b.iter().take_while(|c| **c != 0).copied().collect()).unwrap();
        let octal = |b: &[u8]| u64::from_str_radix(text(b).trim(), 8).unwrap_or(0);
        while at + 512 <= raw.len() && raw[at..at + 512].iter().any(|b| *b != 0) {
            let header = &raw[at..at + 512];
            let size = octal(&header[124..136]) as usize;
            let body = raw[at + 512..at + 512 + size].to_vec();
            let kind = header[156];
            if kind == b'x' {
                let record = String::from_utf8(body.clone()).unwrap();
                pax_path = record.split_once(" path=").map(|(_, v)| v.trim_end().to_owned());
            } else {
                let name = pax_path.take().unwrap_or_else(|| {
                    let prefix = text(&header[345..500]);
                    if prefix.is_empty() { text(&header[0..100]) } else { format!("{prefix}/{}", text(&header[0..100])) }
                });
                out.push((name, octal(&header[100..108]) as u32, kind, text(&header[157..257]), body));
            }
            at += 512 + size.div_ceil(512) * 512;
        }
        out
    }

    #[cfg(unix)]
    #[test]
    fn debian_ar_and_tar_members_have_real_package_layout() {
        let dir = TempDir::new("xtask-deb-").unwrap();
        let portable = portable(dir.path(), b"synthetic ELF");
        let package = dir.path().join("screenshots.deb");
        deb_package(&portable, &package, &serde_json::json!({"version": "0.1.0", "target": "x86_64-unknown-linux-gnu", "source_date_epoch": 17, "linux_abi": {"glibc": "2.39"}})).unwrap();
        let data = fs::read(&package).unwrap();
        assert!(data.starts_with(b"!<arch>\n"));
        let mut members = std::collections::BTreeMap::new();
        let mut at = 8;
        while at < data.len() {
            let header = &data[at..at + 60];
            assert_eq!(&header[58..], b"`\n");
            let name = std::str::from_utf8(&header[..16]).unwrap().trim().trim_end_matches('/').to_owned();
            let size: usize = std::str::from_utf8(&header[48..58]).unwrap().trim().parse().unwrap();
            members.insert(name, data[at + 60..at + 60 + size].to_vec());
            at += 60 + size + size % 2;
        }
        assert_eq!(members["debian-binary"], b"2.0\n");
        let control = tar_entries(&members["control.tar.gz"]);
        let control = String::from_utf8(control.iter().find(|e| e.0 == "control").unwrap().4.clone()).unwrap();
        for expected in ["libc6 (>= 2.39)", "libicu72 | libicu74 | libicu76 | libicu78", "libssl3t64 | libssl3", "Architecture: amd64"] {
            assert!(control.contains(expected), "{expected}");
        }
        let entries = tar_entries(&members["data.tar.gz"]);
        let find = |name: &str| entries.iter().find(|e| e.0 == name).unwrap_or_else(|| panic!("{name} missing"));
        let link = find("usr/bin/screenshots");
        assert_eq!((link.2, link.3.as_str()), (b'2', "../lib/screenshots/screenshots"));
        assert_eq!(find("usr/lib/screenshots/screenshots").4, b"synthetic ELF");
        assert_eq!(find("usr/lib/screenshots/screenshots-install").1, 0o755);
        assert_eq!(find("usr/lib/screenshots/BUILD-INFO.json").4, b"build info");
        assert_eq!(find("usr/share/doc/screenshots/LICENSE").4, b"license retained");
        assert!(String::from_utf8_lossy(&find("usr/share/applications/me.lejy.screenshots.desktop").4).contains("--open %f"));
    }

    #[test]
    fn published_checksum_names_the_artifact() {
        let dir = TempDir::new("xtask-publish-").unwrap();
        let (out, a, _) = sources(dir.path());
        publish_all(std::slice::from_ref(&a), &out).unwrap();
        assert_eq!(fs::read_to_string(out.join("a.sha256")).unwrap(), format!("{}  a\n", sha(&a).unwrap()));
    }
}
