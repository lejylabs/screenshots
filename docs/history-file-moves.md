# Recoverable history file moves

Author: NewtTheWolf
Date: 2026-11-15
Status: implemented, final regression/release checks in progress; Linux-only mutation.

Add `history-manage move DATABASE ID NEW.png`, `history-manage pending-move
DATABASE`, and `history-manage recover-move DATABASE OPERATION finish|cancel`.
Move one referenced PNG on the same filesystem without replacing another file.
This is an explicit physical-file operation; existing remove stays metadata-only.
Other operating systems reject move/recovery before any mutation until their
native rename/durability path is implemented. Pending-journal inspection remains
portable. No UI or automatic recovery is added.

Move/recovery success prints `operation_id`, `capture_id`, `source_path`,
`destination_path` and `status` (`moved` or `canceled`). Pending inspection prints
`pending: null` or an object with the same ID/path fields and no status. Paths
are exact UTF-8; inspection reads no image bytes and never migrates or creates
the database. Older supported schemas without this journal report no pending
move. Malformed/unsupported journal data fails explicitly.

Schema version 5 records at most one unresolved move per database. Persist the
operation ID, complete expected capture row, old/new absolute UTF-8 paths and
opened file identity/size/SHA256 before changing the filesystem. Capture-table
triggers reserve the pending ID and both paths against ordinary import/relink/
remove updates. A new move is refused until the pending operation is recovered.

Linux opens the source without following a final symlink, with nonblocking file
admission, validates PNG header/dimensions and hashes at most 272 MiB in 64 KiB
chunks with a 30-second cooperative deadline. Retain the handle and source/dest
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
PNG bytes/dimensions retain existing admission limits. Each fingerprint pass has
a 30-second cooperative budget; repeated verification may hash the same file
several times, so this is not a 30-second whole-move deadline. One file/hash buffer,
bounded journal and synchronous headless work add no resident threads/timers.
Linux source-path replacement can race with rename despite a held descriptor;
post-rename identity verification prevents a wrong database update but cannot
promise arbitrary external writers never move a replacement file. Parent path
changes and kernel stalls remain explicit native limits. This is recoverability,
not a filesystem-wide transaction or guaranteed resistance to malicious writers.
Reservations compare exact stored path strings; aliases or hardlinks in other
history entries are not discovered or rewritten. The selected row's canonical
parent is recorded for file operations while its original path remains part of
the database comparison.

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
references remain correct. Windows cross-compilation and Clippy passed; mutation remains
unsupported on Windows/macOS. Whole-workspace and release results are recorded
in the delivery log after completion.
