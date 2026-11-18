# Complete custom-uploader execution

Author: NewtTheWolf
Date: 2026-11-15
Status (2026-11-17): the corrected execution source passed fresh independent
reviews 13/14 and the focused optimized/private HTTPS checks below. Native
platform, account and release acceptance remain separate.

Status (2026-11-19): the private .NET helper was replaced by an in-process Rust
interpreter (`crates/app-core/src/sharex_interpreter/`) that keeps the same
request/result JSON protocol, limits and refusal codes. The C# sources, SDK
lock, build script and packaged `lib/screenshots-sxcu` runtime are gone. Evidence
sections below dated before this note describe the former helper and are kept
as history; they are not claims about the Rust implementation.

This work implements the published ShareX custom-uploader request and function
inventory. It does not rename the current static importer “full compatibility”.
The current native JSON profile and object-valued Data extension keep their
existing behavior. New official `.sxcu` execution uses a separate admitted
profile/input path. UI design and modal input windows remain excluded; callers
supply explicit answers and receive output events.

## Reference and required behavior

The functional reference is ShareX commit
`d6a36956e9dc5d5ff445b1ee668a4bacb4a6aaf1` (develop, inspected 2026-11-15).
Reference source is kept only under ignored `target/reference/sharex`; no GPL
ShareX implementation is copied into the MIT application/helper.

All fifteen published fields are admitted: Version, Name, DestinationType,
RequestMethod, RequestURL, Parameters, Headers, Body, Arguments, FileFormName,
Data, URL, ThumbnailURL, DeletionURL and ErrorMessage. Enum names and their
published integer representations are supported. Destination flags are image,
text, file, URL shortener and URL sharing. HTTP methods are GET, POST, PUT, PATCH
and DELETE. Body modes are None, MultipartFormData, FormURLEncoded, JSON, XML and
Binary. Unknown profile fields, duplicate JSON fields and invalid enum/version
values fail before networking.

The image/file request contract supports multipart and binary streams. Text
supports all body modes, including multipart without a file part when
FileFormName is empty. URL shortener/sharing supports all modes except Binary
and sends multipart arguments without a file part. Unsupported input/body pairs
fail before credentials, a helper or networking. FileFormName and map keys are
literal; map values run the documented parser. RequestURL parses functions with
input/filename URL encoding; query parameters are then encoded separately.
URL, ThumbnailURL and DeletionURL run response functions. ErrorMessage runs
only on unsuccessful HTTP responses.

Data is a string: NameParser runs first, then case-insensitive input/filename
substitution with JSON/XML escaping appropriate to the body. Upstream does not
run its general function interpreter on Data; this distinction is preserved.
The existing object-valued native Data extension remains routed to the legacy
native importer, rather than being accepted as official ShareX Data.

Version missing or at/below 12.3.1 is unsupported in official execution. Versions
through 13.7.1 receive the documented old-dollar-syntax migration; modern versions
use brace syntax. Embedded request query parameters are extracted, with explicit
Parameters taking precedence, before old-template migration. Native legacy
imports retain their separate existing compatibility boundary.

## Interpreter strategy

An in-process Rust interpreter runs only for an explicit inspection, preparation
or response-interpretation stage, on the caller's upload worker thread. It
performs no upload, credential lookup, browser/clipboard operation, process
execution, arbitrary file read or script evaluation. Rust callers retain source
admission, HTTP transport, credential-origin checks and result/link validation.
No helper process, idle worker, timer or service is created.

A clean-room bounded scanner builds function nodes and evaluates nested calls
and backslash escapes. Function names and aliases are case-insensitive. Each
function preserves upstream minimum arity and parameter-selection behavior;
additional ignored parameters do not become a different function overload.
The complete function inventory is:

| Function | Behavior |
| --- | --- |
| filename, input | Supplied public filename/text; URL encoding in URL contexts only |
| response, responseurl | Response text and actual request URL; no redirect following |
| header | Case-insensitive response-header lookup with combined duplicate values |
| base64 | UTF-8 base64 of its first evaluated parameter |
| random | Uniformly select an evaluated argument, minimum two |
| select | Select an explicitly supplied nonempty argument; never silently choose |
| inputbox, prompt | Explicit supplied answer, with upstream title/default metadata |
| outputbox | Bounded explicit output event, returns an empty string |
| json | Json.NET-style SelectToken JSONPath: names (incl. `-`), quoted names, indexes, slices, wildcards, recursive descent, `?()` filters with `==,!=,<,<=,>,>=,=~,&&,\|\|` |
| regex | `fancy-regex`: numeric/named groups, lookaround and backreferences |
| xml | XPath 1.0 subset over `roxmltree` for SelectSingleNode/node.Value: axes, `@attr`, `text()`, positional and comparison predicates, common string functions |

Json.NET reading leniency is kept where profiles rely on it (comments, unquoted
or single-quoted names, single-quoted strings); constructors are refused. A
multi-result SelectToken expression or an array/object read as a scalar is an
error, `true`/`false` read as `True`/`False`, and floats follow .NET's
round-trip formatting. Known deviations from the former .NET engines: .NET-only
regex constructs (balancing groups, `(?'name')`, conditional groups) are
refused; catastrophic backtracking is bounded by a one-million-step backtrack
limit (`regex_timeout`) instead of a 100 ms timer, and patterns without
backtracking-only syntax run on a linear engine; XPath outside the documented
subset (unions, prefixed names, non-node results) is refused; calendar names
are English. XML forbids DTDs. Dependency cost: `fancy-regex` (plus `bit-set`/
`bit-vec`); `roxmltree`, `encoding_rs` and `getrandom` were already in the graph.

NameParser expansion covers its published date/time/month/week, increment/base,
random-character/GUID/word/emoji, dimension/title/process, user/domain/computer
and newline codes. Expansion observes escape boundaries where upstream does;
Data uses its own upstream NameParser ordering. `%rf(...)` remains disabled,
as it is in current upstream custom uploaders. Random word/emoji dictionaries
are independently authored or permissively licensed, not copied GPL resources;
selection is random with the same value kind, not a claim of identical random
vocabulary/distribution. Ambient user/domain/computer values require explicit
caller opt-in or supplied synthetic/context values. No imported profile alone
causes user identity, clipboard or filesystem data to be read.

Interactive calls use an answer map keyed by stable field/function position,
with the required title/choices available through an explicit inspection
command. A missing answer fails before the request; provided select answers
must name an offered nonempty choice. Input defaults are metadata, not implicit
consent. Outputbox yields a result event and may intentionally yield no link.
No modal UI or clipboard side effect is simulated. Returned response-derived
messages are bounded and sanitized; raw engine diagnostics are discarded.

## Admission, transport and privacy

A profile is a regular frozen opened file of at most 16 KiB. Ordinary file sources are regular files of at most 64 MiB; encoded image
inputs retain the existing MAX_CAPTURE_BYTES +16 MiB PNG upload ceiling. Files
stream from one admitted handle's initial extent with the shared 64 KiB
tracked-reader/cancellation rules. Every multipart argument has its own exact
byte quota and counter, including text/URL requests without a file part; 2xx
requires their bounded aggregate consumption as well as the file/data counter.
The same reqwest part metadata, ordering, lengths and UTF-8 bytes are retained.
One duplicate handle retains that exact
opened identity/version and length from before helper preparation until HTTP
completion. Before a successful result, the shared upload guard checks full
payload consumption and rechecks that retained object's version/length on the
blocking executor. Same-size in-place edits, shrinkage or growth prevent success
and later Task Link/QR publication; the remote object may already exist and is
never retried or deleted automatically. Memory PNG inputs keep their in-memory
path and use byte accounting without a file guard. Native filesystem calls
retain the shared hard-interruption limitations; no metadata work is added to
GPUI's foreground executor. Text/URL inputs are
at most 64 KiB. Public filenames are generated by default; callers may explicitly
supply a path-free filename to obtain original-name template behavior. A full
local source path never reaches the helper or remote service.

The separate runtime API admits a profile plus explicit execution options. An
optional existing native credential profile supplies only an origin-scoped
header. Its canonical HTTPS origin must equal the final interpreted request
origin before native lookup. Dynamic request origins cannot redirect an attached
credential to a new origin. The source `.sxcu` is never remembered, rewritten or
exported with credentials. Existing native JSON/header validation is unchanged.

Explicit transient `.sxcu` execution can consume literal authentication headers
already present in that caller-provided file; their values travel only through
private in-memory pipes and HTTPS. Their origin must be statically admitted,
or explicitly bound by the caller for a dynamic origin. They are not copied
into app settings, backups or debug output. Combining a literal authentication
header with a conflicting native header is rejected. Framing/proxy/Host header
overrides are rejected; body Content-Type customization remains supported.
Known authentication material is masked from explicit error/output events.

HTTPS is the default. A deliberate execution option enables configured HTTP,
without allowing credentials over HTTP. Final request URLs are absolute,
canonical and free of userinfo/fragment; result links must be bounded absolute
HTTP(S) URLs without embedded credentials. Schemeless RequestURL values receive the upstream HTTPS prefix before final
validation. No redirect, automatic retry, URL opening or follow-up deletion
occurs. HTTP
connect/request/response limits remain five seconds/sixty seconds/one MiB.
Failures after transfer identify that a remote object may remain.

## Bounds and isolation

The interpreter takes one request and returns one JSON result in memory.
No secret/frame config file, argv or child process exists. No user profile
source path or program path is accepted by the protocol.

Each stage is bounded to five seconds (checked during evaluation, together with
upload cancellation), 4 MiB request and 2 MiB result.
The request carries profile/context/answers and a base64 bounded response body,
never streamed file bytes. Template depth is at most 16, calls at most 128, intermediate expansion at most
2 MiB with a 16 MiB materialization budget, ordinary final fields at most 64 KiB, aggregate headers/arguments at most
64 KiB, encoded text body at most 512 KiB, result links at most 2048 bytes and
at most sixteen output events of 4096 bytes. Regex work is bounded by the
backtrack limit and JSONPath/XPath results by 65536 selected nodes. Refusals
produce fixed error codes, never expression/input/response/stack dumps.

Expected cost is operation-local JSON/text copies bounded above; no process
launch or packaged runtime. These active costs do not change the app's resident
idle budgets. Startup/RSS has not been re-measured for the Rust interpreter.

## Files and integration ownership

This implementation owns `crates/app-core/src/sharex.rs`,
`crates/app-core/src/sharex_interpreter/` (with its ported fixture tests),
`scripts/test-sharex-upload.py`, focused Rust tests and this document.
Existing `upload.rs` changes are limited to shared private source/header/result
helpers or official-vs-legacy routing where needed; do not replace native JSON
semantics. Existing OAuth/provider/Task process modules remain frozen.

The root owns app-core exports/dependency feature wiring, desktop main/CLI,
Task action integration, CI/native packaging/installer wiring and commits.
The concrete frozen API is `classify_upload_profile(profile, kind, options)`
returning Native or ShareX, and `ShareXPrepared::read/inspect_with_cancel`,
`upload_file` and `upload_png_bytes`. The last consumes finalized encoded PNG
bytes directly. New CLI `sharex inspect PROFILE [--options FILE]` or
`sharex image|file|text|url|url-sharing PROFILE INPUT_FILE [--options FILE]`
executes the full path. Existing `upload-file`/image CLI detects official SXCU
and routes it through that path, preserving the app-local legacy Data object
route. Task uses a frozen prepared ShareX profile/options. An explicitly saved artifact
streams in its chosen format; otherwise KindImage accepts the finalized
in-memory PNG directly, preserving the capture → recipe → upload path without
a save or disk round trip. No original pre-redaction path/frame is substituted.

## Verification gates

A table-driven golden fixture inventory covers every field, enum/body/input
combination, function/alias/arity, nested escape, current/old syntax, JSONPath
filters/recursive selections, .NET named/backreference/lookbehind/balancing
patterns, XPath axes/predicates/attributes and NameParser family. Invalid,
oversized, deep, catastrophic-regex, DTD/XXE, missing-answer, wrong-origin and
runtime-unavailable cases fail safely and before secret/network access where
applicable. Cancellation tests verify no blocked pipe/child survives.

The interpreter fixtures (ported from the former helper self-test) run as
ordinary `cargo test -p app-core` unit tests on every CI target; no separate
runtime build or packaging step exists.

Actual private loopback HTTPS tests execute binary multipart, binary request,
text form/JSON/XML, bodyless URL requests, all methods, response/header/link/
error functions, explicit prompts/output events and origin-bound native
credentials. A finalized redaction Task → save → SXCU upload → link/QR chain
verifies exact remote pixels, preserving the original. No real account,
user capture, clipboard operation or 1Password access is required.

Primary references: [ShareX item](https://github.com/ShareX/ShareX/blob/d6a36956e9dc5d5ff445b1ee668a4bacb4a6aaf1/ShareX.UploadersLib/CustomUploader/CustomUploaderItem.cs),
[functions](https://github.com/ShareX/ShareX/tree/d6a36956e9dc5d5ff445b1ee668a4bacb4a6aaf1/ShareX.UploadersLib/CustomUploader/Functions),
[Json.NET SelectToken](https://www.newtonsoft.com/json/help/html/SelectToken.htm),
[.NET regex](https://learn.microsoft.com/dotnet/standard/base-types/regular-expressions),
[.NET XPath](https://learn.microsoft.com/dotnet/api/system.xml.xpath.xpathnavigator.selectsinglenode),
[.NET release metadata](https://dotnetcli.blob.core.windows.net/dotnet/release-metadata/10.0/releases.json).

## Focused implementation evidence

The private self-contained Linux helper passes its native function/API fixture
suite (58 assertions). The final optimized Rust run passed eighteen tests (nine new pure, five process
and four existing legacy contract tests). The Rust suite covers all methods/body/input combinations, strict and
legacy admission, scoped authentication, frozen snapshot dispatch, ordered form
maps, bounded literal redaction, source extent and in-memory source accounting.
Process tests cover private anonymous input, blocked-input cancellation, the
five-second deadline, missing/changed/unlisted runtime members, portable/app/
owned-install layouts and a copied old process finding its retained runtime
after its launcher and mutable current state change.

`scripts/test-sharex-upload.py` passed 23 actual HTTPS requests using only local
synthetic services and a unique native Secret Service entry. It exercises five
methods, all six text modes, multipart/binary files, JSONPath filters, .NET
lookbehind extraction, XPath predicates, response headers/plain links, nested
functions, explicit prompts/output events, error parsing/redaction and rejected
wrong-origin/missing-answer requests. No ambient TLS keylog file was created.

Both finalized-image Task paths passed: recipe → in-memory PNG → upload without
Save, and recipe → explicit saved artifact → upload. Independent FFmpeg decoding
verified exact redacted RGBA pixels, remote bytes preserved the chosen saved
representation, QR scanning matched the returned canonical link, and the
original PNG remained unchanged. The helper never received the file's image
bytes or local source path.

The same fixture ran actual copied-portable and owned-installed CLI inspect,
then upgraded a private installation while an old full Task CLI was waiting at
an explicit test checkpoint. It deliberately made the newer runtime manifest
unavailable; the old process still uploaded through its retained runtime.
Fixture packages intentionally reuse the compiled old binary, so this verifies
runtime pinning, not new-release code acceptance. Debug information was stripped
only from a frozen test copy; the executable text-section hash was unchanged.
The tested copy's SHA256 is
`30c1b828e33a04eb013df5862ecce2665d377c1924163b1548707346199f52fd`.

A five-run native Linux pilot (warm filesystem, startup plus one response
JSONPath filter) measured 90 ms median and 52,248 KiB maximum process RSS. It is
not a physical cold-launch, whole-app idle or reference-hardware result. The
managed heap limit is 64 MiB; RSS/native overhead is reported separately. The
current bundle is about 83.5 MB unpacked, including runtime/dependency notices;
the app loads no optional runtime while idle.

Evidence lives in `target/sharex-helper-locked-build.log`,
`target/sharex-core-process-tests.log`, `target/sharex-retained-process-test.log`,
`target/sharex-e2e.log`, `target/verification/sharex-e2e.json`, and
`target/verification/sharex-runtime-cost.json`. Core-only Windows GNU metadata
compilation passed. A full desktop GNU check stopped in the unrelated missing
generated GPUI `shaders_bytes.rs`; it is not counted as passed.

Native execution of the packaged runtime remains required on every CI RID.
The vendor's current .NET OS support policy covers maintained macOS versions;
the inspected arm64 libcoreclr/libhostfxr load commands target macOS 12.0, but
that does not establish runtime acceptance on older macOS. The app's aspirational
minimum was not silently raised. Real account, signed macOS permissions,
reference-hardware and complete frozen-release gates remain separate.

The final core-lib Clippy attempt was blocked during compilation by concurrent
`history_reservations.rs` u64/FromSql edits; it is not reported as passed. SXCU
specific lints from the preceding run were corrected. The root owns the final
whole-workspace Clippy/native matrix after the shared source batches settle.

## Review hardening: authentication and response interaction

The auth-name classifier shares the native uploader's reserved query names and
also recognizes explicit auth/key/access-key variants, including X-Auth. The
helper applies the same policy after expansion. A statically known cleartext
request containing known authentication material is refused during profile
admission, before runtime admission, native-secret lookup or networking. A
resolved dynamic destination must still match its explicit HTTPS origin binding.
Known values are redacted from error/output events and refused in returned links.

Outgoing Cookie headers contribute the bounded value of every cookie pair to
that redaction set. Parsing splits on the first equals sign, preserving padding
or additional equals signs in values, and handles quoted values. Set-Cookie
contributes its first cookie value; public attributes such as Path/SameSite do
not become arbitrary redaction patterns. More than 128 outgoing pairs is a
configuration error before networking.

Preparation now structurally preflights interaction across every request and
response tree, including success-only and error-only branches. Missing answers
can no longer first be discovered after an upload. OPTIONS adds two explicit
bounded maps alongside answers:

```json
{"version":1,"answers":{"URL#0":"https://example.invalid/chosen"},
 "answer_indices":{"ThumbnailURL#0":1},
 "function_names":{"DeletionURL#0":"inputbox"}}
```

Inputbox/prompt always requires a value answer, even when a default is configured.
A select whose choices are known from literals or frozen request/context accepts
an offered value or an index, validated before the request. Unknown/dynamic
choices require an explicit answer_indices entry. Indices are zero-based after
empty choices are removed; no first-choice fallback exists. An index that is out
of range in the actual response is a bounded response-data failure, with the
remote-object uncertainty retained, rather than a new request for user input.

Computed names that can be proven from frozen input/context and pure functions
need no additional binding. An uncertain or response-dependent computed name
requires a function_names entry naming a published function. Preflight applies
that function's answer/arity contract and runtime asserts that the actual name
matches the binding. A bound inputbox therefore also requires its value answer
before transfer. This retains computed syntax without allowing a response to
introduce an unconfigured late interaction. Nondeterministic names that cannot
be proven to one value also require a binding.

Inspect exposes stable IDs and required answer kinds for all stages in
answer_contracts, including uncertain computed names. It reveals no auth values
or response bodies. Static prompt metadata retains its existing redaction;
response-dependent choices remain data and are not fetched by inspection.

Generic upload dispatch also retains extension-directed parsing: .sxcu selects
the official/legacy-SXCU classifier, while native JSON under that extension is
invalid. A ShareX document under .json stays a native-parser error. All decisions
use one opened byte snapshot; the profile is never reopened to choose semantics.

The review regression pass completed 21 optimized Rust/process/legacy tests and
92 native helper assertions. The unchanged CLI extension-selection regression
also passed. The expanded actual fixture completed 33 HTTPS requests, with
additional zero-transfer refusal cases. Cleartext auth aliases were tested with
an actual copied CLI that had no helper runtime, absent DBus and an unreadable
native credential reference, confirming refusal happens before those resources
are needed. Dynamic-origin auth was refused; bare cookie echoes were redacted
and reflected-cookie links were rejected. Every response field's missing prompt
or selection answer was refused before transfer, while explicit response indices,
computed bindings and provably frozen computed names executed successfully.

Latest evidence: `target/sharex-review-unit.log`,
`target/sharex-review-helper-build.log`,
`target/sharex-extension-cli-regression.log`, `target/sharex-review-e2e.log`, and
`target/verification/sharex-review-e2e.json`. The debug-stripped frozen test
copy's SHA256 was
`0f83f7db9edeaeb459b019734a6040b834a285eefc5ae009200922ed4cf9de9d`;
its executable text section was unchanged by stripping. Fresh review/native
release acceptance remains distinct from this synthetic regression evidence.

The separate Task-failure regression run also passed both generic upload and
explicit sharex_upload branches: each emits one sanitized action_failed result
containing the parsed ErrorMessage and outputbox event, and executes no later
command or complete event. This run completed 34 HTTPS requests with install
lifecycle deliberately skipped; the preceding 33-request run supplies that
installed-runtime evidence. Report:
`target/verification/sharex-review-task-e2e.json` (frozen copy SHA256
`ae2270dce3a841579bdbc4af4f1a27202b22c6b4f062679696a4e21e6bc8518d`).

## Query delivery and known-credential encoding projections

Literal query keys retain upstream wire spelling. Security classification uses
the name delivered by ordinary percent/form decoding: explicit Parameters keys
receive one projection; embedded RequestURL query keys account for upstream's
extraction decode followed by delivery. The final resolved wire query is checked
again before native-secret lookup/networking, and its known-auth values enter the
redaction set. Projection is byte-bounded and requires valid UTF-8 for query
names. It does not rewrite names, fold credential case or recursively guess
arbitrary encodings.

Link checks now search the decoded URL projection of known credentials. Percent
escapes accept either hex case. Path/fragment plus signs remain literal; only
query components apply form's plus-as-space rule. Event/ErrorMessage redaction
checks raw, percent and form views and maps matches back to original byte spans,
so UTF-8 text and untouched escaping remain intact. Inserted placeholders are
never processed again. This handles standard encoded versions of known values,
including reserved characters and literal plus, without hiding an unrecognized
public share token merely because a returned parameter is called token/key.

Post-transfer header, body, interpretation and validation failures retain the
remote-object uncertainty and explicit no-retry context.

Round-three regression verification passed 23 optimized Rust/process/legacy
tests, 100 native helper assertions and 47 actual HTTPS requests. Encoded auth
names were refused with no helper runtime and absent keyring; computed-origin
bindings were enforced. UTF-8/reserved/literal-plus credentials were redacted in
raw, mixed-case percent and form projections; responseurl, Task Link and QR
could not publish their known encoded values. An unrelated public share token
remained usable. This regression run skips the already verified install
lifecycle. Evidence: `target/sharex-round3-unit.log`,
`target/sharex-round3-helper-build.log`, `target/sharex-round3-e2e.log` and
`target/verification/sharex-round3-e2e.json`. Frozen debug-stripped test-copy
SHA256: `d1efcb6b1129616b3b066ccd3ae18ee17f9c571cbe4afd66ac98a9dc7d5f4acb`.

## Raw spelling and inspection metadata hardening

Known-credential link refusal checks the visible canonical URL spelling as well
as its decoded component projection. A literal plus or literal percent escape
must not erase an exact known-value match merely because decoding changes its
meaning. Neither raw nor encoded known values can proceed into Task Link/QR;
unrelated public share tokens remain valid.

Inspect collects known literal/frozen authentication values from the admitted
profile itself, including bearer/basic payloads, cookie pairs, delivered query
names and literal JSON/XML auth fields. It performs no native-secret lookup or
network request. Matching prompt titles/defaults and individual choice values
are redacted in raw/percent/form views; unrelated choices and defaults remain
visible. IDs, contracts, profile literals and runtime prompt behavior are
unchanged. The private native helper sanitizes its own inspection response,
rather than relying solely on a desktop output filter.

The final regression pass completed 24 optimized Rust/process/legacy tests,
108 native helper assertions and 55 actual HTTPS requests. It independently
checks raw plus/literal-percent links and Task suppression, inspection title/
default/choice reflection without DBus or a readable native secret, encoded
known values, and preservation of public metadata. Reports:
`target/sharex-round4-unit.log`, `target/sharex-round4-helper-build.log`,
`target/sharex-round4-e2e.log`, `target/verification/sharex-round4-e2e.json`.
Frozen test-copy SHA256:
`972b1a375852e9091fae36ddadddc756cc02d813f92ab6ed515dec0da524c780`.
This run skips the previously tested installation lifecycle and makes no new
real-account or native release-matrix claim.

## Configured body authentication and frozen inspection

One bounded helper classifier now handles configured and resolved body values
for inspection and preparation. Rust admission and the final pre-transfer guard
independently enforce the same known-authentication policy. It covers sensitive
JSON properties and their nested scalar values, XML sensitive elements,
attributes and descendants, and raw form-urlencoded Data under a custom
Content-Type. Cookie/Set-Cookie kind is inherited independently through nested
objects and arrays; ordinary descendant property names retain it, explicit
Cookie/Set-Cookie descendant keys override it and leaving a container restores
the parent kind. Cookie values keep every bare pair-value projection; Set-Cookie
keeps only its first cookie pair, so attributes do not become secret masks.
Bearer/Basic payload projections remain independent of generic auth field names.
XML uses the nearest explicit Cookie/Set-Cookie element ancestor for each
individual direct text value and ordinary attribute; an explicit Cookie or
Set-Cookie attribute overrides that kind for its own scalar only. Sibling
containers restore their inherited kind. Cookie header values are individual
XML text/attribute scalars; text from separate child elements is never joined
into a guessed header. Pure formatting whitespace is ignored for cookie text.
Unrelated fields remain ordinary data; no classifier rewrites the transmitted body.
Unused Data under FormURLEncoded remains unused, matching upstream behavior.
Binary text input receives the same resolved-body policy, while opaque image
and file streams are not decoded or passed to the helper for this purpose.

JSON numeric authentication retains both its parser-canonical value and the
original numeric lexeme (for example `100000` and `1e5`) in that same bounded
mask set. Each raw lexeme is tied to the actual numeric token in the parser's
sensitive context, including nested arrays and duplicate properties. Rust
advances a borrowed token cursor with every strict-JSON numeric visit; the helper
uses JsonTextReader's own token line/position, including its comment/newline
semantics. Public numeric occurrences do not create authentication projections
or consume mask slots merely because they equal an authenticated number.
Existing known-value substring redaction still masks canonical credentials
wherever returned text contains them. No raw lexeme is guessed from unrelated
string credentials. Json.NET constructor tokens (`new Name(...)`) are explicitly
unsupported in configured/resolved JSON-like request bodies, whether their field
is sensitive or public. They fail with a fixed sanitized error during inspection
or preparation, before native-secret lookup, output publication or networking.
Quoted text containing that spelling remains ordinary string data. Standard JSON
objects, arrays, duplicate-property scanning and nested numeric/scalar support
retain their existing behavior; response-selector semantics are unchanged.

Classification is limited to 512 KiB and depth 64. Each body-classifier scan and
each helper stage separately admit at most 128 unique mask values totalling
256 KiB, including raw/canonical and context-derived projections. Repeated
scalars are charged once, but each Cookie/Set-Cookie context is still projected.
The helper's emitted set is deduplicated and bounded before it reaches Rust;
existing independent native/response mask validation remains. Genuine
unique-value overflow remains a hard failure; XML has a 65,536-node bound. DTD declarations and
external entities remain forbidden. Configured literal body credentials trigger
the HTTPS and dynamic-origin requirements before helper/runtime or native-secret
lookup. Resolved body credentials are checked again before native-secret lookup
and networking, then retained for response links, events, ErrorMessage and Task
output without re-expanding time or random Data.

Inspection expands only frozen Data context. An explicitly supplied clock,
counter, title, process, dimensions or identity value can identify the known
credential produced by NameParser. Unavailable input/filename, random values,
ambient identity and an unsupplied clock use an internal unknown marker and are
not guessed. This keeps JSON/XML parsing valid where possible while retaining
other statically known sibling credentials. Inspection performs no keyring or
network operation, preserves the original profile literals, stable IDs and
runtime prompt semantics, and redacts matching titles/defaults/individual
choices without hiding unrelated choice data.

The body regression pass completed 25 optimized Rust/process/legacy tests,
126 native helper assertions and 77 actual HTTPS requests. Independent cases
cover XML elements/attributes, nested JSON, JSON Cookie pairs, raw form Data,
zero-transfer HTTP/origin refusal with no runtime or readable native secret,
bound-origin refusal before credential lookup, sanitized errors/events,
reflected-link refusal and later Task Link/QR suppression. Frozen JSON and XML
Data `%y` expansion with an explicit 2020 clock is redacted by actual CLI inspect
with absent DBus. Literal/unrelated data and unused FormURLEncoded Data retain
their behavior. Evidence: `target/sharex-body-unit.log`,
`target/sharex-body-helper-build.log`, `target/sharex-body-e2e.log` and
`target/verification/sharex-body-e2e.json`. Frozen debug-stripped test-copy
SHA256: `9b238f6e15cc865bcd89b992413297a8a76df8457db77a0ccc6942365e63ad66`.
This run skips the previously tested installation lifecycle and makes no new
real-account, other-platform or reference-hardware acceptance claim. The
classifier adds bounded operation-time parsing only, with no resident runtime,
background work or idle allocation.


The fifth (capped) fresh source review found one further parser-sniffing Major:
a raw form body whose first field began with `{`, `[` or `<` was ignored after
JSON/XML parsing failed. The Elephant fix falls through to form classification
on ordinary parse failure, while retaining hard depth/DTD/byte failures and
preserving successful JSON/XML interpretation. New regressions cover all three
prefixes, trailing data after a valid JSON prefix, bare Cookie values and
inspection defaults. Final correction checks passed 26 optimized Rust/process tests, 134 native
helper assertions and 94 actual HTTPS requests with CLI/Task/installed-runtime
coverage. Evidence: `target/verification/sharex-prefix-e2e.json` and
`target/sharex-prefix-{rust-tests,helper-build,e2e}.log`. The fifth review itself
was not a clean convergence report. At that historical checkpoint no further
fresh round had run; later actual reviews and corrections are recorded below.


The later shared Windows launcher now associates the interpreter's Job Object
atomically during native process creation, with an explicit inherited-handle
list. Its one-process/256MiB Job limits are installed before child creation;
startup fails closed if containment cannot be established. This supersedes the
older suspended-create/late-assignment gap. Native Windows acceptance still
requires the exact source and runtime artifact; source compilation is separate.


## Corrected parser, source and result integration (2026-11-17)

Actual source reviews 6–12 found and corrected raw numeric credential spelling,
Cookie/Set-Cookie ancestry in JSON/XML, parser constructor rejection, repeated
mask charging, incomplete multipart argument accounting and final opened-source
verification. Reviews 13/14 are independently source-clean on `2aea8f90…`.
A subsequent compiler-only change renamed two conflicting SelfTest loop variables;
production bytes were unchanged, and the corrected helper compiled and passed.
The frozen reviewed predecessor, test-only delta and exact applied patch are in
`target/verification/sharex-review6-fixes/`. No earlier finding round is called clean.

Actual optimized verification passes 33 SXCU/process/legacy Rust cases, three
shared native-source regressions and 11 executable Link/QR/result cases. The
pinned self-contained helper passes 357 assertions. The full executable/private
HTTPS fixture passes 137 requests across all five supported HTTP methods, six
text body modes, text/shortener/sharing multipart, actual Task and sealed raw
schedule execution, link/QR, copied/installed runtime and running-old-process
upgrade checks. Source changes after headers—including same-length and growing
files—produce no successful publication or later Task Link/QR/retry. The fixture
uses its own D-Bus and unlocked synthetic collection, generated CA/loopback
servers and redirected user-data paths. No existing keyring entries, user
clipboard, display, real provider account or remote upload service are used.
The separately extracted test provider/libraries were checked against official
repository hashes and Arch public-key signatures; nothing was installed globally.

The tested debug-stripped copy preserves the original `.text` section and has
SHA256 `d36af72b6deace110f77c111f7d6f85d6fa2f70747f8c194bd37f68607b9419a`.
Optimized all-target workspace Clippy, Windows GNU metadata and formatting pass.
Logs, source/runtime identities, initial test compiler failure and successful
results are in `target/verification/sharex-review6-integration/`. Native
Windows/macOS runtime and resource acceptance are not inferred from these Linux
checks. Standalone result memory and limits are in [the result contract](upload-results.md).
