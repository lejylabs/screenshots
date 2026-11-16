//! Explicit capture catalogs; all edits publish a new file without replacing input.
use anyhow::{Context, Result, bail, ensure};
use app_core::{
    CaptureBackend, CaptureCatalog, CaptureMonitorBinding, CaptureOptions, CaptureRegionTemplate,
    CaptureTemplateGeometry, PixelRegion, PreparedRegionTemplate, XcapBackend,
};
use std::{
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
};

fn text(value: &OsString) -> Result<&str> {
    value
        .to_str()
        .context("Capture catalog arguments must be UTF-8")
}
fn new_output(path: &Path) -> Result<()> {
    ensure!(
        !path.as_os_str().is_empty() && path.as_os_str().as_encoded_bytes().len() <= 4096,
        "Output path must contain 1–4096 native bytes"
    );
    match path.symlink_metadata() {
        Ok(_) => bail!("Output already exists"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
fn options_json(value: &OsString) -> Result<CaptureOptions> {
    let value = text(value)?;
    ensure!(
        value.len() <= 4096 && value.trim_start().starts_with('{'),
        "Capture options must be one JSON object within 4096 bytes"
    );
    let options: CaptureOptions = serde_json::from_str(value).context("Invalid capture options")?;
    options.validate()?;
    Ok(options)
}
fn result_line(value: serde_json::Value, published: bool) -> Result<()> {
    let mut output = std::io::stdout().lock();
    writeln!(output, "{value}")
        .and_then(|_| output.flush())
        .context(if published {
            "Capture/catalog output was already published, but its result could not be written"
        } else {
            "Cannot write capture catalog result"
        })
}
fn bind(args: &[OsString], mut catalog: CaptureCatalog) -> Result<()> {
    ensure!(
        args.len() >= 7 && (args.len() - 7).is_multiple_of(2),
        "Usage: capture-catalog bind OUTPUT NAME DISPLAY X Y WIDTH HEIGHT [--connector true] [--profile FILE] [--hotkey CHORD] [--defaults JSON]; add INPUT OUTPUT NAME DISPLAY X Y WIDTH HEIGHT [OPTIONS]"
    );
    let output = Path::new(&args[0]);
    new_output(output)?;
    let name = text(&args[1])?.to_owned();
    ensure!(
        !name.is_empty()
            && name.len() <= 64
            && name.trim() == name
            && !name.chars().any(char::is_control),
        "Invalid template name"
    );
    ensure!(
        catalog.templates.len() < 16
            && catalog
                .templates
                .iter()
                .all(|template| template.name != name),
        "Template catalog is full or name already exists"
    );
    let display_id = text(&args[2])?;
    ensure!(
        !display_id.is_empty()
            && display_id.len() <= 64
            && !display_id.chars().any(char::is_control),
        "Invalid display ID"
    );
    let mut numbers = [0; 4];
    for (number, value) in numbers.iter_mut().zip(&args[3..7]) {
        let value = text(value)?;
        ensure!(
            !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()),
            "Region coordinates must be nonnegative whole numbers"
        );
        *number = value
            .parse::<u32>()
            .context("Region coordinate exceeds u32")?;
    }
    let area = PixelRegion {
        x: numbers[0],
        y: numbers[1],
        width: numbers[2],
        height: numbers[3],
    };
    app_core::validate_image_size(area.width, area.height)?;
    ensure!(
        area.x.checked_add(area.width).is_some() && area.y.checked_add(area.height).is_some(),
        "Region coordinate overflow"
    );
    let mut connector = None;
    let mut profile = None;
    let mut hotkey = None;
    let mut defaults = None;
    for pair in args[7..].chunks_exact(2) {
        match text(&pair[0])? {
            "--connector" => {
                ensure!(connector.is_none(), "Duplicate --connector option");
                connector = Some(match text(&pair[1])? {
                    "true" => true,
                    "false" => false,
                    _ => bail!("--connector requires true or false"),
                });
            }
            "--profile" => {
                ensure!(profile.is_none(), "Duplicate --profile option");
                let path = std::fs::canonicalize(Path::new(&pair[1]))
                    .context("Task profile is unavailable")?;
                ensure!(
                    path.as_os_str().as_encoded_bytes().len() <= 4096 && path.metadata()?.is_file(),
                    "Task profile must be a bounded regular-file path"
                );
                profile = Some(path);
            }
            "--hotkey" => {
                ensure!(hotkey.is_none(), "Duplicate --hotkey option");
                hotkey = Some(text(&pair[1])?.to_owned());
            }
            "--defaults" => {
                ensure!(defaults.is_none(), "Duplicate --defaults option");
                defaults = Some(options_json(&pair[1])?);
            }
            _ => bail!("Unknown capture-catalog bind option"),
        }
    }
    if let Some(defaults) = defaults {
        catalog.defaults = defaults;
    }
    // Validate all metadata before enumeration; binding itself never captures pixels.
    let placeholder = CaptureMonitorBinding::Connector {
        key: "preflight".into(),
    };
    catalog.templates.push(CaptureRegionTemplate {
        name: name.clone(),
        monitor: placeholder,
        geometry: CaptureTemplateGeometry {
            width: area.x.checked_add(area.width).unwrap(),
            height: area.y.checked_add(area.height).unwrap(),
            scale_factor: 1.0,
            rotation: 0.0,
        },
        x: area.x,
        y: area.y,
        width: area.width,
        height: area.height,
        profile,
        hotkey,
    });
    catalog.validate()?;
    let backend = XcapBackend::new(PathBuf::new());
    let displays = backend.list_displays()?;
    let mut selected = displays.iter().filter(|display| display.id == display_id);
    let display = selected.next().context("Selected display is unavailable")?;
    ensure!(
        selected.next().is_none(),
        "Selected display ID is ambiguous"
    );
    app_core::RememberedRegion::from_display(display, area)?;
    ensure!(
        display.scale_factor == 1.0 && display.rotation == 0.0,
        "Template capture currently requires unrotated 100% physical-pixel coordinates"
    );
    let identities = app_core::capture_monitor_identities(&displays)?;
    let mut matches = identities.into_iter().filter(|identity| {
        identity.display_id == display_id
            && match identity.binding {
                CaptureMonitorBinding::Connector { .. } => connector.unwrap_or(false),
                CaptureMonitorBinding::Physical { .. } => !connector.unwrap_or(false),
            }
    });
    let identity = matches.next().context("Physical monitor identity is unavailable; select --connector true explicitly to bind this connector instead")?;
    ensure!(
        matches.next().is_none(),
        "Selected monitor identity is ambiguous"
    );
    let template = catalog.templates.last_mut().unwrap();
    template.monitor = identity.binding;
    template.geometry = CaptureTemplateGeometry::from_display(display);
    catalog.write_new(output)?;
    result_line(
        serde_json::json!({"published":true,"templates":catalog.templates.len(),"template":name,"connector_binding":connector.unwrap_or(false)}),
        true,
    )
}

pub(crate) fn run(args: &[OsString]) -> Result<()> {
    let command = args.first().and_then(|value| value.to_str()).context("Usage: capture-catalog monitors|validate|bind|add|defaults|capture|host-config|run-task ...")?;
    let args = &args[1..];
    match command {
        "monitors" => {
            ensure!(args.is_empty(), "Usage: capture-catalog monitors");
            let displays = XcapBackend::new(PathBuf::new()).list_displays()?;
            let identities = app_core::capture_monitor_identities(&displays)?;
            let monitors = identities.into_iter().map(|identity| serde_json::json!({"display":identity.display_id,"binding":identity.binding})).collect::<Vec<_>>();
            result_line(serde_json::json!({"monitors":monitors}), false)
        }
        "validate" => {
            ensure!(args.len() == 1, "Usage: capture-catalog validate FILE");
            let catalog = CaptureCatalog::read(Path::new(&args[0]))?;
            for template in &catalog.templates {
                if let Some(profile) = &template.profile {
                    crate::task::validate_template_profile(
                        profile,
                        Path::new(&args[0]),
                        &template.name,
                    )?;
                }
            }
            result_line(
                serde_json::json!({"valid":true,"templates":catalog.templates.len(),"defaults":catalog.defaults}),
                false,
            )
        }
        "bind" => bind(
            args,
            CaptureCatalog {
                version: 1,
                ..Default::default()
            },
        ),
        "add" => {
            ensure!(
                !args.is_empty(),
                "Usage: capture-catalog add INPUT OUTPUT NAME DISPLAY X Y WIDTH HEIGHT [OPTIONS]"
            );
            let catalog = CaptureCatalog::read(Path::new(&args[0]))?;
            bind(&args[1..], catalog)
        }
        "defaults" => {
            ensure!(
                args.len() == 3,
                "Usage: capture-catalog defaults INPUT OUTPUT OPTIONS_JSON"
            );
            new_output(Path::new(&args[1]))?;
            let mut catalog = CaptureCatalog::read(Path::new(&args[0]))?;
            catalog.defaults = options_json(&args[2])?;
            catalog.write_new(Path::new(&args[1]))?;
            result_line(
                serde_json::json!({"published":true,"templates":catalog.templates.len()}),
                true,
            )
        }
        "capture" => {
            ensure!(
                args.len() == 3,
                "Usage: capture-catalog capture FILE NAME OUTPUT"
            );
            let output = Path::new(&args[2]);
            new_output(output)?;
            let format = app_core::ImageExportFormat::from_path(output)?;
            let prepared = PreparedRegionTemplate::read(Path::new(&args[0]), text(&args[1])?)?;
            let frame = prepared.capture_with_cancel(PathBuf::new(), &|| {
                new_output(output)?;
                Ok(())
            })?;
            frame.save_image_as(
                output,
                app_core::DEFAULT_JPEG_QUALITY,
                app_core::ExportSize::Original,
            )?;
            result_line(
                serde_json::json!({"published":true,"width":frame.width,"height":frame.height,"format":format.extension()}),
                true,
            )
        }
        "run-task" => {
            ensure!(args.len() == 2, "Usage: capture-catalog run-task FILE NAME");
            crate::task::prepare_template_task(Path::new(&args[0]), text(&args[1])?)?.execute()
        }
        "host-config" => {
            ensure!(
                args.len() == 2,
                "Usage: capture-catalog host-config FILE OUTPUT.json"
            );
            let output = Path::new(&args[1]);
            new_output(output)?;
            let catalog = CaptureCatalog::read(Path::new(&args[0]))?;
            let mut mappings = Vec::new();
            let mut bindings = Vec::new();
            for (index, template) in catalog.templates.iter().enumerate() {
                let Some(profile) = &template.profile else {
                    continue;
                };
                crate::task::validate_template_profile(
                    profile,
                    Path::new(&args[0]),
                    &template.name,
                )?;
                let id = format!("region-{:02}", index + 1);
                bindings.push(serde_json::json!({"id":id,"task":profile,"hotkey":template.hotkey.as_deref().unwrap_or("")}));
                mappings.push(serde_json::json!({"id":id,"template":template.name}));
            }
            ensure!(
                !bindings.is_empty(),
                "Catalog has no assigned task profiles"
            );
            let host =
                serde_json::json!({"version":1,"task_timeout_ms":5_400_000,"bindings":bindings});
            let bytes = serde_json::to_vec_pretty(&host)?;
            ensure!(bytes.len() <= 65536, "Generated host config exceeds 64 KiB");
            let parent = output
                .parent()
                .context("Host config output has no parent")?;
            let temporary = parent.join(format!("catalog-host-{}.part", uuid::Uuid::new_v4()));
            let result = (|| -> Result<()> {
                let mut options = std::fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut file = options.open(&temporary)?;
                file.write_all(&bytes)?;
                file.sync_all()?;
                drop(file);
                std::fs::hard_link(&temporary, output)
                    .context("Cannot publish host config without replacing an existing path")?;
                std::fs::remove_file(&temporary)
                    .context("Host config published, but staging cleanup failed")?;
                Ok(())
            })();
            if result.is_err() {
                let _ = std::fs::remove_file(&temporary);
            }
            result?;
            result_line(
                serde_json::json!({"published":true,"profiles":mappings}),
                true,
            )
        }
        _ => bail!("Unknown capture-catalog command"),
    }
}
