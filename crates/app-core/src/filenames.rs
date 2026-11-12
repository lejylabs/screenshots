use anyhow::{Result, ensure};

pub fn validate_capture_name_template(template: &str) -> Result<()> {
    ensure!(
        template.len() <= 96,
        "The filename template must be at most 96 UTF-8 bytes"
    );
    let literal = template.replace("{date}", "").replace("{time}", "");
    ensure!(
        literal
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_')),
        "Use letters, digits, spaces, - or _, and {{date}}/{{time}} only; no paths or extension"
    );
    Ok(())
}

/// Explicit automatic save only; call on the output worker, not during capture/preview.
pub fn capture_file_name(template: &str) -> Result<Option<String>> {
    validate_capture_name_template(template)?;
    let template = template.trim();
    if template.is_empty() {
        return Ok(None);
    }
    file_name_at(template, chrono::Utc::now(), uuid::Uuid::new_v4()).map(Some)
}

fn file_name_at(
    template: &str,
    time: chrono::DateTime<chrono::Utc>,
    id: uuid::Uuid,
) -> Result<String> {
    let stem = template
        .replace("{date}", &time.format("%Y-%m-%d").to_string())
        .replace("{time}", &time.format("%H-%M-%S-%3f").to_string());
    let name = format!("{stem}-{id}.png");
    ensure!(name.len() <= 240, "The expanded filename is too long");
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_stay_single_component_bounded_unique_and_predictable() -> Result<()> {
        let time = chrono::DateTime::parse_from_rfc3339("2026-11-12T18:04:05.123Z")?.to_utc();
        assert_eq!(
            file_name_at("Shot-{date}_{time}", time, uuid::Uuid::nil())?,
            "Shot-2026-11-12_18-04-05-123-00000000-0000-0000-0000-000000000000.png"
        );
        assert!(capture_file_name("")?.is_none());
        for template in [
            "Screenshot",
            "Büro_{date}",
            "CON",
            "LPT1",
            &"{time}".repeat(16),
        ] {
            validate_capture_name_template(template)?;
            let first = capture_file_name(template)?.unwrap();
            assert_ne!(first, capture_file_name(template)?.unwrap());
            assert!(first.len() <= 240);
            assert_eq!(std::path::Path::new(&first).components().count(), 1);
            assert!(first.ends_with(".png"));
        }
        for template in [
            "../Shot",
            "a/b",
            "a\\b",
            "C:shot",
            "shot.png",
            "{unknown}",
            "{date",
            "bad\nname",
            "a*",
            &"x".repeat(97),
        ] {
            assert!(capture_file_name(template).is_err(), "{template:?}");
        }
        Ok(())
    }

    #[test]
    #[ignore = "short optimized filename metadata timing; no files, images or native capture"]
    fn filename_template_cost() -> Result<()> {
        for template in ["", "Shot-{date}_{time}"] {
            let mut samples = Vec::with_capacity(1000);
            for _ in 0..1000 {
                let started = std::time::Instant::now();
                std::hint::black_box(capture_file_name(template)?);
                samples.push(started.elapsed());
            }
            samples.sort_unstable();
            eprintln!(
                "Filename template enabled={}: p95={:?}; 1000 synthetic metadata calls; excludes filesystem/capture/presentation",
                !template.is_empty(),
                samples[949]
            );
        }
        Ok(())
    }
}
