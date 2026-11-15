# Optional Windows recording

Windows supports explicit `gdigrab` desktop recording and synthetic `test` mode
through separately installed `ffmpeg.exe` and `ffprobe.exe`:

```powershell
screenshots record gdigrab capture.mkv --seconds 10 --fps 15 --width 1280 --height 720 --x 0 --y 0
screenshots record test pattern.gif --seconds 1 --fps 7 --width 17 --height 13
```

The fixed desktop input is `desktop`; arbitrary window-title, device, network
URL or command inputs are not accepted. `--x` and `--y` must appear together and
are nonnegative signed-coordinate offsets. Test mode does not accept offsets.
Native display geometry, DPI behavior, capture permissions and FFmpeg's
`gdigrab` support remain requirements of the installed Windows environment.
The [FFmpeg gdigrab documentation](https://ffmpeg.org/ffmpeg-devices.html#gdigrab)
describes this input backend. Cursor capture is hidden by default; explicit
`--cursor show|hide` sets FFmpeg's `-draw_mouse` input option and adds the chosen
value to the result's `cursor` field. Omission retains the existing JSON schema.
The option is rejected for `test` input. Native Windows pointer appearance and
position remain unverified. Audio capture is not enabled.

The existing recording limits apply: Matroska/FFV1 accepts 1–60 seconds,
1–30 FPS, 1–1920 width and 1–1080 height; GIF accepts at most 30 seconds,
15 FPS and 1280×720. The command requires every size/time argument, rejects
duplicate/unknown options and never replaces existing output. GIF uses fixed
RGB8 colors and plays once. Exactly `seconds × fps` frames must be verified;
GIF timing allows centisecond quantization. This is nominal frame-count
duration, not a promise of precise wall-clock recording time.

Encoded output goes through stdout into a parent-owned staging file. The parent
reads 64 KiB chunks and rejects a chunk before writing it if the total would
exceed 256 MiB. It does not rely on FFmpeg's soft `-fs` limit and never holds the
whole video in memory. Probe JSON is capped at 8 KiB; stderr is counted and
discarded with a 16 KiB cap rather than disclosed as diagnostics. Exceeding any
limit terminates the engine and prevents publication.

Each encoder/probe process starts suspended, is assigned to a non-inheritable
kill-on-close Windows Job Object, and is resumed only after bounded initial
thread discovery and setup. The [Windows Job Object](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)
contains normal descendants. Closing the job stops its processes before pipe
workers are joined. A controller crash during the narrow suspended-spawn to
job-assignment gap can leave a suspended child; after assignment, controller
handle closure kills the contained tree. Deliberate external process creation
outside job containment and arbitrary kernel stalls are not covered.

One 25 ms deadline check runs only during active encoding/probing. A shared
deadline of requested seconds plus 10 seconds is checked around engine startup,
completion, verification and publication. Native filesystem writes, synchronization
and cleanup waits can outlive it; it is not a hard kernel-call timeout.
Encoding and probing each use two short-lived workers (binary
stdout and discarded stderr), with requested 256 KiB stacks and a two-message
channel. There is no idle polling, resident recorder, optional-runtime startup
or new encoder dependency. FFmpeg receives fixed thread/queue/allocation
options, but those do not bound total external process memory; native codec,
GDI and system allocations still need measurement.

Staging is a unique directory beside the destination, created with a protected
ACL granting the current user full control and inheriting that permission to
its child file. The current user token read is capped at 64 KiB; no SID or
captured content is logged. System administrators can still take ownership;
this is not protection against administrative access. Failures remove only
the known staging file and directory, never other files.
Before starting an engine, the opened directory's volume must advertise
persistent ACLs and its read-back DACL must be protected and byte-identical to
the generated current-user-only ACL. A filesystem that ignores these permissions
is refused while the directory is still empty.

After encoding, header validation and the existing ffprobe verifier require
one stream, the selected container/codec, exact dimensions/frame count and the
same frame-rate/GIF timing checks as Unix recording. The file is opened with
write access and synchronized before same-filesystem no-replace hard-link
publication. Windows directory-entry crash durability is not confirmed by a
parent-directory flush; a successful command is not a power-loss guarantee.
No output is published on encoding, overflow, deadline or verification failure.
If cleanup or stdout fails after publication, the error says the recording was
already published. There is no rollback of a completed destination.

Ctrl+C or abrupt process termination uses Windows process behavior; a custom
cooperative cancellation handler is not installed. Closing the controller's
job handle terminates contained engines, but forced termination may leave the
private staging directory. There is no automatic janitor. Neither FFmpeg nor
ffprobe is bundled, installed or downloaded, and the installed FFmpeg build's
licensing is separate from this application.

Verification used a Windows-target cross-check and an isolated optimized test
crate under Wine. Five synthetic process/helper fixtures passed fixed encoder
arguments, private staging admission/cleanup, pre-write output bounds,
probe/stderr caps, nonzero exits and deadline termination of a descendant
holding inherited pipes. These do not exercise the complete real FFmpeg
encode/probe/publication pipeline. The Unix recorder's 11 default integration tests
also passed after sharing only its unchanged verifier. Real Windows FFmpeg,
gdigrab capture, native Windows filesystem/ACL behavior, login/session/permission behavior and
hardware CPU/memory/latency acceptance were not exercised by those checks.
An ignored Windows-only integration fixture runs installed Windows FFmpeg and
ffprobe on synthetic `test` input for both Matroska and GIF, checking output
headers and verified dimensions/frame count. It performs no screen capture and
does not install engines; run it deliberately where those engines are available.
