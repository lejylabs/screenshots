# Capture and Library format policy

Status: implementation staged in owned modules; registration, shared pipeline
integration and optimized checks are pending. This document does not claim the
running application already honors the new policy.

The capture policy is separate from the existing Save as preference. An absent
`capture-format.json` keeps normal capture PNG and Library export byte preserving.
The file is a strict versioned JSON object, at most 16 KiB. It has no secrets,
pixels or network destinations. Explicit read/write/backup/import run on the
existing blocking worker; no startup watcher or idle I/O is introduced.

```json
{
  "version": 1,
  "capture": { "format": "png" },
  "library_export": { "mode": "preserve" }
}
```

Capture accepts `png`, `jpeg` with an integer `quality` from 1 through 100,
`webp_lossless`, or `webp_lossy` with explicit quality from 1 through 100.
Lossless policies reject a quality field. Unsupported fields, duplicate fields,
missing required quality and unsupported versions fail before capture output.
Library export accepts `preserve` or
`{"mode":"convert","encoding":{"format":"jpeg","quality":90}}`.
Conversion is an explicit new encoding; quality 100 does not make JPEG or lossy
WebP lossless. Existing UI Save as preferences retain their established meaning.

Every new encoded capture must have a matching extension, actual encoded kind
and history MIME. JPEG refuses nonopaque pixels before journal reservation; it
never discards alpha or guesses a background. PNG and lossless WebP retain RGBA.
Lossy WebP explicitly changes RGB and keeps the encoder's exact alpha policy.
All encoders receive finalized pixels without source metadata, use the existing
single heavy-image admission and write once into the owned staging object.
Encoding errors retain unsaved source pixels and cannot publish or index a file.

JPEG and WebP output and later saved input are capped at 32 MiB encoded bytes.
PNG retains its existing 272 MiB encoded bound. Decoded images retain the 256 MiB
limit, including actual caller-owned raster capacity. Lossy WebP retains the
existing tighter 64 MiB input limit and 16383-pixel axis limit; lossless WebP
allows axes through 16384, and JPEG through 65535. Encoded output is rejected
before publication when it exceeds its format's limit.

Saved-image inspection uses magic independently of the source extension and
admits dimensions without pixel decoding. JPEG header inspection uses one
65533-byte metadata buffer and at most 4096 segments; WebP preflight examines
the existing bounded container snapshot and every advertised frame. Decode
errors can still occur after header admission. The retained file is checked
before and after consumption; concurrent growth cannot bypass byte admission.

Library default export copies the exact admitted encoded bytes. It preserves
JPEG coefficients, WebP compressed payload, PNG/APNG ancillary chunks and all
originals; it never adds a hidden lossy generation. Misleading source suffixes
are corrected on the new output using actual magic. Existing files are never
overwritten. Selected-page export remains 1–20 files with partial completion
and cancellation reported per source.

Metadata stripping combined with Preserve creates a correctly named lossless
PNG. Removing ICC or EXIF directly can change visible color or orientation;
re-encoding JPEG would add a hidden lossy generation. JPEG/WebP sanitization
therefore uses the existing managed decoder, orientation and color conversion,
then emits PNG with PNG MIME and an explicit output report. A configured
conversion instead emits its explicitly chosen format from decoded pixels,
without metadata. Source files remain unchanged. Filename collisions such as
`a.jpg` and `a.png` are reported individually without replacement.

Saved PNG decoding retains its historical raw EXIF orientation and default APNG
still image. PNG-to-PNG Preserve retains encoded bytes. The existing PNG
sanitizer's EXIF-orientation policy and 32 MiB input limit remain available through
its compatibility wrapper. Ordinary saved PNG load/preview and byte preservation
retain 272 MiB admission. New JPEG/WebP decode uses the existing bounded managed
input pipeline and applies orientation consistently with admitted dimensions.

## Shared integration contract

Register the policy and saved-image modules only with their companion libwebp
writer extraction. The extraction exposes encoding into a caller-owned writer;
it must retain the existing external no-overwrite Save API and its tests.

Keep existing PNG APIs as compatibility wrappers. Add a format-explicit journal
save and byte-copy API, and choose a normal capture destination on the worker
before calling it. The `CapturedImage.png_path` legacy field must never cause
PNG bytes to be written under a selected JPEG/WebP suffix. Existing clipboard
output continues from in-memory finalized pixels; generic saved-file clipboard
copy decodes the actual source kind only when no pixel consumer remains.

Journal payload version 2 records the encoded format. Version 1 remains PNG;
default/omitted format is accepted only consistently with that version. Both
target suffix and ready/recovery content magic must match the journal format.
Fingerprint and native object/parent proofs remain authoritative through
publication and recovery. The indexed MIME comes from the admitted encoding,
with no database migration merely for MIME. Existing v1 PNG intents serialize
compatibly so exact stored-payload guards keep working.

Do not enable JPEG/WebP capture or import until Library consumers are coherent:
import/relink metadata and MIME; editor source loading; visible-page thumbnails;
copy; Preserve/Convert export; metadata sanitization; uploads' source kind and
MIME; and history move/transfer/trash/restore/purge. Native lifecycle fingerprints
must carry the actual kind with PNG default for legacy journals. Destinations
must preserve that kind, and owned staging/trash names must retain an appropriate
extension. Source files with misleading extensions must remain movable using
their admitted contents, without relabeling compressed data.

The separate policy belongs in the explicit settings/configuration backup and
restore command family. Its own export refuses replacement, and import rejects a
missing or malformed backup. No new UI controls are required by this slice.

## Resources and verification

Policy is bounded scalar metadata. Streaming output, copy and hashing use 64 KiB
buffers. JPEG inspection adds at most one 65533-byte metadata buffer; WebP
inspection temporarily snapshots at most 32 MiB. A decode or conversion retains
one admitted source and one output representation under existing active-image
budgets. Sequential inspection and decode release their encoded snapshots rather
than retaining a second input. The existing codec scratch allocations still count
toward active memory and are not claimed as measured process/RSS bounds. No
dependencies, threads, runtimes, services, timers, caches or worker queues are
added. Operation deadlines and cancellation are cooperative across codec calls;
native/codec blocking internals retain their existing lifecycle limitations.

Owned tests cover strict policy/quality/schema, absent-file defaults, persistence
and no-overwrite backups; real format magic and independent decoding of encoded
pixels/alpha; misleading suffixes; EXIF and APNG compatibility; default exact byte
copies; explicit conversion; truthful sanitized PNG output; no-overwrite,
partial completion, cancellation, encoded bounds, changed sources and FIFO
refusal. They have not yet been compiled or run. Shared journal recovery,
lifecycle, CLI, UI-consumer and optimized performance checks remain pending;
native user clipboard/printing/capture, user credentials and provider services
must not be touched by these private fixtures.
