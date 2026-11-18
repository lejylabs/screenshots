//! `screenshots-install`: user-owned install, upgrade, rollback, recovery, removal
//! and explicitly signed updates. Prints one JSON result line on success.
use std::path::Path;

const USAGE: &str = "usage: screenshots-install <command> ...
  install <package> <root>
  status <root>
  rollback <root>
  uninstall <root>
  pin-key <root> <public_key.pem>
  recover <root> current|previous
  update <root> <manifest_url>
  apply-signed <root> <manifest> <signature> <artifact>";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    use screenshots_install as install;
    let result = match words.as_slice() {
        ["install", package, root] => install::install(Path::new(package), Path::new(root)),
        ["status", root] => install::status(Path::new(root)),
        ["rollback", root] => install::rollback(Path::new(root)),
        ["uninstall", root] => install::uninstall(Path::new(root)),
        ["pin-key", root, key] => install::pin_key(Path::new(root), Path::new(key)),
        ["recover", root, choice @ ("current" | "previous")] => install::recover(Path::new(root), choice),
        ["update", root, url] => install::update(Path::new(root), url),
        ["apply-signed", root, manifest, signature, artifact] => {
            install::apply_signed(Path::new(root), Path::new(manifest), Path::new(signature), Path::new(artifact))
        }
        ["--help" | "-h"] => {
            println!("{USAGE}");
            return;
        }
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    match result {
        Ok(outcome) => println!("{}", install::outcome_json(&outcome)),
        Err(error) => {
            #[cfg(debug_assertions)]
            if std::env::var_os("SCREENSHOTS_INSTALL_DEBUG_ERRORS").is_some() {
                eprintln!("{error:#}");
            }
            #[cfg(not(debug_assertions))]
            let _ = error;
            // Paths, signed URLs and server response payloads are not diagnostic text.
            eprintln!(
                "Installation/update completion was not confirmed; completed changes may remain. Inspect status before retrying."
            );
            std::process::exit(1);
        }
    }
}
