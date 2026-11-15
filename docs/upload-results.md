# Explicit upload-result formatting and QR output

Author: NewtTheWolf
Date: 2026-11-15
Status: implemented; focused executable, independent renderer and release
checks passed. Complete regression evidence is in the delivery log.

## Smallest useful scope

Add a standalone `link` postprocessor for one existing upload-result JSON object
received on nonterminal stdin. It turns a confirmed link into copy-ready text or
a QR PNG without uploading again, reading account credentials, opening a browser
or changing the clipboard. Existing upload/profile/request behavior and result
schemas stay unchanged. This covers a first finite set of link formats and
explicit QR output; it does not complete custom link templates, shortening,
automatic copying or general after-upload task chains.

Changing existing upload flags or result schemas would couple this operation
to every transport and caller. A separate postprocessor preserves those
contracts. Batch/nested-result extraction is deferred because it needs explicit
completion and partial-success rules. A general template engine or task graph
adds unnecessary configuration and resource costs for these three fixed formats.

```sh
screenshots upload-file destination.json image.png | screenshots link format markdown --label Screenshot
screenshots upload destination.json image.png | screenshots link qr share.png
screenshots link format html --label Screenshot < upload-result.json
```

The first command returns JSON whose `text` field is ready for a Markdown
document. It does not copy it. With shell pipelines, enable the shell's pipeline
failure propagation or inspect both process exit statuses. This postprocessor
cannot recover an upstream exit status from bytes received on stdin; a valid
result object is not proof of a complete upstream workflow.

## CLI and input contract

Exact grammar, after the `screenshots` executable:

```text
link format plain
link format markdown [--label TEXT]
link format html [--label TEXT]
link qr OUTPUT.png
```

No option reordering beyond the displayed optional label pair, unknown flags,
duplicates, extra arguments or omitted values. Mode names are lowercase ASCII.
The label must be valid UTF-8, contain 1–256 bytes, and contain no Unicode control
characters, U+2028 or U+2029. Do not trim or normalize it. Labels are allowed
only for Markdown and HTML; their default is the literal `Screenshot`. No label
or URL is a command, filename template or executable expression.

Validate arguments before reading stdin. For QR, also validate the output
extension is case-insensitive `.png`, its native argument is at most 4096 encoded
bytes, and `symlink_metadata` reports NotFound. Other errors, existing regular
files, directories, and broken symlinks fail before reading input. Relative
output paths use the invoking directory. The atomic PNG writer supplies the
final no-replace collision check.

Require nonterminal stdin using the same `IsTerminal` policy as the existing
credential commands. Read at most **8193 bytes** into one operation-local vector
and reject lengths above **8192 bytes**, including JSON whitespace. Empty input,
invalid UTF-8/JSON, a BOM, comments, trailing non-whitespace bytes, a second JSON
value, arrays, duplicate fields and unknown fields fail before output. JSON
field order and surrounding JSON whitespace are unrestricted.

Accept exactly this object shape:

```json
{"url":"https://example.invalid/image.png","thumbnail":null,"deletion":null}
```

`url` is required and must be a string. `thumbnail` and `deletion` are optional
strings or null; omitted values mean null. Reject null/empty primary URLs and
empty optional strings. Decode with a private `Deserialize` struct using
`deny_unknown_fields`, after requiring the first non-JSON-whitespace byte to
be `{` (derived struct deserialization also accepts sequence form). Do not use
a general `serde_json::Value` that would
silently replace duplicates or allocate arbitrary nested data. Construct the
existing `app_core::UploadOutcome` from the parsed strings. Validate every
present URL even though only the primary URL is formatted in this slice.

This shape directly accepts `upload` and `upload-file` output. It deliberately
rejects `upload-images`/`upload-files` NDJSON, their `type`/`index`/`total` fields,
and their final summary. It also rejects nested `capture-task` output and S3
bucket/key results. Do not select the first line, discard trailing errors,
extract a nested object implicitly or fabricate a public S3 URL. Batch and
nested-result selection need a separate contract that preserves their completion
and partial-success semantics.

No output is emitted until the entire bounded input has ended and passed
validation. EOF immediately after incomplete JSON fails. Input ending after a
complete object may be accepted, but cannot prove that its producer succeeded
after writing it. A producer holding a pipe open can delay EOF indefinitely;
the read has no hard timeout, worker or polling loop. Ctrl+C/process termination
remains the explicit escape, as with current bounded credential stdin reads.

## URL and output semantics

Reuse `upload.rs`'s existing `validate_link` parser through a new narrow public
core method:

```rust
impl UploadOutcome {
    pub fn validate_links(self) -> anyhow::Result<Self>;
}
```

The method validates all three URLs and returns the owned canonical strings.
Require each supplied URL to contain 1–2048 UTF-8 bytes, no Unicode control
characters, U+2028/U+2029, or leading/trailing Unicode whitespace. Use the
existing HTTP(S), host and no-userinfo checks; then recheck each canonical
serialized URL is at most 2048 bytes. This second check is necessary because
canonicalization can expand Unicode or spaces into encoded bytes. HTTP links
are allowed because output is inert; HTTPS remains the upload transport default.
Do not resolve DNS, contact the URL, infer public reachability, remove query
tokens, or redact signed URLs from explicitly requested output. Errors are
fixed operation descriptions without input URLs, labels or parser payloads.
Leave existing upload validation behavior unchanged; this method is an explicit
postprocessing boundary, not a new policy for existing requests.

`format` succeeds with one newline-terminated JSON object and exit status zero:

```json
{"format":"markdown","text":"[Screenshot](<https://example.invalid/image.png>)"}
```

Exactly `format` and `text` are returned. `format` is `plain`, `markdown` or
`html`; `text` contains no appended newline. Write and flush JSON explicitly.
All validation errors exit 1, report a fixed message on stderr and emit no
success JSON. A stdout failure exits 1 and may leave partial JSON; it creates
no persistent output and does not imply a remote upload was undone.

Formatting is deterministic:

- **plain:** the canonical primary URL alone.
- **markdown:** `[LABEL](<DESTINATION>)`. Escape every ASCII punctuation
  character in the label with a preceding backslash. In the canonical URL,
  percent-encode literal ASCII whitespace, `<`, `>` and backslash bytewise with
  uppercase hex. Escape every literal ampersand as `&amp;` so CommonMark
  entity decoding preserves the canonical URL, including entity-looking
  query values. Preserve existing percent escapes and all other bytes.
  No image embedding, raw HTML, title or arbitrary template is generated.
- **html:** `<a href="URL">LABEL</a>`. Escape `&`, `<`, `>`, double quote and
  apostrophe as `&amp;`, `&lt;`, `&gt;`, `&quot;` and `&#39;` in both the URL
  attribute and label. This produces one anchor with no additional attributes.

JSON escaping is a final serialization layer and does not replace Markdown or
HTML escaping. The formatter does not run a markup renderer. The label and URL
must stay inside the one link generated by the selected format.

`qr` encodes the **canonical primary URL**, not a formatted Markdown/HTML string.
Reuse existing QR semantics: normal versions 1–40, error correction M, opaque
black/white RGBA, four-module quiet zone and four pixels per module. A valid
2048-byte URL fits the pinned normal version-40/M byte capacity (2331 bytes).
Keep the generator's defensive capacity error without truncating a URL; it is
not an expected failure for validated input with this pinned encoder.

PNG publication uses the existing atomic no-overwrite writer. Success emits
exactly the current QR metadata shape, without echoing the URL or path:

```json
{"format":"png","width":116,"height":116,"error_correction":"M"}
```

Dimensions depend on the encoded URL. Encode/flush/sync/publication failure
returns exit 1; known temporary-file cleanup follows the existing writer.
If publication succeeds but stdout fails, explicitly report that the QR PNG
was saved and remains. A crash can leave an unpublished `.part` file; a completed
PNG remains. No retry, rollback deletion or provider request follows.

## Implementation and resources

Create `apps/desktop/src/upload_results.rs` for strict argument/input decoding,
bounded formatting and the two command branches. Add the private module in
`apps/desktop/src/main.rs` and dispatch/help in `apps/desktop/src/cli.rs` before
GPUI startup. Add `UploadOutcome::validate_links` beside the existing struct in
`crates/app-core/src/upload.rs`, with focused unit tests and no request-path
change. The type is already exported; no extra core module or dependency is
needed.

In `apps/desktop/src/qr.rs`, extract the current code construction/pixel rendering
into `pub(crate) fn generate_pixels(text: &str) -> anyhow::Result<RgbaImage>`.
Both the existing QR generator and `link qr` call it, retaining current QR
limits/defaults. Keep the existing standalone QR command's public JSON schema
and image appearance unchanged. `link qr` performs its own checked result
write so post-publication failures have the exact behavior above. Do not route
through `qr::run` or introduce another QR encoder.

One operation owns at most 8193 input bytes and three bounded URL strings.
The label is at most 256 bytes; bound formatted `text` at 16 KiB before growth
and final JSON at 32 KiB including its newline. Those limits accommodate the
specified escaping while remaining hard output caps. Use checked appends or
precomputed capacity, not an unbounded template engine. Do not retain input or
formatted results after output.

QR rendering is at most 740×740 RGBA (2,190,400 bytes, about 2.09 MiB), plus the
existing bounded QR matrix/encoder and PNG writer scratch. It allocates no
source screenshot or GPU texture. No new dependency, thread, runtime, idle
action, file cache, network request, credential access or clipboard ownership
is added. Input/format/encode work is synchronous in the explicitly launched
headless process; native I/O and QR encoding have no hard mid-call cancellation
deadline. Existing [PLAN resource/privacy requirements](../PLAN.md#performance-contract--all-versions-and-features)
continue to apply.

## Focused verification and documentation

Create `apps/desktop/tests/upload_results.rs`. Test the actual executable with
synthetic stdin, isolated user-state paths, removed display/session access and
a test-only outer deadline. The fixture must close stdin after writing and
read both child output streams to avoid pipe deadlocks.

- Exact plain/Markdown/HTML results, omitted/default/custom Unicode labels,
  punctuation escaping, ampersands/quotes/brackets/backslash, existing percent
  escapes, query and fragment preservation, and normalization-expanded URLs.
- Valid primary-only `upload` shape and optional/null links from `upload-file`;
  malformed/duplicate/unknown/nested fields, bad UTF-8, truncated JSON, a second
  object/NDJSON, missing/empty/invalid URLs, credentials, non-HTTP schemes and
  canonical-length overflow. Check 8192-byte input acceptance with whitespace,
  rejection at 8193, label/output boundaries and no payloads in stderr.
- Invalid arguments/output collision fail before stdin is read, tested with a
  deliberately open input pipe. A valid object with the pipe held open must not
  emit early output; terminate that fixture deliberately without claiming a
  production read timeout.
- QR decode recovers the exact canonical URL using the existing independent
  `quircs` scanner; check opaque pixels, quiet zone, 2048-byte URL success, case-folded
  PNG extension, no-overwrite/broken-symlink races, atomic failure cleanup,
  source-result preservation and saved-file/stdout-failure reporting.
- Preserve existing QR generation tests. Add a focused loopback upload-to-link
  executable composition fixture using a generated file and existing local
  server patterns; check no second network request, unchanged file/profile and
  no clipboard/config/history side effects. Do not use real account URLs,
  credentials or remote services. Separate uploader and formatter exit statuses
  in the fixture; do not treat shell pipeline status as combined success.

Run formatting, optimized targeted core/link/QR tests, existing upload-result
regressions, Clippy and platform compilation. Then validate the frozen release
binary and package with an inert URL. Record elapsed time and peak RSS for that
small standalone operation separately from reference-hardware acceptance;
there is no reason to run a capture benchmark for this path.

Buildable sequence: add the core method and QR helper extraction with their
focused regressions; add CLI routing and isolated executable tests; then update
documentation and run frozen release, independent renderer and package checks.

Update `README.md`, `docs/qr.md`, `docs/file-uploads.md`, `docs/upload-batch.md`,
`docs/capture-task.md`, `docs/non-ui-status.md`, `docs/gap-analysis.md`,
`docs/non-ui-delivery.md` and `docs/goldfish-review-report.md` only for the actual
new command/support and completed evidence. Explain unsupported batch/nested
input next to examples. Keep custom templates, automatic copy, URL shortening,
new UI and complete after-upload workflow parity marked open.

Development observations (three interleaved runs on an inert URL): median peak
RSS was 10,928 KiB for Markdown and 12,360 KiB for a 180×180 QR output. Wrapper
times were 1.84/9.54 ms including process startup; GNU time rounded all durations
to 0.00 s, so these small samples do not establish timing budgets. The synthetic
QR used 129,600 RGBA bytes and encoded to 2,987 PNG bytes. Raw observations are
stored locally in `target/verification/link-performance.json`. This is not
reference-hardware, private-memory or native-desktop acceptance.

The maximum-length repeated-`x` URL fixture exposes a quircs scanner limitation:
ZBar and ZXing recover the exact 2048 bytes from the generated QR, while the
app's existing scanner reports a decode error. A second maximum-length fixture
decodes with quircs. Do not infer universal reader compatibility from successful
generation. The external verification script requires `markdown-it-py` and
ZBar (`zbarimg`) and tests both markup links and the dense QR explicitly.
