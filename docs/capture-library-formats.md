# Capture and Library format policy

Status (2026-11-17): integrated into the shared pipeline, CLI and existing UI
consumers. Local optimized verification passes, and consecutive fresh
source/document reviews 14/15 are clean on the recorded frozen checkpoint. Integration does not establish
native capture, account, hardware or performance acceptance.

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
Configured CLI export reports `metadata_policy: "configured"` with the resolved
`library_export_policy` object, alongside each output's `reencoded` and `lossy`
fields. Explicit Original/Strip requests retain their legacy report fields.

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
re-encoding JPEG would add a hidden lossy generation. PNG/JPEG/WebP sanitization
therefore uses the existing managed decoder, orientation and color conversion,
then emits PNG with PNG MIME and an explicit output report. A configured
conversion instead emits its explicitly chosen format from decoded pixels,
without metadata. Source files remain unchanged. Filename collisions such as
`a.jpg` and `a.png` are reported individually without replacement.
Converted outputs are still images using the decoder's default still frame;
lossless describes those encoded pixels. Preserve retains animation bytes.

Saved PNG decoding retains its historical raw EXIF orientation and default APNG
still image. PNG-to-PNG Preserve retains encoded bytes. The existing PNG
sanitizer's EXIF-orientation policy and 32 MiB input limit remain available through
its compatibility wrapper. Ordinary saved PNG load/preview and byte preservation
retain 272 MiB admission. Generic explicit conversion/sanitization uses the managed
PNG/JPEG/WebP decoder with its 32 MiB input bound and applies ICC conversion and
orientation before dropping metadata. Existing PNG-specific Save as and metadata
wrappers keep their prior pixel/byte contracts. New JPEG/WebP saved decode uses
the managed input pipeline and applies orientation consistently with admitted
dimensions.

```sh
screenshots formats show
screenshots formats set-capture '{"format":"jpeg","quality":90}'
screenshots formats set-library '{"mode":"convert","encoding":{"format":"webp_lossless"}}'
screenshots formats export NEW.json
screenshots formats import BACKUP.json
screenshots history-manage export DATABASE DIRECTORY configured ID...
```

`--config FILE` selects a private catalog for preference inspection/update or
backup commands, and for `history-manage export ... configured ID... --config FILE`.
All preference and configured-export fixtures pass explicit private paths on all
platforms; environment overrides alone do not isolate Windows Known Folders.
`formats validate FILE` checks an explicit backup without
writing. Headless history export still accepts explicit `original` and
`strip-metadata`; those choices do not consult the stored conversion policy.

## Shared integration contract

The registered policy and saved-image modules use the companion libwebp writer
extraction. It encodes into a caller-owned writer and retains the existing
external no-overwrite Save API and tests.

Existing PNG APIs remain compatibility wrappers. Format-explicit journal save
and byte-copy APIs choose a normal capture destination on the worker before
publication. The `CapturedImage.png_path` legacy field must never cause
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

JPEG/WebP capture and import share coherent Library consumers: import/relink
metadata and MIME, editor source loading, visible-page thumbnails, copy,
Preserve/Convert export, metadata sanitization, uploads' source kind and MIME,
and history move/transfer/trash/restore/purge. Native lifecycle fingerprints
must carry the actual kind with PNG default for legacy journals. Destinations
must preserve that kind, and owned staging/trash names must retain an appropriate
extension. Source files with misleading extensions must remain movable using
their admitted contents, without relabeling compressed data.

The separate policy belongs in the explicit settings/configuration backup and
restore command family. Its own export refuses replacement, and import rejects a
missing or malformed backup. No new UI controls are required by this slice.
Backup import/validation performs one required open and parses that retained
object; a missing or disappearing backup is an error. Optional normal catalog
loading alone may supply defaults for an absent file. Presence observation
retains an explicit Absent/Present distinction before comparing policy values.
The existing settings export includes a `capture_formats` section. Both sections
are validated from one bounded source snapshot before any apply. Legacy backups
without that section preserve the current catalog. Applying the two files is
explicit: a format-write error after the main-settings save reports the observed
catalog state and successful main-settings apply, including an ambiguous
durability warning when a rename succeeded before a directory barrier failed.
An absent catalog is reported as absent with built-in defaults, never as an
imported file merely because its effective values equal those defaults.

UI workers receive an explicit catalog context. Every test constructor owns a
private catalog path, and a missing test context fails closed instead of resolving
a user Known Folder. Capture saves, Library export, combined backup and restore
all use that context; backup accepts its supplied policy snapshot. Affected test
imports also persist main settings under that private context. Test-only shared
guards retain private folders through worker completion and remove only their
known files. Production resolves the existing user location only on an explicit
worker action; the resident field is one optional path, with no eager config I/O.

## Resources and verification

Policy is bounded scalar metadata. Streaming output, copy and hashing use 64 KiB
buffers. JPEG inspection adds at most one 65533-byte metadata buffer; WebP
inspection temporarily snapshots at most 32 MiB. A decode or conversion retains
one admitted source and one output representation under existing active-image
budgets. Sequential inspection and decode release their encoded snapshots rather
than retaining a second input. Nested encoder/journal buffering can hold two
64 KiB writer buffers. Explicit sanitized upload holds one PNG representation,
whose Vec grows in bounded chunks with actual capacity capped at 272 MiB.
The existing codec scratch allocations still count
toward active memory and are not claimed as measured process/RSS bounds. No
dependencies, threads, runtimes, services, timers, caches or worker queues are
added. Operation deadlines and cancellation are cooperative across codec calls;
native/codec blocking internals retain their existing lifecycle limitations.

Owned tests cover strict policy/quality/schema, absent-file defaults, persistence
and no-overwrite backups; real format magic and independent decoding of encoded
pixels/alpha; misleading suffixes; EXIF and APNG compatibility; default exact byte
copies; explicit conversion; truthful sanitized PNG output; genuine Display P3
ICC plus EXIF conversion against independently calculated LittleCMS oracles;
no-overwrite,
partial completion, cancellation, encoded bounds, changed sources and FIFO
refusal. The generic PNG decoder reuses the legacy admitted-extent reader and
adds bounded 64 KiB reads, seek bounds and cancellation/deadline checks before
codec I/O; tests append data after admission and check that the decoder cannot
read the appended extent. Missing trash destinations retain reservation through
known PNG/JPG/JPEG/WebP names for move, transfer, restore and capture-save; unknown
extensions remain unowned. Companion private tests cover journal-v2 recovery, native lifecycle,
combined backups and CLI commands. Optimized execution exposed ignored quality
fields on fieldless Serde variants; strict wire variants now reject them. A
fresh review also found positional arrays accepted at public JSON boundaries;
map-only visitors now retain strict duplicate/unknown-field rejection. The
expanded optimized policy test and three private retained-source upload tests
pass. The final optimized workspace passes 1,044 regular tests, zero failures,
with 71 explicit opt-ins excluded across 60 outer suites; nested child summaries
are counted once. All-target optimized Clippy, formatting, Windows GNU app/test
metadata compilation and default advisory checks pass without ignores.
Fifty-four independent private CLI requests pass preference/backup/schema,
magic/MIME, Preserve/Convert, sanitization and move/relink/transfer/trash/restore/
purge checks. Pillow/FFmpeg independently decode outputs; JPEG decoder
differences use bounded error rather than false exact lossy equality.
LittleCMS reproduces the committed color oracle byte-for-byte. Private fixtures do not access native user clipboard/capture,
user credentials or provider services.

The Library editor load now passes its actual operation cancellation through
encoded reads, decoding and managed color work, while foreground delivery keeps
the existing generation/token guard. Its private regression cancels from inside
the real loader checks and refuses a late result after a subsequent editor opens.

Filesystem uploads retain the admitted file identity/length through body
consumption. One duplicate handle pins the object for a metadata-only postcheck;
it is never read or sought while the transport uses the shared stream cursor.
Binary, multipart and JSON bodies use bounded reads, declared-length quotas,
cancellation and consumption accounting. Both success return paths verify full
admitted consumption and unchanged source version before providing a link or
allowing history success. Generic images, legacy PNG and arbitrary-file upload
paths share this protection. An early response or changed/canceled source yields
an error; the server may already have received bytes, and no rollback is claimed.
The extra active cost is one duplicate handle, bounded identity metadata and one
atomic counter per upload, released with the operation; no extra full readback,
runtime, queue or idle service is added. Private loopback regressions cover
unchanged sources, mutation after headers, cancel/signal and early 200 responses.

## Short optimized resource observation

A synthetic 3840×2160 journal fixture uses one shared 33,177,600-byte source.
Three samples per policy observed median encode plus durable publication of
11.163 ms for both legacy and policy PNG, 98.197 ms for JPEG quality 91,
11.950 ms for lossless WebP and 435.741 ms for lossy WebP quality 67.
Encoded sizes were respectively 205,947, 205,947, 1,146,838, 4,414 and
81,086 bytes. Whole-fixture peak RSS was 84,136 KiB. These are short synthetic
operation/process observations, not per-policy private allocations, GPU usage,
tail latency, native capture or reference-hardware acceptance. The existing
one-heavy-image admission and bounded writers remain; no idle service is added.
Raw records are in `target/verification/formats-wayland-integration/`.

Report correction (2026-11-17): configured export now identifies its resolved
Preserve/Convert policy instead of labeling converted bytes Original. Both
configured modes and unchanged explicit Original/Strip response fields pass
actual optimized CLI checks. The rebuilt execution source again passes the
1,044-test workspace and 54 independent private CLI requests. Resource
observations above belong to the earlier integration image; this correction
changes only bounded CLI result metadata, with no encoder/startup/lifecycle
code or resident resource change.
