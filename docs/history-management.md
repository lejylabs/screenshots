# Explicit history management

Status: headless metadata operations using the existing capture-history APIs.
The original metadata commands do not change image files. Explicit Linux
[journaled file moves](history-file-moves.md) now rename one referenced PNG
without replacing another file, with pending-operation inspection and recovery.

```sh
screenshots history-manage import DATABASE PNG [PNG...]
screenshots history-manage remove DATABASE ID [ID...]
screenshots history-manage relink DATABASE ID PNG
screenshots history-manage move DATABASE ID NEW.png
screenshots history-manage pending-move DATABASE
screenshots history-manage recover-move DATABASE OPERATION finish|cancel
```

Import accepts 1–100 PNG paths, resolves relative inputs against the invoking
directory and stores absolute UTF-8 references through the existing importer.
It checks PNG headers/dimensions and records metadata; this is not a full image
decode or proof that all pixels can be decoded. Existing paths are counted as
`existing`, new references as `added`. Individual rejected inputs are listed in
`failures` with their paths and error messages; `failed` reports their count.
This partial-success JSON is emitted with exit status zero, including when every
input was rejected. A database/transaction error instead fails the command and
rolls back its metadata changes. Explicit import may create and migrate the
requested database and its parent directory, even if all inputs are rejected.

Remove accepts 1–20 positive integer IDs and deletes only matching capture
metadata in one transaction. Repeated or missing IDs are harmless and the JSON
`removed` count is the actual number of rows deleted. No referenced image or
upload-history entry is removed. Invalid IDs/counts fail before opening storage.
Remove and relink require an existing regular database file rather than creating
a missing database. Existing supported schemas can be migrated by the mutation
API. Database paths ending in a symlink are rejected.

Relink accepts one positive capture ID and an existing PNG path. An indexed
lookup captures the current reference; the existing optimistic update rejects a
row removed or whose path/creation timestamp changed before the update.
Changes only to dimensions, file size or source are not version markers for this
existing optimistic check. It keeps the ID, original creation
time and capture/import source while replacing the path/dimensions/file-size
metadata and invalidating the old thumbnail reference. The original and new PNG
are unchanged. Linking to a path already referenced by another entry fails.
JSON includes the committed ID, absolute path, width and height. A failure to
read the row after a successful update explicitly reports that the relink was
committed. Concurrent later changes can affect the reread; the command does not
claim to lock a row after its transaction completes.

Results are bounded by their input limits. Input paths are at most 4,096 native
encoded bytes; input-error strings are capped at 2,048 UTF-8 bytes and indicate
truncation. JSON paths are rendered as lossy native text where required, labeled
with `path_encoding: "lossy-native"`; successful imported/relinked references
must still satisfy the existing UTF-8 storage requirement. IDs, user files and
error details are printed only in the explicit command result, not background
logs. Output write/flush failures report completed mutations truthfully rather
than imply a rollback.

## Resources and verification

These commands use one synchronous metadata operation in the explicitly started
headless process. Import reads headers rather than retaining decoded pixels;
relink adds one indexed lookup and a bounded reread. No new dependency, runtime,
thread, timer, queue, image cache or resident work is added. The existing SQLite
cache/busy-wait and mutation durability rules apply. Native filesystem/SQLite
calls do not promise cooperative cancellation; Ctrl+C terminates the process.

Actual-executable fixtures check partial imports, repeated references, exact
relink ID/dimensions, missing/conflicting IDs, atomic metadata removal, malformed
arguments, absent database handling, bounded reports, unchanged original/new
files and absent unrelated user-state writes. Optimized results are recorded
after execution. These operations do not establish retention, physical-file
deletion, native UI/library or reference-hardware performance acceptance.
