//! Explicit, bounded processing of a single upload result; no network or clipboard work.

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use std::{
    ffi::OsString,
    io::{IsTerminal, Read, Write},
    path::Path,
};

const USAGE: &str = "Usage: screenshots link format plain|markdown|html [--label TEXT]; screenshots link qr OUTPUT.png (upload-result JSON on stdin)";
const MAX_TEXT: usize = 16 * 1024;
const MAX_JSON: usize = 32 * 1024;

enum Action<'a> {
    Format { format: &'a str, label: &'a str },
    Qr(&'a Path),
}

fn action(args: &[OsString]) -> Result<Action<'_>> {
    match args.first().and_then(|value| value.to_str()) {
        Some("format") if matches!(args.len(), 2 | 4) => {
            let format = args[1].to_str().context(USAGE)?;
            ensure!(matches!(format, "plain" | "markdown" | "html"), USAGE);
            let label = if args.len() == 4 {
                ensure!(format != "plain" && args[2] == "--label", USAGE);
                args[3].to_str().context("Link label must be UTF-8")?
            } else {
                "Screenshot"
            };
            ensure!(
                !label.is_empty()
                    && label.len() <= 256
                    && !label
                        .chars()
                        .any(|ch| ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}')),
                "Link label must contain 1–256 UTF-8 bytes without controls"
            );
            Ok(Action::Format { format, label })
        }
        Some("qr") if args.len() == 2 => {
            ensure!(
                args[1].as_encoded_bytes().len() <= 4096,
                "QR output path exceeds 4096 bytes"
            );
            let path = Path::new(&args[1]);
            ensure!(
                path.extension()
                    .is_some_and(|value| value.eq_ignore_ascii_case("png")),
                "QR output must be PNG"
            );
            match path.symlink_metadata() {
                Ok(_) => bail!("QR output already exists"),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => bail!("Could not inspect QR output"),
            }
            Ok(Action::Qr(path))
        }
        _ => bail!(USAGE),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    url: String,
    thumbnail: Option<String>,
    deletion: Option<String>,
}

fn read_result() -> Result<app_core::UploadOutcome> {
    ensure!(
        !std::io::stdin().is_terminal(),
        "Supply upload-result JSON on nonterminal stdin"
    );
    let mut bytes = Vec::with_capacity(8193);
    std::io::stdin()
        .lock()
        .take(8193)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow::anyhow!("Could not read upload-result JSON"))?;
    ensure!(
        !bytes.is_empty() && bytes.len() <= 8192,
        "Upload-result JSON must contain 1–8192 bytes"
    );
    ensure!(
        bytes
            .iter()
            .copied()
            .find(|byte| !matches!(byte, b' ' | b'\n' | b'\r' | b'\t'))
            == Some(b'{'),
        "Upload-result JSON must be an object"
    );
    let input: Input = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("Invalid upload-result JSON"))?;
    app_core::UploadOutcome {
        url: input.url,
        thumbnail: input.thumbnail,
        deletion: input.deletion,
    }
    .validate_links()
}

fn append(text: &mut String, value: &str) -> Result<()> {
    ensure!(
        text.len()
            .checked_add(value.len())
            .is_some_and(|length| length <= MAX_TEXT),
        "Formatted link exceeds its output limit"
    );
    text.push_str(value);
    Ok(())
}

fn html(text: &mut String, value: &str) -> Result<()> {
    for ch in value.chars() {
        let mut utf8 = [0; 4];
        append(
            text,
            match ch {
                '&' => "&amp;",
                '<' => "&lt;",
                '>' => "&gt;",
                '"' => "&quot;",
                '\'' => "&#39;",
                _ => ch.encode_utf8(&mut utf8),
            },
        )?;
    }
    Ok(())
}

fn format_link(format: &str, label: &str, url: &str) -> Result<String> {
    let mut text = String::with_capacity(url.len() + label.len() + 32);
    match format {
        "plain" => append(&mut text, url)?,
        "markdown" => {
            append(&mut text, "[")?;
            for ch in label.chars() {
                if ch.is_ascii_punctuation() {
                    append(&mut text, "\\")?;
                }
                append(&mut text, ch.encode_utf8(&mut [0; 4]))?;
            }
            append(&mut text, "](<")?;
            for byte in url.bytes() {
                match byte {
                    b'&' => append(&mut text, "&amp;")?,
                    b'<' => append(&mut text, "%3C")?,
                    b'>' => append(&mut text, "%3E")?,
                    b'\\' => append(&mut text, "%5C")?,
                    b' ' => append(&mut text, "%20")?,
                    _ => append(
                        &mut text,
                        std::str::from_utf8(&[byte]).context("Canonical URL must be ASCII")?,
                    )?,
                }
            }
            append(&mut text, ">)")?;
        }
        "html" => {
            append(&mut text, "<a href=\"")?;
            html(&mut text, url)?;
            append(&mut text, "\">")?;
            html(&mut text, label)?;
            append(&mut text, "</a>")?;
        }
        _ => bail!(USAGE),
    }
    Ok(text)
}

struct JsonBuffer(Vec<u8>);
impl Write for JsonBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self
            .0
            .len()
            .checked_add(bytes.len())
            .is_none_or(|length| length > MAX_JSON)
        {
            return Err(std::io::Error::other("Link JSON exceeds its output limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn output(value: serde_json::Value) -> Result<()> {
    let mut buffer = JsonBuffer(Vec::with_capacity(4096));
    serde_json::to_writer(&mut buffer, &value).context("Could not encode link result")?;
    buffer.write_all(b"\n")?;
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(&buffer.0)
        .and_then(|_| stdout.flush())
        .context("Could not write link result")
}

pub(crate) fn run(args: &[OsString]) -> Result<()> {
    let action = action(args)?;
    let result = read_result()?;
    match action {
        Action::Format { format, label } => output(serde_json::json!({
            "format": format, "text": format_link(format, label, &result.url)?
        })),
        Action::Qr(path) => {
            let pixels = crate::qr::generate_pixels(&result.url)
                .map_err(|_| anyhow::anyhow!("Upload link cannot fit a normal QR code"))?;
            let side = pixels.width();
            crate::cli::frame(pixels).save_png_as(path)?;
            output(serde_json::json!({"format":"png", "width":side, "height":side, "error_correction":"M"}))
                .context("QR PNG was saved but its result could not be written")
        }
    }
}
