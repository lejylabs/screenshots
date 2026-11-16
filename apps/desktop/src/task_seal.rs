//! Seal the actual immutable prepared graph; no persisted credentials or config payloads.
use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::atomic::AtomicBool;
use zeroize::Zeroizing;
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TaskSeal {
    version: u32,
    digest: [u8; 32],
}
pub(crate) struct PreparedScheduledTask(Task);
fn field(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

impl Task {
    fn config_digest(&mut self) -> Result<[u8; 32]> {
        let mut hash = Sha256::new();
        field(&mut hash, b"screenshots-prepared-task-v1");
        field(&mut hash, &self.configuration);
        if let Some(template) = &self.template {
            field(&mut hash, &template.config_digest()?);
        }
        if let Some(recipe) = &self.recipe {
            field(&mut hash, &recipe.config_digest()?);
        }
        match &self.source {
            Source::File { path } | Source::RawFile { path } => {
                if self.source_identity.is_none() {
                    self.source_identity = Some(app_core::FileIdentity::open(path)?.1);
                }
                field(&mut hash, self.source_identity.as_ref().unwrap().bytes());
            }
            _ => {}
        }
        for action in &self.actions {
            match action {
                Ready::Upload(app_core::ClassifiedUploadProfile::Native(value)) => {
                    let bytes = Zeroizing::new(serde_json::to_vec(value)?);
                    field(&mut hash, bytes.as_slice());
                }
                Ready::Upload(app_core::ClassifiedUploadProfile::ShareX(value))
                | Ready::Sharex(value, _) => field(&mut hash, &value.config_digest()?),
                Ready::ProviderUpload(value) => field(&mut hash, &value.config_digest()?),
                Ready::S3Upload(value) => {
                    let bytes = Zeroizing::new(serde_json::to_vec(
                        &serde_json::json!({"endpoint":value.endpoint,"region":value.region,"bucket":value.bucket,"key_prefix":value.key_prefix,"credential":value.credential,"public_base":value.public_base}),
                    )?);
                    field(&mut hash, bytes.as_slice());
                }
                Ready::Command(path, _, _) => {
                    if let Some(identity) = self.identities.get(path) {
                        field(&mut hash, identity.bytes());
                        continue;
                    }
                    #[cfg(target_os = "windows")]
                    let resolved = app_core::windows_child::resolved_program(
                        &std::process::Command::new(path),
                    )?;
                    #[cfg(not(target_os = "windows"))]
                    let resolved = path.clone();
                    let (_, identity) = app_core::FileIdentity::open(&resolved)?;
                    field(&mut hash, identity.bytes());
                    self.identities.insert(path.clone(), identity);
                }
                // Static source/action semantics (including unexpanded paths) are in configuration.
                Ready::Save(_, _)
                | Ready::Link(_, _)
                | Ready::Qr(_)
                | Ready::CopyImage
                | Ready::CopyLink => {}
            }
        }
        Ok(hash.finalize().into())
    }
}
impl TaskSeal {
    pub(crate) fn read(profile: &Path) -> Result<Self> {
        let mut prepared = Task::read(profile)?;
        Ok(Self {
            version: 1,
            digest: prepared.config_digest()?,
        })
    }
    pub(crate) fn into_bytes(self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(&self)?)
    }
}
impl PreparedScheduledTask {
    pub(crate) fn read(profile: &Path, seal: &[u8]) -> Result<Self> {
        ensure!(seal.len() <= 4096, "Task seal is oversized");
        let seal: TaskSeal =
            serde_json::from_slice(seal).context("Invalid task configuration seal")?;
        ensure!(seal.version == 1, "Unsupported task seal version");
        let mut prepared = Task::read(profile)?;
        ensure!(
            prepared.config_digest()? == seal.digest,
            "Scheduled Task or one of its references changed; explicit replacement is required"
        );
        Ok(Self(prepared))
    }
    pub(crate) fn execute(self, cancel: Arc<AtomicBool>) -> Result<()> {
        self.0.execute_with(None, Some(cancel), &mut |_| Ok(()))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("screenshots-task-seal-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        root
    }
    static NATIVE_CAPTURES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static NATIVE_FAIL: AtomicBool = AtomicBool::new(false);
    static LAST_FRAME: std::sync::Mutex<Option<std::sync::Weak<image::RgbaImage>>> =
        std::sync::Mutex::new(None);
    fn synthetic_native(
        call: app_core::NativeCaptureCall,
        check: &app_core::NativeCaptureCheck<'_>,
    ) -> std::result::Result<app_core::NativeCaptureReply, app_core::CaptureError> {
        use app_core::{CaptureRequest as Q, NativeCaptureCall as C, NativeCaptureReply as R};
        check()?;
        if matches!(call, C::Displays) {
            return Ok(R::Displays(vec![app_core::DisplayInfo {
                id: "fixture-display".into(),
                name: "private synthetic".into(),
                x: 0,
                y: 0,
                width: 2,
                height: 1,
                scale_factor: 1.0,
                rotation: 0.0,
                is_primary: true,
            }]));
        }
        if NATIVE_FAIL.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(app_core::CaptureError::Backend(
                "Synthetic topology changed".into(),
            ));
        }
        let window = match &call {
            C::Batch { requests, .. } => match &requests[0] {
                Q::Window(id) => Some(id.clone()),
                _ => None,
            },
            C::FocusedWindow { .. } => Some("fixture-window".into()),
            _ => None,
        };
        NATIVE_CAPTURES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let pixels = Arc::new(image::RgbaImage::from_pixel(
            2,
            1,
            image::Rgba([19, 23, 29, 255]),
        ));
        *LAST_FRAME.lock().unwrap() = Some(Arc::downgrade(&pixels));
        Ok(R::Frames(vec![CapturedImage {
            display_id: match &call {
                C::Portal { source, .. } => source.identifier().into(),
                _ => "fixture-display".into(),
            },
            window_id: window,
            width: 2,
            height: 1,
            png_path: PathBuf::new(),
            pixels,
            elapsed_ms: 0,
            source_format: app_core::PixelFormat::Rgba8UnormSrgb,
            display_geometry: None,
        }]))
    }
    #[test]
    fn native_schedule_sources_fixture() -> Result<()> {
        let Some(root) = std::env::var_os("SCREENSHOTS_SCHEDULE_SOURCE_FIXTURE") else {
            return Ok(());
        };
        let root = PathBuf::from(root);
        app_core::install_native_capture_executor(synthetic_native)?;
        let catalog = root.join("catalog.json");
        std::fs::write(
            &catalog,
            serde_json::to_vec(
                &serde_json::json!({"version":1,"templates":[{"name":"crop","monitor":{"kind":"connector","key":"fixture:monitor"},"geometry":{"width":2,"height":1,"scale_factor":1.0,"rotation":0.0},"x":0,"y":0,"width":2,"height":1}]}),
            )?,
        )?;
        let sources = vec![
            serde_json::json!({"type":"primary_display"}),
            serde_json::json!({"type":"active_display"}),
            serde_json::json!({"type":"active_window"}),
            serde_json::json!({"type":"display","id":"fixture-display"}),
            serde_json::json!({"type":"window","id":"fixture-window"}),
            serde_json::json!({"type":"region","display":"fixture-display","x":0,"y":0,"width":2,"height":1}),
            serde_json::json!({"type":"template","catalog":catalog,"name":"crop"}),
        ];
        #[cfg(target_os = "linux")]
        let sources = sources
            .into_iter()
            .chain([
                serde_json::json!({"type":"portal","source":"display"}),
                serde_json::json!({"type":"portal","source":"window","cursor":"include"}),
            ])
            .collect::<Vec<_>>();
        for (index, source) in sources.into_iter().enumerate() {
            let profile = root.join(format!("task-{index}.json"));
            std::fs::write(
                &profile,
                serde_json::to_vec(
                    &serde_json::json!({"version":1,"source":source,"actions":[{"type":"save","path":format!("source-{index}-{{uuid}}.png")}]}),
                )?,
            )?;
            let before = NATIVE_CAPTURES.load(std::sync::atomic::Ordering::SeqCst);
            let seal = TaskSeal::read(&profile)?.into_bytes()?;
            let task = PreparedScheduledTask::read(&profile, &seal)?;
            assert_eq!(
                NATIVE_CAPTURES.load(std::sync::atomic::Ordering::SeqCst),
                before,
                "Waiting/admission captured a native frame"
            );
            task.execute(Arc::new(AtomicBool::new(false)))?;
            assert_eq!(
                NATIVE_CAPTURES.load(std::sync::atomic::Ordering::SeqCst),
                before + 1
            );
            assert!(
                LAST_FRAME
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .upgrade()
                    .is_none(),
                "Task retained native pixels between occurrences"
            );
            let task = PreparedScheduledTask::read(&profile, &seal)?;
            NATIVE_FAIL.store(true, std::sync::atomic::Ordering::SeqCst);
            assert!(task.execute(Arc::new(AtomicBool::new(false))).is_err());
            NATIVE_FAIL.store(false, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(())
    }
    #[test]
    fn all_native_task_sources_admit_without_pixels_and_revalidate_at_execution() -> Result<()> {
        let root = root();
        let child = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "task::seal::tests::native_schedule_sources_fixture",
            ])
            .env("SCREENSHOTS_SCHEDULE_SOURCE_FIXTURE", &root)
            .env("XDG_SESSION_TYPE", "x11")
            .env("DISPLAY", ":schedule-private")
            .env_remove("WAYLAND_DISPLAY")
            .output()?;
        ensure!(
            child.status.success(),
            "Synthetic scheduling source fixture failed: {}",
            String::from_utf8_lossy(&child.stderr)
        );
        assert_eq!(
            std::fs::read_dir(&root)?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|v| v == "png"))
                .count(),
            if cfg!(target_os = "linux") { 9 } else { 7 }
        );
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
    #[test]
    fn seal_uses_loaded_graph_and_dynamic_outputs_change_without_changing_configuration()
    -> Result<()> {
        let root = root();
        let input = root.join("input.png");
        image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255])).save(&input)?;
        let profile = root.join("task.json");
        let output = root.join("{uuid}.png");
        std::fs::write(
            &profile,
            serde_json::to_vec(
                &serde_json::json!({"version":1,"source":{"type":"file","path":input},"actions":[{"type":"save","path":output}]}),
            )?,
        )?;
        let seal = TaskSeal::read(&profile)?.into_bytes()?;
        let a = PreparedScheduledTask::read(&profile, &seal)?;
        let b = PreparedScheduledTask::read(&profile, &seal)?;
        std::fs::write(&profile, b"replaced after preparation")?;
        a.execute(Arc::new(AtomicBool::new(false)))?;
        b.execute(Arc::new(AtomicBool::new(false)))?;
        assert_eq!(
            std::fs::read_dir(&root)?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|v| v == "png"))
                .count(),
            3
        );
        assert!(PreparedScheduledTask::read(&profile, &seal).is_err());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
    #[test]
    fn file_replacement_after_preparation_is_rejected_before_decode_and_save() -> Result<()> {
        let root = root();
        let input = root.join("input.png");
        let profile = root.join("task.json");
        let output = root.join("output.png");
        image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255])).save(&input)?;
        std::fs::write(
            &profile,
            serde_json::to_vec(
                &serde_json::json!({"version":1,"source":{"type":"file","path":input},"actions":[{"type":"save","path":output}]}),
            )?,
        )?;
        let seal = TaskSeal::read(&profile)?.into_bytes()?;
        let task = PreparedScheduledTask::read(&profile, &seal)?;
        std::fs::rename(&input, root.join("prior.png"))?;
        image::RgbaImage::from_pixel(1, 1, image::Rgba([99, 2, 3, 255])).save(&input)?;
        assert!(task.execute(Arc::new(AtomicBool::new(false))).is_err());
        assert!(!output.exists());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
    #[test]
    fn raw_replacement_after_preparation_never_reaches_private_http_listener() -> Result<()> {
        let root = root();
        let input = root.join("input.bin");
        std::fs::write(&input, b"original")?;
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let upload = root.join("upload.json");
        std::fs::write(
            &upload,
            serde_json::to_vec(
                &serde_json::json!({"url":format!("http://{}/upload",listener.local_addr()?),"allow_http":true}),
            )?,
        )?;
        let profile = root.join("task.json");
        std::fs::write(
            &profile,
            serde_json::to_vec(
                &serde_json::json!({"version":1,"source":{"type":"raw_file","path":input},"actions":[{"type":"upload","profile":upload}]}),
            )?,
        )?;
        let seal = TaskSeal::read(&profile)?.into_bytes()?;
        let task = PreparedScheduledTask::read(&profile, &seal)?;
        std::fs::rename(&input, root.join("prior.bin"))?;
        std::fs::write(&input, b"injected")?;
        let before = Instant::now();
        let error = task.execute(Arc::new(AtomicBool::new(false))).unwrap_err();
        ensure!(
            format!("{error:#}").contains("Prepared input/program changed"),
            "Unexpected raw refusal: {error:#}"
        );
        ensure!(
            before.elapsed() < Duration::from_secs(5),
            "Changed input reached network waiting"
        );
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
    #[test]
    fn command_replacement_after_preparation_cannot_run_its_marker() -> Result<()> {
        let root = root();
        let marker = root.join("marker");
        #[cfg(unix)]
        let program = root.join("command.sh");
        #[cfg(target_os = "windows")]
        let program = root.join("command.cmd");
        let body = |label: &str| {
            #[cfg(unix)]
            {
                format!("#!/bin/sh\nprintf '%s' {label} > '{}'\n", marker.display())
            }
            #[cfg(target_os = "windows")]
            {
                format!("@echo off\r\necho {label}>\"{}\"\r\n", marker.display())
            }
        };
        std::fs::write(&program, body("original"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))?;
        }
        let profile = root.join("task.json");
        std::fs::write(
            &profile,
            serde_json::to_vec(
                &serde_json::json!({"version":1,"source":{"type":"none"},"actions":[{"type":"command","program":program,"timeout_ms":1000}]}),
            )?,
        )?;
        let seal = TaskSeal::read(&profile)?.into_bytes()?;
        let task = PreparedScheduledTask::read(&profile, &seal)?;
        std::fs::rename(&program, root.join("prior-command"))?;
        std::fs::write(&program, body("injected"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))?;
        }
        assert!(task.execute(Arc::new(AtomicBool::new(false))).is_err());
        assert!(!marker.exists());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
    #[test]
    fn referenced_upload_replacement_is_rejected_before_effects() -> Result<()> {
        let root = root();
        let input = root.join("input.png");
        image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255])).save(&input)?;
        let profile = root.join("task.json");
        let upload = root.join("upload.json");
        std::fs::write(
            &upload,
            br#"{"url":"http://127.0.0.1:9/first","allow_http":true}"#,
        )?;
        std::fs::write(
            &profile,
            serde_json::to_vec(
                &serde_json::json!({"version":1,"source":{"type":"file","path":input},"actions":[{"type":"upload","profile":upload}]}),
            )?,
        )?;
        let seal = TaskSeal::read(&profile)?.into_bytes()?;
        std::fs::write(
            &upload,
            br#"{"url":"http://127.0.0.1:9/replaced","allow_http":true}"#,
        )?;
        assert!(PreparedScheduledTask::read(&profile, &seal).is_err());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}
