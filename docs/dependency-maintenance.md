# Maintained native dependency families

Current audit update (2026-11-17): default `cargo deny check advisories` now
fails on Chrono 0.4.45 after RUSTSEC-2026-0543 surfaced. The
[official advisory](https://github.com/RustSec/advisory-db/blob/main/crates/chrono/RUSTSEC-2026-0543.md)
marks all Chrono versions unmaintained with no patched version; the
[maintainer announcement](https://github.com/chronotope/chrono/issues/1768)
describes the maintenance withdrawal. This is a maintenance gate, not a newly
asserted memory-safety vulnerability. The prior passing snapshots below remain
historical. Full active-graph migration is required without ignores or
misrepresenting a renamed/version-altered fork; current date/schedule/naming
formats and chronological precision must remain compatible.


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
