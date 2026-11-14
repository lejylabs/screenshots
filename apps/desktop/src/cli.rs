use anyhow::{Context, Result, bail, ensure};
use app_core::{CaptureBackend, CapturedImage, ExportSize, PixelFormat, XcapBackend};
use serde_json::json;
use std::{ffi::OsString, path::Path, sync::Arc};

const HELP: &str = "screenshots — native capture and image tools

Desktop: screenshots [--show]
Headless commands (no desktop UI, clipboard writes or implicit uploads):
  screenshots --help | --version
  screenshots image convert INPUT OUTPUT [--width N] [--height N]
      [--rotate 90|180|270] [--flip-horizontal] [--flip-vertical]
      [--jpeg-quality 1..100]
  screenshots image compose INPUT OUTPUT [--padding N] [--background RRGGBB|RRGGBBAA]
      [--watermark FILE --at X Y]
  screenshots image resize INPUT OUTPUT WIDTH HEIGHT
  screenshots list displays|windows
  screenshots capture display|window ID OUTPUT
  screenshots capture region DISPLAY X Y WIDTH HEIGHT OUTPUT
  screenshots history DATABASE [--before ID] [--search WORDS]
  screenshots storage check DATABASE
  screenshots storage backup DATABASE NEW_DATABASE
  screenshots storage inspect-parts DIRECTORY
  screenshots upload PROFILE_JSON PNG
  screenshots credentials target|store|remove PROFILE_JSON [--username NAME]
  screenshots qr generate TEXT OUTPUT_PNG
  screenshots qr scan INPUT
  screenshots ocr INPUT [--language LANG]

Image input: PNG/JPEG/WebP, <=32 MiB encoded and <=256 MiB decoded.
Output: PNG/JPEG/lossless WebP; existing files are never replaced.
Convert resize does not upscale; image resize permits nearest-neighbour upscaling.
Convert width alone or height alone keeps the aspect ratio.
Capture reads desktop pixels only when explicitly requested; it saves one image
without history or clipboard output. Window capture is unavailable on Linux.
Upload is an explicit network action and prints the server-returned URL.
Native platform capabilities/permissions still apply.
Credential store reads a secret from piped stdin; secrets are never arguments.
The profile must already contain an HTTPS credential reference/authentication.
";

/// Return false only for the pre-existing desktop launch arguments.
pub fn run(args: Vec<OsString>) -> Result<bool> {
    let Some(command) = args.first().and_then(|value| value.to_str()) else {
        ensure!(args.is_empty(), "Command must be UTF-8; use --help");
        return Ok(false);
    };
    match command {
        "--show" | "--resource-probe" => return Ok(false),
        value if value.starts_with("--resource-probe=") || value.starts_with("--exit-after=") => {
            return Ok(false);
        }
        "--help" | "-h" => {
            ensure!(args.len() == 1, "--help takes no arguments");
            print!("{HELP}");
        }
        "--version" | "-V" => {
            ensure!(args.len() == 1, "--version takes no arguments");
            println!("screenshots {}", app_core::version());
        }
        "image"
            if args
                .get(1)
                .is_some_and(|arg| arg == "compose" || arg == "resize") =>
        {
            crate::image_tools::run(&args[1..])?
        }
        "image" => convert(&args[1..])?,
        "list" => list(&args[1..])?,
        "capture" => capture(&args[1..])?,
        "history" => history(&args[1..])?,
        "storage" => storage(&args[1..])?,
        "credentials" => credentials(&args[1..])?,
        "qr" => crate::qr::run(&args[1..])?,
        "ocr" => crate::ocr::run(&args[1..])?,
        "upload" => {
            ensure!(
                args.len() == 3,
                "Usage: screenshots upload PROFILE_JSON PNG"
            );
            let destination = app_core::UploadDestination::load(Path::new(&args[1]))?;
            let source = std::path::absolute(&args[2])?;
            let url = app_core::upload_png(
                &destination,
                &source,
                app_core::UploadCancellation::default(),
            )?;
            println!("{}", json!({"url": url}));
        }
        _ => bail!("Unknown command; use --help"),
    }
    Ok(true)
}

fn text(value: &OsString) -> Result<&str> {
    value.to_str().context("Option must be valid UTF-8")
}

fn number<T: std::str::FromStr>(value: &OsString) -> Result<T> {
    text(value)?
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid numeric argument"))
}

pub(crate) fn frame(pixels: image::RgbaImage) -> CapturedImage {
    CapturedImage {
        display_id: String::new(),
        window_id: None,
        width: pixels.width(),
        height: pixels.height(),
        png_path: Default::default(),
        pixels: Arc::new(pixels),
        elapsed_ms: 0,
        source_format: PixelFormat::Rgba8UnormSrgb,
        display_geometry: None,
    }
}

fn convert(args: &[OsString]) -> Result<()> {
    ensure!(
        args.len() >= 3 && text(&args[0])? == "convert",
        "Usage: screenshots image convert INPUT OUTPUT [options]"
    );
    let mut width = None;
    let mut height = None;
    let mut rotation = 0;
    let mut horizontal = false;
    let mut vertical = false;
    let mut quality = app_core::DEFAULT_JPEG_QUALITY;
    let mut seen = std::collections::HashSet::new();
    let mut index = 3;
    while index < args.len() {
        let option = text(&args[index])?;
        ensure!(seen.insert(option), "Duplicate option");
        match option {
            "--flip-horizontal" => horizontal = true,
            "--flip-vertical" => vertical = true,
            "--width" | "--height" | "--rotate" | "--jpeg-quality" => {
                index += 1;
                let value = args.get(index).context("Missing option value")?;
                match option {
                    "--width" => width = Some(number::<u32>(value)?),
                    "--height" => height = Some(number::<u32>(value)?),
                    "--rotate" => {
                        rotation = number::<u16>(value)?;
                        ensure!(
                            matches!(rotation, 90 | 180 | 270),
                            "Rotation must be 90, 180 or 270"
                        );
                    }
                    _ => {
                        quality = number::<u8>(value)?;
                        ensure!((1..=100).contains(&quality), "JPEG quality must be 1..100");
                    }
                }
            }
            _ => bail!("Unknown image option; use --help"),
        }
        index += 1;
    }
    let destination = Path::new(&args[2]);
    app_core::ImageExportFormat::from_path(destination)?;
    let size = match (width, height) {
        (Some(width), Some(height)) => ExportSize::Exact(width, height),
        (Some(width), None) => ExportSize::Width(width),
        (None, Some(height)) => ExportSize::Height(height),
        _ => ExportSize::Original,
    };
    size.validate()?;
    ensure!(!destination.try_exists()?, "Output already exists");
    let mut pixels = app_core::load_editor_image(Path::new(&args[1]))?;
    match rotation {
        90 => pixels = image::imageops::rotate90(&pixels),
        180 => image::imageops::rotate180_in_place(&mut pixels),
        270 => pixels = image::imageops::rotate270(&pixels),
        _ => {}
    }
    if horizontal {
        image::imageops::flip_horizontal_in_place(&mut pixels);
    }
    if vertical {
        image::imageops::flip_vertical_in_place(&mut pixels);
    }
    let image = frame(pixels);
    let dimensions = size.dimensions(image.width, image.height)?;
    let format = image.save_image_as(destination, quality, size)?;
    println!(
        "{}",
        json!({"format": format.extension(), "width": dimensions.0, "height": dimensions.1})
    );
    Ok(())
}

fn list(args: &[OsString]) -> Result<()> {
    ensure!(args.len() == 1, "Usage: screenshots list displays|windows");
    let backend = XcapBackend::new(std::path::PathBuf::new());
    let rows = match text(&args[0])? {
        "displays" => backend
            .list_displays()?
            .into_iter()
            .map(|d| {
                json!({
                    "id": d.id, "name": d.name, "x": d.x, "y": d.y,
                    "width": d.width, "height": d.height, "scale": d.scale_factor,
                    "primary": d.is_primary, "rotation": d.rotation
                })
            })
            .collect::<Vec<_>>(),
        "windows" => backend
            .list_windows()?
            .into_iter()
            .map(|w| {
                json!({
                    "id": w.id, "title": w.title
                })
            })
            .collect(),
        _ => bail!("Choose displays or windows"),
    };
    println!("{}", json!(rows));
    Ok(())
}

fn capture(args: &[OsString]) -> Result<()> {
    ensure!(
        args.len() >= 3,
        "Usage: screenshots capture display|window ID OUTPUT; or region DISPLAY X Y WIDTH HEIGHT OUTPUT"
    );
    let kind = text(&args[0])?;
    ensure!(
        matches!(
            (kind, args.len()),
            ("display" | "window", 3) | ("region", 7)
        ),
        "Invalid capture arguments; use --help"
    );
    let destination = Path::new(args.last().context("Missing output path")?);
    app_core::ImageExportFormat::from_path(destination)?;
    ensure!(!destination.try_exists()?, "Output already exists");
    let backend = XcapBackend::new(std::path::PathBuf::new());
    let id = text(&args[1])?;
    let image = match kind {
        "display" => backend.capture_display(id)?,
        "window" => backend.capture_window(id)?,
        _ => backend.capture_region(
            id,
            number(&args[2])?,
            number(&args[3])?,
            number(&args[4])?,
            number(&args[5])?,
        )?,
    };
    image.save_image_as(
        destination,
        app_core::DEFAULT_JPEG_QUALITY,
        ExportSize::Original,
    )?;
    println!("{}", json!({"width": image.width, "height": image.height}));
    Ok(())
}

fn history(args: &[OsString]) -> Result<()> {
    ensure!(
        !args.is_empty() && args.len() % 2 == 1,
        "Usage: screenshots history DATABASE [--before ID] [--search WORDS]"
    );
    let database = Path::new(&args[0]);
    ensure!(database.is_file(), "History database does not exist");
    app_core::check_history_database(database)?;
    let mut before = None;
    let mut search = None;
    for pair in args[1..].chunks_exact(2) {
        match text(&pair[0])? {
            "--before" if before.is_none() => {
                let value = number::<i64>(&pair[1])?;
                ensure!(value > 0, "History cursor must be positive");
                before = Some(value);
            }
            "--search" if search.is_none() => search = Some(text(&pair[1])?),
            _ => bail!("Unknown or duplicate history option"),
        }
    }
    let mut entries = if let Some(search) = search {
        app_core::search_capture_history(
            database,
            before,
            app_core::HistoryFilter::All,
            search,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )?
    } else {
        app_core::capture_history_page(database, before)?
    };
    let has_more = entries.len() > 20;
    entries.truncate(20);
    let cursor = entries.last().map(|entry| entry.id);
    println!(
        "{}",
        json!({"entries": entries.into_iter().map(|entry| json!({
        "id": entry.id, "path": entry.path, "width": entry.width,
        "height": entry.height, "created_at": entry.created_at
    })).collect::<Vec<_>>(), "has_more": has_more, "next_before": if has_more {cursor} else {None}})
    );
    Ok(())
}

fn storage(args: &[OsString]) -> Result<()> {
    ensure!(
        args.len() >= 2,
        "Usage: screenshots storage check|backup|inspect-parts PATH [NEW_DATABASE]"
    );
    match text(&args[0])? {
        "check" if args.len() == 2 => {
            let health = app_core::check_history_database(Path::new(&args[1]))?;
            println!(
                "{}",
                json!({"schema_version": health.schema_version, "quick_check": health.quick_check})
            );
        }
        "backup" if args.len() == 3 => {
            app_core::backup_history_database(Path::new(&args[1]), Path::new(&args[2]), || false)?;
            println!("{}", json!({"completed": true}));
        }
        "inspect-parts" if args.len() == 2 => {
            let inspection = app_core::inspect_part_files(Path::new(&args[1]), || false)?;
            let candidates = inspection
                .candidates
                .iter()
                .map(|path| path.to_string_lossy())
                .collect::<Vec<_>>();
            println!(
                "{}",
                json!({"candidates": candidates, "path_encoding": "lossy UTF-8 display", "inspected_entries": inspection.inspected_entries, "truncated": inspection.truncated,
                "ownership": "unknown; candidates may still be active; nothing was deleted"})
            );
        }
        _ => bail!("Invalid storage command; use --help"),
    }
    Ok(())
}

fn credentials(args: &[OsString]) -> Result<()> {
    use std::io::{IsTerminal, Read};
    ensure!(
        args.len() >= 2,
        "Usage: screenshots credentials target|store|remove PROFILE_JSON [--username NAME]"
    );
    let action = text(&args[0])?;
    let destination = app_core::UploadDestination::load(Path::new(&args[1]))?;
    let target = destination
        .credential_target()
        .context("Profile needs an HTTPS credential reference")?;
    match action {
        "target" if args.len() == 2 => println!("{}", json!({"target": target})),
        "remove" if args.len() == 2 => {
            destination.detach_credential()?;
            println!(
                "{}",
                json!({"removed": true, "profile_file_changed": false})
            );
        }
        "store" if args.len() == 2 || (args.len() == 4 && args[2] == "--username") => {
            ensure!(
                !std::io::stdin().is_terminal(),
                "Pipe the secret through stdin; interactive secret echo is disabled"
            );
            let mut bytes = zeroize::Zeroizing::new(Vec::new());
            std::io::stdin().lock().take(8193).read_to_end(&mut bytes)?;
            ensure!(bytes.len() <= 8192, "Credential input exceeds its limit");
            if bytes.last() == Some(&b'\n') {
                bytes.pop();
                if bytes.last() == Some(&b'\r') {
                    bytes.pop();
                }
            }
            let secret = std::str::from_utf8(&bytes).context("Credential input must be UTF-8")?;
            let reference = destination
                .credential_reference()
                .context("Missing credential reference")?
                .to_owned();
            let authentication = destination
                .authentication()
                .unwrap_or(app_core::UploadAuthentication::Bearer);
            let username = if args.len() == 4 {
                text(&args[3])?.to_owned()
            } else {
                String::new()
            };
            destination.attach_credential(
                reference,
                authentication,
                username,
                secret.to_owned(),
            )?;
            println!("{}", json!({"stored": true, "profile_file_changed": false}));
        }
        _ => bail!("Invalid credentials command; use --help"),
    }
    Ok(())
}
