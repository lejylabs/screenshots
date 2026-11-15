//! Explicit offline animation assembly with no recording or persistent runtime.

use anyhow::{Context, Result, ensure};
use std::{ffi::OsString, io::Write, path::Path};

pub(crate) fn run(args: &[OsString]) -> Result<()> {
    ensure!(
        (5..=19).contains(&args.len()) && args[1] == "--delay-ms",
        "Usage: screenshots gif OUTPUT.gif --delay-ms N INPUT INPUT... (2–16 images)"
    );
    ensure!(
        args.iter().all(|arg| arg.as_encoded_bytes().len() <= 4096),
        "GIF path or argument exceeds 4096 bytes"
    );
    let delay: u32 = args[2]
        .to_str()
        .context("GIF delay must be UTF-8")?
        .parse()
        .context("GIF delay must be a whole number")?;
    let inputs = args[3..]
        .iter()
        .map(std::path::absolute)
        .collect::<std::io::Result<Vec<_>>>()?;
    let destination = Path::new(&args[0]);
    app_core::assemble_gif(&inputs, destination, delay)?;
    let mut stdout = std::io::stdout().lock();
    writeln!(
        stdout,
        "{}",
        serde_json::json!({"format":"gif","frames":inputs.len(),"delay_ms":delay})
    )
    .and_then(|_| stdout.flush())
    .context("GIF was saved but its result could not be written")
}
