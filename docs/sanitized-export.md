# Explicit history export metadata policy

`screenshots history-manage export DATABASE DIRECTORY POLICY ID...` exports
1–100 explicit capture-history references selected across pages. `POLICY` is
`original`, `strip-metadata` or `configured`. Configured export also accepts
`--config FILE` for an explicit private format catalog. All selected IDs must
exist before export; the database is read without migrations and is unchanged.

`original` uses Preserve: exact admitted PNG/JPEG/WebP bytes, including source
metadata and animation, without pixel decoding. Admission inspects actual magic,
dimensions and the retained regular file. The whole-file encoded ceiling is
272 MiB for PNG and 32 MiB for JPEG/WebP; the signature is included in that limit.
Decoded-dimension admission remains 256 MiB. This is not a promise that a
preserved file's entire pixel stream is decodable.

`strip-metadata` uses the generic managed decoder with a 32 MiB input ceiling,
applies supported ICC/color conversion and EXIF orientation, then writes one
metadata-free RGBA8 still PNG with `.png` extension and PNG MIME. Higher bit
depths are reduced and animated input uses the decoder's still frame; animation
is retained only by Preserve. This is explicit re-encoding, not forensic erasure
or a guarantee that visible pixels contain no sensitive information.

`configured` reads the separate Library policy. Preserve copies admitted bytes;
Convert decodes with managed color/orientation and encodes the explicitly chosen
PNG, JPEG or WebP policy, including explicit lossy quality where requested.
JPEG rejects alpha rather than inventing a background. Output reports identify
actual format, re-encoding and lossiness. Original files and rows stay unchanged.
See the [capture/Library format contract](capture-library-formats.md).

The narrow `load_saved_png`, PNG Save-as and PNG metadata-wrapper APIs retain
their compatibility semantics. They are not the current generic CLI export
consumer map. Ordinary saved-PNG loading still admits 272 MiB and keeps its
historical EXIF policy; generic sanitization/conversion uses the 32 MiB managed
pipeline. Decoder/conversion scratch remains additional to pixel admission;
neither is a total-process memory or hard native-call deadline guarantee.
Publication keeps exclusive staging and no-replace behavior; unsupported native
filesystem semantics fail without overwriting a destination.

The destination must be an existing absolute directory after CLI path resolution.
Source stems are retained with extensions matching the actual output kind;
misleading source suffixes are corrected. Existing outputs and collisions are
reported as failures, never overwritten. Each output uses the same temporary
file, sync and no-replace publication as the existing exporter. Completed outputs
remain if another entry fails. JSON reports exported, failed, not_exported,
failures, metadata_policy and per-output path/format/reencoded/lossy fields; partial failures are a report, not an implicit
rollback. Output errors state that published files remain.
Legacy `original` and `strip-metadata` requests keep their existing
`metadata_policy` values. Configured export reports `metadata_policy: "configured"`
and `library_export_policy` containing the exact resolved Preserve or Convert
policy. A configured conversion is never labeled byte-preserving Original.
Each failure has `path` and `error`; errors are truncated to 2,048 UTF-8 bytes.
`path_encoding` is `lossy-native`. A completed report exits zero even when every
file failed; inspect the counts. Invalid arguments/storage lookup fail with
exit one and no report. This matches the existing partial export convention.

Processing is sequential with one decoded image plus encoder scratch at a time;
no thumbnails, undo history, permanent cache or new runtime is retained.
The loader can temporarily hold another full-sized representation during
orientation or color-type conversion, so peak decoded memory can reach two
admitted image buffers. This is not a single-buffer peak-memory guarantee.
The image loader rejects nonregular inputs and uses nonblocking opens on Unix
before checking the opened handle, so an obvious FIFO cannot stall admission.
Native filesystem calls can still stall independently of the decoder deadline.
Cancellation is checked before decode, after decode, between outputs and before
publication, with a cooperative 60-second per-file deadline. Synchronous decode/encode/filesystem calls are not interruptible
mid-call. The existing UI uses its configured Library Preserve/Convert policy; built-in
Preserve remains the default, with no new controls.

The original PNG compatibility tests use synthetic metadata-bearing PNGs to verify byte-preserving default,
ancillary metadata removal, exact output pixels/alpha, no overwrite, cancellation
and unchanged source/database bytes. Native platform and reference-hardware
performance acceptance remains separate.

A historical PNG-only checkpoint observation (2026-11-15) used one solid
3840×2160 PNG with a text chunk, one warmup and five samples per policy. End-to-end process medians were
2.10 ms for byte-preserving export and 13.30 ms for metadata-stripped export;
maximum RSS observations were 14.63 MiB and 47.20 MiB respectively. The latter
does pixel decoding/encoding rather than a byte copy. This is a synthetic local
measurement, not a reference-hardware or native UI performance pass.
