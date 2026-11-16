mod audio_record;
mod autostart;
#[cfg(any(target_os = "macos", all(test, unix)))]
mod autostart_macos;
#[cfg(target_os = "windows")]
mod autostart_windows;
mod capture_catalog;
mod capture_portal;
mod capture_regions;
mod capture_scheduling;
mod capture_sequence;
mod capture_task;
mod capture_worker;
mod cli;
mod clipboard_command;
mod desktop_integration;
mod gif_assembly;
mod hash_file;
mod history_audit;
mod history_manage;
mod image_collection;
mod image_info;
mod image_tools;
mod image_url;
mod metadata_jpeg;
mod metadata_webp;
mod oauth_provider;
mod ocr;
#[cfg(target_os = "windows")]
mod ocr_windows;
mod png_metadata;
mod qr;
mod record;
#[cfg(target_os = "windows")]
mod record_windows;
mod region_analysis;
mod s3;
mod screen_tools;
mod scroll_capture;
mod scroll_stitch;
mod sharex;
mod sharing_provider;
#[cfg(unix)]
mod sharing_provider_unix;
#[cfg(target_os = "windows")]
mod sharing_provider_windows;
#[cfg(any(unix, target_os = "windows"))]
mod sharing_provider_worker;
mod task;
mod task_clipboard;
mod task_host;
mod upload_batch;
mod upload_file;
mod upload_files;
mod upload_results;
mod workflow;

fn main() {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    match capture_worker::try_child(&arguments) {
        Ok(true) => return,
        Ok(false) => {}
        Err(error) => {
            eprintln!("screenshots: {error:#}");
            std::process::exit(1);
        }
    }
    if let Err(error) = capture_worker::install() {
        eprintln!("screenshots: {error:#}");
        std::process::exit(1);
    }
    match cli::run(arguments) {
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

    let mut args = std::env::args_os().skip(1);
    if args.next().is_some_and(|arg| arg == "--open") {
        app_ui::run_open_image(
            std::path::absolute(args.next().unwrap()).expect("Could not resolve image path"),
        );
    } else {
        app_ui::run();
    }
}
