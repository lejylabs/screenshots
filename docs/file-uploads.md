# Explicit arbitrary-file uploads

`screenshots upload-file PROFILE FILE` uploads one regular file without
decoding, converting, inspecting its extension or changing its bytes. Empty
files are accepted. The maximum size is 64 MiB. Relative input paths resolve
against the caller's current directory; the core API requires an absolute path.
Folders, unavailable files and larger files fail before a request is sent.

`PROFILE` uses the [documented ShareX profile subset](../README.md#capture-home)
for a case-insensitive `.sxcu` extension; every other extension uses native
destination JSON. Contents are not auto-detected. Both require a regular file
of at most 16 KiB, including symlinks to regular files. The profile's optional
name is ignored; loading it creates no saved profile, history or credentials.
Credential-management commands remain native-JSON-only.

The command supports multipart POST and raw binary PUT. Both use
`application/octet-stream`; multipart uses the generic filename `file.bin`,
preserving the privacy of the source filename/path.
POST form fields, custom nonsecret headers/query parameters, HTTPS defaults,
origin-scoped credentials and existing response parsers remain available.
Image-specific JSON/base64 request bodies are rejected before the upload runtime
starts. A `.sxcu` profile must include `FileUploader` or the previously supported
`ImageUploader` token and use HTTPS. The explicit file commands accept
FileUploader-only profiles; image commands and UI import still require
ImageUploader. JSON bodies are rejected even when both destination tokens are
present. The profile parser, secret rejection and transport limits remain the
same bounded subset, not full ShareX/FileUploader compatibility.

Configured `.sxcu` `ErrorMessage` templates are rejected before network access;
absent, null or empty/whitespace strings are accepted. The app does not silently
discard a requested error parser or expose its potentially sensitive template.

Official [ShareX FileUploader](https://github.com/ShareX/ShareX/blob/develop/ShareX.UploadersLib/FileUploaders/CustomFileUploader.cs)
and [ImageUploader](https://github.com/ShareX/ShareX/blob/develop/ShareX.UploadersLib/ImageUploaders/CustomImageUploader.cs)
implement multipart and binary upload bodies. The app's existing object-valued
JSON `Data` and bare `{base64}` template are a local image-profile extension;
they are not an official interoperable image/file `.sxcu` representation.

The source is opened once and its handle's size fixes the Content-Length. The
stream reads at most that original length, so a growing file cannot increase
the request. A shrinking file can fail the transfer. Concurrent in-place changes
can change the bytes being sent; callers that need immutable input must avoid
modifying it while uploading. No local file copy or full-file buffer is created.
The existing 64 KiB file/stream buffers, per-operation HTTP runtime and client
are reused, with the same 5-second connection and 60-second operation deadlines.
No resident service, timer, cache or extra worker pool is added.

Success prints one JSON object with `url`, `thumbnail` and `deletion`; optional
links are null when absent. The links are not opened or fetched. The operation
does not copy the clipboard, save history, inspect archive contents or create
local output files. It sends data only because the explicit command was invoked.
`upload` and `upload-images` keep their separate PNG/image validation; arbitrary
bytes do not become accepted by those commands.

Redirects and retries remain disabled. An interrupted or failed transfer may
already have created a remote object; the command does not retry or remove it.
PUT retains `If-None-Match: *`; no-overwrite behavior depends on the remote service
honoring that precondition. Native credential stores and actual provider
interoperability remain separate acceptance checks.

Subprocess fixtures exercise exact binary PUT bytes including NULs, multipart
generic filename/MIME, empty input, oversized/directory/JSON-body rejection before
networking, unchanged source bytes and absent user-state writes. Tests use local
HTTP and generated files only, without real credentials or remote accounts.
The opt-in direct-profile fixture uses isolated local HTTPS trust to verify
`.sxcu` and `.SXCU` across all three upload commands, exact PNG/file bytes,
batch NDJSON, and unchanged source/profile files. Invalid and secret-bearing
profiles are rejected before connecting; native JSON selection remains tested.
The FileUploader extension passed this same explicit TLS fixture with binary
PUT and multipart POST; the entire fixture made ten loopback HTTPS uploads.

A successful remote upload followed by a stdout failure returns an error that
explicitly says the remote file remains; the client does not retry. Native
filesystem operations are not covered by a hard wall-clock timeout: Tokio's
blocking file calls can outlive cancellation, and runtime teardown waits for
them. The shared PNG/file opener performs regular-file/size admission before
and after opening one handle on the existing blocking pool. On Unix it uses
`O_NONBLOCK`, so replacing the pathname with a FIFO cannot wait for a writer.
This does not promise interruptible network/FUSE/device calls or an atomic
snapshot of a concurrently modified regular file. Other platforms retain
their native open behavior; avoid concurrently replacing upload inputs.

## Explicit file batches

`screenshots upload-files PROFILE FILE...` accepts 1–20 files and uses the same
profile rules and unchanged-byte transport as `upload-file`. All paths receive
metadata/type/size preflight as regular files of at most 64 MiB before the first
request; profile and absolute input path strings are limited to 4096 native
encoded bytes. Empty files and duplicate paths are allowed. A duplicate is
uploaded again in argument order. Symlinks to regular files remain usable.
Preflight is not a snapshot: each handle is opened and revalidated only when
that file is uploaded, so a later open/read failure can follow earlier success.

One file is uploaded at a time, with no decode, local copy or retained batch
buffer. Each completed request emits and flushes one NDJSON object containing
`type: "uploaded"`, `index` (one-based), `total`, `url`, `thumbnail` and `deletion`.
Full completion adds `{"type":"complete","uploaded":N}`. On a later failure,
earlier lines remain valid; no completion line, retry or later upload follows.
The failed request may have created a remote file. A stdout failure can leave
the current line partial after the remote upload; the error identifies the
current ordinal and prior confirmed result count.

The existing 5-second connect/60-second per-request limits apply; there is no
separate whole-batch deadline. Native calls retain the limits above. Binary PUT
profiles reuse their configured endpoint and `If-None-Match: *`, so servers that
enforce create-only semantics may reject the second file unless the endpoint
supports distinct remote objects. The command does not silently alter URLs,
discover names, open links, update history or copy the clipboard.
