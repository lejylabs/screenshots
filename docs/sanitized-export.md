# Explicit history export metadata policy

`screenshots history-manage export DATABASE DIRECTORY POLICY ID...` exports
1–20 explicit capture-history references. `POLICY` is `original` or
`strip-metadata`. All IDs must exist before files are exported; the database is
read without migrations and is never changed by export.

`original` preserves stored PNG bytes, including metadata. `strip-metadata`
loads PNG pixels with the shared 32 MiB encoded/256 MiB decoded admission,
applies EXIF orientation and creates a fresh PNG without ancillary source
metadata. Original files and history rows remain unchanged. This is pixel
conversion to one RGBA8 still image: higher bit depths are reduced and animated
PNG input uses its decoded still image, without preserving animation. It is
re-encoding, not forensic erasure, color-profile preservation or a general
metadata inspector. The existing UI export still preserves bytes by default.
The original policy checks the PNG signature and streams at most 272 MiB after
that signature (272 MiB plus eight bytes in total); it does not validate the
complete image or decode its pixels. The stripped policy instead requires a
fully decodable admitted input and has the smaller encoded limit stated above.
This command still uses its separate 32 MiB loader and applies EXIF orientation.
The [saved-PNG admission path](saved-png-admission.md) used by history previews,
recopy, saved-image editing and other pixel re-exports instead accepts whole
files up to 272 MiB, streams a regular-file handle with a fixed readable extent,
and checks both native decoded bytes and RGBA before decoding. It continues to
ignore saved-image EXIF orientation. That new path's verification is pending;
it does not enlarge this command's stripped-export limit or alter the
byte-preserving export above. Decoder/conversion scratch remains additional
to the stated pixel admission, so neither path claims a total-memory bound.
On non-Windows systems the existing publication path requires same-filesystem
hard-link support. Unsupported filesystems fail rather than falling back to a
partial copy or overwriting a destination. Windows uses native no-replace move.

The destination must be an existing absolute directory after CLI path resolution.
Original basenames are retained; existing outputs and duplicate basenames are
reported as failures, never overwritten. Each output uses the same temporary
file, sync and no-replace publication as the existing exporter. Completed outputs
remain if another entry fails. JSON reports exported, failed, not_exported,
failures and metadata_policy; partial failures are a report, not an implicit
rollback. Output errors state that published files remain.
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
mid-call. Routine UI behavior and its default metadata policy are unchanged.

Tests use synthetic metadata-bearing PNGs to verify byte-preserving default,
ancillary metadata removal, exact output pixels/alpha, no overwrite, cancellation
and unchanged source/database bytes. Native platform and reference-hardware
performance acceptance remains separate.

An optimized development observation used one solid 3840×2160 PNG with a text
chunk, one warmup and five samples per policy. End-to-end process medians were
2.10 ms for byte-preserving export and 13.30 ms for metadata-stripped export;
maximum RSS observations were 14.63 MiB and 47.20 MiB respectively. The latter
does pixel decoding/encoding rather than a byte copy. This is a synthetic local
measurement, not a reference-hardware or native UI performance pass.
