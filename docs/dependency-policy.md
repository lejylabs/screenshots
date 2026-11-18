# Native dependency policy

CI has two advisory checks and one actual-source license check. Cargo Deny
0.20.2 skips advisory matching for packages whose metadata source is `None`;
its registry advisory pass alone does not cover our vendored patches. The
supplied metadata checkpoint contains 996 packages, including 27 vendored
third-party packages and 4 first-party workspace packages without a registry
source. Counts describe that snapshot, not an assumed permanent graph size.

`deny.toml` selects exactly the existing Linux x86_64/aarch64, Windows x86_64
MSVC and macOS ARM64/x86_64 targets, normal/build dependencies and default
features. It enables no optional backend, all-features, browser or fuzzing
configuration. Unpublished roots stay checked; workspace dev dependencies
are excluded. Only advisories and licenses run, without bans/source checks.
Unknown/unlicensed packages have no allowance.

The explicit SPDX families are MIT, Apache-2.0 (including its separately named
LLVM exception), BSD-2/3-Clause, ISC, Zlib, BSL-1.0, 0BSD, CC0-1.0 and Unicode-3.0.
MPL-2.0 is limited to exact cbindgen 0.28.0, dwrote 0.11.5 and option-ext 0.2.0.
The main policy has no advisory ignores; vulnerabilities, all selected
unmaintained/unsound matches and yanked versions fail.

## Vendored upstream advisory coverage

The job retains one actual locked default-feature Cargo metadata snapshot.
The main check uses it for registry advisories and licenses on actual local
source. Cargo Deny's own filtered crate listing identifies selected native
packages. The explicit `--exclude-dev` flag is used for listings and checks:
the pinned listing route does not merge every graph flag from configuration
like the check route.

`scripts/vendored-advisory-view.py` creates a separately labeled published-base
advisory view. Each selected local third-party package requires a reviewed
name/version/path mapping in `config/vendored-upstream-advisories.json`. The
script verifies its complete regular source-byte fingerprint, downloads the
exact published base archive, verifies its pinned publisher checksum and checks
its original manifest identity. The committed checksums were independently
confirmed through the official crates.io version API. Unknown mappings, changed
source and archive mismatches fail. First-party workspace packages are classified
separately and never assigned a false crates.io identity.

Only a verified vendor package's advisory source identity changes. Actual IDs,
versions, checksums, resolve edges, features and target predicates stay unchanged;
no upstream dependency graph is resolved. The sparse crates.io identity is
deliberate: pinned Cargo Deny recognizes it for RustSec, while pinned krates 0.21.2
does not replace local features with published index features for that spelling.
The job compares the entire actual/projected selected crate listing and fails
on any difference.

The second check runs advisories only against this view and the same freshly
fetched RustSec snapshot, with fetching disabled in that second stage. The view
is never used for builds, dependency resolution or license acceptance. It reports
advisories conservatively against the published base version; it does not claim
our patched source is an unmodified registry package. Listing licenses in the
view is solely a selected-graph comparison, not a license-policy pass.

## Exact-source mitigation

GLib 0.18.5's published base matches RUSTSEC-2024-0429. Actual vendored source
already contains the official two-line out-argument mutability fix. The
supplemental policy may recognize only that mitigation after checking unique
GLib identity in the entire metadata snapshot, the exact reviewed source-tree
fingerprint, verified original archive/file, exact two-line correction and
patched-file hash, unchanged advisory body, and retained upstream/prior-test
evidence. A changed advisory reopens review even with the same ID. A different
GLib package/version, absent fix or unknown advisory cannot inherit the disposition.

The main policy remains ignore-free. The generated advisory-only policy records
one proof-scoped disposition visibly; its report retains the conservative
upstream match and exact source/evidence identities. No fake fixed version or
checksum is manufactured. New advisories fail until code is corrected or a new
exact-source mitigation is independently established. This is not a blanket
waiver for vendored packages. Prior tests are dated evidence, not new application
compilation or execution by this policy job.

## Provenance, obligations and cost

The dedicated Ubuntu job pins Rust 1.100.0 and cargo-deny 0.20.2, verifies the tool
archive before compilation, and records its source lock and compiled binary
hashes. Checkout/artifact actions use pinned commits. No compiled-tool/advisory
cache is restored. RustSec is fetched afresh and its origin/commit recorded.
Logs, actual metadata, selected listings, advisory-only view, source verification
and dispositions are retained for 7 days, including failures. Input hashes are
recorded; a successful combined gate must leave Cargo.lock unchanged.

This machine compatibility policy does not establish legal approval or complete
distribution compliance. Preserve original copyright/license/patent notices,
MPL-covered source availability and modification notices, native-library terms
and separate runtime notices. Existing package notice/source checks remain
required. The application's MIT license does not replace dependency terms or
approve a future FFmpeg bundle.

Fingerprinting excludes only root `target`, `.git` and `.cargo-ok` bookkeeping.
Each package admits at most 4096 regular files, 8192 entries and 32 MiB of source;
each file is at most 16 MiB. Selected source totals are capped at 256 MiB. Archives
are at most 8 MiB each and 64 MiB in total, processed sequentially with fixed public
URLs and checksum checks. Each unpacked tar is capped at 64 MiB/4096 members;
reader/container copies add bounded CI memory.
Metadata is capped at 16 MiB, provenance at 1 MiB and selected listings at 4 MiB.
This is CI-only work: no application dependency, startup action, resident worker,
runtime/timer, background network check, pixel allocation or PLAN budget change.

After the normal checks and selected-list comparison, CI runs
`scripts/test-dependency-policy-engine.py` with the installed pinned engine and
already checked metadata/policies. In one private bounded copy of the freshly
fetched database it requires actual/projected baselines to pass, removal of only
the generated mitigation to fail specifically on RUSTSEC-2024-0429, and an added
test GLib vulnerability with a distinct ID to fail despite that mitigation. It
also requires license rejection for private metadata copies carrying GPL-3.0-only
on app-core and option-ext 0.2.1 with its original MPL-2.0 license. The latter is
an artificial version-scope probe, not a claim about a published future release.
A fresh empty private database root and one owned HTTPS localhost endpoint prove
fetch failure remains a failure; endpoint contact and the exact fetch diagnostic
are required. No production database, metadata, vendor source or lockfile is
changed. Expected advisory/fetch exits are 1 and license rejection exits are 4;
unrelated errors cannot satisfy the specific negative predicates.

Controls retain individual logs, input hashes and results under the evidence
artifact, including failures; temporary database/metadata copies are removed.
The database copy admits at most 32768 entries/256 MiB, individual files 64 MiB;
each subprocess has a 180-second deadline (40 seconds for fetch failure), process
group cleanup and an 8 MiB log cap. Controls run sequentially and add no package
resolution, download of production archives or application build. The one
operation-scoped loopback listener/thread is bounded to 45 seconds. These are
CI resource limits, not application resource acceptance measurements.

Current local execution passes the online primary advisory/license gate, verified
published-base advisory view, exact selected-list equality and all seven actual
engine controls. Expected exits are 0,0,1,1,4,4,1; each negative also requires its
specific advisory/license/fetch evidence. RustSec snapshot:
`472a4c9a785030434f15d7da84964d60fbe789ab`. Cargo.lock and protected inputs remain
unchanged. Two fresh source reviews are clean. Hosted execution of the current
workflow remains pending; local or synthetic checks are not a hosted CI pass.

References: [Cargo Deny configuration](https://embarkstudios.github.io/cargo-deny/checks/cfg.html),
[pinned advisory matching](https://github.com/EmbarkStudios/cargo-deny/blob/bca0dde53651ee946720e4540b5ce2610bec8f06/src/advisories/helpers/db.rs),
[GLib fix](https://github.com/gtk-rs/gtk-rs-core/commit/b5a4071e439bef2b5eea76c3aa25e5ae84839e34),
[RustSec advisory](https://rustsec.org/advisories/RUSTSEC-2024-0429.html).
