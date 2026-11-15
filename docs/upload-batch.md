# Explicit image upload batches

`screenshots upload-images PROFILE_JSON IMAGE...` sends one to twenty PNG,
JPEG or WebP images through the existing uploader. The command itself is the
explicit network action; merely opening images, loading profiles or starting the
desktop app does not upload them. It changes no source file, clipboard, capture
history or settings. Duplicate paths are allowed and upload again in argument
order. Relative paths resolve against the caller's current directory; native
non-UTF-8 paths remain usable where the OS supports them.

The complete argument count, destination profile and each input's regular-file
metadata/32 MiB size limit are checked before the first request. Image contents
are decoded one at a time with the shared loader's 256 MiB decoded admission,
EXIF orientation and WebP validation. Each decoded result is flattened into an
in-memory PNG, without original metadata, before transfer. The source pixels are
released before networking and the PNG/request/runtime are released before the
next image starts. The existing PNG encoder/transfer limit is 272 MiB. Decoder
and encoder scratch, WebP's at-most-32 MiB immutable input snapshot and transport
copies remain part of the active-image resource accounting.

Transfers are sequential. Existing HTTPS requirements, optional explicit
anonymous HTTP, origin-scoped native credentials, response validation, no
redirects/retries and the 60-second per-upload deadline are unchanged. The command
does not expose a parallelism setting or initialize a batch service, background
watcher, idle timer, cache or new worker pool. Authentication is read from the OS
store by the existing uploader, never from secret command-line arguments.

## Result stream and partial failures

Unlike single-image commands, a batch prints **newline-delimited JSON** (NDJSON).
Each confirmed upload is written and flushed immediately:

```json
{"type":"uploaded","index":1,"total":2,"url":"https://example.invalid/one","thumbnail":null,"deletion":null}
{"type":"uploaded","index":2,"total":2,"url":"https://example.invalid/two","thumbnail":null,"deletion":null}
{"type":"complete","uploaded":2}
```

`index` is one-based and `total` is the number of input arguments. Optional
thumbnail/deletion result URLs are strings or null; the application never fetches
or invokes them. Only a fully successful batch emits `type:"complete"` and exits
zero. A decoding, encoding, transfer or output-stream failure stops the batch and
exits one. Previously flushed `uploaded` lines preserve confirmed result links;
they are not rolled back or hidden. A concise error states the failing ordinal
and completed count. The failing remote request may already have stored bytes,
so automatic retries or remote deletion are not attempted. Local preflight errors
occur before any result line or request. Content validation occurs per item and
can therefore fail after earlier successful items.

Ctrl+C follows the headless process-termination behavior. It may interrupt the
current upload after remote receipt; earlier complete NDJSON lines remain the
only confirmed outcomes. Consumers must require the final summary to consider
the whole batch complete. If stdout itself fails, the current remote result may
have succeeded without a recoverable link; this is reported as command failure.

## Verification

Real-executable fixtures use only synthetic PNG/JPEG/WebP files and a local HTTP
server. They inspect the uploaded PNG pixels, preserve source bytes, exercise a
failure on the second transfer, prove no third request, reject invalid argument
counts and metadata before networking, and verify no user-state writes. Native
credentials and real third-party services remain separate acceptance checks.
This adds explicit image batching, not arbitrary-file uploads, queued workflows,
watched folders or a batch UI.

For distinct remote files, choose an endpoint that creates a new resource per
upload, typically POST. A PUT profile reuses the exact same configured URL and
sends the existing `If-None-Match: *` precondition each time. A conforming server
can accept the first item and reject the second with 412; this command stops and
preserves the first emitted link. It does not invent destination URLs or bypass
server preconditions.
