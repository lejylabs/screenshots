mod audio_record;
mod autostart;
#[cfg(any(target_os = "macos", all(test, unix)))]
mod autostart_macos;
#[cfg(target_os = "windows")]
mod autostart_windows;
mod capture_catalog;
mod capture_format_command;
mod capture_portal;
mod capture_regions;
mod capture_scheduling;
mod capture_sequence;
mod capture_task;
mod capture_worker;
mod cli;
mod clipboard_command;
mod desktop_integration;
#[cfg(target_os = "linux")]
mod desktop_services_process;
mod editor_document;
mod file_logging;
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
mod print_image;
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
#[cfg(target_os = "windows")]
mod windows_drop_spawn;
mod workflow;

fn main() {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    #[cfg(all(target_os = "windows", screenshots_native_drop_acceptance))]
    if arguments
        .first()
        .is_some_and(|value| value == "--owned-native-drop-acceptance")
    {
        let result = (|| {
            anyhow::ensure!(
                arguments.len() == 2,
                "One explicit acceptance case is required"
            );
            windows_drop_spawn::install()?;
            let case = arguments[1]
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Invalid acceptance case"))?;
            if case.starts_with("gpui-") {
                app_ui::run_windows_native_drop_editor_acceptance(case)
            } else {
                app_ui::run_windows_native_drop_acceptance(case)
            }
        })();
        if let Err(error) = result {
            eprintln!("owned native acceptance: {error:#}");
            std::process::exit(1);
        }
        return;
    }
    let result = launch(
        arguments,
        |arguments| {
            #[cfg(target_os = "linux")]
            if desktop_services_process::try_child(arguments)? {
                return Ok(true);
            }
            #[cfg(target_os = "windows")]
            if windows_drop_spawn::try_child(arguments)? {
                return Ok(true);
            }
            if print_image::try_child(arguments)? {
                return Ok(true);
            }
            capture_worker::try_child(arguments)
        },
        || {
            #[cfg(target_os = "linux")]
            desktop_services_process::install()?;
            capture_worker::install()?;
            #[cfg(target_os = "windows")]
            windows_drop_spawn::install()?;
            Ok(())
        },
        cli::run,
        || {
            let mut args = std::env::args_os().skip(1);
            let open_image = if args.next().is_some_and(|arg| arg == "--open") {
                Some(
                    std::path::absolute(args.next().unwrap())
                        .expect("Could not resolve image path"),
                )
            } else {
                None
            };
            app_ui::run_desktop(open_image, file_logging::init);
        },
    );
    if let Err(error) = result {
        eprintln!("screenshots: {error:#}");
        std::process::exit(1);
    }
}

fn launch(
    arguments: Vec<std::ffi::OsString>,
    try_child: impl FnOnce(&[std::ffi::OsString]) -> anyhow::Result<bool>,
    install: impl FnOnce() -> anyhow::Result<()>,
    cli: impl FnOnce(Vec<std::ffi::OsString>) -> anyhow::Result<bool>,
    run_desktop: impl FnOnce(),
) -> anyhow::Result<()> {
    if try_child(&arguments)? {
        return Ok(());
    }
    install()?;
    if cli(arguments)? {
        return Ok(());
    }
    run_desktop();
    Ok(())
}

#[cfg(test)]
#[path = "desktop_startup_tests.rs"]
mod desktop_startup_tests;
