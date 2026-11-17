# Saved tasks and ordered actions

User-authorized non-UI implementation, 2026-11-15. This extends the existing
image recipes and fixed capture-task sequence into reusable JSON profiles.

`task validate PROFILE.json` validates a bounded profile and all referenced
recipes/upload configurations/output paths without capturing, uploading,
opening a clipboard or executing programs. `task run PROFILE.json` executes it.
Profiles are one strict version-1 JSON object within 64 KiB, with 1–16 actions.
Paths resolve against the profile's directory; no shell expansion occurs.
Save/QR output paths may contain `{uuid}` and `{timestamp}`; one UUID and one
UTC timestamp are chosen per invocation and shared across that run's outputs.
Use `{uuid}` to make hotkey/IPC profiles repeatable without replacing files.
Input/recipe/program paths are literal. Command substitutions are single-pass:
server URL text containing braces is never interpreted as another placeholder.

Example:

```json
{
  "version": 1,
  "source": {"type":"file", "path":"input.png"},
  "recipe":"redact.json",
  "actions":[
    {"type":"save", "path":"final.png"},
    {"type":"upload", "profile":"destination.json"},
    {"type":"link", "format":"markdown", "label":"Screenshot"},
    {"type":"qr", "path":"share.png"}
  ]
}
```

Sources are `file` (bounded existing image loader), `display`/`window` with `id`,
or `region` with `display,x,y,width,height`. The optional existing image recipe
runs in memory before any action, so every output/upload uses finalized pixels.
`primary_display`, `active_window` and `active_display` choose their native
target at invocation, with focus/process guards. `raw_file` streams an arbitrary
file unchanged for HTTP/provider/S3 transfers; it cannot use image recipes,
image save or image copy. `none` runs command/utility tasks without creating a
dummy screenshot. `{input}` expands to the resolved file-source path.
Capture permissions, geometry/native limits and hard-call timeout caveats remain
those of the existing backend. Validation cannot prove a device is available
or an image decodes; runtime failure produces no synthetic success.

Actions execute in supplied order:

- `save`: a new PNG/JPEG/WebP path, optional `jpeg_quality` 1–100 (default 90).
- `upload`: existing destination profile, including its supported SXCU subset;
  explicit finalized PNG upload, or unchanged bytes for a raw-file source, with
  the latest confirmed result retained.
- `provider_upload` / `s3_upload`: validated provider/S3 profile and the latest
  explicitly saved artifact (or the raw-file source when no output was saved).
  The profile is snapshotted before capture/actions. A public base URL must be
  explicitly configured to enable later link/QR/copy-link actions; storage
  endpoints are never guessed as public downloads.
- `link`: latest uploaded URL as plain/Markdown/HTML, optional label. Reuses
  the bounded, escaping-aware formatter. No browser or clipboard side effect.
- `qr`: latest uploaded URL written as a new PNG using existing QR generation.
- `copy_image`/`copy_link`: explicitly update the clipboard with final pixels
  or latest confirmed URL. The task retains one clipboard owner until exit.
  On Linux require top-level `clipboard_hold_ms` 1000–300000; after the task's
  complete result it keeps the process alive for that interval to serve paste.
  This is finite clipboard ownership, not a silently forked daemon. Other OSes
  may use zero. No tests touch the real user clipboard.
- `command`: absolute or profile-relative regular executable `program`, up to
  32 literal UTF-8 `args` (4096 bytes each), required `timeout_ms` 1–300000.
  Only `{url}` and `{output}` are substituted, from an earlier successful
  upload or saved output. No shell is inserted. A task author explicitly
  authorizes that program when invoking the profile. Existing recorder child
  process-group/Job-Object isolation supplies deadline/cancellation cleanup,
  bounded diagnostic/stdout drains and child file caps. Output is discarded,
  never replayed as a command. Native kernel/process calls remain potentially
  uninterruptible. Task profiles must be trusted like executable scripts.

Invalid action dependencies, repeated output paths and existing outputs fail
preflight. Each completed action emits flushed NDJSON with index, total and
result, followed by one complete event. A later error retains earlier files
and remote uploads and reports the failed ordinal and completed count; no retry,
rollback deletion or silent restart follows. Successful remote upload followed
by a link/parser/stdout error may still leave that remote object. Final-output
publication retains the existing atomic no-replace behavior, including races.

SXCU file uploads retain the exact opened-file version through helper preparation
and HTTP acknowledgment. The shared upload source guard checks complete tracked payload
consumption and the retained identity/length before any successful result reaches
Link, QR or clipboard actions. Same-size in-place changes and growth fail with
remote-object uncertainty; scheduled raw-file uploads keep their existing prepared
source identity check as well. No retry or automatic remote cleanup follows.

Memory is one current image, optional one clipboard representation, the existing
encoder/upload buffers and a bounded profile/action list. Recipe operations
release the previous raster. No resident runtime, background task graph, timer,
database mutation or cache is added. Explicit finite clipboard retention is the
only post-completion wait. External commands are sequential, not queued in
parallel. The per-action native/decoder limits do not establish the full PLAN
hardware acceptance budgets.

Unix Task signals share one cancellation flag across source admission, commands,
HTTP/provider/S3 upload and result output. Ordinary UI upload cancellation remains
notification-driven. Only an active signal-aware network request observes the
flag at 25 ms intervals; no resident timer is added. S3 checks the same flag while
hashing and drops a waiting request on cancellation. A remote object may already
exist and is never retried or automatically deleted. Executable tests verify
SIGTERM during a real loopback upload and cleanup after sequential commands;
core HTTP/S3 fixtures verify admission refusal and waiting-connection closure.

Files: desktop `task.rs`, private formatter visibility, main/CLI routing and
`tests/task.rs`; this contract and delivery/status evidence. Implement core
validation/execution, test actual executable file/save/upload/link/QR/command
chains and partial failures with isolated loopback/fixtures, then integrate
hotkey/task IPC through native event-driven routing. No task UI is added.

Use the explicit [durable scheduler](capture-scheduling.md) for finite or
repeating UTC sequences with these source/action profiles. It seals the actual
prepared configuration, pauses changed profiles/references and never retries
an uncertain Task or upload. Scheduled receipts omit action payloads and URLs;
standalone `task run` receipts retain their documented behavior.


Source-verification update (2026-11-17): the actual private HTTPS fixture now
mutates sealed raw-file sources after request headers, exercises real schedule
add/serve/list, and verifies an uncertain run with no automatic retry and no
later Link/QR output. Unchanged actual file/memory/recipe chains still pass.
The 137-request run uses an isolated synthetic Secret Service and loopback TLS;
no user clipboard or real provider account is involved. See
[the current SXCU evidence](sharex-runtime.md#corrected-parser-source-and-result-integration-2026-11-17).
