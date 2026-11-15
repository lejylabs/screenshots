# Explicit S3-compatible object upload

Author: NewtTheWolf
Date: 2026-11-15
Status: implemented; final regression/release verification in progress, not provider acceptance.

## Commands and profile

Add `s3-upload PROFILE FILE`, `s3-credentials target PROFILE`,
`s3-credentials store PROFILE` and `s3-credentials remove PROFILE`. Store accepts
only piped JSON with `access_key_id` and `secret_access_key`, at most 4096 bytes;
credentials never appear in arguments, profile JSON or output. It does not read
1Password, discover environment credentials or provision a bucket.
The native Windows generic-credential blob is limited to 2560 bytes; normalize
the validated key fields to compact JSON before storage, and fail rather than
truncate if the native limit is exceeded.

The strict 16 KiB profile contains `endpoint` (canonical HTTPS origin), `region`
(explicit, `auto` for R2), `bucket` (3–63 lowercase DNS-style characters, labels
starting/ending alphanumeric, no empty labels or IP address), `key_prefix`
(optional prefix of at most 512 bytes: nonempty segments using ASCII letters,
digits, `-._~`, separated by one slash; no leading/trailing slash or dot/dot-dot
segments) and `credential` (1–64 ASCII
letters/digits/underscore/hyphen).
Region is 1–64 lowercase ASCII letters/digits/hyphens. Access-key IDs are
1–128 ASCII letters/digits; secret access keys are 1–256 ASCII letters/digits
or `+`, `/`, `=`. Credential JSON uses literal strings without escape sequences;
unsupported spellings fail before storage or network access.
Reject unknown fields, credentials in URLs, query strings/fragments, endpoint
paths, unsafe bucket/prefix names and templates before reading input or secrets.
Use path-style addressing; generate one UUID `.bin` key below the prefix. Never
copy a local filename into the object key. Return `bucket`, `key` and `etag`;
these identifiers do not imply a public download URL.
`etag` is a string or null. Only HTTP 200 confirms this first PutObject path;
409/412, redirects, duplicate/non-ASCII/oversized ETags and other statuses fail
without retry. An ETag is treated as an opaque string of at most 256 printable
ASCII bytes; it is not a verified checksum. Response validation or
stdout failures after an HTTP 200 still report that a remote object may exist.

## Transfer and security

Single regular file, 0–64 MiB, one open source handle. Hash SHA-256 from that
handle in bounded chunks, rewind and stream only its admitted length. Check
metadata before sending; a concurrent mutation after hashing may still fail
server-side hash verification. No staged local copy or whole-file buffer.
Send one signed PUT using AWS4-HMAC-SHA256 and `If-None-Match: *`. Sign host,
date, payload hash, content type and precondition; preserve exact canonical path
encoding. Use existing HMAC/SHA-256 crates in the lockfile, not hand-written
cryptography or the full AWS SDK. HTTPS only, no proxy, redirects, retries,
credential fallback, ACLs, multipart upload, STS or automatic refresh.

Store access/secret keys in a separate versioned native payload scoped to
`Screenshots/s3/<canonical-origin>/<bucket>/<reference>`, zeroizing plaintext
buffers. Reuse the existing Secret Service/Keychain adapter and Windows generic
credential APIs with bounded raw payloads. A locked/unavailable store fails
explicitly without a prompt. File parsing, native secret reads and native file
calls retain their documented OS blocking limitations.

One explicit operation owns its HTTP runtime/client and releases them afterward.
Bound connect to five seconds, transfer to 60 seconds, read/hash buffers to
64 KiB, ETag to 256 bytes and response admission to 1 MiB. The 60-second deadline
covers hashing plus request/response work, beginning after native credential
lookup; secret-store and native filesystem/kernel stalls are not interrupted by
that cooperative deadline. Check a cooperative
deadline while hashing; native calls themselves cannot be interrupted reliably.
On failure the remote object may exist; do not retry or delete it. No resident
worker, polling, GPU allocation, UI, history or clipboard change is added.

## Files and verification

Create core `s3_upload.rs` for profile/signing/transfer and `s3_credentials.rs`
for native storage. Modify core Cargo.toml/lib.rs for existing crypto crates and
exports. Create desktop `s3.rs`, add CLI routing/help/main module, executable
rejection tests, `scripts/test-s3-upload.py`, README and gap/delivery records.
Reuse local TLS fixture
patterns, isolated synthetic secret entries and official signing examples.

Test known AWS signatures, path/header encoding, strict profiles, source
admission, missing/locked credentials, exact signed loopback request/body,
conditional-write failure, no redirects/retries, output bounds and unchanged
source/state. Real AWS/R2 credentials and provider acceptance remain unavailable;
no real bucket uploads are required to finish this code slice.

The explicit Linux fixture `scripts/test-s3-upload.py` generates a local TLS
certificate, stores one uniquely scoped synthetic key through the real CLI and
verifies three HTTPS PUTs with independent Python HMAC/SHA-256 code. It checks
actual signed headers, payload hashes, a binary and empty file, HTTP 412 without
retry, unchanged source/profile/state and key deletion afterward. It does not
contact AWS/R2 or change system TLS trust. Requirements are Python, OpenSSL, an
unlocked local Secret Service and an optimized application binary:

```sh
python3 scripts/test-s3-upload.py --binary target/release-fast/screenshots
```

The optimized fixture passed. A separate native-store test round-tripped raw
bytes under another unique S3 target and deleted it. Windows GNU compilation
passed; Windows Credential Manager and macOS Keychain runtime acceptance remain
open for the new raw payload. The reused native source module is compiled into
each credential namespace; this adds code only, no duplicate persistent store
connection or resident service.

References: [AWS signing and vectors](https://docs.aws.amazon.com/AmazonS3/latest/developerguide/sig-v4-header-based-auth.html),
[AWS conditional writes](https://docs.aws.amazon.com/AmazonS3/latest/userguide/conditional-writes.html),
[R2 endpoint/region compatibility](https://developers.cloudflare.com/r2/api/s3/api/),
[R2 conditional operations](https://developers.cloudflare.com/r2/api/s3/conditional-operations/).
