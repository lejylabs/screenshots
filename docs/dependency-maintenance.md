# Maintained native dependency families

Current audit update (2026-11-17): default `cargo deny check advisories` passes
again after moving the active application graph from Chrono to maintained Jiff
0.2.43/jiff-core 0.1.5. No advisory ignore, version spoof or renamed fork is used.
The earlier failure under RUSTSEC-2026-0543 remains archived: the
[official advisory](https://github.com/RustSec/advisory-db/blob/main/crates/chrono/RUSTSEC-2026-0543.md)
marks every Chrono version unmaintained with no patched release, as explained in
the [maintainer announcement](https://github.com/chronotope/chrono/issues/1768).
This was a maintenance gate, not a newly asserted memory-safety vulnerability.
The original snapshots below remain historical.


2026-11-15. Remove the abandoned crates without advisory ignores or disabling
fonts, accessibility, portals, native credential stores or supported targets.

The graph's five previously remaining abandoned packages were rustybuzz,
ttf-parser, zbus_names, zvariant and zvariant_derive. The upstream replacements
are maintained Harfrust/Skrifa/fontdb and the integrated zbus 6 wire/names API.
The existing GLib source safety backport stays intact; a version alert does not
prove that backport absent or close the separate alert by itself.

Font/SVG phase: retain GPUI 0.3.12 and Kit 0.9.1 behavior, narrowly patch its
published source to resvg/usvg 0.50 and direct Skrifa cmap glyph-presence lookup.
Patch cosmic-text 0.19's fontdb requirement to upstream 0.24 and the Windows
native menu's resvg requirement to 0.50. Keep all original source/licenses and
record LOCAL-PATCH files. No UI layout or rendering-backend replacement is
authorized by this maintenance. Test embedded icon rendering, SVG text/emoji
fallback, fonts and the normal GPUI suite; compile native Windows/macOS paths.
Record active resource changes rather than treating successful resolution as
performance acceptance.

D-Bus phase: migrate the complete consuming family to zbus 6 compatible releases
or narrow source adapters. Maintain native accessibility, portal and keyring
functionality. Existing raw libdbus transport for explicit task-hotkey portal
requests is independent and does not require the abandoned wire-format crates.
Never rename/version-spoof those crates or add policy ignores to manufacture a
green advisory result.

Implemented graph: resvg/usvg 0.50, fontdb 0.24, Skrifa 0.49, ashpd 0.14,
oo7 0.7, xcap 0.9.10 and zbus 6.2.1. The three-day minimum publication age
stays enabled. Narrow patches retain GPUI Linux 0.3.12, AccessKit's existing
0.24 data model and AT-SPI consumer APIs. Wire-format types now come from
zbus itself; no replacement package masquerades as zvariant or zbus_names.
The existing AT-SPI accessibility thread runs `zbus::block_on` and drives
the maintained connection runtime directly; obsolete per-connection executor
tick tasks are removed. Object-server insertion tests use the new `Option`
result semantics (a newly inserted interface returns `None`). No polling timer,
new optional service startup or extra permanent application worker is added.

Dependency-internal runtime cost: ashpd/oo7 futures running outside
`zbus::block_on` can use zruntime's one process-shared event-driven helper. Its
source creates the helper on work and lets it exit when no work remains; this
is separate from AccessKit's existing dedicated thread. This source-derived
bound is not an idle CPU, RAM or native-keyring measurement.

The active application graph across supported native targets uses these
maintained families. Cargo.lock also records GPUI Kit's dormant optional
`gpui-fast` backend and its older dependencies; the application does not select
that backend. This is not an all-features claim for upstream Kit.

Current evidence: `cargo check --workspace` passed and `cargo deny check
advisories` reports `advisories ok` with the default policy and no ignores.
The optimized workspace run passed 597 tests with 56 explicitly ignored;
Windows GNU workspace/all-target metadata checks also passed. Protocol/rendering
regression fixtures passed: both embedded-icon/text SVG fixtures and four
isolated D-Bus tests, including the real AccessKit adapter's activation, actions,
tree updates, teardown and reactivation. The latter use an independent libdbus
observer and two private buses. Native CI execution remains in progress; this entry does not establish native
screen-reader, keyring or portal acceptance.

SFTP build dependency: the published curl-sys source is narrowly patched to
honor an explicitly configured pkg-config library on Apple rather than always
selecting system libcurl. Native macOS/Windows CI builds a pinned static
curl/libssh2/OpenSSL/zlib family; both architectures' package notices are
verified by eight portable-package fixtures. Linux uses the external system
libcurl. The library is initialized only by the explicit isolated SFTP child.
This build selection still needs native macOS CI/runtime confirmation.

Root owns Cargo.toml/lock, narrowly vendored manifests/adapters, provenance
notices and native compilation/regression evidence. Third-party sources remain
under their original licenses. Re-run cargo-deny against the exact final graph;
list remaining findings honestly while each phase is in progress.

Migration contract: [zbus 6 upgrade guide](https://z-galaxy.github.io/zbus/upgrading-to-6.html).

## Integrated Chrono maintenance migration (2026-11-17)

Actual timestamp/calendar/clock consumers now use Jiff 0.2.43/jiff-core 0.1.5,
with default timezone features disabled (`std` plus browser `js`). Rusqlite's
unused Chrono conversion feature is removed. The narrow original RFC3339
boundary preserves leap seconds, all admitted fractional history digits and
stable keysets; checked civil-day integers preserve the complete final day of
year 9999 and normalization into UTC year 10000. The last scheduled millisecond
remains 253402300799999. Current-clock conversions floor negative fractions as
before. No SQL schema or user timestamp-format migration is added.

Calendar/debug local conversion uses OS timezone rules on demand. Unix explicitly
calls native `tzset` before caller-owned `localtime_r`; Windows uses the current
dynamic native rule for the same captured instant. There is no application TZ
mutation, timezone database/cache, idle timer, extra worker or optional runtime.
The native C library may retain its own state. Ordinary new UI controls, layout
and month-grid behavior are unchanged.

The genuine same-version GPUI scheduler and Kit source copies retain original
Apache-2.0 licenses, archive checksums and local-patch explanations: 10 files /
136,965 bytes and 55 files / 1,108,577 bytes respectively. Kit's retained date
fixture uses matching Jiff civil types. The lockfile changes only these two
same-version source overrides, Jiff/Jiff-core and compatible jiff-static 0.2.43,
and the necessary direct dependency edges; no unrelated version is upgraded.
Cargo's three-day publication-age rule stays enabled. Jiff-static is retained
in the resolved target-all lock graph; actual selected native features and
artifact evidence are recorded separately.

Two actual fresh source reviews (7/8) are clean on patch
`1d3c531c81cb72abc7b260537527240ebf34a9b70be5c12daf2a82e3047cd0fd`.
Activation uses a real three-way merge that preserves the newer journal CAS fix.
Optimized Linux all-target checking, all-target Clippy and formatting pass.
Windows GNU workspace/all-target metadata passes with the real cached cross
toolchain (28.72 s); this neither links nor executes Windows code. Earlier
missing-toolchain and unsupported Linux-host release-shader attempts remain
archived and are not passing results.
The full optimized workspace passes **1,098 tests, zero failures, 73 ignored,
61 outer suites**, excluding child-harness summaries (log SHA256
`5e680cca7006d89fce8aff18e597414299d3b5409944d3cb0779f2c1438ade9b`).
An isolated diagnostic linked to the actual published old Chrono library agrees
on 80,004 admission/format/order/duration cases, including 13,830 valid inputs.
Chrono is diagnostic-only, not added back to the application graph. The owned
single-thread timezone-change probe preserves instant/fraction and passes;
the exact pre-refresh source fails its negative control with the old offset.
Proof lives under `target/verification/chrono-migration-integration/`.

The default advisory check passes. A separate bare `cargo deny check licenses`
uses Cargo Deny's empty default allowlist because this repository has no Deny
license configuration; it rejects even existing MIT/Apache packages and is
recorded as failed, not relabeled as a license-policy pass. Distribution notices
and source/license provenance remain required. No allowlist or exception was
introduced by this migration.

Native Windows/macOS/browser runtime, DST/locale/clock-change and reference
hardware resource acceptance are separate. Direct upstream Kit testing selects
its unconditional dormant `gpui-fast` dev backend, whose old Clock implementation
still uses Chrono; that excluded backend is not claimed migrated. Focused
retained-source calendar fixtures must use the actual selected application
backend, preserving their assertions and keeping private harness changes apart
from production source/lock. Historical upstream packaged locks/manifests and
optional GPUI Fast entries are provenance, not a claim that every unused feature
is maintained.

Five exact upstream retained Date-Picker interaction tests pass in an isolated
optimized harness using the current Kit/Base/Component source and the real
application backend. No production source/assertion is modified and no dormant
GPUI Fast dev backend is selected. They cover open/preset/clear/cancel, disabled
admission, date/time edits and events, typed 12-hour period and range date edits.
All 31 actual scheduler package tests also pass. These headless GPUI interactions
are regressions of existing controls, not new UI work or OS/IME/render acceptance.

The first native ARM macOS run of the new standalone timezone runner failed at
its offline metadata setup: Cargo needed an inactive target-all crate archive,
although the application's native build did not use it. Its earlier native
SDK/control/printing steps remain source-specific evidence, not a whole-job
pass. The runner now resolves/fetches the private fixture's pinned manifests
before its locked offline executable run, retaining the exact application-lock
version/source check. A private native-only cache reproduces the old failure
and passes the corrected setup; no global cache, dependency version or advisory
policy is changed. Proof: `cold-cache-metadata-result.json` under the integration
artifact directory. This setup fetch is a test-build operation, not application
startup network work.
