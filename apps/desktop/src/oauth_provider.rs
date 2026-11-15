//! Explicit OAuth CLI adapter; authorization URLs/codes appear only in login events.

use anyhow::{Context, Result, ensure};
use std::{
    ffi::OsString,
    io::Write,
    path::Path,
    time::{Duration, Instant},
};

fn output(value: &impl serde::Serialize) -> Result<()> {
    let mut stream = std::io::stdout().lock();
    serde_json::to_writer(&mut stream, value)
        .map_err(|_| anyhow::anyhow!("Could not encode OAuth result"))?;
    stream
        .write_all(b"\n")
        .and_then(|_| stream.flush())
        .context(
            "OAuth action completed but its result could not be written; credentials may remain",
        )
}

pub(crate) fn run(args: &[OsString]) -> Result<()> {
    ensure!(
        (2..=3).contains(&args.len()) && (args.len() == 2 || args[2] == "--allow-loopback-http"),
        "Usage: screenshots oauth login|refresh|remove|target PROFILE [--allow-loopback-http]"
    );
    let action = args[0].to_str().context("Invalid OAuth action")?;
    ensure!(
        matches!(action, "login" | "refresh" | "remove" | "target"),
        "Unknown OAuth action"
    );
    let profile = app_core::OAuthProfile::load(Path::new(&args[1]), args.len() == 3)?;
    if action == "target" {
        let (upload, refresh) = profile.targets();
        return output(&serde_json::json!({"upload_target":upload,"refresh_target":refresh}));
    }
    #[cfg(unix)]
    let cancellation = crate::record::unix::Cancellation::new()?;
    let deadline = Instant::now() + Duration::from_secs(180);
    let check = || {
        #[cfg(unix)]
        cancellation.check(deadline)?;
        ensure!(
            Instant::now() < deadline,
            "OAuth operation exceeded its deadline"
        );
        Ok(())
    };
    let result = match action {
        "login" => serde_json::to_value(profile.login(&check, &mut |event| {
            #[cfg(unix)]
            cancellation.output_phase(true);
            let result = output(&event);
            #[cfg(unix)]
            cancellation.output_phase(false);
            result
        })?)?,
        "refresh" => serde_json::to_value(profile.refresh(&check)?)?,
        "remove" => {
            profile.remove(&check)?;
            serde_json::json!({"removed":true})
        }
        _ => unreachable!(),
    };
    #[cfg(unix)]
    drop(cancellation);
    output(&serde_json::json!({"type":"complete","result":result,"profile_file_changed":false}))
}
