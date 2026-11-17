//! Explicit saved task profiles: one source and ordered, bounded output actions.
use anyhow::{Context, Result, bail, ensure};
use app_core::{CaptureBackend, CapturedImage, ImageExportFormat, UploadOutcome, XcapBackend};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Deserialize, Serialize)]
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

#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Source {
    None,
    RawFile {
        path: PathBuf,
    },
    Portal {
        source: app_core::PortalCaptureSource,
        #[serde(default)]
        cursor: app_core::PortalCaptureCursor,
    },
    ImageUrl {
        url: String,
        #[serde(default)]
        allow_http: bool,
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
    Template {
        catalog: PathBuf,
        name: String,
    },
}

#[derive(Deserialize, Serialize)]
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
    SharexUpload {
        profile: PathBuf,
        #[serde(default)]
        options: Option<PathBuf>,
        #[serde(default)]
        kind: Option<app_core::ShareXInputKind>,
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

#[path = "task_seal.rs"]
mod seal;
pub(crate) use seal::{PreparedScheduledTask, TaskSeal};

struct Task {
    configuration: [u8; 32],
    source_identity: Option<app_core::FileIdentity>,
    identities: std::collections::HashMap<PathBuf, app_core::FileIdentity>,
    source: Source,
    template: Option<app_core::PreparedRegionTemplate>,
    recipe: Option<crate::workflow::Recipe>,
    actions: Vec<Ready>,
    clipboard_hold_ms: u64,
}
struct SelectedTemplate {
    catalog: PathBuf,
    name: String,
    template: app_core::PreparedRegionTemplate,
}
pub(crate) struct PreparedTemplateTask {
    task: Task,
    bridge: Option<crate::task_clipboard::Client>,
}
impl PreparedTemplateTask {
    fn read(
        selected: SelectedTemplate,
        bridge: Option<crate::task_clipboard::Client>,
    ) -> Result<Self> {
        let profile = selected
            .template
            .profile()
            .context("Template has no assigned task profile")?
            .to_owned();
        let task = Task::read_selected(&profile, Some(selected))?;
        Ok(Self { task, bridge })
    }
    pub(crate) fn execute(self) -> Result<()> {
        self.task.execute(self.bridge)
    }
}
enum Ready {
    Save(PathBuf, u8),
    Upload(app_core::ClassifiedUploadProfile),
    Sharex(app_core::ShareXPrepared, app_core::ShareXInputKind),
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
        Self::read_selected(path, None)
    }
    fn read_selected(path: &Path, mut selected: Option<SelectedTemplate>) -> Result<Self> {
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
        let configuration = {
            use sha2::{Digest, Sha256};
            let bytes = zeroize::Zeroizing::new(serde_json::to_vec(&profile)?);
            Sha256::digest(bytes.as_slice()).into()
        };
        let base = path.parent().context("Task profile has no parent")?;
        ensure!(
            selected.is_none() || matches!(profile.source, Source::Template { .. }),
            "Capture template task must use a template source"
        );
        let mut template = None;
        match &mut profile.source {
            Source::ImageUrl { url, allow_http } => {
                app_core::ImageUrlSource::new(url.clone(), *allow_http)?;
            }
            Source::Portal { .. } => ensure!(
                cfg!(target_os = "linux"),
                "Portal still capture requires Linux"
            ),
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
            Source::Template { catalog, name } => {
                *catalog = resolve(base, catalog)?;
                if let Some(selected) = selected.take() {
                    ensure!(
                        *name == selected.name
                            && std::fs::canonicalize(&*catalog)? == selected.catalog,
                        "Capture template task source must reference this catalog and template"
                    );
                    *catalog = selected.catalog;
                    template = Some(selected.template);
                } else {
                    template = Some(app_core::PreparedRegionTemplate::read(catalog, name)?);
                }
            }
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
        let timestamp = jiff::Timestamp::now()
            .strftime("%Y%m%dT%H%M%S%3fZ")
            .to_string();
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
                    let profile = resolve(base, &profile)?;
                    let prepared = crate::sharex::prepare(
                        &profile,
                        if raw_source {
                            app_core::ShareXInputKind::File
                        } else {
                            app_core::ShareXInputKind::Image
                        },
                    )?;
                    has_link = match &prepared {
                        app_core::ClassifiedUploadProfile::Native(_) => true,
                        app_core::ClassifiedUploadProfile::ShareX(profile) => {
                            profile.may_return_link()
                        }
                    };
                    Ready::Upload(prepared)
                }
                Action::SharexUpload {
                    profile,
                    options,
                    kind,
                } => {
                    let kind = kind.unwrap_or(if has_image {
                        app_core::ShareXInputKind::Image
                    } else {
                        app_core::ShareXInputKind::File
                    });
                    ensure!(
                        raw_source
                            || (kind == app_core::ShareXInputKind::Image && has_image)
                            || (kind == app_core::ShareXInputKind::File && has_output),
                        "Task SXCU input requires an image source or an explicit local artifact"
                    );
                    let options = options.map(|p| resolve(base, &p)).transpose()?;
                    let prepared = app_core::ShareXPrepared::read(
                        &resolve(base, &profile)?,
                        kind,
                        options.as_deref(),
                    )?;
                    has_link = prepared.may_return_link();
                    Ready::Sharex(prepared, kind)
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
            configuration,
            source_identity: None,
            identities: Default::default(),
            source: profile.source,
            template,
            recipe,
            actions,
            clipboard_hold_ms: profile.clipboard_hold_ms,
        })
    }

    fn capture(&self, check: &impl Fn() -> Result<()>) -> Result<Option<CapturedImage>> {
        check()?;
        let native_check =
            || check().map_err(|error| app_core::CaptureError::Backend(error.to_string()));
        if matches!(self.source, Source::None | Source::RawFile { .. }) {
            return Ok(None);
        }
        #[cfg(target_os = "windows")]
        let _dpi = if matches!(&self.source, Source::File { .. } | Source::ImageUrl { .. }) {
            None
        } else {
            Some(crate::region_analysis::DpiContext::enter()?)
        };
        let backend = XcapBackend::new(PathBuf::new());
        Ok(Some(match &self.source {
            Source::None | Source::RawFile { .. } => unreachable!(),
            Source::ImageUrl { url, allow_http } => {
                app_core::ImageUrlSource::new(url.clone(), *allow_http)?
                    .capture_with_cancel(&native_check)?
            }
            Source::Portal { source, cursor } => {
                app_core::capture_portal_with_cancel(*source, *cursor, &native_check)?
            }
            Source::PrimaryDisplay => {
                let mut displays = backend
                    .list_displays_with_cancel(&native_check)?
                    .into_iter()
                    .filter(|d| d.is_primary);
                let display = displays.next().context("Primary display is unavailable")?;
                ensure!(
                    displays.next().is_none(),
                    "Primary display identity is ambiguous"
                );
                backend
                    .capture_batch_with_cancel(
                        vec![app_core::CaptureRequest::Display(display.id)],
                        &native_check,
                    )?
                    .pop()
                    .context("Primary capture returned no frame")?
            }
            Source::ActiveDisplay => backend.capture_focused_display_with_cancel(&native_check)?,
            Source::ActiveWindow => backend.capture_focused_window_with_cancel(&native_check)?,
            Source::Template { .. } => self
                .template
                .as_ref()
                .context("Task template was not prepared")?
                .capture_with_cancel(PathBuf::new(), check)?,
            Source::File { path } => {
                crate::cli::frame(if let Some(expected) = self.source_identity.as_ref() {
                    app_core::load_editor_image_with_identity_and_limit_with_cancel(
                        path,
                        expected,
                        app_core::MAX_CAPTURE_BYTES,
                        &|| check().is_err(),
                    )?
                } else {
                    app_core::load_editor_image_with_cancel(path, &|| check().is_err())?
                })
            }
            Source::Display { id } => backend
                .capture_batch_with_cancel(
                    vec![app_core::CaptureRequest::Display(id.clone())],
                    &native_check,
                )?
                .pop()
                .context("Display capture returned no frame")?,
            Source::Window { id } => backend
                .capture_batch_with_cancel(
                    vec![app_core::CaptureRequest::Window(id.clone())],
                    &native_check,
                )?
                .pop()
                .context("Window capture returned no frame")?,
            Source::Region {
                display,
                x,
                y,
                width,
                height,
            } => backend
                .capture_batch_with_cancel(
                    vec![app_core::CaptureRequest::Region {
                        display_id: display.clone(),
                        x: *x as i32,
                        y: *y as i32,
                        width: *width,
                        height: *height,
                    }],
                    &native_check,
                )?
                .pop()
                .context("Region capture returned no frame")?,
        }))
    }

    fn execute(self, bridge: Option<crate::task_clipboard::Client>) -> Result<()> {
        self.execute_with(bridge, None, &mut result_line)
    }
    fn execute_with(
        mut self,
        mut bridge: Option<crate::task_clipboard::Client>,
        external_cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
        receipt: &mut dyn FnMut(serde_json::Value) -> Result<()>,
    ) -> Result<()> {
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
        let cancel = if let Some(flag) = external_cancel {
            crate::record::unix::Cancellation::with_flag(flag)?
        } else {
            crate::record::unix::Cancellation::new()?
        };
        #[cfg(unix)]
        let upload_cancel = app_core::UploadCancellation::with_atomic_flag(cancel.flag());
        #[cfg(not(unix))]
        let task_flag =
            external_cancel.unwrap_or_else(|| Arc::new(std::sync::atomic::AtomicBool::new(false)));
        #[cfg(not(unix))]
        let upload_cancel = app_core::UploadCancellation::with_atomic_flag(task_flag.clone());
        let raw_upload_cancel = if matches!(&self.source, Source::RawFile { .. }) {
            if let Some(identity) = self.source_identity.as_ref() {
                upload_cancel.clone().with_source_identity(identity.clone())
            } else {
                upload_cancel.clone()
            }
        } else {
            upload_cancel.clone()
        };
        let task_deadline = Instant::now() + Duration::from_secs(5400);
        let check = || -> Result<()> {
            ensure!(!upload_cancel.is_canceled(), "Task canceled");
            ensure!(
                Instant::now() < task_deadline,
                "Task exceeded its duration budget"
            );
            Ok(())
        };
        let mut frame = self.capture(&|| {
            check()?;
            ensure!(
                Instant::now() < task_deadline,
                "Task exceeded its duration budget"
            );
            Ok(())
        })?;
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
                        check()?;
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
        let mut last_saved_image: Option<PathBuf> = None;
        let mut clipboard = app_core::ImageClipboard::default();
        let mut owns_clipboard = false;
        let total = self.actions.len();
        for (index, action) in self.actions.into_iter().enumerate() {
            check().with_context(||format!("Task stopped before action {} after {index} completed actions; previous outputs and uploads remain",index+1))?;
            let outcome = (|| -> Result<serde_json::Value> {
                Ok(match action {
                    Ready::Save(path, quality) => {
                        let frame = frame.as_ref().context("Save requires an image source")?;
                        let format = frame.save_image_as_with_cancel(&path, quality, app_core::ExportSize::Original, || upload_cancel.is_canceled() || Instant::now() >= task_deadline)?;
                        last_saved_image = Some(path.clone());
                        last_output = Some(path);
                        serde_json::json!({"action":"save","format":format.extension(),"width":frame.width,"height":frame.height})
                    }
                    Ready::Upload(destination) => {
                        let result = if let Some(path) = &raw_source { crate::sharex::upload_file(&destination,path,raw_upload_cancel.clone())? }
                            else if matches!(&destination,app_core::ClassifiedUploadProfile::ShareX(_)) && last_saved_image.is_some() {crate::sharex::upload_file(&destination,last_saved_image.as_deref().unwrap(),upload_cancel.clone())?}
                            else {crate::sharex::upload_png(&destination, frame.as_ref().context("Upload requires an image source")?.png_bytes_with_cancel(|| upload_cancel.is_canceled() || Instant::now() >= task_deadline)?, upload_cancel.clone())?};
                        task_confirm_upload(&result,index,total,|| {
                            #[cfg(unix)]
                            cancel.output_phase(true);
                        })?;
                        upload = task_upload_link(&result)?;
                        let mut value = result;
                        value.as_object_mut().context("Upload result must be an object")?.insert("action".into(),"upload".into());
                        value
                    }
                    Ready::Sharex(prepared,kind) => {
                        let result = if kind==app_core::ShareXInputKind::Image && raw_source.is_none() && last_saved_image.is_none() {
                            prepared.upload_png_bytes(frame.as_ref().context("SXCU requires finalized image pixels")?.png_bytes_with_cancel(||upload_cancel.is_canceled() || Instant::now()>=task_deadline)?,upload_cancel.clone())?
                        } else {
                            let path = if kind==app_core::ShareXInputKind::Image {last_saved_image.as_deref().or(raw_source.as_deref())} else {raw_source.as_deref().or(last_output.as_deref())}.context("SXCU requires a local artifact")?;
                            prepared.upload_file(path,if raw_source.is_some(){raw_upload_cancel.clone()}else{upload_cancel.clone()})?
                        };
                        let value = serde_json::to_value(result)?;
                        task_confirm_upload(&value,index,total,|| {
                            #[cfg(unix)]
                            cancel.output_phase(true);
                        })?;
                        upload = task_upload_link(&value)?;
                        serde_json::json!({"action":"sharex_upload","upload":value})
                    }
                    Ready::ProviderUpload(prepared) => {
                        let source=last_output.as_deref().or(raw_source.as_deref()).context("Provider upload requires a local artifact")?;
                        let result = prepared.upload_file_guarded(source, || {
                            check()?;
                            Ok(())
                        },if raw_source.as_deref()==Some(source){self.source_identity.as_ref()}else{None})?;
                        upload = result.get("url").and_then(serde_json::Value::as_str).map(|url| UploadOutcome{url:url.into(),thumbnail:None,deletion:None}.validate_links()).transpose()?;
                        serde_json::json!({"action":"provider_upload","upload":result})
                    }
                    Ready::S3Upload(prepared) => {
                        let result = crate::s3::upload_prepared_with_cancellation(&prepared,last_output.as_deref().or(raw_source.as_deref()).context("S3 upload requires a local artifact")?,if last_output.is_none() && raw_source.is_some(){raw_upload_cancel.clone()}else{upload_cancel.clone()})?;
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
                        let identity=self.identities.get(&program);
                        let mut command = std::process::Command::new(program);
                        command.env_remove(crate::task_clipboard::ENV);
                        for arg in args {
                            command.arg(expand_argument(&arg, upload.as_ref().map(|u|u.url.as_str()), last_output.as_deref(), input_file.as_deref())?);
                        }
                        let deadline = (Instant::now() + timeout).min(task_deadline);
                        #[cfg(unix)] {
                            crate::record::unix::cooperative_engine_guarded(command, &cancel, deadline,identity)?;
                        }
                        #[cfg(target_os = "windows")] { crate::record_windows::engine_with_identity(command, None, deadline, task_flag.clone(),identity)?; }
                        serde_json::json!({"action":"command"})
                    }
                })
            })().with_context(|| format!("Task action {} failed after {index} completed actions; outputs from this or earlier actions may remain and uploads may exist; no retry", index + 1))?;
            #[cfg(unix)]
            cancel.output_phase(true);
            receipt(serde_json::json!({"type":"action","index":index+1,"total":total,"result":outcome}))
                .with_context(|| format!("Task action {} completed but its result could not be written; previous outputs and remote files remain", index + 1))?;
            #[cfg(unix)]
            cancel.output_phase(false);
        }
        check().with_context(|| {
            format!(
                "Task stopped after {total} completed actions; previous outputs and uploads remain"
            )
        })?;
        drop(frame);
        #[cfg(unix)]
        cancel.output_phase(true);
        receipt(
            serde_json::json!({"type":"complete","actions":total,"clipboard_hold_ms":if owns_clipboard { self.clipboard_hold_ms } else { 0 }}),
        ).with_context(||format!("Task completed {total} actions but its completion result could not be written; previous outputs and uploads remain"))?;
        // A Linux owner must stay alive to serve its explicitly requested clipboard data.
        if owns_clipboard && self.clipboard_hold_ms != 0 {
            let until = Instant::now() + Duration::from_millis(self.clipboard_hold_ms);
            while Instant::now() < until {
                check()?;
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
    fn snapshot_capture(
        call: app_core::NativeCaptureCall,
        check: &app_core::NativeCaptureCheck<'_>,
        _context: app_core::NativeCaptureContext,
    ) -> std::result::Result<app_core::NativeCaptureReply, app_core::CaptureError> {
        check()?;
        let app_core::NativeCaptureCall::Template { template, options } = call else {
            return Err(app_core::CaptureError::Backend(
                "Unexpected snapshot fixture call".into(),
            ));
        };
        if template.width != 2
            || template.height != 1
            || options.delay_ms != 0
            || template.monitor
                != (app_core::CaptureMonitorBinding::Connector {
                    key: "fixture:original".into(),
                })
        {
            return Err(app_core::CaptureError::Backend(
                "A later catalog replaced the admitted snapshot".into(),
            ));
        }
        Ok(app_core::NativeCaptureReply::Frames(vec![CapturedImage {
            desktop_layout: None,
            display_id: "fixture".into(),
            window_id: None,
            width: 2,
            height: 1,
            png_path: PathBuf::new(),
            pixels: Arc::new(image::RgbaImage::from_pixel(
                2,
                1,
                image::Rgba([7, 11, 13, 255]),
            )),
            elapsed_ms: 0,
            source_format: app_core::PixelFormat::Rgba8UnormSrgb,
            display_geometry: None,
        }]))
    }
    #[test]
    fn template_snapshot_fixture() -> Result<()> {
        let Some(root) = std::env::var_os("SCREENSHOTS_TEMPLATE_SNAPSHOT_FIXTURE") else {
            return Ok(());
        };
        let root = PathBuf::from(root);
        let catalog = root.join("catalog.json");
        let profile = root.join("task.json");
        let original = root.join("original.png");
        let injected = root.join("injected.png");
        std::fs::write(
            &profile,
            serde_json::to_vec(
                &serde_json::json!({"version":1,"source":{"type":"template","catalog":catalog,"name":"crop"},"actions":[{"type":"save","path":original}]}),
            )?,
        )?;
        let initial = serde_json::json!({"version":1,"templates":[{"name":"crop","monitor":{"kind":"connector","key":"fixture:original"},
            "geometry":{"width":10,"height":8,"scale_factor":1.0,"rotation":0.0},"x":0,"y":0,"width":2,"height":1,"profile":profile}]});
        std::fs::write(&catalog, serde_json::to_vec(&initial)?)?;
        let selected = selected_template(&catalog, "crop")?;
        // Deterministic catalog replacement before Task reading must not replace this binding/options.
        let mut replacement = initial;
        replacement["defaults"] = serde_json::json!({"delay_ms":3600000});
        replacement["templates"][0]["width"] = 3.into();
        replacement["templates"][0]["monitor"]["key"] = "fixture:replacement".into();
        std::fs::write(&catalog, serde_json::to_vec(&replacement)?)?;
        let admitted = PreparedTemplateTask::read(selected, None)?;
        let source = root.join("replacement.png");
        image::RgbaImage::from_pixel(1, 1, image::Rgba([199, 201, 203, 255])).save(&source)?;
        // Deterministic source AND action replacement after validation must never execute.
        std::fs::write(
            &profile,
            serde_json::to_vec(
                &serde_json::json!({"version":1,"source":{"type":"file","path":source},"actions":[{"type":"save","path":injected}]}),
            )?,
        )?;
        app_core::install_native_capture_executor(snapshot_capture)?;
        admitted.execute()?;
        ensure!(!injected.exists(), "Replaced Task actions ran");
        let pixels = image::open(&original)?.into_rgba8();
        ensure!(
            pixels.dimensions() == (2, 1)
                && pixels.pixels().all(|pixel| pixel.0 == [7, 11, 13, 255]),
            "Admitted source pixels were replaced"
        );
        Ok(())
    }
    #[test]
    fn assigned_template_executes_one_validated_task_and_initial_catalog_snapshot() -> Result<()> {
        let root = std::env::temp_dir().join(format!(
            "screenshots-task-snapshot-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root)?;
        let mut child = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "task::tests::template_snapshot_fixture",
                "--nocapture",
            ])
            .env("SCREENSHOTS_TEMPLATE_SNAPSHOT_FIXTURE", &root)
            .env("DISPLAY", ":snapshot-private")
            .env("XDG_SESSION_TYPE", "x11")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove(crate::task_clipboard::ENV)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if child.try_wait()?.is_some() {
                break;
            }
            if Instant::now() >= until {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_dir_all(&root);
                bail!("Selected catalog was reread or snapshot fixture stalled");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let result = child.wait_with_output()?;
        let cleanup = std::fs::remove_dir_all(&root);
        ensure!(
            result.status.success(),
            "Task/catalog replacement fixture failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        cleanup?;
        Ok(())
    }
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

fn task_upload_link(value: &serde_json::Value) -> Result<Option<UploadOutcome>> {
    value
        .get("url")
        .and_then(serde_json::Value::as_str)
        .map(|url| {
            UploadOutcome {
                url: url.into(),
                thumbnail: value
                    .get("thumbnail")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
                deletion: value
                    .get("deletion")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
            }
            .validate_links()
        })
        .transpose()
}

fn task_confirm_upload(
    value: &serde_json::Value,
    index: usize,
    total: usize,
    before_output: impl FnOnce(),
) -> Result<()> {
    if value.get("success").and_then(serde_json::Value::as_bool) == Some(false) {
        before_output();
        result_line(
            serde_json::json!({"type":"action_failed","index":index+1,"total":total,"result":value}),
        )?;
    }
    crate::sharex::confirmed(value)
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

pub(crate) fn validate_template_profile(profile: &Path, catalog: &Path, name: &str) -> Result<()> {
    let selected = selected_template(catalog, name)?;
    Task::read_selected(profile, Some(selected)).map(|_| ())
}
fn selected_template(catalog: &Path, name: &str) -> Result<SelectedTemplate> {
    let catalog = std::fs::canonicalize(catalog)?;
    let template = app_core::PreparedRegionTemplate::read(&catalog, name)?;
    Ok(SelectedTemplate {
        catalog,
        name: name.into(),
        template,
    })
}
pub(crate) fn prepare_template_task(catalog: &Path, name: &str) -> Result<PreparedTemplateTask> {
    // Accept inherited clipboard handles before any later task command can run.
    let bridge = crate::task_clipboard::Client::from_env()?;
    let selected = selected_template(catalog, name)?;
    PreparedTemplateTask::read(selected, bridge)
}
