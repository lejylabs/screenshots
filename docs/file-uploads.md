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
`ImageUploader` token and use HTTPS. Only this explicit file command accepts
FileUploader-only profiles; image commands and UI import still require
ImageUploader. JSON bodies are rejected even when both destination tokens are
present. The profile parser, secret rejection and transport limits remain the
same bounded subset, not full ShareX/FileUploader compatibility.

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
them. A stalled filesystem or a concurrent replacement by a special file can
therefore delay return beyond the HTTP deadline. Initial metadata validation
rejects obvious nonregular files, but it is not an atomic no-special-file open
guarantee. Avoid concurrently replacing upload inputs.
