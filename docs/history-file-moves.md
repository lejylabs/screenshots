# Recoverable history file moves

Author: NewtTheWolf
Date: 2026-11-15
Status: Linux implementation verified; native macOS/Windows adapters added.
Native acceptance is tracked separately in [Library lifecycle](history-lifecycle.md).

Add `history-manage move DATABASE ID NEW_IMAGE`, `history-manage pending-move
DATABASE`, and `history-manage recover-move DATABASE OPERATION finish|cancel`.
Move one referenced PNG/JPEG/WebP image on the same filesystem without replacing another file.
This is an explicit physical-file operation; existing remove stays metadata-only.
macOS uses held POSIX descriptors with `renameatx_np(RENAME_EXCL)`. Windows uses
128-bit native identities, reparse-point refusal and no-replace write-through
rename. Their runtime/durability acceptance remains separate from Linux evidence.
Pending inspection stays portable. No UI or automatic recovery is added.

Move/recovery success prints `operation_id`, `capture_id`, `source_path`,
`destination_path`, `kind` and `status` (`moved` or `canceled`). Pending inspection prints
`pending: null` or an object with the same ID/path fields and no status. Paths
are exact UTF-8; inspection reads no image bytes and never migrates or creates
the database. Older supported schemas without this journal report no pending
move. Malformed/unsupported journal data fails explicitly.

Schema version 5 introduced one unresolved move per database. Version 6 extends
that same journal with the explicit [Library lifecycle](history-lifecycle.md);
ordinary move/recover-move semantics remain unchanged. Schema 7 also guards
SQLite-assigned capture IDs after insertion, preserving pending restoration
reservations. Version 8 adds native alias checks and full staging-path reservation,
with bounded identity preparation during explicit physical work. Persist the
operation ID, complete expected capture row, old/new absolute UTF-8 paths and
opened file identity/size/SHA256 before changing the filesystem. Capture-table
triggers reserve the pending ID and both paths against ordinary import/relink/
remove updates. A new move is refused until the pending operation is recovered.

Linux opens the source without following a final symlink, with nonblocking file
admission, validates actual image magic/dimensions and hashes at most 272 MiB
for PNG or 32 MiB for JPEG/WebP in 64 KiB chunks with a 30-second cooperative deadline. Retain the handle and source/dest
directory handles. Rename through `renameat2(RENAME_NOREPLACE)` on the same
filesystem, then verify destination identity/hash and sync both parent directories.
Commit the new capture path and remove the journal together, conditional on the
original row still matching. Clear the thumbnail reference; existing FTS triggers
update the search path. Preserve original capture ID, date and source category.
After the durable intent commits, the move acquires an immediate SQLite write
transaction and rechecks the complete journal/row before renaming. It holds that
transaction through native verification, synchronization and finalization, so a
second process cannot cancel the intent while the first process moves its file.
If cancellation wins before that transaction starts, the mover stops before any
filesystem mutation.

After preparing the durable intent, acquire a fresh IMMEDIATE transaction and
recheck the exact journal and row before any rename. Hold that write reservation
through native rename/verification/directory sync and final metadata update.
Recovery likewise holds IMMEDIATE from journal/state checks through its commit.
This prevents a concurrent cancel from deleting the intent under an active
mover. If recovery canceled first, the mover fails before renaming.
It also holds off other database writers during repeated hashing and native
calls; their existing busy timeouts can fail rather than wait indefinitely.

Failures before final commit retain the journal unless another explicit
recovery already completed. A failed/ambiguous SQLite commit does not prove
whether completion persisted; inspect both paths and the pending operation
before retrying.
Do not retry or automatically rename/delete anything. `finish` only updates the
row if the destination is the expected file, the old path is absent and the
original row matches. `cancel` only clears an unapplied intent when the expected
source remains, the destination is absent and the row matches. All ambiguous
states report conflict and preserve files/journal. Directory sync occurs again
before finishing recovery.
Recovery holds the same immediate transaction across journal and file checks;
finishing verifies and synchronizes the destination file through an opened handle
before synchronizing its parent directories. The complete original row and file
stamp are bounded JSON; the journal columns and payload must agree. Pending
inspection validates this stored structure without accessing the image paths,
creating a database or upgrading an older schema.

Successful move/recovery JSON contains `operation_id`, `capture_id`,
`source_path`, `destination_path` and `status` (`moved` or `canceled`).
`pending-move` returns those pending operation/path fields without a status, or
null when no operation exists. Native filesystem paths must be absolute UTF-8;
parent paths are resolved while the final component is checked without following
a symlink. The journal retains the exact original database row as well as the
resolved operation paths; a rebound parent alias causes recovery to fail closed.

No cross-device copy/delete fallback. Input paths are at most 4096 UTF-8 bytes;
PNG/JPEG/WebP bytes and dimensions retain their format-specific admission limits. Each fingerprint pass has
a 30-second cooperative budget; repeated verification may hash the same file
several times, so this is not a 30-second whole-move deadline. One file/hash buffer,
bounded journal and synchronous headless work add no resident threads/timers.
Linux source-path replacement can race with rename despite a held descriptor;
post-rename identity verification prevents a wrong database update but cannot
promise arbitrary external writers never move a replacement file. Parent path
changes and kernel stalls remain explicit native limits. This is recoverability,
not a filesystem-wide transaction or guaranteed resistance to malicious writers.
The first schema-5 delivery compared stored path strings. Current reservations
also use bounded native identity and indexed candidate checks for aliases and
hardlinks, as described in [Library lifecycle](history-lifecycle.md); they do
not rewrite other references or promise hostile-writer atomicity. The selected row's canonical
parent is recorded for file operations while its original path remains part of
the database comparison.

The current [format contract](capture-library-formats.md) extends the same
move/recovery protocol to JPEG/WebP. Fingerprints retain the actual kind,
destinations keep a matching extension, and legacy stamps default to PNG.
Source compressed bytes remain unchanged; moving never converts an image.

Core `history_file_ops.rs` is nested under history for private DB helpers and
`history_file_ops_native.rs` for native identity/open/rename/sync/hash. Modify
history.rs migration and guard SQL, storage.rs/history_query.rs version checks,
lib.rs exports, desktop history_manage.rs/help and focused core/executable tests.
Update history-management and delivery docs. Test crashes/failures at each durable
boundary, no-overwrite/cross-device/identity mismatch, pending reservations,
finish/cancel/repeated recovery, unchanged IDs/FTS and no automatic deletion.

Seven optimized orchestration fixtures pass with synthetic PNGs and temporary
databases: complete move/FTS preservation, pending reservations and cancellation,
post-rename finishing, conflicting files/tampered rows, invalid journal stamps,
read-only old-schema inspection and the prepare-versus-cancel race. Native
filesystem fixtures, executable checks and other platform acceptance are tracked
separately; these tests do not establish hostile-writer resistance or hardware
durability.

Two native filesystem fixtures and seven journal fixtures passed, as did three
actual executable tests covering move/history updates, destination collision
and invalid-command preservation. All operate only on synthetic temporary PNGs
and databases. `scripts/test-history-move.py` also passed against the release
binary: it injects a SQLite abort trigger only into its fixture database to
interrupt finalization after rename, verifies the durable journal, then exercises
explicit finish and cancel recovery. Exact bytes, inode, row metadata and FTS
references remain correct. At that first Linux delivery, Windows cross-compilation
and Clippy passed while non-Linux mutation was unavailable. The adapters added
later and their current checks are recorded in the lifecycle document. Whole-workspace and release results are recorded
in the delivery log after completion.

That first delivery's complete optimized workspace run passed 464 regular tests, with 51 opt-in
cases excluded and all doctests passing. Release artifact and recovery script
checks passed. Native Windows/macOS moves and hostile external-writer guarantees
remain outside this first implementation.
