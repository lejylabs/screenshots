//! Explicit saved task profiles: one source and ordered, bounded output actions.
use anyhow::{Context, Result, bail, ensure};
use app_core::{CaptureBackend, CapturedImage, ImageExportFormat, UploadOutcome, XcapBackend};
use serde::Deserialize;
use std::{
    ffi::OsString,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    version: u32,
    source: Source,
    #[serde(default)]
    recipe: Option<PathBuf>,
    actions: Vec<Action>,
    #[serde(default)]
    clipboard_hold_ms: u64,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Source {
    None,
    RawFile {
        path: PathBuf,
    },
    PrimaryDisplay,
    ActiveDisplay,
    ActiveWindow,
    File {
        path: PathBuf,
    },
    Display {
        id: String,
    },
    Window {
        id: String,
    },
    Region {
        display: String,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Save {
        path: PathBuf,
        #[serde(default = "quality")]
        jpeg_quality: u8,
    },
    Upload {
        profile: PathBuf,
    },
    ProviderUpload {
        profile: PathBuf,
    },
    S3Upload {
        profile: PathBuf,
    },
    Link {
        format: String,
        #[serde(default = "label")]
        label: String,
    },
    Qr {
        path: PathBuf,
    },
    CopyImage,
    CopyLink,
    Command {
        program: PathBuf,
        #[serde(default)]
        args: Vec<String>,
        timeout_ms: u64,
    },
}
fn quality() -> u8 {
    app_core::DEFAULT_JPEG_QUALITY
}
fn label() -> String {
    "Screenshot".into()
}

struct Task {
    source: Source,
    recipe: Option<crate::workflow::Recipe>,
    actions: Vec<Ready>,
    clipboard_hold_ms: u64,
}
enum Ready {
    Save(PathBuf, u8),
    Upload(app_core::UploadDestination),
    ProviderUpload(crate::sharing_provider::PreparedUpload),
    S3Upload(app_core::S3Profile),
    Link(String, String),
    Qr(PathBuf),
    CopyImage,
    CopyLink,
    Command(PathBuf, Vec<String>, Duration),
}

fn resolve(base: &Path, path: &Path) -> Result<PathBuf> {
    ensure!(
        !path.as_os_str().is_empty() && path.as_os_str().as_encoded_bytes().len() <= 4096,
        "Task path must contain 1–4096 native bytes"
    );
    Ok(std::path::absolute(if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    })?)
}
fn output_path(
    base: &Path,
    path: &Path,
    outputs: &mut std::collections::HashSet<PathBuf>,
    run_id: &str,
    timestamp: &str,
) -> Result<PathBuf> {
    let expanded = path.to_str().map(|value| {
        PathBuf::from(
            value
                .replace("{uuid}", run_id)
                .replace("{timestamp}", timestamp),
        )
    });
    let path = resolve(base, expanded.as_deref().unwrap_or(path))?;
    match path.symlink_metadata() {
        Ok(_) => bail!("Task output already exists"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    ensure!(
        outputs.insert(path.clone()),
        "Task outputs must use distinct paths"
    );
    Ok(path)
}

impl Task {
    fn read(path: &Path) -> Result<Self> {
        let path = std::path::absolute(path)?;
        ensure!(
            path.metadata()?.is_file(),
            "Task profile must be a regular file"
        );
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = options.open(&path)?;
        let metadata = file.metadata()?;
        ensure!(
            metadata.is_file() && metadata.len() <= 65536,
            "Task profile must be a regular file within 64 KiB"
        );
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(65537).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= 65536
                && bytes.iter().copied().find(|b| !b.is_ascii_whitespace()) == Some(b'{'),
            "Task profile must be one JSON object within 64 KiB"
        );
        let mut profile: Profile =
            serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("Invalid task profile"))?;
        ensure!(
            profile.version == 1 && (1..=16).contains(&profile.actions.len()),
            "Task version must be 1 with 1–16 actions"
        );
        ensure!(
            profile.clipboard_hold_ms <= 300000,
            "Clipboard hold must be 0–300000 milliseconds"
        );
        let base = path.parent().context("Task profile has no parent")?;
        match &mut profile.source {
            Source::None
            | Source::PrimaryDisplay
            | Source::ActiveDisplay
            | Source::ActiveWindow => {}
            Source::File { path } | Source::RawFile { path } => {
                *path = resolve(base, path)?;
                ensure!(
                    path.metadata()?.is_file(),
                    "Task input must be a regular file"
                );
            }
            Source::Display { id } | Source::Window { id } => ensure!(
                !id.is_empty() && id.len() <= 64,
                "Task capture ID must contain 1–64 bytes"
            ),
            Source::Region {
                display,
                x,
                y,
                width,
                height,
            } => {
                ensure!(
                    !display.is_empty() && display.len() <= 64,
                    "Task display ID must contain 1–64 bytes"
                );
                app_core::validate_image_size(*width, *height)?;
                ensure!(
                    *x <= i32::MAX as u32
                        && *y <= i32::MAX as u32
                        && x.checked_add(*width).is_some()
                        && y.checked_add(*height).is_some(),
                    "Task region coordinates overflow"
                );
            }
        }
        let has_image = !matches!(profile.source, Source::None | Source::RawFile { .. });
        let raw_source = matches!(profile.source, Source::RawFile { .. });
        let has_input_file = matches!(profile.source, Source::File { .. } | Source::RawFile { .. });
        ensure!(
            profile.recipe.is_none() || has_image,
            "Image recipes require an image source"
        );
        let recipe = profile
            .recipe
            .map(|p| resolve(base, &p).and_then(|p| crate::workflow::Recipe::read(&p)))
            .transpose()?;
        let mut actions = Vec::with_capacity(profile.actions.len());
        let run_id = uuid::Uuid::new_v4().to_string();
        let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ").to_string();
        let mut outputs = std::collections::HashSet::new();
        let mut has_link = false;
        let mut has_output = false;
        for action in profile.actions {
            actions.push(match action {
                Action::Save { path, jpeg_quality } => {
                    ensure!(has_image, "Task image save requires an image source");
                    has_output = true;
                    let path = output_path(base, &path, &mut outputs, &run_id, &timestamp)?;
                    ImageExportFormat::from_path(&path)?;
                    ensure!(
                        (1..=100).contains(&jpeg_quality),
                        "Task JPEG quality must be 1–100"
                    );
                    Ready::Save(path, jpeg_quality)
                }
                Action::Upload { profile } => {
                    ensure!(
                        has_image || raw_source,
                        "Task upload requires an image or raw file source"
                    );
                    has_link = true;
                    let profile = resolve(base, &profile)?;
                    Ready::Upload(if raw_source {
                        crate::upload_file::load_profile(&profile)?
                    } else {
                        crate::cli::load_upload_profile(&profile)?
                    })
                }
                Action::ProviderUpload { profile } => {
                    ensure!(
                        has_output || raw_source,
                        "Provider upload requires an earlier saved artifact"
                    );
                    let prepared =
                        crate::sharing_provider::PreparedUpload::read(&resolve(base, &profile)?)?;
                    has_link = prepared.has_public_link();
                    Ready::ProviderUpload(prepared)
                }
                Action::S3Upload { profile } => {
                    ensure!(
                        has_output || raw_source,
                        "S3 upload requires an earlier saved artifact or raw file source"
                    );
                    let prepared = app_core::S3Profile::load(&resolve(base, &profile)?)?;
                    has_link = prepared.public_base.is_some();
                    Ready::S3Upload(prepared)
                }
                Action::Link { format, label } => {
                    ensure!(has_link, "A task link action requires an earlier upload");
                    crate::upload_results::format_link(
                        &format,
                        &label,
                        "https://example.invalid/",
                    )?;
                    ensure!(
                        matches!(format.as_str(), "plain" | "markdown" | "html")
                            && (1..=256).contains(&label.len())
                            && !label
                                .chars()
                                .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}')),
                        "Invalid task link format or label"
                    );
                    Ready::Link(format, label)
                }
                Action::Qr { path } => {
                    has_output = true;
                    ensure!(has_link, "A task QR action requires an earlier upload");
                    let path = output_path(base, &path, &mut outputs, &run_id, &timestamp)?;
                    ensure!(
                        ImageExportFormat::from_path(&path)? == ImageExportFormat::Png,
                        "Task QR output must be PNG"
                    );
                    Ready::Qr(path)
                }
                Action::CopyImage => {
                    ensure!(has_image, "Task image copy requires an image source");
                    Ready::CopyImage
                }
                Action::CopyLink => {
                    ensure!(has_link, "A task link copy requires an earlier upload");
                    Ready::CopyLink
                }
                Action::Command {
                    program,
                    args,
                    timeout_ms,
                } => {
                    let program = resolve(base, &program)?;
                    ensure!(
                        program.metadata()?.is_file(),
                        "Task command must name a regular executable file"
                    );
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        ensure!(
                            program.metadata()?.permissions().mode() & 0o111 != 0,
                            "Task command file is not executable"
                        );
                    }
                    ensure!(
                        (1..=300000).contains(&timeout_ms)
                            && args.len() <= 32
                            && args.iter().all(|v| v.len() <= 4096 && !v.contains('\0')),
                        "Task command exceeds argument/time limits"
                    );
                    ensure!(
                        has_link || !args.iter().any(|v| v.contains("{url}")),
                        "Task command URL requires an earlier upload"
                    );
                    ensure!(
                        has_output || !args.iter().any(|v| v.contains("{output}")),
                        "Task command output requires an earlier save or QR action"
                    );
                    ensure!(
                        has_input_file || !args.iter().any(|v| v.contains("{input}")),
                        "Task command input requires a file source"
                    );
                    Ready::Command(program, args, Duration::from_millis(timeout_ms))
                }
            });
        }
        Ok(Self {
            source: profile.source,
            recipe,
            actions,
            clipboard_hold_ms: profile.clipboard_hold_ms,
        })
    }

    fn capture(&self) -> Result<Option<CapturedImage>> {
        if matches!(self.source, Source::None | Source::RawFile { .. }) {
            return Ok(None);
        }
        #[cfg(target_os = "windows")]
        let _dpi = if matches!(&self.source, Source::File { .. }) {
            None
        } else {
            Some(crate::region_analysis::DpiContext::enter()?)
        };
        let backend = XcapBackend::new(PathBuf::new());
        Ok(Some(match &self.source {
            Source::None | Source::RawFile { .. } => unreachable!(),
            Source::PrimaryDisplay => {
                let mut displays = backend
                    .list_displays()?
                    .into_iter()
                    .filter(|d| d.is_primary);
                let display = displays.next().context("Primary display is unavailable")?;
                ensure!(
                    displays.next().is_none(),
                    "Primary display identity is ambiguous"
                );
                backend.capture_display(&display.id)?
            }
            Source::ActiveDisplay => backend.capture_focused_display()?,
            Source::ActiveWindow => backend.capture_focused_window()?,
            Source::File { path } => crate::cli::frame(app_core::load_editor_image(path)?),
            Source::Display { id } => backend.capture_display(id)?,
            Source::Window { id } => backend.capture_window(id)?,
            Source::Region {
                display,
                x,
                y,
                width,
                height,
            } => backend.capture_region(display, *x as i32, *y as i32, *width, *height)?,
        }))
    }

    fn execute(mut self, mut bridge: Option<crate::task_clipboard::Client>) -> Result<()> {
        ensure!(
            !cfg!(target_os = "linux")
                || bridge.is_some()
                || self.clipboard_hold_ms >= 1000
                || !self
                    .actions
                    .iter()
                    .any(|a| matches!(a, Ready::CopyImage | Ready::CopyLink)),
            "Standalone Linux clipboard tasks require clipboard_hold_ms of 1000–300000"
        );
        #[cfg(unix)]
        let cancel = crate::record::unix::Cancellation::new()?;
        #[cfg(unix)]
        let upload_cancel = app_core::UploadCancellation::with_atomic_flag(cancel.flag());
        #[cfg(not(unix))]
        let upload_cancel = app_core::UploadCancellation::default();
        let task_deadline = Instant::now() + Duration::from_secs(5400);
        let mut frame = self.capture()?;
        let raw_source = match &self.source {
            Source::RawFile { path } => Some(path.clone()),
            _ => None,
        };
        let input_file = match &self.source {
            Source::RawFile { path } | Source::File { path } => Some(path.clone()),
            _ => None,
        };
        if let Some(recipe) = self.recipe.take() {
            let image = frame.take().context("Recipe requires image pixels")?;
            frame = Some(crate::cli::frame(
                recipe.apply_with_cancel(
                    Arc::try_unwrap(image.pixels)
                        .map_err(|_| anyhow::anyhow!("Task source pixels are still shared"))?,
                    &|| {
                        #[cfg(unix)]
                        cancel.check(task_deadline)?;
                        ensure!(
                            Instant::now() < task_deadline,
                            "Task exceeded its duration budget"
                        );
                        Ok(())
                    },
                )?,
            ));
        }
        let mut upload: Option<UploadOutcome> = None;
        let mut last_output: Option<PathBuf> = None;
        let mut clipboard = app_core::ImageClipboard::default();
        let mut owns_clipboard = false;
        let total = self.actions.len();
        for (index, action) in self.actions.into_iter().enumerate() {
            #[cfg(unix)]
            cancel.check(task_deadline).with_context(|| format!("Task stopped before action {} of {total}; previous completed outputs and uploads remain", index+1))?;
            let outcome = (|| -> Result<serde_json::Value> {
                Ok(match action {
                    Ready::Save(path, quality) => {
                        let frame = frame.as_ref().context("Save requires an image source")?;
                        let format = frame.save_image_as_with_cancel(&path, quality, app_core::ExportSize::Original, || upload_cancel.is_canceled() || Instant::now() >= task_deadline)?;
                        last_output = Some(path);
                        serde_json::json!({"action":"save","format":format.extension(),"width":frame.width,"height":frame.height})
                    }
                    Ready::Upload(destination) => {
                        let result = if let Some(path) = &raw_source { app_core::upload_file(&destination,path,upload_cancel.clone())? }
                            else {app_core::upload_png_bytes_with_progress(&destination, frame.as_ref().context("Upload requires an image source")?.png_bytes()?, upload_cancel.clone(), None)?};
                        let result = result.validate_links()?;
                        let value = serde_json::json!({"action":"upload","url":result.url,"thumbnail":result.thumbnail,"deletion":result.deletion});
                        upload = Some(result);
                        value
                    }
                    Ready::ProviderUpload(prepared) => {
                        let result = prepared.upload_file_with_cancel(last_output.as_deref().or(raw_source.as_deref()).context("Provider upload requires a local artifact")?, || {
                            #[cfg(unix)]
                            cancel.check(task_deadline)?;
                            Ok(())
                        })?;
                        upload = result.get("url").and_then(serde_json::Value::as_str).map(|url| UploadOutcome{url:url.into(),thumbnail:None,deletion:None}.validate_links()).transpose()?;
                        serde_json::json!({"action":"provider_upload","upload":result})
                    }
                    Ready::S3Upload(prepared) => {
                        let result = crate::s3::upload_prepared_with_cancellation(&prepared,last_output.as_deref().or(raw_source.as_deref()).context("S3 upload requires a local artifact")?,upload_cancel.clone())?;
                        upload = result.get("url").and_then(serde_json::Value::as_str).map(|url| UploadOutcome{url:url.into(),thumbnail:None,deletion:None}.validate_links()).transpose()?;
                        serde_json::json!({"action":"s3_upload","upload":result})
                    }
                    Ready::Link(format, label) => serde_json::json!({"action":"link","format":format,"text":crate::upload_results::format_link(&format,&label,&upload.as_ref().context("Task upload returned no shareable link")?.url)?}),
                    Ready::Qr(path) => {
                        let pixels = crate::qr::generate_pixels(&upload.as_ref().context("Task upload returned no shareable link")?.url)?;
                        let (width, height) = pixels.dimensions();
                        crate::cli::frame(pixels).save_png_as(&path)?;
                        last_output = Some(path);
                        serde_json::json!({"action":"qr","width":width,"height":height})
                    }
                    Ready::CopyImage => {
                        let pixels=&frame.as_ref().context("Copy requires an image source")?.pixels;
                        if let Some(client)=bridge.as_mut() {client.copy_image(pixels)?;} else {clipboard.copy_image(pixels)?;owns_clipboard=true;}
                        serde_json::json!({"action":"copy_image"})
                    }
                    Ready::CopyLink => {
                        let text=&upload.as_ref().context("Task upload returned no shareable link")?.url;
                        if let Some(client)=bridge.as_mut() {client.copy_text(text)?;} else {clipboard.copy_text(text.clone())?;owns_clipboard=true;}
                        serde_json::json!({"action":"copy_link"})
                    }
                    Ready::Command(program, args, timeout) => {
                        let mut command = std::process::Command::new(program);
                        command.env_remove(crate::task_clipboard::ENV);
                        for arg in args {
                            command.arg(expand_argument(&arg, upload.as_ref().map(|u|u.url.as_str()), last_output.as_deref(), input_file.as_deref())?);
                        }
                        let deadline = (Instant::now() + timeout).min(task_deadline);
                        #[cfg(unix)] {
                            crate::record::unix::cooperative_engine(command, &cancel, deadline)?;
                        }
                        #[cfg(target_os = "windows")] { crate::record_windows::engine(command, None, deadline)?; }
                        serde_json::json!({"action":"command"})
                    }
                })
            })().with_context(|| format!("Task action {} failed after {index} completed actions; outputs from this or earlier actions may remain and uploads may exist; no retry", index + 1))?;
            #[cfg(unix)]
            cancel.output_phase(true);
            result_line(serde_json::json!({"type":"action","index":index+1,"total":total,"result":outcome}))
                .with_context(|| format!("Task action {} completed but its result could not be written; previous outputs and remote files remain", index + 1))?;
            #[cfg(unix)]
            cancel.output_phase(false);
        }
        #[cfg(unix)]
        cancel.check(task_deadline).with_context(|| {
            format!(
                "Task stopped after {total} completed actions; previous outputs and uploads remain"
            )
        })?;
        drop(frame);
        #[cfg(unix)]
        cancel.output_phase(true);
        result_line(
            serde_json::json!({"type":"complete","actions":total,"clipboard_hold_ms":if owns_clipboard { self.clipboard_hold_ms } else { 0 }}),
        ).with_context(||format!("Task completed {total} actions but its completion result could not be written; previous outputs and uploads remain"))?;
        // A Linux owner must stay alive to serve its explicitly requested clipboard data.
        if owns_clipboard && self.clipboard_hold_ms != 0 {
            let until = Instant::now() + Duration::from_millis(self.clipboard_hold_ms);
            while Instant::now() < until {
                #[cfg(unix)]
                cancel.check(task_deadline)?;
                std::thread::sleep(
                    Duration::from_millis(20).min(until.saturating_duration_since(Instant::now())),
                );
            }
        }
        Ok(())
    }
}

fn expand_argument(
    template: &str,
    url: Option<&str>,
    output: Option<&Path>,
    input: Option<&Path>,
) -> Result<String> {
    let mut result = String::with_capacity(template.len());
    let mut rest = template;
    while !rest.is_empty() {
        let next = [("{url}", 0), ("{output}", 1), ("{input}", 2)]
            .into_iter()
            .filter_map(|(token, kind)| rest.find(token).map(|at| (at, token.len(), kind)))
            .min_by_key(|v| v.0);
        let Some((at, length, kind)) = next else {
            result.push_str(rest);
            break;
        };
        result.push_str(&rest[..at]);
        let replacement = if kind == 0 {
            url.context("Task command requires an earlier upload")?
        } else {
            (if kind == 1 { output } else { input })
                .context("Task command requires its input or output artifact")?
                .to_str()
                .context("Task command output path must be UTF-8")?
        };
        ensure!(
            result.len() + replacement.len() <= 4096,
            "Expanded task argument exceeds 4096 bytes"
        );
        result.push_str(replacement);
        rest = &rest[at + length..];
    }
    ensure!(
        result.len() <= 4096,
        "Expanded task argument exceeds 4096 bytes"
    );
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_arguments_expand_once_without_interpreting_replacement_text() {
        let url = "https://example.invalid/?token={output}";
        assert_eq!(
            expand_argument("{url}", Some(url), None, None).unwrap(),
            url
        );
        assert_eq!(
            expand_argument(
                "{url}:{output}",
                Some(url),
                Some(Path::new("/tmp/{url}")),
                None
            )
            .unwrap(),
            format!("{url}:/tmp/{{url}}")
        );
    }
}

fn result_line(value: serde_json::Value) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{value}")
        .and_then(|_| stdout.flush())
        .context("Could not write task result")
}

pub(crate) fn run(args: &[OsString]) -> Result<()> {
    ensure!(
        args.len() == 2 && matches!(args[0].to_str(), Some("run" | "validate")),
        "Usage: screenshots task run|validate PROFILE.json"
    );
    let bridge = crate::task_clipboard::Client::from_env()?;
    let task = Task::read(Path::new(&args[1]))?;
    if args[0] == "validate" {
        result_line(serde_json::json!({"valid":true,"version":1,"actions":task.actions.len()}))
    } else {
        task.execute(bridge)
    }
}

pub(crate) fn validate_profile(path: &Path) -> Result<()> {
    Task::read(path).map(|_| ())
}
