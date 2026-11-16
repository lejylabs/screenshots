# Portable builds and manual updates

The workspace pins Rust 1.100.0 in `rust-toolchain.toml` and commits Cargo.lock.
Build on the intended native OS/architecture with the system dependencies listed
in `.github/workflows/rust.yml`. Python 3.11 or newer is required for packaging.
Do not use `--ignore-rust-version` or update dependencies during release builds.

```sh
python3 scripts/test-package.py
python3 scripts/build-sharex-helper.py --output target/release/lib/screenshots-sxcu
python3 scripts/package.py --target x86_64-unknown-linux-gnu
```

On Windows use `python scripts/package.py --target x86_64-pc-windows-msvc`.
On an Apple Silicon Mac use `python3 scripts/package.py --target aarch64-apple-darwin`.
The target must match the installed compiler's native host. Other architectures
require a native build and their own runtime acceptance. `--binary PATH` packages
an explicitly supplied prebuilt executable instead of running Cargo; its
provenance is recorded as prebuilt and must be verified by the caller.
Release binaries omit debug sections; optimized development/test builds retain
limited symbols and may exceed the 512 MiB package-executable admission limit.
An explicitly prebuilt input is copied and hashed from one admitted handle so
a concurrent rebuild cannot give the archive a different binary hash.

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

On Linux, retain the complete extracted version folder, including `BUILD-INFO.json`
and `lib/screenshots-sxcu`. Expose its executable through a symlink in
`~/.local/bin`, then point the optional desktop launcher at that symlink. Copying
only the executable breaks private-runtime discovery. Confirm that PATH resolves
to the selected version. For managed user installation, invoke the archive's
`install-user.py install PACKAGE ROOT`; its owned launcher/version layout and
rollback/uninstall commands are described in [desktop integration](desktop-integration.md).
To remove a manual install, remove its own symlink/launcher and version folder;
retain user settings, history and exported images. Autostart and authenticated
updates require explicit commands; no periodic updater is started.

Windows portable executables are unsigned. Windows may display a SmartScreen
warning; no reputation or code-signing acceptance is claimed. The macOS portable archive retains the executable and private runtime together.
Native `.app`/`.pkg` development bundles are also unsigned and not notarized. It does
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

The full custom-uploader helper is a private self-contained bundle under
`lib/screenshots-sxcu`. Build it with pinned .NET SDK 10.0.404 and locked packages;
the build script stores mutable SDK/NuGet state under `target`. Packaging
requires its native runtime manifest, validates every regular member/hash and
its 512-file / 256 MiB aggregate bounds, preserves executable flags and records
the manifest hash in build info. `--sharex-runtime PATH` supplies an explicit
build bundle; it cannot change the application's runtime search path. The
extracted-package smoke executes its self-tests and the application's profile
inspection, with isolated user paths and no upload.

User installs retain each version's runtime with that version. An old running
process uses its compiled version/target folder after an upgrade. macOS bundles
place the private runtime in `Contents/Resources/lib`; the Debian layout keeps
the real executable, build info and runtime under `/usr/lib/screenshots` with
a controlled `/usr/bin/screenshots` symlink. These locations keep runtime
discovery tied to owned package layouts. Native signing and OS runtime acceptance
remain distinct release gates.

An intermediate Linux debug-stripped working-tree package measured 60.76 MiB
compressed and passed its actual extracted helper/application smoke. This is
development packaging evidence, not the final frozen release. Raw evidence is
`target/verification/complete-runtime-package.json`.

Clippy enforces the repository's configured denied lints. Existing compiler/
Clippy warnings are reported; this new workflow does not claim a warning-free
baseline. The current active native dependency graph passes the default
`cargo deny check advisories` policy, without ignores. Dormant upstream optional
backends in the lockfile and the separate default-branch GLib alert are described
in `dependency-maintenance.md`; package creation is not security signoff.


Linux build hosts determine the executable's glibc requirement. Packaging reads
its ELF symbol versions with `readelf`, records `linux_abi.glibc` in build info,
and the Debian control file requires at least that version. A binary built on a
newer development distribution must not be advertised as compatible with the
Ubuntu CI baseline. The intermediate local package required glibc 2.44; it is
host-only development evidence. Native Ubuntu 24.04 CI builds provide the separate
baseline artifact.

The self-contained SXCU helper includes the managed runtime, not Linux's native
libraries. Debian dependencies therefore include ICU, OpenSSL, zlib, the C++
runtime, Kerberos and certificate roots alongside desktop libraries. ICU and
OpenSSL alternatives follow the
[Microsoft Ubuntu dependencies](https://learn.microsoft.com/en-us/dotnet/core/install/linux-ubuntu-install)
and [Debian dependencies](https://learn.microsoft.com/en-us/dotnet/core/install/linux-debian).
A dependency declaration is not runtime acceptance on every listed distribution.


The refreshed ABI-bearing intermediate archive and Debian package also passed
extracted help/version, real profile inspection and all 134 helper assertions.
The `.deb` controlled launcher resolves to the owned private executable/runtime;
its control file declares the actual staged ELF glibc 2.44 floor. No system
installation occurred: Python inspected its ar/tar payload because this host has
no dpkg-deb. The report pins binary/archive hashes and unchanged isolated user
state: `target/verification/complete-runtime-package-abi.json`. Concurrent feature
work and this explicit prebuilt binary remain distinct from final frozen-source,
native CI, signing and standard-user installation acceptance.


## Explicit identity-backed native signing

`scripts/package-app.py --package ARCHIVE --output DIRECTORY --sign-profile PROFILE`
uses an existing native identity and verifies requested signatures before
publishing. Profile JSON contains public selectors only, not private keys or
passwords. macOS optionally accepts `--notary-profile EXISTING_KEYCHAIN_PROFILE`;
it requires an Accepted notary result and validated staple. No credentials are
created, imported or unlocked by this tooling. Unsigned development packaging
remains available when no profile is supplied.

Windows profiles specify version 1, platform `windows`, a 40-digit certificate
thumbprint, exact publisher DN and HTTPS timestamp URL. The generated MSIX uses
that publisher with proper XML escaping. macOS profiles specify version 1,
platform `macos`, stable application/installer identity names and ten-character
team ID. The .NET helper receives only its required allow-jit exception; nested
native signatures are verified and their changed hashes update the runtime
manifest/anchor. The enclosing macOS bundle records the input binary hash with
an explicit pre-bundle-signature scope; final artifact SHA and native signatures
cover the published output without a circular resource hash.

See [native signing](native-signing.md) for the exact source/verification plan.
Synthetic tests exercise tool arguments, signature-induced hash changes,
rejection/no-publication, resource consistency and timeout/output bounds. No
production identity or real notary submission was available during implementation;
source/mocked tests do not establish trusted-signature or Gatekeeper acceptance.
