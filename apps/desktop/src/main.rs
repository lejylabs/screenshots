mod autostart;
#[cfg(any(target_os = "macos", all(test, unix)))]
mod autostart_macos;
#[cfg(target_os = "windows")]
mod autostart_windows;
mod capture_sequence;
mod capture_task;
mod cli;
mod hash_file;
mod history_audit;
mod history_manage;
mod image_collection;
mod image_info;
mod image_tools;
mod ocr;
#[cfg(target_os = "windows")]
mod ocr_windows;
mod png_metadata;
mod qr;
mod record;
#[cfg(target_os = "windows")]
mod record_windows;
mod scroll_stitch;
mod upload_batch;
mod upload_file;
mod workflow;

fn main() {
    match cli::run(std::env::args_os().skip(1).collect()) {
        Ok(true) => return,
        Err(error) => {
            eprintln!("screenshots: {error:#}");
            std::process::exit(1);
        }
        Ok(false) => {}
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    app_ui::run();
}
