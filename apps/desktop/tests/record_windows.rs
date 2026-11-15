#[cfg(target_os = "windows")]
#[test]
fn invalid_windows_recording_never_starts_an_engine_or_creates_output() {
    let root = std::env::temp_dir().join(format!(
        "screenshots-record-windows-cli-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir(&root).unwrap();
    for mode in ["gdigrab", "test", "x11"] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_screenshots"))
            .arg("record")
            .arg(mode)
            .arg(root.join("absent.mkv"))
            .args([
                "--seconds",
                "0",
                "--fps",
                "2",
                "--width",
                "16",
                "--height",
                "12",
            ])
            .env("USERPROFILE", root.join("profile"))
            .env("APPDATA", root.join("appdata"))
            .env("LOCALAPPDATA", root.join("localappdata"))
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(std::fs::read_dir(&root).unwrap().next().is_none());
    }
    std::fs::remove_dir(root).unwrap();
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires separately installed Windows FFmpeg/ffprobe; synthetic pixels only"]
fn real_windows_engines_verify_synthetic_matroska_and_gif() {
    let root = std::env::temp_dir().join(format!(
        "screenshots-record-windows-native-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir(&root).unwrap();
    for extension in ["mkv", "gif"] {
        let destination = root.join(format!("synthetic.{extension}"));
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_screenshots"))
            .args(["record", "test"])
            .arg(&destination)
            .args([
                "--seconds",
                "1",
                "--fps",
                "2",
                "--width",
                "17",
                "--height",
                "13",
            ])
            .env("USERPROFILE", root.join("profile"))
            .env("APPDATA", root.join("appdata"))
            .env("LOCALAPPDATA", root.join("localappdata"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["frames"], 2);
        assert_eq!(result["width"], 17);
        assert_eq!(result["height"], 13);
        let bytes = std::fs::read(&destination).unwrap();
        assert!(bytes.len() <= 256 * 1024 * 1024);
        if extension == "mkv" {
            assert_eq!(&bytes[..4], &[0x1a, 0x45, 0xdf, 0xa3]);
            assert_eq!(result["codec"], "ffv1");
        } else {
            assert!(bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"));
            assert_eq!(result["codec"], "gif");
        }
        assert_eq!(
            std::fs::read_dir(&root).unwrap().count(),
            if extension == "mkv" { 1 } else { 2 }
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}
