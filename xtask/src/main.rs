//! Release tooling, run as `cargo xtask <command>`.
mod advisory;
mod ci;
mod package;
mod package_app;
mod policy_controls;
mod run;
mod sign;

const USAGE: &str = "usage: cargo xtask <command>
  package --target <triple> [--binary <path> --installer <path>] [--output <dir>]
  package-app --package <archive> --output <dir> [--sign-profile <json>] [--notary-profile <name>]
  package-smoke <archive>
  vendored-advisory-view prepare --metadata .. --selected .. --provenance .. --root .. --policy .. --advisory-db-root .. --output ..
  vendored-advisory-view verify-selection <before> <after>
  dependency-policy-controls --tool .. --evidence .. --policy .. --advisory-db-root ..";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("package") => package::main(&args[1..]),
        Some("package-app") => package_app::main(&args[1..]),
        Some("package-smoke") => package::smoke_main(&args[1..]),
        Some("vendored-advisory-view") => advisory::main(&args[1..]),
        Some("dependency-policy-controls") => policy_controls::main(&args[1..]),
        Some("ci") => ci::main(&args[1..]),
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    if let Err(error) = result {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
