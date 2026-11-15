mod capture_sequence;
mod cli;
mod hash_file;
mod history_audit;
mod history_manage;
mod image_collection;
mod image_info;
mod image_tools;
mod ocr;
mod png_metadata;
mod qr;
mod record;
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
