# Optional Unix screen recording

Record a finite rectangle from a local X11 display with separately installed
`ffmpeg` and `ffprobe` executables:

```sh
screenshots record x11 capture.mkv --seconds 10 --fps 15 --width 1280 --height 720 --x 0 --y 0
```

This is a partial screen-recording capability: Unix, local X11, one video stream,
Matroska container and FFV1 lossless codec. It does not provide audio, GIF output,
Windows/macOS native capture, Wayland portals, cursor capture, editor integration
or a recording UI. X11 support on a Unix system still depends on the installed
FFmpeg build, permissions and an available X11 session. The application does not
install, download or distribute either external executable. The user's installed
FFmpeg build and its licensing remain separate from the application package.

All four options are required: `--seconds` accepts 1–60, `--fps` accepts 1–30,
`--width` accepts 1–1920 and `--height` accepts 1–1080. Duplicate or unknown options
are rejected. `--x` and `--y` are optional nonnegative coordinates, used together,
and default to zero. Their rectangle must fit signed 32-bit X11 coordinates and
the actual display. The command records exactly `seconds × fps` frames, at most
1800. Native failures such as an out-of-display rectangle stop without publishing.

X11 capture reads `DISPLAY` from the environment and accepts only the local
numeric form `:NUMBER` or `:NUMBER.SCREEN`, at most 64 bytes. Remote hosts and
user-supplied input URLs are rejected. Native X11 authentication still uses the
session's existing environment. The supplied filename is a native filesystem
path, relative to the invoking directory; it must end in `.mkv` and must not
already exist, including as a symlink. Paths are passed as arguments, never
through a shell. No audio device, network input, clipboard, history or application
configuration is opened by the command.

For synthetic verification without reading desktop pixels:

```sh
screenshots record test pattern.mkv --seconds 1 --fps 2 --width 16 --height 12
```

`test` mode uses FFmpeg's RGB `testsrc` pattern and does not accept offsets. The
RGB source preserves exact requested odd dimensions and the 1×1 boundary that
the subsampled `testsrc2` source cannot represent. It is explicitly synthetic
video, with the same limits, encoder, verification and publication as X11 mode.

FFmpeg runs with fixed arguments for FFV1 level 3, one-frame GOP, BGR0 pixels,
two codec threads, one filter thread, a one-packet input queue and a one-packet
initial mux queue. No unbounded application frame queue or raw frame copy enters
the Rust process. A 1920×1080 four-byte frame is about 7.9 MiB; external codec
references, X11 buffers and encoder/filter scratch add to this. Each external
allocation is limited to 64 MiB with FFmpeg's `max_alloc`, which is **not** a
total-process memory limit. The actual combined application/subprocess peak
RSS and CPU require measurement; native performance acceptance is not implied.

At most one engine process runs at a time. Its dedicated process group is
terminated and the direct child reaped before moving to the next phase or
returning an error. Encoding needs one diagnostic-drain worker; verification
adds a second stdout-drain worker. Workers request 256 KiB stacks, use one bounded
two-message channel and exit with the operation. Diagnostics are counted and
discarded at 16 KiB rather than logged; probe JSON is capped at 8 KiB. A 25 ms
check runs only during the active operation. There is no resident polling,
background recording, eager runtime, network check or additional idle work.

The overall deadline is the requested duration plus 10 seconds, including the
verification phase. An OS `RLIMIT_FSIZE` of 256 MiB applies to child processes,
so a cap hit cannot grow the recording past that limit. Child core dumps are
disabled with `RLIMIT_CORE=0`, including limit-triggered termination, to avoid
persisting raw process memory. Missing executables,
overflow, nonzero exits, cancellation and deadline expiry fail the operation.
Native process spawning, filesystem calls and kernel waits can still block;
the deadline is checked around them, not a hard interruption guarantee for
arbitrary kernel stalls. Unexpected descendants that deliberately escape the
child process group are outside this isolation guarantee.

Recording writes to a private mode-0700 UUID directory beside the destination.
FFmpeg gets a new filename with overwrite disabled. After encoding, the command
checks the file is regular, bounded and has an EBML header, then runs `ffprobe`
with the local file protocol and forced Matroska demuxer. Publication requires
one FFV1 video stream, Matroska format, the requested dimensions/average frame-rate metadata and
the exact decoded frame count. A cap-truncated or short recording is rejected.
This verifies the completed output, rather than treating a successful encoder
exit as proof of a full recording. The command syncs the file, publishes it with
a same-filesystem no-replace hard link, and syncs the parent directory. Filesystems
without hard-link support cannot publish through this implementation.

SIGINT/Ctrl+C and SIGTERM stop the active child process group, reap the child,
join drain workers and remove only this operation's known staging file/directory.
Cleanup errors are reported. SIGKILL, power loss or a process crash can leave the
private staging directory; there is no automatic janitor or deletion of unknown
files. A signal arriving after atomic publication cannot retract the completed
destination. If parent-directory sync or staging cleanup fails after publication,
the error explicitly states that the recording was published; that output is
preserved. Existing user files are never overwritten or removed.

Successful stdout contains one JSON object, for example:

```json
{"mode":"test","container":"matroska","codec":"ffv1","seconds":1,"fps":2,"width":16,"height":12,"frames":2,"bytes":12345}
```

Engine diagnostics and captured content do not appear in logs or error output.
The only persistent intended result is the explicit video destination. The
default executable tests use fake local engines to exercise admission, bounded
streams, validation, no-overwrite publication, cleanup and signal termination
without capturing a desktop. The ignored real-engine test uses only synthetic
pixels and requires installed FFmpeg/ffprobe. Neither constitutes native X11,
Wayland or other-platform acceptance.

Relevant upstream contracts: [X11 capture device](https://ffmpeg.org/ffmpeg-devices.html#x11grab),
[FFmpeg options](https://ffmpeg.org/ffmpeg.html#Main-options) and
[ffprobe verification](https://ffmpeg.org/ffprobe.html).

`--seconds` is a nominal frame-count duration: the command requests and verifies
exactly `seconds * fps` frames with the requested nominal frame rate. X11 input
timestamps pass through; delayed/dropped native acquisition can stretch the
encoded timeline. This is not an exact wall-clock timer or a claim that stream
duration equals seconds. The independent wall-clock deadline is seconds plus
ten and aborts rather than publishing an incomplete frame count. Exact timeline
duration/variable-frame-rate normalization is separate recording work.

## Development verification

Eight optimized executable fixtures passed, including the real installed FFmpeg
9.0.2/ffprobe synthetic test. A separate three-sample 1280×720, 5 fps, 5-frame
recording observation produced five verified frames each time, about 0.606 s
process wall time and maximum reported process/child RSS 124.25 MiB. This synthetic
source result is not an X11 capture timing, total concurrent-memory measurement
or reference-hardware gate. Frames have a nominal one-second timeline; the
synthetic source can be produced faster than wall time. Native capture remains
unrun. The known per-process external memory cost is additional to app admission.
