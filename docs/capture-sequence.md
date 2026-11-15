# Finite interval still captures

Capture a bounded sequence from one explicitly selected display:

```sh
mkdir captures
screenshots list displays
screenshots capture-sequence display DISPLAY_ID captures --count 10 --interval-ms 1000
```

`--count` is required and accepts 1–1000. `--interval-ms` is required and accepts
100–3600000 milliseconds. `--start-at`, `--format`, `--jpeg-quality` and
`--webp-quality` are optional.
Options may appear in any order, once each; duplicates and unknown options are
rejected. The
display ID must contain 1–64 UTF-8 bytes and identify an available display on
the current platform. Window/region sequences, video encoding, audio and
unbounded monitoring are outside this command.

PNG remains the default. Select `--format png`, `--format jpeg` or `--format webp`
for the entire invocation. JPEG uses quality 90 by default; override it with
`--jpeg-quality 1` through `--jpeg-quality 100` and an explicit `--format jpeg`.
JPEG quality with PNG, WebP or an omitted format is rejected instead of ignored.
These options do not read or change saved UI export preferences. PNG and default WebP
preserve alpha and pixels losslessly; JPEG is lossy and refuses any non-opaque
pixel instead of silently flattening it. The original dimensions are preserved.

`--webp-quality 1` through `--webp-quality 100` explicitly selects lossy RGB WebP
and requires `--format webp`. Alpha remains exact. WebP without a quality flag
remains lossless. JPEG and WebP quality flags cannot be combined. Lossy WebP
accepts at most 64 MiB of RGBA input and dimensions 1–16383; larger frames stop
the sequence without publishing that frame. Already completed files remain.

```sh
screenshots capture-sequence display DISPLAY_ID captures --count 10 --interval-ms 1000 --format jpeg --jpeg-quality 85
```

To start one finite sequence later, supply an RFC 3339 timestamp with an explicit
timezone: `Z` or a numeric offset such as `+01:00`.
Fractional seconds support up to 9 digits (nanosecond precision). The RFC 3339
unknown-offset form `-00:00` is rejected; use `Z` or `+00:00` for UTC.

```sh
screenshots capture-sequence display DISPLAY_ID captures --count 10 --interval-ms 1000 --start-at 2030-01-01T09:30:00+01:00
```

Replace the example timestamp with a future time. At initial argument validation,
the instant must be strictly later than the current time and at most exactly
7 days (604800 seconds) away, including fractional seconds. A timestamp equal to
the validation time, one nanosecond beyond 7 days, or one without a timezone is
rejected before any wait or native capture. Count and interval remain required.
The timestamp is converted to UTC, and that validation-time difference sets a
monotonic deadline. After initial validation, the invoking headless thread sleeps
once for the remaining duration (zero if validation passed the deadline). It does
not poll the clock. Later wall-clock corrections do not change the accepted
deadline. Slow validation, OS scheduling and native capture latency can make
capture start later than the requested instant; precise real-time delivery is
not guaranteed.

The invoking process must remain alive. This is one scheduled invocation, with
no saved schedule, service, resident scheduler, repeat rule, missed-run recovery
or ability to wake the machine. Suspend/resume and native permission prompts can
delay execution, and elapsed suspend time follows the platform's sleep behavior.
No native backend is constructed and no frame is captured or retained during
the initial wait.

The output directory must already exist and be empty. Validation checks only
the first directory entry; it does not enumerate an arbitrarily large directory.
Relative paths resolve from the invoking directory. Native non-UTF-8 directory
paths work. A directory symlink is followed, so choose its target deliberately.
Initial validation never creates a directory or modifies its existing contents.
For a scheduled start, the same directory checks run again after the sleep and
before constructing the native backend. A destination that becomes missing,
non-directory or occupied while waiting fails without capture. No extra directory
check is inserted in the immediate-start path.
The shared exporter creates missing parent directories during each save: if this
directory is removed between validation and output it can be recreated. Existing
files are never overwritten, but this is not a pinned-directory guarantee.

Each invocation generates one UUID prefix and saves separate images named
`capture-UUID-0001.png`, `capture-UUID-0002.png` and so on by default. Explicit
JPEG uses `.jpg`, and WebP uses `.webp`. The core export path
publishes each completed image atomically without replacing an existing file.
This remains the final collision protection if directory contents change after
validation. A directory rename or symlink change during execution follows normal
filesystem path resolution; the command does not pin an opened directory handle.

Without `--start-at`, the first capture starts immediately after validation.
With it, the first capture follows the initial wait and directory recheck. After
a capture is saved and its image released, the command sleeps for the interval
before starting the next capture. It performs no catch-up work: capture and encoding
time add to the time between starts. It does not sleep after the final capture.
There is one capture/save at a time and no frame queue. The operation exits after
the requested count, with no resident worker or timer left behind.

Successful stdout is one JSON object. `files` contains at most 1000 generated
basenames, relative to `directory`, in capture order:

```json
{
  "completed": 2,
  "interval_ms": 1000,
  "directory": "captures",
  "path_encoding": "lossy-native",
  "files": [
    "capture-xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx-0001.png",
    "capture-xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx-0002.png"
  ]
}
```

`directory` uses the native path's display representation and may replace invalid
Unicode; `path_encoding` makes this explicit. Filesystem operations retain the
original native path. ASCII generated basenames are exact and do not duplicate
the full directory path for every frame.
Only scheduled invocations add `"start_at":"2030-01-01T08:30:00Z"` to the
successful object: the accepted requested instant normalized to UTC, not the
actual capture time. Explicit `--format` adds `"format":"png"`, `"format":"jpeg"`
or `"format":"webp"`; JPEG also adds the effective `"jpeg_quality":90` (or the
requested quality). Omitting `--format` retains the existing JSON fields.
Explicit lossy WebP also adds `"webp_quality":N`; it is absent otherwise.

A capture or save failure stops the sequence immediately. Stderr states the
number of completed captures, the directory and the invocation's prefix; no
success JSON is printed. Completed images remain. Earlier captures are never
rolled back or deleted because a later display or filesystem operation failed.

Ctrl+C or process termination uses the existing headless OS termination behavior.
No custom signal handling is added. During either the initial scheduled wait or
an interval it stops the process;
during a synchronous capture or save, cancellation and native stalls retain the
platform's existing limits. An abrupt exit can leave completed images and one
unpublished UUID `.part` file. `storage inspect-parts` can report such candidates
without deleting them. No cooperative cancellation, hard native-call timeout or
whole-sequence transaction is claimed. With the maximum count and interval,
interval sleeps alone total just under 1000 hours; the optional initial wait adds
at most 7 days. Use a smaller count/interval for shorter runs and terminate the
process to stop early. Termination during the initial wait leaves no captured
pixels or output files from this invocation.

The command reuses Xcap still capture and the existing 256 MiB per-frame decoded
admission ceiling. It keeps only one frame through export, drops it, then
waits. Existing native capture and encoder scratch remain additional bounded
costs; process high-water memory and native latency are not measured by the unit
tests. Completion metadata consists of at most 1000 short basenames. Scheduling
adds one UTC instant and a bounded monotonic deadline, uses the existing workspace
Chrono dependency, and adds one operation-scoped sleep before native initialization.
The only other sleeps are the intervals between successful captures. There are
no additional worker threads, runtimes, network requests, streams, history writes,
clipboard changes, uploads, UI windows or changes to resident startup. No timer
or retained pixels remain after the invocation exits.
Format selection adds two small per-invocation values and reuses the existing
encoders and their scratch allocations; no new image-sized buffer, dependency,
queue or cache is added by the sequence command. Encoder and process high-water
memory remain part of the existing unmeasured native acceptance work.
Explicit lossy WebP uses the bounded core encoder with a 32 MiB encoded-output
cap and cooperative 30-second deadline. Its admitted source is at most 64 MiB;
libwebp imports YUV/alpha storage and codec scratch, released after each frame.
Those native allocations are not a hard total-memory bound. No extra queue or
retained frame is introduced.

Default tests do not read desktop pixels. Deterministic unit fixtures supply
synthetic capture, save and wait callbacks to prove serial ordering, exact saved
pixels, frame release before waiting, and stopping after capture/save/wait
failure while retaining completed results. Injected validation time and wait
callbacks cover timezone normalization, nanosecond boundaries, immediate-start
compatibility, and initial waiting/revalidation before native initialization or
pixel allocation. Format fixtures check exact PNG/WebP alpha and pixels, JPEG
dimensions and alpha rejection, correct extensions, configured quality,
unchanged default PNG bytes, no-overwrite behavior and frame release before the
next wait. Executable tests reject malformed arguments, invalid export options,
invalid schedules and missing, non-directory or occupied destinations before any
future wait or capture. Two additional executable checks exercise a real two-second initial
wait followed by destination revalidation, and forced termination during a
far-future wait. Both remove native display/session access and verify unchanged
destination contents, no capture output and no user state. They do not capture
desktop pixels. The optimized focused checks passed 17 unit tests and 9 CLI
tests; the native capture test below remained ignored. These short functional
checks do not establish timer accuracy or reference-hardware resource budgets.
The ignored `native_cli_captures_two_explicit_stills_without_user_state` test
requires both `SCREENSHOTS_NATIVE_CAPTURE=1` and `SCREENSHOTS_TEST_DISPLAY_ID`;
it reads the selected display and writes only to an isolated temporary directory.
Run it deliberately in a suitable native session. Test definitions alone do not
establish native permission, display-loss, latency or resource acceptance.
