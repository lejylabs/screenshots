# Journaled capture saves

Normal application PNG saves and original-size PNG Save As record one durable
capture-save intent before writing pixels. This closes the interruption between
publishing an image and inserting its capture-history row. It does not scan the
save folder or import unrelated files. JPEG/WebP and resized export copies keep
their existing explicit export behavior.

The worker reserves a uniquely named, empty `.screenshots-save-UUID.part` file,
holds its native object and exclusive file lock, and commits the intent to the
history database. No pixels are written before that commit. A crash before the
intent commit can therefore leave only an empty file. Encoding uses the existing
Fast/Adaptive PNG policy and a checked 64 KiB writer, computing SHA256 while
writing. Original-size saved-PNG copies stream the admitted source bytes without
decoding or re-encoding them.

After file sync, the worker commits the encoded length, dimensions, SHA256 and
stable native object identity to the intent. It publishes without replacement
on the same filesystem, using held-parent relative operations on Linux/macOS and
write-through native move on Windows. Immediately before publication, the
staging name must still identify the held encoded object. The selected parent
is checked again; its original absolute spelling is retained only when opening
that spelling proves the same native directory identity as the canonical path.
Unix publication includes a parent-directory sync. The history insertion and
intent deletion then commit together. Publication changes such as ctime do not
invalidate stable object identity. Source images, clipboard ownership and output
profiles retain their existing semantics.

Copy and Save still report independent outcomes. A published image whose history
or final barrier fails remains a **saved, unindexed** output with a pending
recovery record. A missing history database path refuses PNG publication rather
than silently producing an untracked capture. The in-memory source remains
available after a failed save. No error claims that another application pasted a
clipboard image or that a failed publication saved a file.

If a multi-display save partly or completely fails, every unsaved source stays
in memory as shared immutable pixels. The retained group admits at most 32
handles (including actual vector capacity) and a checked sum of actual decoded
buffer capacities no larger than 256 MiB. Successful sources are released; the
existing preview identifies the latest unsaved source. Status reports the saved
count and how many remain unsaved, independently of the clipboard outcome.

The existing **Save as** action retries the entire unsaved group when the user
chooses PNG at Original size. Its suggested filename identifies the unsaved
batch. The chosen filename applies to the displayed final image; other unsaved
images retain their filenames in the same chosen folder. No target is replaced.
Canceling the dialog preserves the complete group. JPEG/WebP and resized exports
copy only the displayed image and keep the unsaved group available. Starting a
new capture/input session or quitting releases the old group; capture admission
prevents a new native allocation from overlapping an active save, and generation
checks prevent obsolete save results from restoring a discarded group. This
explicit unsaved-image consumer counts above the empty-resident baseline.

## Explicit recovery

```sh
screenshots history-manage pending-save DATABASE
screenshots history-manage recover-save DATABASE OPERATION_UUID finish
screenshots history-manage recover-save DATABASE OPERATION_UUID cancel
```

`pending-save` reads the one known intent and reports its destination, staging
path, operation UUID, whether encoding completed and whether cancellation began. It does not enumerate a
directory, decode images or run recovery automatically.

`finish` refuses an active writer. It verifies the recorded parent and regular
file, native object identity, encoded length and SHA256, using one 64 KiB read
buffer and a cooperative 30-second deadline per inspection (at most two hash
passes during finish). If only a complete
staging file exists, it publishes without replacement. If the destination is
already published, it indexes that exact file. History and intent changes commit
together, preventing duplicate rows after a repeated recovery attempt. A later
attempt after successful completion reports that no intent remains.
Hash reads use the held locked file handle; named reopens check metadata only,
including on Windows where exclusive byte-range locks prevent reads through
another handle.

On Unix, `cancel` removes a complete verified staging file, or an unchanged empty stage,
then clears the intent only after its parent barrier succeeds. If an earlier
unlink succeeded but parent sync failed, a later cancel retries that barrier
even when both paths are absent; a repeated barrier failure preserves the intent.
Once the PNG is published, cancellation preserves both
the file and pending intent and returns `saved_unindexed`; `finish` indexes it.
An interrupted nonempty stage without a committed content fingerprint is
conservatively preserved and remains pending. The result names that exact stage
and explains the preservation requirement. On Unix the user can preserve/move
it explicitly, then cancel the now-missing intent after its parent barrier.
Windows still requires the flushed-empty proof described below. It is never
deleted based on its filename alone. If both
staging and destination exist, or either file/parent was replaced or its content
changed, recovery refuses mutation and preserves the intent and files.

On Windows, cancellation first commits a bounded cancellation fingerprint for
the verified owned staging object. It refuses an object with additional hardlinks.
It truncates that same locked writable handle to zero bytes, flushes the file,
then commits a separate `zero_flushed` state. Only then does it mark the empty
file for deletion, close both owned handles, observe namespace absence and clear
the intent. Crashes between these steps retain the cancellation state. Changed
nonempty content is preserved; a missing name without a prior flushed-empty
proof returns `cancellation_absence_unconfirmed` and keeps the intent. A
canceling image cannot be finished as a capture.

This ordering follows Microsoft's distinction between marking deletion and
closing a handle. A file-scoped flush requires a writable handle; flushing an
entire volume requires administrative privileges, which this service never
requests. [SetFileInformationByHandle](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-setfileinformationbyhandle),
[SetEndOfFile](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-setendoffile),
[FlushFileBuffers](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-flushfilebuffers).

Windows parent verification is **not** a directory flush. Its cancellation
result reports the remaining namespace power-loss limitation: an empty entry
may reappear after a power loss. The flushed-empty protocol protects the logical
staging contents; it is not forensic deletion or proof that hardware honored
flushes. Real Windows filesystem/power-loss acceptance remains an unresolved
release gate. `MOVEFILE_WRITE_THROUGH` is used for publication, following its
documented move contract; it is not presented as a general ancestor-directory
barrier. [MoveFileExW](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw).

Only the exact journaled paths are inspected. Symlinks/reparse points and device
names are refused. Existing destinations are never overwritten, published
outputs are never deleted by cancellation, and unknown or changed files are
preserved. These checks are not an isolation boundary against a hostile process
running as the same user or a privileged filesystem writer.

## Storage and resource cost

Schema 10 is an additive migration from every released schema 1–9. It adds one
row with a payload of at most 16 KiB, two path columns of at most 4 KiB each,
a 36-byte operation UUID, and SQLite/index overhead, plus reservation triggers.
Newer unknown schemas
are rejected before migration or journal-mode changes. Read-only health checks,
backup, queries and existing physical-history services recognize schema 10.
While a save is pending, import/relink cannot adopt its paths, another capture
save is refused, and journaled physical-history operations wait for explicit
save recovery. The ordinary one-job worker admission remains in force.

Missing capture-output and database ancestors are prepared by one bounded
worker helper, also used by settings/background folder creation. It starts at
the first existing native directory, plans at most 64 missing components and a
4096-byte resolved path, then creates children relative to the held parent. On
Unix it syncs each new child and parent before continuing. A retry first repairs
the immediately relevant existing ancestor's parent-entry barrier; it does not
scan or sync the whole home tree. The helper holds at most two directory objects
plus one short metadata-verification handle, bounded component strings and one
30-second cooperative deadline. New Unix directories use mode 0700. First-run
Pictures/Screenshots and nested save folders remain supported. Windows uses the
same bounded creation and identity checks; durable ancestor namespace behavior
remains in its explicit native/power-loss release gate.

There is no new dependency, service, thread, runtime, queue, timer, startup scan,
image cache or full-image snapshot. The active cost is bounded path/intent/hash
metadata, a held parent and output file, the existing 64 KiB encoding buffer and
encoder scratch. Original-file copies also hold one source handle and a 64 KiB
stream-read buffer (128 KiB application streaming buffers in total). Recovery
hashes only an explicitly selected intent's encoded file;
normal saving hashes during output and adds no full output readback. SQLite
intent commits and native barriers add operation-scoped I/O. Native filesystem
calls remain noninterruptible within their calls; deadlines are cooperative.
Windows cancellation adds two bounded metadata updates and a 64 KiB hash-read
buffer only on cancellation/error paths; it adds no successful-save readback.

## Verification scope

Private fixtures cover pixel/alpha and byte-copy preservation, interruption with
ready staging, published output and failed metadata insertion, normal
cancellation/error cleanup, active-writer refusal, changed/replaced files,
symlinks/parent identity, conflicting history operations, malformed intent,
encoded bounds and additive upgrades from every released version. The tests use
temporary databases and files; no user's library or desktop is opened.
New state fixtures interrupt cancellation before truncation, after truncation,
after flush, after recording the zero proof and after owned-handle closure;
other fixtures preserve changed content and hardlinks, and inject ancestor
parent-barrier failures before any pixels/history are published. Portable state
fixtures do not establish Windows native flush/power-loss acceptance.

The coordinated `release-fast` workspace passed 953 tests with zero failures,
69 explicit opt-ins excluded and 59 outer Cargo suites. It includes 24 portable
save-journal fixtures and all 105 UI fixtures, including unsaved-batch retention
and preview regressions. Two actual main-executable recovery tests additionally
exercise a published PNG with a failed history insertion, and a substituted
staging name followed by recovery of the original held file. They verify
`pending-save`, `recover-save finish/cancel`, exact pixels/bytes, one history row,
foreign-file preservation and unchanged isolated user-state paths. The complete
log and its outer-suite summary are
`target/verification/moxcms-fix-workspace-tests.log{,.outer-summary.json}`.

Source reviews five and six were clean. Final formatting, all-target optimized
Clippy, default advisory checks and all-workspace/all-target Windows GNU metadata
checking pass. The latter is type/API checking, with 417 source/provenance files
unchanged during the checks, and does not execute a Windows binary. Details are
in `target/verification/current-three-final-checks.json`.
Physical power loss, native Windows/macOS behavior, actual disk-full or detached
volumes and full reference-hardware performance acceptance remain separate.


The third fresh source review found that the shared busy-operation helper also
retired unsaved sources when loading an uncached history preview. Preview now
uses the same bounded/cancellable work admission without starting a replacement
capture/input session. Both successful and failed uncached preview regressions
check that every unsaved source and the Save As retry path remain intact, with
clipboard state unchanged. Review three was not clean; both regressions passed
in the coordinated optimized run above.

## JSON-layout-independent journal CAS (2026-11-17)

`cc38b9c` compares the validated, bounded typed intent against the expected state,
then uses the exact observed original JSON payload in the conditional SQL write.
Equivalent whitespace/property ordering no longer falsely blocks recovery;
changed intent state still fails closed and preserves the journal and stage.
The regression checks pretty JSON recovery, a changed digest refusing cleanup,
and exact recovered pixels. No schema change or relaxed payload/path/hash bound
is involved. The optimized save/recovery group passes 29 cases (one opt-in
ignored), and the complete core run passes 523 (53 ignored), with optimized
all-target Clippy and formatting passing. The earlier core-only failure exposed
a real difference between serde feature graphs and remains archived under
`target/verification/sharex-review6-integration/`. Native crash/full-volume and
reference-hardware acceptance are not inferred from these tests.
