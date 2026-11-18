//! One explicit current clipboard snapshot per invocation; no admission-time reads.
use super::*;
use app_core::{
    ClipboardInspectKind, ClipboardPathEncoding, ClipboardSnapshot, NativeClipboardBackend,
};
use zeroize::Zeroizing;

fn file_path(snapshot: ClipboardSnapshot) -> Result<PathBuf> {
    snapshot.validate()?;
    let ClipboardSnapshot::Files { mut files } = snapshot else {
        bail!("Clipboard source did not return file references")
    };
    ensure!(
        files.len() == 1,
        "Clipboard file Task requires exactly one file reference"
    );
    let file = files.pop().unwrap();
    let path = match file.encoding {
        #[cfg(unix)]
        ClipboardPathEncoding::UnixBytes => {
            use std::os::unix::ffi::OsStringExt;
            PathBuf::from(OsString::from_vec(file.native))
        }
        #[cfg(target_os = "windows")]
        ClipboardPathEncoding::WindowsUtf16Le => {
            use std::os::windows::ffi::OsStringExt;
            let units = file
                .native
                .as_chunks::<2>()
                .0
                .iter()
                .map(|units| u16::from_le_bytes(*units))
                .collect::<Vec<_>>();
            PathBuf::from(OsString::from_wide(&units))
        }
        _ => bail!("Clipboard file encoding does not match this platform"),
    };
    ensure!(
        path.is_absolute(),
        "Clipboard file reference must be absolute"
    );
    ensure!(
        path.capacity() <= 8192,
        "Clipboard file path backing exceeds its bound"
    );
    Ok(path)
}

impl Task {
    pub(super) fn read_clipboard_source(
        &mut self,
        check: &impl Fn() -> Result<()>,
    ) -> Result<Option<Zeroizing<String>>> {
        let kind = match self.source {
            Source::ClipboardText { .. } => ClipboardInspectKind::Text,
            Source::ClipboardFile { .. } => ClipboardInspectKind::Files,
            _ => return Ok(None),
        };
        check()?;
        let native_check = || check().map_err(|_| app_core::CaptureError::Clipboard);
        let snapshot = app_core::inspect_clipboard_with_cancel(
            kind,
            NativeClipboardBackend::Automatic,
            &native_check,
        )
        .map_err(|_| anyhow::anyhow!("Clipboard Task input is unavailable or invalid"))?;
        check()?;
        if let Source::ClipboardText { .. } = self.source {
            let ClipboardSnapshot::Text { text } = snapshot else {
                bail!("Clipboard Task requires text")
            };
            return Ok(Some(Zeroizing::new(text)));
        }
        let path = file_path(snapshot).map_err(|_| {
            anyhow::anyhow!("Clipboard Task requires one valid absolute native file reference")
        })?;
        let (_, identity) = app_core::FileIdentity::open(&path).map_err(|_| {
            anyhow::anyhow!("Clipboard Task reference is not an accessible regular file")
        })?;
        check()?;
        self.source = match self.source {
            Source::ClipboardFile {
                kind: ClipboardFileKind::Image,
                ..
            } => Source::File { path },
            Source::ClipboardFile {
                kind: ClipboardFileKind::RawFile,
                ..
            } => Source::RawFile { path },
            _ => unreachable!(),
        };
        self.source_identity = Some(identity);
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    fn reference(bytes: Vec<u8>) -> app_core::ClipboardFileReference {
        app_core::ClipboardFileReference {
            display: String::from_utf8_lossy(&bytes).into_owned(),
            encoding: ClipboardPathEncoding::UnixBytes,
            native: bytes,
        }
    }
    #[test]
    #[cfg(unix)]
    fn authoritative_native_file_bytes_never_use_display_as_the_path() {
        use std::os::unix::ffi::OsStrExt;
        let native = b"/synthetic/\xff.png".to_vec();
        let value = reference(native.clone());
        assert_eq!(
            file_path(ClipboardSnapshot::Files { files: vec![value] })
                .unwrap()
                .as_os_str()
                .as_bytes(),
            native
        );
        let mut forged = reference(native);
        forged.display = "/synthetic/different.png".into();
        assert!(
            file_path(ClipboardSnapshot::Files {
                files: vec![forged]
            })
            .is_err()
        );
        assert!(file_path(ClipboardSnapshot::Files { files: vec![] }).is_err());
        assert!(
            file_path(ClipboardSnapshot::Files {
                files: vec![reference(b"/a".to_vec()), reference(b"/b".to_vec())]
            })
            .is_err()
        );
        assert!(
            file_path(ClipboardSnapshot::Files {
                files: vec![reference(b"relative".to_vec())]
            })
            .is_err()
        );
        assert!(
            file_path(ClipboardSnapshot::Files {
                files: vec![reference(vec![b'a'; 4097])]
            })
            .is_err()
        );
    }
    #[test]
    #[cfg(target_os = "windows")]
    fn authoritative_windows_path_preserves_unpaired_native_units() {
        use std::os::windows::ffi::OsStrExt;
        let units = [b'C' as u16, b':' as u16, b'\\' as u16, 0xd800];
        let value = app_core::ClipboardFileReference {
            display: String::from_utf16_lossy(&units),
            encoding: ClipboardPathEncoding::WindowsUtf16Le,
            native: units.into_iter().flat_map(u16::to_le_bytes).collect(),
        };
        let path = file_path(ClipboardSnapshot::Files { files: vec![value] }).unwrap();
        assert_eq!(path.as_os_str().encode_wide().collect::<Vec<_>>(), units);
    }
    #[test]
    fn private_source_consent_is_required_in_strict_profiles() {
        for source in [
            serde_json::json!({"type":"clipboard_image"}),
            serde_json::json!({"type":"clipboard_text"}),
            serde_json::json!({"type":"clipboard_file","kind":"image"}),
        ] {
            assert!(serde_json::from_value::<Source>(source).is_err());
        }
    }
    static FILE: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);
    static READS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    fn synthetic(
        call: app_core::NativeCaptureCall,
        check: &app_core::NativeCaptureCheck<'_>,
        _: app_core::NativeCaptureContext,
    ) -> std::result::Result<app_core::NativeCaptureReply, app_core::CaptureError> {
        check()?;
        READS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        match call {
            app_core::NativeCaptureCall::ClipboardImage { .. } => {
                let mut frame = crate::cli::frame(image::RgbaImage::from_pixel(
                    2,
                    1,
                    image::Rgba([17, 23, 29, 0]),
                ));
                frame.display_id = "clipboard:image".into();
                Ok(app_core::NativeCaptureReply::Frames(vec![frame]))
            }
            app_core::NativeCaptureCall::ClipboardInspect {
                kind: ClipboardInspectKind::Text,
                ..
            } => Ok(app_core::NativeCaptureReply::Clipboard(
                ClipboardSnapshot::Text {
                    text: "synthetic private text\n€".into(),
                },
            )),
            app_core::NativeCaptureCall::ClipboardInspect {
                kind: ClipboardInspectKind::Files,
                ..
            } => {
                let path = FILE
                    .lock()
                    .unwrap()
                    .clone()
                    .ok_or(app_core::CaptureError::Clipboard)?;
                #[cfg(unix)]
                let file = {
                    use std::os::unix::ffi::OsStrExt;
                    reference(path.as_os_str().as_bytes().to_vec())
                };
                #[cfg(target_os = "windows")]
                let file = {
                    use std::os::windows::ffi::OsStrExt;
                    app_core::ClipboardFileReference {
                        display: path.display().to_string(),
                        encoding: ClipboardPathEncoding::WindowsUtf16Le,
                        native: path
                            .as_os_str()
                            .encode_wide()
                            .flat_map(u16::to_le_bytes)
                            .collect(),
                    }
                };
                Ok(app_core::NativeCaptureReply::Clipboard(
                    ClipboardSnapshot::Files { files: vec![file] },
                ))
            }
            _ => Err(app_core::CaptureError::Clipboard),
        }
    }
    #[test]
    fn synthetic_current_clipboard_fixture() -> Result<()> {
        let Some(root) = std::env::var_os("SCREENSHOTS_TASK_CLIPBOARD_FIXTURE") else {
            return Ok(());
        };
        let root = PathBuf::from(root);
        app_core::install_native_capture_executor(synthetic)?;
        let profile = root.join("task.json");
        let image_profile = serde_json::json!({"version":1,"source":{"type":"clipboard_image","allow_private_source":true},"actions":[{"type":"save","path":"{uuid}.png"}]});
        std::fs::write(&profile, serde_json::to_vec(&image_profile)?)?;
        let seal = TaskSeal::read(&profile)?.into_bytes()?;
        let scheduled = PreparedScheduledTask::read(&profile, &seal)?;
        assert_eq!(READS.load(std::sync::atomic::Ordering::SeqCst), 0);
        scheduled.execute(Arc::new(std::sync::atomic::AtomicBool::new(false)))?;
        assert_eq!(READS.load(std::sync::atomic::Ordering::SeqCst), 1);
        let output = std::fs::read_dir(&root)?
            .filter_map(|entry| entry.ok())
            .find(|entry| entry.path().extension().is_some_and(|e| e == "png"))
            .unwrap()
            .path();
        assert_eq!(
            image::open(output)?.to_rgba8().into_raw(),
            [17, 23, 29, 0, 17, 23, 29, 0]
        );
        let canceled = PreparedScheduledTask::read(&profile, &seal)?;
        assert!(
            canceled
                .execute(Arc::new(std::sync::atomic::AtomicBool::new(true)))
                .is_err()
        );
        assert_eq!(READS.load(std::sync::atomic::Ordering::SeqCst), 1);
        for source in [
            serde_json::json!({"type":"clipboard_image","allow_private_source":false}),
            serde_json::json!({"type":"clipboard_text","allow_private_source":false}),
            serde_json::json!({"type":"clipboard_file","kind":"raw_file","allow_private_source":false}),
        ] {
            let mut value = image_profile.clone();
            value["source"] = source;
            std::fs::write(&profile, serde_json::to_vec(&value)?)?;
            assert!(Task::read(&profile).is_err());
        }
        let mut task = Task::read_selected(
            &{
                std::fs::write(&profile, serde_json::to_vec(&image_profile)?)?;
                profile.clone()
            },
            None,
        )?;
        task.source = Source::ClipboardText {
            allow_private_source: true,
        };
        let text = task.read_clipboard_source(&|| Ok(()))?.unwrap();
        assert_eq!(text.as_str(), "synthetic private text\n€");
        assert!(task.capture(&|| Ok(())).unwrap().is_none());
        assert_eq!(READS.load(std::sync::atomic::Ordering::SeqCst), 2);
        std::fs::write(&profile, serde_json::to_vec(&image_profile)?)?;
        assert!(PreparedScheduledTask::read(&profile, &seal).is_ok());
        let input = root.join("input.png");
        image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 4])).save(&input)?;
        *FILE.lock().unwrap() = Some(input.clone());
        let mut file_task = Task::read(&profile)?;
        file_task.source = Source::ClipboardFile {
            allow_private_source: true,
            kind: ClipboardFileKind::Image,
        };
        file_task.read_clipboard_source(&|| Ok(()))?;
        assert!(matches!(file_task.source, Source::File { .. }));
        assert!(file_task.source_identity.is_some());
        std::fs::rename(&input, root.join("original.png"))?;
        image::RgbaImage::from_pixel(1, 1, image::Rgba([9, 8, 7, 6])).save(&input)?;
        assert!(
            file_task.capture(&|| Ok(())).is_err(),
            "A replaced clipboard file bypassed identity admission"
        );
        let mut raw_task = Task::read(&profile)?;
        raw_task.source = Source::ClipboardFile {
            allow_private_source: true,
            kind: ClipboardFileKind::RawFile,
        };
        raw_task.read_clipboard_source(&|| Ok(()))?;
        assert!(matches!(raw_task.source, Source::RawFile { .. }));
        assert!(raw_task.source_identity.is_some());
        assert!(raw_task.capture(&|| Ok(())).unwrap().is_none());
        assert_eq!(READS.load(std::sync::atomic::Ordering::SeqCst), 4);
        let mut changed = image_profile;
        changed["source"]["allow_private_source"] = false.into();
        std::fs::write(&profile, serde_json::to_vec(&changed)?)?;
        assert!(PreparedScheduledTask::read(&profile, &seal).is_err());
        Ok(())
    }
    #[test]
    fn saved_tasks_read_only_at_execution_and_refuse_canceled_sources() -> Result<()> {
        let root = std::env::temp_dir().join(format!(
            "screenshots-task-clipboard-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root)?;
        let child = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "task::clipboard_input::tests::synthetic_current_clipboard_fixture",
            ])
            .env("SCREENSHOTS_TASK_CLIPBOARD_FIXTURE", &root)
            .env("XDG_SESSION_TYPE", "x11")
            .env("DISPLAY", ":clipboard-synthetic")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("WAYLAND_SOCKET")
            .output()?;
        std::fs::remove_dir_all(root)?;
        ensure!(
            child.status.success(),
            "Synthetic clipboard Task child failed: {}",
            String::from_utf8_lossy(&child.stderr)
        );
        Ok(())
    }

    #[test]
    fn private_text_task_upload_fixture() -> Result<()> {
        let Some(root) = std::env::var_os("SCREENSHOTS_TASK_CLIPBOARD_HTTPS_FIXTURE") else {
            return Ok(());
        };
        let root = PathBuf::from(root);
        app_core::install_native_capture_executor(synthetic)?;
        for mode in [
            "None",
            "MultipartFormData",
            "FormURLEncoded",
            "JSON",
            "XML",
            "Binary",
        ] {
            let profile = root.join(format!("task-{mode}.json"));
            let before = READS.load(std::sync::atomic::Ordering::SeqCst);
            let seal = TaskSeal::read(&profile)?.into_bytes()?;
            let _prepared = PreparedScheduledTask::read(&profile, &seal)?;
            let prepared = Task::read(&profile)?;
            let Ready::Sharex(text_upload, _) = &prepared.actions[0] else {
                panic!("expected SXCU")
            };
            assert!(
                text_upload
                    .upload_text(&"x".repeat(65537), app_core::UploadCancellation::default())
                    .is_err()
            );
            assert!(
                text_upload
                    .upload_text("nul\0text", app_core::UploadCancellation::default())
                    .is_err()
            );
            let canceled = app_core::UploadCancellation::with_atomic_flag(Arc::new(
                std::sync::atomic::AtomicBool::new(true),
            ));
            assert!(
                text_upload
                    .upload_text("synthetic refused", canceled)
                    .is_err()
            );
            assert_eq!(READS.load(std::sync::atomic::Ordering::SeqCst), before);
            let mut receipts = Vec::new();
            prepared.execute_with(None, None, &mut |value| {
                receipts.push(value);
                Ok(())
            })?;
            assert_eq!(READS.load(std::sync::atomic::Ordering::SeqCst), before + 1);
            let receipts = serde_json::to_string(&receipts)?;
            assert!(!receipts.contains("synthetic private text"));
            assert!(receipts.contains("https://example.invalid/public/"));
        }
        Ok(())
    }
}
