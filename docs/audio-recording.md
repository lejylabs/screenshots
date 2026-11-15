# Finite audio recording

Author: NewtTheWolf
Date: 2026-11-15
Status: implemented; focused optimized and installed-engine synthetic checks
passed. Native device and cross-platform runtime acceptance remain separate.

## Scope and exact command

Add an explicit headless audio recording command using the separately installed
FFmpeg and ffprobe already used for screen recording:

```text
screenshots record-audio test OUTPUT.wav --seconds N [--channels 1|2]
screenshots record-audio pulse|alsa OUTPUT.wav --seconds N --device NAME [--channels 1|2]
screenshots record-audio dshow OUTPUT.wav --seconds N --device NAME [--channels 1|2]
screenshots record-audio avfoundation OUTPUT.wav --seconds N --device INDEX [--channels 1|2]
```

`test` is synthetic on all supported desktop platforms. `pulse` and `alsa` are
Linux inputs; `dshow` is Windows; `avfoundation` is macOS. No native device is
opened during startup, parsing, enumeration or synthetic tests. Native recording
requires the explicit command and device argument. The command records only
audio, with no video, screenshot, clipboard, upload, history or user settings.
Native microphone/system-mix source availability and OS permissions remain
properties of the selected device and installed FFmpeg build. Pulse monitor
sources can record a system mix when explicitly selected; this does not create
an implicit loopback source on other systems.

Seconds are 1–300, output rate is fixed 48,000 Hz, and the default is stereo;
`--channels 1` selects mono. Require paired options, reject unknown/duplicate
options and enforce a case-insensitive `.wav` destination of at most 4096 native
encoded bytes. Refuse every existing final path, including broken symlinks,
before starting an engine. Parent directories must already exist. Device names
are 1–256 UTF-8 bytes without controls or leading/trailing whitespace. Pulse
names admit ASCII letters/digits and `_.@-`; ALSA names additionally admit
`:`, `,` and `=`. DirectShow uses one friendly audio-device name and rejects
colon, equal-sign and quote separators so a second/video device cannot be
injected. AVFoundation accepts only a nonnegative decimal audio index or
`default`, passed as `none:INDEX` so no video device is selected. Do not print
device names in errors or successful JSON. No shell invocation is involved.

The output codec is signed 16-bit little-endian PCM in RIFF/WAVE. It requires
no lossy encoder, codec pack or application-distributed FFmpeg library. This
does not change the license of the user's external FFmpeg build. Screen/GIF
recording keeps its existing codec policy and public result schemas.

## Process, validation and publication

Reuse the existing recorder process isolation rather than add another runtime
or child-process framework. On Unix reuse the process-group engine, bounded
diagnostic drain, SIGINT/SIGTERM handling, parent-death policy on Linux and
`RLIMIT_FSIZE`/zero core-file limit. On Windows reuse the suspended process,
Job Object, bounded stdout-to-file pipe and private ACL staging directory.
Only narrowly expose the existing private recorder helpers needed by this
command; do not change their behavior or screen-recorder arguments.

The deadline is seconds plus ten, covering encoding, validation and publication.
The existing 256 MiB child file/output cap remains; at the maximum settings the
WAV has 57,600,000 PCM bytes plus a 44-byte header. Each FFmpeg allocation-helper
call remains capped at 64 MiB, not a whole-process allocation guarantee. One
engine runs at a time. No resident timer, stream, runtime or worker is added.

Use fixed FFmpeg audio mapping, disable other media types, strip metadata and
chapters, resample to 48 kHz, limit output to the exact requested sample count,
and write a bit-exact PCM WAV. Native input ends early or an incorrect number of
samples fails verification; do not silently pad silence. FFmpeg diagnostics are
bounded and discarded rather than logged, preserving device/audio privacy.

Require exactly the expected 44-byte RIFF/WAVE PCM header and exact overall
length `44 + seconds * 48000 * channels * 2`, regular-file type and the existing
output cap. The Windows nonseekable WAV pipe writes unknown RIFF/data lengths;
after a successful engine exit, replace only those two unknown size words with
the already checked lengths, then sync. Unix sized headers must already match.
Accept neither extra chunks nor malformed/rate/channel/bit-depth mismatches.
Run the existing bounded ffprobe engine over the local forced WAV demuxer and
require one PCM s16le audio stream, matching channel/rate/bit depth, 1/48000 time
base and exact duration_ts sample count, and WAV format. No network protocol is
enabled in the probe.

Sync and publish with same-filesystem no-replace hard linking and preserve the
existing platform durability behavior. Clean only the known private staging
file/directory. Cancellation, timeout, engine/probe failure or malformed output
must publish nothing. After successful publication, later sync/cleanup/stdout
failure must explicitly say the WAV was published and remains; do not retry or
delete the completed destination. Power loss/SIGKILL can leave private staging;
the existing unfinished-file maintenance behavior applies. Native process,
filesystem or kernel calls are not claimed hard-interruptible.

Success prints one flushed JSON object with `mode`, `container: "wav"`,
`codec: "pcm_s16le"`, `seconds`, `sample_rate: 48000`, `channels`, `samples` and
`bytes`. `samples` is the per-channel sample-frame count. No device/path/audio
payload is returned. A failed stdout write after publication states that the
recording was published; a process signal remains able to terminate blocked
result output through the reused Unix recorder handling.

## Resource cost

The Rust process retains only small command/probe structures, a fixed 44-byte
header and the existing bounded process drains. PCM is streamed by FFmpeg to
one staging file, never collected in a Rust Vec or reread into an audio buffer.
Validation reads the header and bounded probe JSON only. Two external phases
run sequentially, not concurrently. Existing codec/OS buffers are additional
costs; measure combined active CPU/RSS separately from idle or hardware budgets.
The feature adds no dependency, persistent cache, application thread pool or UI.

## Files and verification

Create `apps/desktop/src/audio_record.rs`, `apps/desktop/tests/audio_record.rs`
and this document. Root adds module/CLI routing. The only shared production
changes are visibility of the current Unix `Cancellation`, `Staging`, `engine`
helpers and the Windows `Staging`, `engine` helpers; staging path access remains
crate-private. Keep all existing screen-recording tests intact.

Implementation order: expose unchanged helpers, implement strict arguments and
fixed encoder/probe/header verification, then executable fixtures and real
synthetic verification. Run focused optimized audio and recording tests, format,
Clippy and Windows GNU compilation before the full regression/release checks.

Deterministic tests cover argument/device/output preflight, platform modes,
fixed encoder arguments, PCM header/length/probe mismatches, no overwrite,
staging cleanup, bounded diagnostics/probe data, timeout and signal cleanup,
source/user-state preservation and truthful saved-output/stdout failures. Reuse
the existing engine lifecycle fixtures rather than repeat every screen-specific
process test. A real installed-engine synthetic fixture records mono/stereo,
independently verifies WAV parameters and exactly 48,000 frames per second,
and checks that samples contain a signal without opening a physical device.
Native audio-device runtime acceptance is listed separately and never inferred
from synthetic input or cross-compilation.

Upstream input and container contracts:
[PulseAudio/ALSA/DirectShow/AVFoundation](https://ffmpeg.org/ffmpeg-devices.html),
[WAV muxer](https://ffmpeg.org/ffmpeg-formats.html#wav),
[FFmpeg options](https://ffmpeg.org/ffmpeg.html),
[ffprobe](https://ffmpeg.org/ffprobe.html).

## Completed focused verification

The optimized application unit filter passed four audio-contract tests. The
actual CLI target passed five regular cases covering preflight, strict output/
probe validation, diagnostic overflow, no-overwrite publication, SIGINT engine
cleanup and published-output/stdout failure. Its explicitly invoked installed-
FFmpeg fixture also passed, recording only synthetic 440 Hz mono/stereo audio
and independently checking exactly 48,000 sample frames, signed PCM16 data,
channel layout and nonzero signal. No native microphone/source was opened;
the isolated user-state paths remained absent and staging was removed.

Commands/results are logged locally in `target/audio-delivery-unit.log`,
`target/audio-delivery-cli.log` and `target/audio-delivery-real.log`. These
checks do not establish hardware capture, latency, memory budgets or Windows/
macOS native runtime acceptance. The root coordinates full regression, platform
compilation, final review and release/package evidence separately.

## Combined screen and audio recording

Extend the existing Matroska `record` command with explicit optional audio:

```text
screenshots record test OUTPUT.mkv --seconds 2 --fps 10 --width 640 --height 360 --audio-mode test
screenshots record x11 OUTPUT.mkv --seconds 10 --fps 15 --width 1280 --height 720 --audio-mode pulse --audio-device default --audio-channels 2
```

`--audio-mode`, `--audio-device` and `--audio-channels` use the same validated
native/synthetic audio input as `record-audio`. Device/channels without an audio
mode fail before recording. Audio is rejected for GIF. Omitting all audio
options retains the existing screen arguments, one-stream validation and JSON
result shape. Native video and audio devices are opened only for explicit
combined recording. The selected platform must support both input modes.

For combined capture FFmpeg reads one video and one audio input, explicitly
maps only their first respective streams, encodes FFV1 and 48 kHz PCM16, and
retains the existing frame-count and 256 MiB output bounds. Audio resampling,
trimming and timestamp normalization produce exactly `seconds * 48000` sample
frames; no padding or whole-recording buffer is added. Group PCM output into
one-second packets so the independent per-frame sample-count probe remains
bounded (at most sixty expected packets and the existing 8 KiB probe ceiling).

The first probe requires exactly one matching video and one PCM audio stream.
A second local-only, audio-selected ffprobe invocation decodes frame metadata;
the sum of bounded positive `nb_samples` values must equal the requested sample
count. This avoids inferring sample count from Matroska's millisecond stream
time base. Both probe phases share the existing seconds-plus-ten deadline and
run sequentially. Any short audio/video, stream mismatch, extra stream, probe
overflow, codec failure or timeout prevents publication. Success adds one
`audio` object containing mode, codec, sample_rate, channels and samples; it
never includes a device name.

Audio PTS is normalized to sample time. Native video retains its existing input
timestamps; this does not claim hardware-clock synchronization, drift correction,
or an exact wall-clock capture duration. A delayed video source can stretch the
video timeline while the exact nominal audio sample duration remains fixed.
The command reports failure for missing samples, rather than silently padding.

Changes stay in `audio_record.rs`, `record.rs`, `record_windows.rs` and their
audio/record tests plus this document. Share audio input/PCM argument helpers;
do not duplicate process isolation. Unit tests cover option combinations,
GIF/device rejection, fixed input mapping, two-stream validation and summed
sample counts. Actual executable tests retain the omitted-audio regression,
reject partial combined outputs, and run installed FFmpeg with synthetic video
and sine audio only, checking exact frames/sample count and no user-state writes.

Focused optimized combined verification passed: eleven recorder/audio unit
checks, seven regular audio/combined CLI cases and twelve existing video/GIF
recording cases. Both installed-engine fixtures passed. Combined synthetic
captures at 3, 7 and 30 FPS with mono/stereo audio were independently decoded
to PCM and probed for video frames, preserving exactly 48,000 audio sample
frames and the requested FFV1 frames per second. Logs are in
`target/audio-av-{unit,cli,real,record-regression}.log`; final fixed-error/
sample-probe-bound assertions are rechecked in `target/audio-av-final-unit.log`.
This is synthetic engine evidence, not native device or hardware-clock
synchronization acceptance.
