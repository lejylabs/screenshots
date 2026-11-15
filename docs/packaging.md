# Portable builds and manual updates

The workspace pins Rust 1.100.0 in `rust-toolchain.toml` and commits Cargo.lock.
Build on the intended native OS/architecture with the system dependencies listed
in `.github/workflows/rust.yml`. Python 3.11 or newer is required for packaging.
Do not use `--ignore-rust-version` or update dependencies during release builds.

```sh
python3 scripts/test-package.py
python3 scripts/package.py --target x86_64-unknown-linux-gnu
```

On Windows use `python scripts/package.py --target x86_64-pc-windows-msvc`.
On an Apple Silicon Mac use `python3 scripts/package.py --target aarch64-apple-darwin`.
The target must match the installed compiler's native host. Other architectures
require a native build and their own runtime acceptance. `--binary PATH` packages
an explicitly supplied prebuilt executable instead of running Cargo; its
provenance is recorded as prebuilt and must be verified by the caller.

The default build uses `cargo build --locked --release -p screenshots`. Archives
and SHA-256 sidecars appear in `target/packages/`; existing outputs are never
replaced. Publication uses same-filesystem hard links and reports an error if
the destination filesystem cannot provide them. Use a fresh output directory
with `--output PATH` for another build of the same version.

Archives contain the executable, application/icon notices, lockfile, README,
these instructions, dependency license metadata and available license/notice
files from Cargo's resolved package roots. Metadata is not a substitute for a
dependency vulnerability/license review. `BUILD-INFO.json` records compiler,
source commit, dirty-tree state, target and binary hash. Archive timestamps use
the commit time or `SOURCE_DATE_EPOCH`. The same binary, metadata and timestamp
produce the same archive; this is not a claim that recompiling produces an
identical binary. Runtime system libraries are not bundled.

Cargo/rustc/git command output is decoded explicitly as UTF-8, including Cargo
metadata on Windows hosts whose default code page is not UTF-8. Generated JSON
and checksum files are UTF-8 with LF newlines on every platform. Copied license
and source files retain their original bytes.

## Install and update manually

Verify the SHA-256 sidecar, extract into a new versioned folder, and test it
before replacing an older version. Quit the running application before switching
executables. Keep the previous folder for rollback. Remove only the old extracted
folder to uninstall the executable; this deliberately preserves user settings,
history and exported images. Back up user data before upgrades or downgrades;
older versions may not understand newer database schemas.

On Linux, place the executable on your PATH (for example `~/.local/bin`) and
optionally copy `screenshots.desktop` to `~/.local/share/applications`. The
desktop entry runs `screenshots --show`. Confirm that PATH resolves to the chosen
version. No installer script changes system files or enables autostart. Remove
the launcher and executable to uninstall. There is no automatic updater.

Windows portable executables are unsigned. Windows may display a SmartScreen
warning; no reputation or code-signing acceptance is claimed. The macOS archive
is a raw unsigned executable, not a signed/notarized application bundle. It does
not establish stable Screen Recording permissions or macOS release readiness.
Linux session/portal support remains as documented in PLAN.md; a Linux package
does not establish Wayland capture acceptance.

## Artifact checks

CI runs optimized tests and packages native Linux, Windows and macOS executables.
Deterministic tests use the existing optimized `release-fast` profile, avoiding
thin-LTO relinking for every integration-test binary. The packaged application
still uses the production `release` profile with thin LTO. Native artifact smoke
tests execute that packaged release binary; test-profile success is not a
substitute for final binary or hardware acceptance.
Downloaded workflow artifacts are development artifacts, not published releases.
Hardware-dependent tests remain opt-in. Test capture/clipboard/permissions and
install/update/uninstall using the actual extracted binary on every supported
platform before release. Tests must isolate configuration/data/cache paths and
must not alter the user's real clipboard or capture the desktop implicitly.

Clippy enforces the repository's configured denied lints. Existing compiler/
Clippy warnings are reported; this new workflow does not claim a warning-free
baseline. The separate advisory audit still reports unmaintained transitive
packages, recorded in the delivery report; package creation is not security signoff.
