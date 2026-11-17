use image::{Rgba, RgbaImage};
use serde_json::Value;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("screenshots-format-cli-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn run(&self, args: &[OsString]) -> Output {
        let preferences = args.first().is_some_and(|arg| arg == "formats");
        let configured_export = args.first().is_some_and(|arg| arg == "history-manage")
            && args.get(1).is_some_and(|arg| arg == "export")
            && args.get(4).is_some_and(|arg| arg == "configured");
        if preferences || configured_export {
            assert!(
                args.len() >= 3 && args[args.len() - 2] == "--config",
                "Preference commands require an explicit private --config on every platform"
            );
            let config = Path::new(&args[args.len() - 1]);
            assert!(
                config.is_absolute()
                    && config.starts_with(&self.0)
                    && !config
                        .components()
                        .any(|part| matches!(part, std::path::Component::ParentDir))
                    && config
                        .parent()
                        .unwrap()
                        .canonicalize()
                        .unwrap()
                        .starts_with(&self.0),
                "Preference commands must never resolve a user known folder"
            );
        }
        Command::new(env!("CARGO_BIN_EXE_screenshots"))
            .args(args)
            .current_dir(&self.0)
            .env("HOME", self.path("home"))
            .env("USERPROFILE", self.path("profile"))
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env("XDG_CACHE_HOME", self.path("cache"))
            .env("APPDATA", self.path("appdata"))
            .env("LOCALAPPDATA", self.path("localappdata"))
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .output()
            .unwrap()
    }
    fn success(&self, args: &[OsString]) -> Value {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn a(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
fn source(path: &Path, format: image::ImageFormat, opaque: bool) {
    let pixels = RgbaImage::from_fn(13, 7, |x, y| {
        Rgba([
            (x * 17) as u8,
            (y * 31) as u8,
            53,
            if opaque { 255 } else { (x * 19) as u8 },
        ])
    });
    if format == image::ImageFormat::Jpeg {
        image::DynamicImage::ImageRgba8(pixels)
            .to_rgb8()
            .save_with_format(path, format)
            .unwrap();
    } else {
        pixels.save_with_format(path, format).unwrap();
    }
}

#[test]
fn explicit_preferences_default_and_backup_roundtrip_never_initialize_desktop_services() {
    let f = Fixture::new();
    let config = f.path("explicit-formats.json");
    let backup = f.path("backup.json");
    let c = config.to_str().unwrap();
    let b = backup.to_str().unwrap();
    let initial = f.success(&a(&["formats", "show", "--config", c]));
    assert_eq!(initial["preferences"]["capture"]["format"], "png");
    assert_eq!(initial["preferences"]["library_export"]["mode"], "preserve");
    assert!(!config.exists());
    f.success(&a(&[
        "formats",
        "set-capture",
        r#"{"format":"jpeg","quality":73}"#,
        "--config",
        c,
    ]));
    f.success(&a(&[
        "formats",
        "set-library",
        r#"{"mode":"convert","encoding":{"format":"webp_lossless"}}"#,
        "--config",
        c,
    ]));
    let bytes = std::fs::read(&config).unwrap();
    f.success(&a(&["formats", "export", b, "--config", c]));
    assert!(
        !f.run(&a(&["formats", "export", b, "--config", c]))
            .status
            .success()
    );
    f.success(&a(&[
        "formats",
        "set-capture",
        r#"{"format":"png"}"#,
        "--config",
        c,
    ]));
    f.success(&a(&["formats", "import", b, "--config", c]));
    assert_eq!(std::fs::read(&config).unwrap(), bytes);
    for policy in [
        r#"{"format":"jpeg","quality":101}"#,
        r#"{"format":"webp_lossless","quality":90}"#,
        r#"{"format":"webp_lossy"}"#,
        r#"["png"]"#,
        r#"["jpeg",90]"#,
    ] {
        let output = f.run(&a(&["formats", "set-capture", policy, "--config", c]));
        assert!(!output.status.success() && output.stdout.is_empty());
        assert_eq!(std::fs::read(&config).unwrap(), bytes);
    }
    for policy in [
        r#"["preserve"]"#,
        r#"{"mode":"convert","encoding":["jpeg",90]}"#,
    ] {
        let output = f.run(&a(&["formats", "set-library", policy, "--config", c]));
        assert!(!output.status.success() && output.stdout.is_empty());
        assert_eq!(std::fs::read(&config).unwrap(), bytes);
    }
    for invalid in [
        r#"[1]"#,
        r#"[1,["jpeg",90],["preserve"]]"#,
        r#"{"version":1,"capture":["png"]}"#,
    ] {
        let array_backup = f.path("array-backup.json");
        std::fs::write(&array_backup, invalid).unwrap();
        for operation in ["validate", "import"] {
            let output = f.run(&a(&[
                "formats",
                operation,
                array_backup.to_str().unwrap(),
                "--config",
                c,
            ]));
            assert!(!output.status.success() && output.stdout.is_empty());
            assert_eq!(std::fs::read(&config).unwrap(), bytes);
        }
    }
    for name in [
        "home",
        "profile",
        "config",
        "data",
        "cache",
        "appdata",
        "localappdata",
    ] {
        assert!(!f.path(name).exists());
    }
}

#[test]
fn cli_import_export_conversion_and_sanitize_keep_true_kind_and_original_bytes() {
    let f = Fixture::new();
    let config = f.path("explicit-formats.json");
    let database = f.path("history.sqlite3");
    let mut sources = Vec::new();
    for (name, format, opaque) in [
        ("png", image::ImageFormat::Png, false),
        ("jpeg", image::ImageFormat::Jpeg, true),
        ("webp", image::ImageFormat::WebP, false),
    ] {
        let path = f.path(&format!("{name}.misleading"));
        source(&path, format, opaque);
        sources.push(path);
    }
    let lossy = f.path("lossy.webp");
    app_core::save_lossy_webp(
        &RgbaImage::from_pixel(13, 7, Rgba([13, 47, 101, 128])),
        &lossy,
        40,
        || false,
    )
    .unwrap();
    sources.push(lossy);
    let original = sources
        .iter()
        .map(|path| std::fs::read(path).unwrap())
        .collect::<Vec<_>>();
    let mut args = vec![
        "history-manage".into(),
        "import".into(),
        database.clone().into_os_string(),
    ];
    args.extend(sources.iter().map(|path| path.as_os_str().to_owned()));
    let imported = f.success(&args);
    assert_eq!(imported["added"], 4);
    assert_eq!(imported["failed"], 0);
    let connection = rusqlite::Connection::open(&database).unwrap();
    let mut query = connection
        .prepare("SELECT id,mime_type FROM captures ORDER BY id")
        .unwrap();
    let rows = query
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        rows.iter().map(|row| row.1.as_str()).collect::<Vec<_>>(),
        ["image/png", "image/jpeg", "image/webp", "image/webp"]
    );
    drop(query);
    drop(connection);
    for (name, policy) in [("original", "original"), ("clean", "strip-metadata")] {
        let directory = f.path(name);
        std::fs::create_dir(&directory).unwrap();
        let mut args = vec![
            "history-manage".into(),
            "export".into(),
            database.clone().into_os_string(),
            directory.into_os_string(),
            policy.into(),
        ];
        args.extend(rows.iter().map(|row| row.0.to_string().into()));
        let result = f.success(&args);
        assert_eq!(result["metadata_policy"], policy);
        assert!(result.get("library_export_policy").is_none());
        assert_eq!(result["exported"], 4);
        assert_eq!(result["failed"], 0);
        for (index, output) in result["outputs"].as_array().unwrap().iter().enumerate() {
            let path = Path::new(output["path"].as_str().unwrap());
            let bytes = std::fs::read(path).unwrap();
            if policy == "original" {
                assert_eq!(bytes, original[index]);
                assert_eq!(output["reencoded"], false);
            } else {
                assert_eq!(
                    image::guess_format(&bytes).unwrap(),
                    image::ImageFormat::Png
                );
                assert_eq!(path.extension().unwrap(), "png");
                assert_eq!(output["format"], "png");
                assert_eq!(output["lossy"], false);
                assert_eq!(
                    image::load_from_memory(&bytes).unwrap().into_rgba8(),
                    app_core::load_saved_image(&sources[index], || false).unwrap()
                );
            }
        }
    }
    for (name, policy, converted) in [
        ("configured-preserve", r#"{"mode":"preserve"}"#, false),
        (
            "configured-convert",
            r#"{"mode":"convert","encoding":{"format":"webp_lossless"}}"#,
            true,
        ),
    ] {
        f.success(&a(&[
            "formats",
            "set-library",
            policy,
            "--config",
            config.to_str().unwrap(),
        ]));
        let directory = f.path(name);
        std::fs::create_dir(&directory).unwrap();
        let mut args = vec![
            "history-manage".into(),
            "export".into(),
            database.as_os_str().to_owned(),
            directory.into_os_string(),
            "configured".into(),
        ];
        args.extend(rows.iter().map(|row| row.0.to_string().into()));
        args.extend(["--config".into(), config.as_os_str().to_owned()]);
        let result = f.success(&args);
        assert_eq!(result["metadata_policy"], "configured");
        assert_eq!(
            result["library_export_policy"],
            serde_json::from_str::<Value>(policy).unwrap()
        );
        assert_eq!(result["exported"], 4);
        for (index, output) in result["outputs"].as_array().unwrap().iter().enumerate() {
            assert_eq!(output["reencoded"], converted);
            assert_eq!(output["lossy"], false);
            if converted {
                assert_eq!(output["format"], "webp");
            } else {
                assert_eq!(
                    std::fs::read(output["path"].as_str().unwrap()).unwrap(),
                    original[index]
                );
            }
        }
    }
    for (source, bytes) in sources.iter().zip(original) {
        assert_eq!(std::fs::read(source).unwrap(), bytes);
    }
    for name in ["data", "cache", "localappdata"] {
        assert!(!f.path(name).exists());
    }
}
