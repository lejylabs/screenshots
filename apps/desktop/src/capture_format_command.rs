//! Explicit format preferences and credential-free backup. No desktop services.
use anyhow::{Context, Result, bail, ensure};
use app_core::{CaptureFormatPolicy, CaptureFormatSettings, LibraryExportPolicy};
use std::{ffi::OsString, path::PathBuf};

const USAGE: &str = "Usage: screenshots formats show | set-capture POLICY_JSON | set-library POLICY_JSON | validate FILE | export NEW.json | import FILE [--config FILE]";

fn path(value: &OsString) -> Result<PathBuf> {
    ensure!(
        !value.is_empty() && value.as_encoded_bytes().len() <= 4096,
        "Format settings path must contain 1–4096 bytes"
    );
    let path = std::path::absolute(PathBuf::from(value))?;
    ensure!(
        path.to_str().is_some(),
        "Format settings paths must be UTF-8 for the JSON report"
    );
    ensure!(
        !path.to_string_lossy().chars().any(char::is_control),
        "Format settings path contains controls"
    );
    Ok(path)
}

pub fn run(args: &[OsString]) -> Result<()> {
    ensure!(!args.is_empty() && args.len() <= 4, "{USAGE}");
    let mut values = args;
    let config = if args.len() >= 3 && args[args.len() - 2] == "--config" {
        values = &args[..args.len() - 2];
        path(&args[args.len() - 1])?
    } else {
        CaptureFormatSettings::config_path()?
    };
    let command = values.first().and_then(|v| v.to_str()).context(USAGE)?;
    let mut settings;
    let status;
    match command {
        "show" if values.len() == 1 => {
            settings = CaptureFormatSettings::load_at(&config)?;
            status = "inspected";
        }
        "set-capture" | "set-library" if values.len() == 2 => {
            let value = values[1].to_str().context("Policy JSON must be UTF-8")?;
            ensure!(value.len() <= 1024, "Policy JSON exceeds 1024 bytes");
            if command == "set-capture" {
                let policy: CaptureFormatPolicy = serde_json::from_str(value)?;
                settings = CaptureFormatSettings::load_at(&config)?;
                settings.capture = policy;
            } else {
                let policy: LibraryExportPolicy = serde_json::from_str(value)?;
                settings = CaptureFormatSettings::load_at(&config)?;
                settings.library_export = policy;
            }
            settings.save_at(&config)?;
            status = "saved";
        }
        "export" if values.len() == 2 => {
            settings = CaptureFormatSettings::load_at(&config)?;
            settings.export(&path(&values[1])?)?;
            status = "exported";
        }
        "import" if values.len() == 2 => {
            settings = CaptureFormatSettings::import(&path(&values[1])?)?;
            settings.save_at(&config)?;
            status = "imported";
        }
        "validate" if values.len() == 2 => {
            settings = CaptureFormatSettings::import(&path(&values[1])?)?;
            status = "validated";
        }
        _ => bail!(USAGE),
    }
    println!(
        "{}",
        serde_json::to_string(
            &serde_json::json!({"status":status,"config":config,"preferences":settings})
        )?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn private_preferences_roundtrip_and_rejections_never_touch_default_state() -> Result<()> {
        let root = std::env::temp_dir().join(format!(
            "screenshots-format-command-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root)?;
        let config = root.join("capture-format.json");
        let backup = root.join("backup.json");
        let config_text = config.to_str().unwrap();
        let backup_text = backup.to_str().unwrap();
        run(&args(&["show", "--config", config_text]))?;
        assert!(!config.exists());
        run(&args(&[
            "set-capture",
            r#"{"format":"jpeg","quality":73}"#,
            "--config",
            config_text,
        ]))?;
        run(&args(&[
            "set-library",
            r#"{"mode":"convert","encoding":{"format":"webp_lossless"}}"#,
            "--config",
            config_text,
        ]))?;
        let saved = std::fs::read(&config)?;
        run(&args(&["export", backup_text, "--config", config_text]))?;
        assert!(run(&args(&["export", backup_text, "--config", config_text])).is_err());
        run(&args(&[
            "set-capture",
            r#"{"format":"png"}"#,
            "--config",
            config_text,
        ]))?;
        run(&args(&["import", backup_text, "--config", config_text]))?;
        assert_eq!(std::fs::read(&config)?, saved);
        for value in [
            r#"{"format":"jpeg","quality":0}"#,
            r#"{"format":"webp_lossless","quality":90}"#,
            r#"{"format":"unknown"}"#,
        ] {
            assert!(run(&args(&["set-capture", value, "--config", config_text])).is_err());
            assert_eq!(std::fs::read(&config)?, saved);
        }
        assert!(
            run(&args(&[
                "import",
                root.join("missing").to_str().unwrap(),
                "--config",
                config_text
            ]))
            .is_err()
        );
        assert_eq!(std::fs::read(&config)?, saved);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}
