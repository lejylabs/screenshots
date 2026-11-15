# Finite interval still captures

Capture a bounded sequence from one explicitly selected display:

```sh
mkdir captures
screenshots list displays
screenshots capture-sequence display DISPLAY_ID captures --count 10 --interval-ms 1000
```

`--count` is required and accepts 1–1000. `--interval-ms` is required and accepts
100–3600000 milliseconds. Options may appear in either order, once each. The
display ID must contain 1–64 UTF-8 bytes and identify an available display on
the current platform. Window/region sequences, video encoding, audio, scheduled
start times and unbounded monitoring are outside this command.

The output directory must already exist and be empty. Validation checks only
the first directory entry; it does not enumerate an arbitrarily large directory.
Relative paths resolve from the invoking directory. Native non-UTF-8 directory
paths work. A directory symlink is followed, so choose its target deliberately.
Initial validation never creates a directory or modifies its existing contents.
The shared exporter creates missing parent directories during each save: if this
directory is removed between validation and output it can be recreated. Existing
files are never overwritten, but this is not a pinned-directory guarantee.

Each invocation generates one UUID prefix and saves separate PNGs named
`capture-UUID-0001.png`, `capture-UUID-0002.png` and so on. The core export path
publishes each completed image atomically without replacing an existing file.
This remains the final collision protection if directory contents change after
validation. A directory rename or symlink change during execution follows normal
filesystem path resolution; the command does not pin an opened directory handle.

The first capture starts immediately after validation. After a capture is saved
and its image released, the command sleeps for the requested interval before
starting the next capture. It performs no catch-up work: capture and encoding
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

A capture or save failure stops the sequence immediately. Stderr states the
number of completed captures, the directory and the invocation's prefix; no
success JSON is printed. Completed PNGs remain. Earlier captures are never
rolled back or deleted because a later display or filesystem operation failed.

Ctrl+C or process termination uses the existing headless OS termination behavior.
No custom signal handling is added. During an interval it stops the process;
during a synchronous capture or save, cancellation and native stalls retain the
platform's existing limits. An abrupt exit can leave completed PNGs and one
unpublished UUID `.part` file. `storage inspect-parts` can report such candidates
without deleting them. No cooperative cancellation, hard native-call timeout or
whole-sequence transaction is claimed. With the maximum count and interval,
requested sleeps alone total just under 1000 hours; use a smaller count/interval
for shorter runs and terminate the process to stop early.

The command reuses Xcap still capture and the existing 256 MiB per-frame decoded
admission ceiling. It keeps only one frame through PNG export, drops it, then
waits. Existing native capture and encoder scratch remain additional bounded
costs; process high-water memory and native latency are not measured by the unit
tests. Completion metadata consists of at most 1000 short basenames. The only
timer is an operation-scoped sleep between successful captures. There are no
additional worker threads, runtimes, network requests, streams, history writes,
clipboard changes, uploads, UI windows or changes to resident startup.

Default tests do not read desktop pixels. Deterministic unit fixtures supply
synthetic capture, save and wait callbacks to prove serial ordering, exact saved
pixels, frame release before waiting, and stopping after capture/save/wait
failure while retaining completed results. Executable tests reject malformed
arguments and missing, non-directory or occupied destinations before capture.
The ignored `native_cli_captures_two_explicit_stills_without_user_state` test
requires both `SCREENSHOTS_NATIVE_CAPTURE=1` and `SCREENSHOTS_TEST_DISPLAY_ID`;
it reads the selected display and writes only to an isolated temporary directory.
Run it deliberately in a suitable native session. Test definitions alone do not
establish native permission, display-loss, latency or resource acceptance.
