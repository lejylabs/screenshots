# Explicit Library file lifecycle

Author: NewtTheWolf
Date: 2026-11-15
Status: implemented with local Linux/private verification. Native Windows/macOS
durability and physical acceptance remain separate. No UI redesign.

The existing [paged queries](history-queries.md), [reference audit](history-audit.md)
and [metadata/export commands](history-management.md) remain available. This
family adds physical trash, restoration, permanent removal, explicit retention,
cross-volume transfer and native move adapters. None runs at startup or while
idle. Read-only maintenance never starts deletion or repair.

```
screenshots history-manage move DATABASE ID NEW_IMAGE
screenshots history-manage transfer DATABASE ID NEW_IMAGE
screenshots history-manage trash DATABASE DIRECTORY ID...     # 1–100 IDs
screenshots history-manage trash-list DATABASE [CURSOR]
screenshots history-manage restore DATABASE TRASH_UUID [NEW_IMAGE]
screenshots history-manage purge DATABASE TRASH_UUID...        # 1–100 IDs
screenshots history-manage pending-move DATABASE
screenshots history-manage recover-file DATABASE OPERATION finish|cancel
screenshots history-manage remove-missing DATABASE ID...       # 1–100 IDs
screenshots history-manage retention DATABASE POLICY.json preview|apply
screenshots history-manage index-identities DATABASE [CURSOR]
```

Physical operations require an existing regular database, absolute UTF-8 paths,
PNG/JPEG/WebP magic/dimension admission and a bounded fingerprint. Images are never
decoded. Missing/modified/unavailable files, destination collisions, final
symlinks/reparse points and ambiguous state fail explicitly. A fingerprint
proves bytes observed at admission, not that a file never changed beforehand.
Trash is reversible; permanent purge requires the identity and bytes recorded
when trashed. Paths belong in user-controlled storage; this is cooperative
ownership and recovery, not isolation against hostile simultaneous writers.

## Bounded selections and missing files

Metadata removal and export accept 1–100 IDs selected across query pages.
Removal commits one transaction and never removes referenced images. Export
preflights IDs and reuses the existing 20-source primitive sequentially.
Original/strip-metadata and no-overwrite rules are unchanged; earlier published
files survive later failure. There is no unbounded selection/full-Library scan.

`remove-missing` removes only selected metadata whose final path is still absent
when checked under one immediate transaction. Existing files, final symlinks,
directories, permission errors and ambiguous checks refuse the whole selection.
It never opens/deletes images. A missing reference may mean an unavailable
drive; the command does not discover why it is absent. Audit remains read-only.

## Trash and restoration

Choose an existing trash directory on the source filesystem. This is an explicit
application trash location, not the OS Recycle Bin. Each file receives a fresh
`.screenshots-trash-UUID` name with the admitted format extension. No global trash directory, marker scan or
automatic file search is installed. Same-volume rename retains native source
permissions/ACLs; choosing a shared folder does not create a private vault.
Cross-volume relocation uses the separate explicit `transfer` command.

Schema version 6 adds bounded indexed trash metadata: original capture row/path,
native identity, size, SHA256 and UTC trash time. The singleton durable intent
reserves the capture ID and source/destination paths before rename. A fresh
immediate transaction rechecks the entire intent/row, performs rename and native
verification/synchronization, then atomically removes the capture reference and
inserts trash metadata. Failure leaves intent for explicit recovery. Existing
capture triggers keep FTS consistent. Schema version 7 adds an AFTER INSERT
reservation guard, so SQLite-assigned IDs from ordinary capture/import insertion
cannot claim a pending restoration ID. The existing BEFORE guards still reject
explicit IDs and reserved paths. Already-created version-6 databases receive
this guard through a new migration, preserving their existing rows.

Schema version 8 extends reservation guards to the transfer staging path for
its whole journal lifetime. Import, relink and path updates cannot adopt a
staging file. Before publication/removal and again before finalization, native
checks reject capture references to source, destination or stage, including a
reference created by a legacy version. Resolving that conflicting metadata is
explicit; recovery preserves the referenced file and intent meanwhile.

Reservations use opened native identities, not SQLite BINARY/ASCII NOCASE as a
filesystem-equivalence claim. Unix compares device/inode; Windows uses the full
128-bit file ID and volume identity. Existing distinct files on a case-sensitive
filesystem stay distinct. Parent aliases and hardlinks are checked through the
opened identities, including Unicode-normalized parent names. For two missing
names, parent identities are compared. Native Windows/macOS directory case
sensitivity narrows plain ASCII case variants where available. Missing Unicode
normalization remains conservative; distinct missing names can be rejected
rather than risk adoption.

Indexed filename keys are deliberately broad: bounded Unicode NFKD/uppercase
ASCII projections are hashed, and native identity/equivalence checks confirm
their candidates. These keys are not proofs that paths are equal. Collisions,
including non-ASCII/diacritic projections, can cause conservative refusals for
ambiguous missing paths. At most 100 candidate references are inspected; larger
ambiguous sets fail explicitly. The pinned key algorithm and Unicode mapping
are persistent metadata and need migration if changed.

Application mutation connections register the reservation functions used by
the triggers. External SQLite capture-path writes without those functions fail
closed; read-only queries/backups need no functions. Migration computes only
metadata keys and stored trash identities, without walking all image paths.
New app-managed inserts/relinks refresh their cached native identities.

Before physical operations, unresolved legacy identities are prepared in
100-reference transactions under a cooperative five-second budget. Completed
metadata batches persist, while image paths, bytes, IDs and creation times stay
unchanged. No file moves/deletion occur until required alias checks succeed.
The explicit `index-identities` command refreshes identities and reports checked,
indexed, OS-missing, unavailable, remaining and protected references, capped
unavailable IDs, cancellation/deadline state and a continuation cursor. Large
libraries can continue from that cursor; unavailable references stay unresolved.
Protected intent rows are deferred and checked directly during recovery.
Schema version 9 adds an unresolved-only partial index. Automatic preparation
uses a direct `file_identity IS NULL` seek and indexed continuation/count queries;
its fully indexed fast path does not traverse the Library before each action.
Explicit refresh remains a separate cursor query.

A scoped SQLite VM progress hook observes cancellation and the five-second
budget during statements, with checks after every query/commit and before
success. The hook owns stable boxed state, borrows the private connection and
caller callback exclusively for this call, contains callback panics, and is
unregistered before that state can expire, including error paths. It adds no
thread or polling timer. Committed metadata progress and its safe resume cursor
remain available on interruption; `remaining` and `protected` are null when
their counts could not be computed within the budget. Such a report never says
complete. Native I/O and SQLite busy waits remain cooperative boundaries.


Cached identities describe cooperative app-managed changes, not arbitrary
external path retargeting forever. After changing symlinks/mounts or otherwise
retargeting references outside the app, run the identity refresh before physical
maintenance. Native mutation still verifies its held source/destination stamp;
there is no claim of a filesystem-wide transaction against simultaneous external
writers. OS `NotFound` counts can include unavailable drives; they never authorize
deleting a referenced image or silently removing its history row.


`trash-list` reads at most 20 metadata entries plus one lookahead, newest sequence
first. Results contain `entries`, `has_more`, `next_cursor`, exact UTF-8 paths,
trash UUID, original capture ID/path, byte count and trash time. Querying opens
no images and never migrates storage.

Restore requires an unchanged trash file and absent destination. Omitting the
path uses the original path when its extension matches the admitted kind;
otherwise it corrects the extension, for example `replacement.wrong` containing
WebP restores as `replacement.webp`. This preserves compressed bytes and does
not convert the image. It reserves a currently unused capture ID and
restores creation time, source category and dimensions, clearing the thumbnail
reference. Its ID can differ because later captures may reuse the old ID.
Rename, trash removal and capture insertion share the journal protocol.

## Permanent removal and recovery

Purge accepts explicit trash UUIDs, never arbitrary filesystem paths. Identity,
size, dimensions and SHA256 must match. Before removal, the file is renamed
without replacement to `.screenshots-purge-UUID` with the admitted format extension in the same directory,
verified and synchronized. Removal checks that quarantined file again. Only
then are trash metadata and pending intent removed together. Changed files
are preserved.

`pending-move` includes the operation `kind`. One unresolved physical operation
is admitted per database; capture/trash triggers reserve its records and paths.
Recovery holds an immediate write transaction through checks and finalization.
`recover-file ... cancel` clears an unapplied move/trash/restore/purge only when
the expected source remains and destination is absent; it never reverses a
completed rename. `finish` requires the expected final file and absent source.
Before publishing any intent, move/transfer/restore also reject destinations
reserved by trash metadata, even if the referenced trash file is missing.
For purge, finish may delete its matching already-quarantined file or finish
metadata after deletion. If deletion already happened, recovery rechecks absence
and repeats the parent barrier before forgetting the intent. Transfer recovery
likewise repeats the missing source's barrier; cancellation repeats the barrier
for a removed verified staging file. On POSIX this holds the parent, verifies
absence, synchronizes the directory and rechecks parent identity/absence. A
failed barrier retains intent and metadata for another explicit attempt. The
Windows adapter verifies held parent identity and absence without claiming a
POSIX directory-fsync guarantee. Conflicts preserve files and intent. The older
`recover-move` accepts only ordinary moves, retaining its no-deletion semantics.

Trash/purge batches flush one JSON outcome per confirmed item and a completion
summary. Failure stops the batch; earlier items remain committed and the failed
item may need recovery. Output write/flush failure truthfully reports completed
mutation. Move/transfer/restore return one outcome.

## Cross-volume transfer

`move` remains a same-volume native rename. `transfer` explicitly streams source
bytes to a new staging file beside the destination using a 64 KiB buffer. It
checks source stability and exact copied bytes, syncs the copy, then durably
records its separate destination identity before publication or source removal.
After no-replace publication and verification, it removes the still-matching
source and updates the capture path, preserving ID and original metadata.

Recovery can finish from a verified staging copy, published copy plus original,
or published copy after source deletion. Changed sources are preserved. Cancel
requires an unchanged original and absent final destination; a matching fully
verified staging copy can be removed. A crash before the copy fingerprint was
committed leaves an unverified candidate. Cancel preserves it and reports
`preserved_path`, without guessing that deletion is safe. Ordinary failure
cleans only the staging file created and still owned by that job. Unknown
staging files are never swept and existing destinations never overwritten.

## Explicit retention

Policies are regular JSON files of at most 4 KiB; unknown/duplicate fields fail.

```json
{
  "version": 1,
  "trash_directory": "/absolute/user-controlled/trash",
  "capture_days": 30,
  "purge_days": 7,
  "include_imports": false,
  "limit": 20
}
```

Capture/purge days are independently optional; at least one explicit action is
required. Days are 1–36,500. Capture retention selects records older than that
many completed UTC calendar days. Imports remain excluded unless explicitly
included. Purge selects existing trash older than the specified elapsed days.
Each phase selects at most `limit` entries (1–100, default 20) using bounded
pages/indexed metadata. Preview returns IDs and `has_more` without filesystem
changes or migrations.

Apply evaluates the policy once and processes its bounded selections, stopping
at the first failure, cancellation or cooperative five-minute batch deadline.
Selections are revalidated before mutation. Confirmed outcomes remain in JSON;
failure returns nonzero and does not roll back earlier files. Old trash is
selected before captures are newly trashed, preventing immediate purge of those
new files. A policy acts only when invoked: no timer, startup scan or background
service is installed.

## Native behavior and resources

Linux uses held descriptors and `renameat2(RENAME_NOREPLACE)`. macOS shares the
POSIX admission/hash path and uses `renameatx_np(RENAME_EXCL)`. Both sync files
and parent directories. Windows uses 128-bit `FILE_ID_INFO`, rejects reparse,
device and alternate-stream paths, calls no-replace
`MoveFileExW(MOVEFILE_WRITE_THROUGH)`, flushes files and checks held parent
identities. Native handle deletion targets the opened file. Windows does not
claim a POSIX directory-fsync equivalent; power-loss acceptance stays separate.

Encoded admission is at most 272 MiB for PNG or 32 MiB for JPEG/WebP, with
existing 256 MiB decoded-dimension admission. Native fingerprints carry the
actual kind; legacy journals default to PNG. Destination and owned staging/trash
extensions match that kind. Byte-preserving file operations never decode or
convert images. See the [format contract](capture-library-formats.md).
Hash/copy uses a 64 KiB buffer, no decoded image and a cooperative 30-second
budget per pass. Repeated passes, SQLite busy waits and native calls can exceed
that wall time; native calls cannot be interrupted mid-call. Retention checks
cancellation between items and before destructive checkpoints. Abrupt exit
leaves durable intent instead of continuing unattended.

Each operation/trash payload is at most 16 KiB. Trash queries validate at most
21 records and return only metadata. Retention retains at most 100 capture IDs,
100 trash UUIDs and bounded outcomes. Existing SQLite cache target is 512 KiB.
Transfer adds one temporary encoded file on the destination volume. The ID guard adds one fixed singleton-journal lookup per capture insertion.
Reservation checks add the pinned `unicode-normalization` crate and SQLite
function support, not another runtime. Name projection is capped at 16 KiB,
its key at 67 bytes, and each native identity at 51 bytes. Metadata columns and
indexes have SQLite page/index overhead; they are not image caches. Priming
retains at most 100 paths of 4,096 bytes plus small identity/progress values,
with cancellation between entries and batches. No thread, runtime, image cache,
full-Library image buffer or idle work is added. Thumbnail prefetch/persistent
caches need a demonstrated UI consumer;
they are not introduced by this headless lifecycle.

Focused optimized native/journal/lifecycle verification covers reservation,
legacy-adoption and priming cases, including
a real transfer between distinct Linux filesystem devices and exact recovery at
verified-copy, publication and source-removal boundaries. The affected history
query/thumbnail/lifecycle suite passed 66 regular cases with five unrelated opt-in
measurements excluded; seven storage regressions and ten existing executable
history cases also passed. These counts describe separate invocations, not a
combined workspace total.

The real CLI script `scripts/test-history-lifecycle.py` passed with 25-item
export/trash pages, actual cross-volume round trips, changed-file preservation,
fixture-only SQLite aborts after trash rename, purge unlink and copy verification,
explicit finish/cancel, retention and missing-reference reconciliation. Its
initial report is `target/verification/history-lifecycle-cli.json`. The Goldfish
follow-up additionally exercised actual native unlink followed by an injected
parent-fsync EIO through a private Linux test interposer. Repeated recovery with
that barrier still failing preserved purge/transfer intents; removing the fault
allowed completion. Normal automatic ID allocation and missing reserved trash
targets were also checked through the real executable. Run
`scripts/test-history-lifecycle.py --native-faults --binary BINARY` on Linux with
a C compiler; this test helper is never linked or packaged with the app. Its
report is `target/verification/history-lifecycle-goldfish-fixes.json`. Round-two
verification additionally stops publication after the verified stage stamp
commits, rejects stage import/relink, injects direct/arbitrary-symlink legacy
adoption, checks finish/cancel preservation, and completes after explicit
metadata resolution. It also runs identity refresh and records actual native
alias probes. The Linux volume here is case sensitive; Windows/macOS runtime
branches are not claimed by those local probes. This report is
`target/verification/history-round2-native-e2e.json`. Round-three regressions
seed 30,000 fully indexed metadata rows, verify the unresolved partial-index
plans and zero full-scan steps, and repeat automatic preparation 100 times under
a small VM-instruction bound. Borrowed callback cancellation, deadline
interruption, panic containment, hook teardown and post-query stopping without
a VM callback are checked separately. Windows GNU core and
complete desktop/test metadata compilation passed, including the opened policy
reparse-point guard. Native Windows/macOS runtime and power-loss acceptance are
not established by cross-compilation.

Three optimized warm 64 MiB encoded-extent transfer samples observed 0.44, 0.85
and 0.43 seconds end to end; maximum RSS was 21,276 KiB versus 14,592 KiB for the
same executable's `--version` baseline. The synthetic PNG carried sparse zero
padding after IEND to exercise bounded byte-preserving file work without image
decode. This is a development observation, not a latency distribution, private
footprint measurement or reference-hardware gate. The exact binary hash, raw
samples and scope are in `target/verification/history-lifecycle-transfer-cost.json`.

Native contracts: [Windows directory case sensitivity](https://learn.microsoft.com/en-us/windows/wsl/case-sensitivity),
[Apple rename API](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/stdio.h),
[Windows moves](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw),
[Windows identity](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfileinformationbyhandleex),
[Windows flushing](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-flushfilebuffers).
