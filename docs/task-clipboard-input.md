# Explicit saved-Task clipboard sources

These source routes are separate from the `clipboard` inspection/export commands
and Task clipboard **output** ownership. They add no controls or final UI design.

```json
{"version":1,"source":{"type":"clipboard_image","allow_private_source":true},"actions":[{"type":"save","path":"final-{uuid}.png"}]}
```

A clipboard source requires the explicit `allow_private_source: true` field.
Missing/false consent is rejected during preflight. A task author thereby
explicitly authorizes reading the **current private clipboard on every run**,
including future hotkey and scheduled invocations. The source declaration is
sealed with the rest of a scheduled profile. Changing it requires the existing
explicit schedule replacement. Clipboard contents are deliberately not part of
the configuration seal, and are never read to validate, seal, register a hotkey,
add a schedule, wait for a due time, or recover a missed occurrence. Existing
scheduler missed-run, uncertain-run and no-retry rules remain unchanged.

A Task reads its source exactly once after bounded profile/action preparation
and the initial cancellation/deadline check, before recipes/actions. An image
request uses the existing isolated `ClipboardImage` operation. Text/file metadata
uses one isolated `ClipboardInspect` request for the requested kind only. Changing
the clipboard later does not change the admitted input. Cancellation before the
read produces no native request; acquisition failure produces no output/upload.
The existing worker deadline, containment and per-platform availability apply:
Automatic clipboard reads on Wayland remain explicitly unsupported by this
adapter. No X11 fallback is inferred on a Wayland session. Native permission,
contention and delayed-rendering acceptance is still platform work.

Supported source shapes are:

- `clipboard_image`: one bounded RGBA frame in the existing image pipeline.
  Recipes run before save/copy/upload. No PNG/disk round trip is introduced.
- `clipboard_text`: bounded UTF-8 text, at most 64 KiB, with no NUL. It supports
  `sharex_upload` with KindText (the default for this source), using an official
  `TextUploader` SXCU profile and optional explicit options. No image recipe,
  image save/copy, native generic upload, provider/S3 upload, URL inference,
  text-to-image conversion or implicit file creation is admitted. Link/QR/copy-link
  still require an earlier confirmed upload. Text that resembles a URL remains
  text; URL shortener/sharing modes require their existing explicit input routes.
- `clipboard_file` additionally requires `kind: "image"` or `kind: "raw_file"`.
  It requires **exactly one** file reference from the bounded native file-list
  reader. Empty/multiple offers fail together; no first-file guessing, truncation
  or multi-file task fan-out occurs. Use image mode for bounded decoding, recipes
  and finalized-image outputs; use raw_file mode for unchanged file transfer via
  the existing upload/provider/S3 paths. The native encoding must match the host,
  the authoritative path must be absolute, and an opened regular-file identity
  is captured before any action. Display strings are never converted back into
  paths. The identity-assisted image loader compares the admitted metadata against
  its opened handle before reading, retains that same handle through decoding,
  and compares it again before returning pixels. Observable replacement/mutation
  fails before an image can reach an action. The existing raw-transfer guard
  remains unchanged. Neither route silently retries or invents remote rollback.

Example explicit text transfer:

```json
{"version":1,"source":{"type":"clipboard_text","allow_private_source":true},"actions":[{"type":"sharex_upload","profile":"paste.sxcu"},{"type":"link","format":"plain"}]}
```

These checks compare native file identity, length and observed version metadata
(including the platform's modification/change stamps). They do not create an OS
immutable snapshot or lock out another writer. A hostile writer that changes
bytes and restores every observed metadata field before the post-read check, or
a filesystem whose metadata granularity hides a change, can evade those checks.
A retained handle prevents a path replacement from switching the object being
decoded during that read; it does not promise a coherent immutable version under
such writes. Pixels are independent in memory once decode returns. A changed
source failure returns no pixels and therefore cannot proceed to save/upload.

Clipboard acquisition neither writes history nor changes clipboard ownership.
Copy actions retain their existing explicit hold/host ownership behavior. Text
stays in operation-owned zeroizing storage and is released before post-completion
clipboard ownership waits. No plaintext temp file is created. The in-memory SXCU
text API accepts KindText only and preserves the existing six bounded text body
modes, upload consent, response limits, credentials, cancellation and uncertain
remote-object/no-retry rules. Imported-profile diagnostic events and error text
are suppressed on that private-text route so an echoing profile cannot print the
source through those fields. Confirmed public links remain explicit action data;
the existing result/authentication policy still applies. No native clipboard
payload or path is explicitly logged by the source admission code.

Resource cost is one existing native operation and its bounded reply. Images keep
existing 32 MiB encoded/256 MiB decoded admission (and existing bounded Windows DIB
allowance). A text Task retains at most 64 KiB source text plus one at-most-64-KiB
zeroizing upload input clone during each sequential upload; the existing helper
JSON/text expansion/body/response bounds remain additional costs. File-list
metadata keeps the existing 32-reference/64-KiB native-byte bound, then retains
one path (at most 4096 native bytes, at most 8192 bytes of PathBuf backing), one
identity and existing bounded image or streaming-file state. Windows authoritative
conversion additionally allocates at most 4096 bytes of transient UTF-16 units. Native format/display-string/packet allocations retain their
existing additional ceilings. No runtime, dependency, production language, thread,
queue, cache, watcher, polling timer or idle access is added. CPU/RAM/GPU/latency
acceptance still requires optimized native measurements against PLAN targets.

The HELD source includes pure native-path/consent tests, an isolated synthetic
executor fixture covering validation/sealing without reads, current image/text
sampling, exact alpha, cancellation and file identity refusal, and an opt-in
private HTTPS fixture covering actual Task → in-memory text → SXCU for all six
body modes. The tests do not touch a user's clipboard or keyring. This HELD slice
has **not been compiled or executed**; it is not implementation/runtime acceptance.

Revision 2 also includes a shared-loader regression that rewrites a same-size BMP
on the same native object after its first 64 KiB snapshot chunk. It forces a
distinct observed mtime without sleeps, checks that decoding itself remains valid,
and requires the post-consumption identity check to refuse the result. The source
file handle remains alive through decode and the final comparison. This adds one
post-decode metadata comparison for identity-assisted imports; borrowing the file
introduces no duplicate handle, encoded copy, raster, cache, timer or runtime.
This regression and every revision-2 check remain uncompiled/unrun.
