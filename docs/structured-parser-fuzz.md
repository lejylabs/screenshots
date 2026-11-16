# Structured parser mutation verification

PLAN.md's release-engineering gate requires fuzzing coordinate/size parsing and
uploader response extraction, plus deterministic regression fixtures. This suite
is a reproducible **deterministic mutational/property fuzz smoke**, using stable
Rust and existing dependencies. It does not use coverage feedback, a long-running
fuzzer, sanitizers or a new framework. Passing it does not complete a coverage-guided
campaign, native platform acceptance or the entire production release gate.

Implemented and verified on 2026-11-16 with Rust 1.100.0, Linux x86_64 and the
optimized `release-fast` profile. All eight pure parser tests passed: 16,618
seed/mutation cases, 24 explicit boundary/privacy cases, 161 fixed pre-mutation
oracle calls and eight trusted float-oracle calls. Two existing pure drag/DPI
geometry tests also passed. These tests create no windows, native captures,
clipboard operations, requests, credential-store operations or image files.

The suite reproduced a real out-of-bounds result in the existing production drag
mapper before correction. Its subnormal viewport caused the f32 scale to become
infinite. The corrected mapper computes bounded endpoints with a finite f64
scale; the original exact 2×2 regression now passes. Registration and correction
are applied to the shared source. No dependency, public API, image allocation,
worker, cache or idle activity was added.

## Production boundary and caller inventory

| Boundary under test | Actual production implementation | Callers / deliberate scope |
| --- | --- | --- |
| Native integer arguments, including invalid native UTF-8 | `cli::number<T>`, `cli::text` | `image convert`, `capture region`, history IDs/cursors. The harness calls only these pure helpers, never `cli::run` or a command handler. |
| Structured region decoding and admission | Derived `PixelRegion` serde parser, `RegionPreset::new`, `RememberedRegion::from_display`, `validate_image_size` | Presets and in-memory region requests; decoded dimensions/coordinate overflow and display bounds. Display data is synthetic. |
| Finite/nonfinite coordinate mapping | `PixelRegion::from_drag`, `from_drag_constrained` | Editor/overlay coordinate mapping, called with bit-exact synthetic floats. No editor or window is created. |
| Export dimensions / path suffix | `ExportSize::dimensions`, `ImageExportFormat::from_path`, `validate_output_size` | Export/conversion preflight. Paths are parsed as strings; no file is opened, created, decoded or encoded. |
| Native and SXCU profiles | `UploadDestination::from_json_snapshot`, `from_sharex_snapshot` | Pure snapshot implementations used by bounded profile readers. No profile loader, OS credential operation or HTTP client is called. |
| Primary and extra result templates | `sharex_response`, `UploadResultParser::from_sharex`, `to_sharex` | The actual JSON path, escape, header, regex, XML and request-URL template implementation. Extra results intentionally reject request-URL templates; primary PUT parsing is exercised separately. |
| Response extraction | `UploadResultParser::extract`, `json_pointer_text`, `regex_capture`, `xml_capture`, `header_link`, `compose_result_link`, `validate_link` | The same pure extraction primitives used by primary uploads and thumbnail/deletion results. Synthetic headers, parsed JSON and text are passed directly; no request/upload is dispatched. |
| Protected errors | SXCU `ErrorMessage` rejection, `UploadOutcome::validate_links` | Synthetic private-marker values must not appear in their diagnostic chains. Valid response URLs may intentionally contain user query data; the harness does not invent a blanket secret-stripping rule. |

The existing UI-only `workspace::exact_region_request`, desktop batch-region
directory checks, response streaming/status/timeout handling and all native APIs
are outside this suite. Response byte admission is owned by the transport; the
pure extractor is tested at the admitted 1 MiB boundary. Existing loopback tests
continue to verify transport behavior separately.

## Corpus, bounds and replay

`crates/app-core/tests/fixtures/structured_parser_fuzz/*.jsonl` contains small,
synthetic structured valid seeds and bit-exact float regressions. The mutation
helper runs every seed first, then 2,048 deterministic mutations per family with
xorshift64 seed `0x5c722714a11ce905`. Its ten scheduled operators preserve a seed,
truncate, delete, insert, replace, swap, append, prepend, replace a numeric token,
or duplicate a fragment. Seed selection and token/offset selection use that same
PRNG. The operator counts are printed, so changed corpus or scheduling is visible.

Additional constructed cases cover u32 overflow/zero/extreme dimensions, NaN,
infinities, signed zero, subnormal floats, invalid UTF-8/UTF-16, Unicode and escape
syntax, truncated JSON/XML, JSON recursion depth 160, regex nesting/index
overflow, XML external-entity syntax, duplicate link headers and the 4,096-node
XML limit. Fixed response seeds have exact expected-link oracles; accepted
templates round-trip through the real exporter/parser. Successful coordinates and
dimensions must stay within their mathematical bounds, and repeated parses must
return identical results or diagnostics. Every mutation runs inside a panic guard.

Before mutation, independent fixed tables require valid presets to retain their
exact name/rectangle, valid remembered regions to retain their rectangle, and
invalid names/overflow/display bounds to fail. Exact positive output dimensions
cover Original, Half, Quarter, Width, Height and Exact on non-divisible and skinny
sources, plus explicit invalid target/source cases. Suffix fixtures require the
specific PNG/JPEG/WebP enum; a fixed both-axis matrix checks 16,384/16,385 WebP,
65,535/65,536 JPEG and the exact 256 MiB RGBA boundary. These accepted and rejected
answers make rejection of every input, accepting every input or always returning
original dimensions fail the harness. CLI fixtures require nonzero u32 values,
u32::MAX, signed i64 extrema and u8 extrema/rejections; mutated tokens compare the
complete returned integer/error result with the native FromStr contract. Returning
zero for every accepted integer therefore fails. Fixed expectations are never
read from mutated oracle fields.

Synthetic display metadata leaves `mac_coordinates` absent. Consequently,
remembered-region acceptance fixtures require their exact rectangle on
Linux/Windows and explicitly require missing-proof rejection on macOS. The suite
does not manufacture a native Mac coordinate proof to make portable tests pass.

Ordinary candidate input is capped at 16 KiB; the boundary test processes one
1 MiB synthetic response sequentially. Normalized profiles are capped by the test
at 64 KiB. Raw canonical extracted links are bounded at `3 * 2048 + 256` bytes to
allow percent-encoding expansion; links accepted by the final
`UploadOutcome::validate_links` boundary must meet its 2,048-byte output ceiling.
No image pixels, native backend, runtime, background thread, queue, cache, network
connection, filesystem mutation or polling loop is created by these tests. Corpus
retention is small bounded metadata; the largest extra buffer is the sequential
1 MiB synthetic response plus bounded parser/header storage. This is a design
bound, not a measured process-memory or latency acceptance claim.

`STRUCTURED_FUZZ` log lines record family, fixed seed, corpus/case/mutation counts,
maximum observed input/output length and elapsed microseconds. A failure records
its family/seed/case, original hex input and a smaller failing witness after at
most 64 deletion attempts. This is a bounded reducer, not a claim of global
minimality. Re-run the same source/corpus revision and test filter to reproduce.
Retain a new counterexample as a JSONL fixture when correcting production code.

The original production drag mapping has a regression counterexample in the
second `drag_bits.jsonl` seed: start `[from_bits(1), 0]`, end `[from_bits(2), 1]`,
viewport `[from_bits(3), 1]`, image `[2, 2]`. Its f32 x scale becomes infinity,
`lo` saturates to u32::MAX and `hi` is 2; subtracting can panic or wrap. The initial
optimized test run failed its rectangle-bounds assertion before correction, at
seed `0x5c722714a11ce905`, case 1. Its exact input and reduced failing witness are
retained in `target/verification/structured-parser-fuzz/execution/baseline-subnormal.log`.
The applied correction computes bounded endpoints with a finite f64 scale.

## Measured smoke result

The fixed six core tests finished in 0.23 seconds; the two CLI tests finished in
less than the test runner's 0.01-second display precision. The CLI numeric family
itself took 529 µs. Build time is recorded separately in the execution report.
These are development test timings, not reference-machine performance,
presentation, memory or latency acceptance results.

| Family | Cases | Largest observed input | Largest successful output | Family time, µs |
| --- | ---: | ---: | ---: | ---: |
| Coordinates and sizes | 2,076 | 99 B | 44 B | 1,113 |
| Float coordinate bits | 2,180 | 190 B | 16 B | 606 |
| Format paths | 2,055 | 71 B | 4 B | 307 |
| Native upload profiles | 2,055 | 381 B | 198 B | 12,432 |
| ShareX upload profiles | 2,054 | 382 B | 236 B | 22,830 |
| Response templates | 2,063 | 133 B | 80 B | 28,940 |
| Uploader responses | 2,067 | 377 B | 126 B | 166,175 |
| CLI numeric tokens | 2,068 | 76 B | 4 B | 529 |
| Explicit boundary/privacy cases | 24 | 1,048,576 B | 1 B | 1,599 |

Successful output means the family-specific serialized region/profile, canonical
template/link, suffix or fixed-size integer/rectangle representation; these byte
counts are not process memory measurements. Ordinary mutation inputs were capped
at 16 KiB but the largest observed candidate was 382 bytes. The separate 1 MiB
case exercises the admitted response boundary; this short smoke does not imply
complete exploration of either bound.

The source manifest for both fixed test runs contains 2,228 Rust/Cargo/native
source inputs and retained SHA256
`808bb044b589943942a7fe400793600039079c8ece9a8b18eab08e806511159c`
without drift during execution. The original failure uses a separate baseline
manifest. Exact commands, fixture hashes, logs and timings are retained under
`target/verification/structured-parser-fuzz/execution/`. The unrelated unregistered
capture-format integration test was held outside Cargo discovery during this run
and restored byte-for-byte afterward; its hash is recorded in `execution.json`.

## Re-run commands

After registration with the macOS coordinate metadata, the complete coordinated
`release-fast` workspace passed **996 tests, zero failed**, with 69 explicit
opt-ins excluded across 59 outer Cargo suites. This includes all eight parser
tests. Final formatting, all-target optimized Clippy, default advisories and
all-workspace/all-target Windows GNU metadata checking pass. All 2,273 inputs in
that broader source/provenance manifest remained unchanged; it additionally
includes tracked Python fixtures and the new registered Mac sources, so its
inventory differs from the focused 2,228-source run above. The unregistered
capture-format CLI test was restored byte-for-byte after temporary exclusion
from Cargo discovery. Exact logs, commands and hashes are recorded under
`target/verification/mac-fuzz-global/`. This does not add coverage-guided,
sanitizer, native-runtime or reference-hardware acceptance.

Run from the repository root. The core module is registered as a child of
`upload` so production functions remain private. The CLI module is a child of
`cli`; no library/API/dependency changes are needed.

```sh
cargo test --locked --offline --profile release-fast -j 2 -p app-core --lib upload::structured_parser_fuzz_tests:: -- --test-threads=1 --nocapture
cargo test --locked --offline --profile release-fast -j 2 -p screenshots --bin screenshots cli::structured_parser_fuzz_tests:: -- --test-threads=1 --nocapture
```

Use an existing private verification fixture environment when collecting logs.
Record source revision, fixture hashes, command, exit status and the emitted
statistics with the result. An optimized run checks release overflow behavior;
a debug rerun can additionally expose checked arithmetic panics. No native or
credential permission is needed. Changing the fixed seed/count for an extended
deterministic run is possible in the helper, but it remains a mutation smoke
without coverage feedback. A coverage-guided campaign must be scheduled and
reported separately, including its engine, duration, corpus and findings.
