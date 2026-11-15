# Sharing providers, OAuth and custom-uploader completion

Author: NewtTheWolf
Date: 2026-11-15
Status: implementation plan; the mechanisms below are not yet runtime evidence.

Transport update: SFTP/FTPS/FTP and their credential mechanism are implemented
with the focused evidence below. OAuth and the remaining custom-uploader
execution sections are still a plan; they are not claimed complete here.

## Scope and source inventory

Complete the non-UI transfer/provider mechanisms with explicit SFTP, TLS-first
FTP, OAuth authorization-code/PKCE and refresh, plus the remaining custom-
uploader execution behavior. Keep existing HTTP/S3 interfaces and user settings
compatible. No provider initializes, logs in, reads secrets, opens a browser,
uploads or polls while the application is idle. No real account is required for
local isolated integration tests. Native/provider account acceptance remains a
separate gate; it is not an excuse to omit implementation.

The current official ShareX `CustomUploaderItem` schema contains Version, Name,
DestinationType, RequestMethod, RequestURL, Parameters, Headers, Body, Arguments,
FileFormName, Data, URL, ThumbnailURL, DeletionURL and ErrorMessage. Official
image/file uploaders send multipart or binary bodies; text/URL workflows also
use None, FormURLEncoded, JSON and XML. `Data` is a template string, not the
app's existing object-valued JSON extension. Preserve that local extension
for existing profiles without representing it as ShareX interoperability.

Official syntax supports escaped/nested function calls and filename, input,
response, responseurl, header, base64, random, select, inputbox/prompt,
outputbox, json, regex and xml functions. The existing importer implements only
a static subset. Full .NET regex/JPath/XPath semantics cannot be inferred from
the current linear Rust regex and restricted XML selectors. Bounded extensions
must explicitly identify unsupported constructs rather than silently change
them. Headless prompt/select calls require supplied answers; do not introduce
unattended prompts or implicit UI. No GPL ShareX implementation code is copied;
use its public configuration/protocol behavior as compatibility references.

## Transfer profiles and commands

Add versioned strict JSON transfer profiles and these headless commands:

```text
provider-upload PROFILE FILE
provider-credentials target|store|remove PROFILE
oauth login|refresh|remove PROFILE
```

Profile files are regular, at most 16 KiB, read once through a frozen extent
with Unix nonblocking open. Paths are at most 4096 native bytes. Transfer
profiles contain `version:1`, `protocol:sftp|ftps|ftp`, canonical host, explicit
port, an absolute remote directory of safe path segments, a credential
reference and optional public HTTPS base URL. SFTP requires a base64 SHA256
known-host public-key pin; no TOFU, ignored mismatch or ambient host-key bypass.
FTP is rejected unless `allow_plaintext:true`; FTPS never falls back to clear
text. TLS certificates are verified by the installed curl trust store, with an
optional explicit CA file for self-hosting/test trust. No `insecure` option.

Each upload generates a fresh UUID remote filename and sends unchanged bytes.
Do not expose the source filename/path remotely. Return protocol, generated
remote key, byte count and optional public URL; this is a confirmed transfer
result, not proof of remote durability or an authenticated public download.
Generated names avoid intentional replacement of user-named remote objects;
FTP/SFTP lack a universal conditional-write guarantee, so do not claim the
same semantics as S3 `If-None-Match:*` across arbitrary servers. No retry,
remote cleanup or automatic follow-up request occurs after ambiguous failure.

The exact transfer JSON schema is:

```json
{"version":1,"protocol":"sftp","host":"files.example.invalid","port":22,
 "remote_directory":"/uploads","credential":"images",
 "host_sha256":"BASE64_SHA256_HOST_KEY","public_base":"https://cdn.example.invalid/uploads/"}
```

Only these keys plus `allow_plaintext` and `ca_file` are accepted. Protocol/port
are explicit (1–65535); hosts are canonical lowercase ASCII DNS names or IP
literals, never URLs, userinfo or embedded ports. `remote_directory` is `/` or
an absolute slash-separated sequence of 1–128-byte ASCII unreserved segments,
excluding empty/dot/dot-dot components, at most 1024 bytes total. FTP URL
construction explicitly preserves server-root directory semantics rather than
silently interpreting it relative to the account home. `public_base`, when
present, is an absolute HTTPS URL without credentials/query/fragment and ends
in `/`; append only the generated UUID filename. `ca_file` is an explicit
regular CA file path used only by FTPS. SFTP requires the host pin; other
protocols reject it. `allow_plaintext` may be true only for FTP.

Credential JSON is tagged `kind:password` with `username,password`, or
`kind:ssh_key` with `username,private_key,passphrase` (SFTP only). Username is
1–128 UTF-8 bytes without colon/control/edge whitespace. Password/passphrase is
at most 1024 bytes without control characters. Key paths are explicit regular
files, never a default discovered key. The versioned encoded credential payload
must fit 2560 bytes on every platform, checked before native-store access.
Credential targets are
`Screenshots/provider/PROTOCOL/SHA256_CANONICAL_HOST_PORT/REFERENCE`, with a
64-hex scope hash and 1–64 ASCII alphanumeric/underscore/hyphen reference.
OAuth uses the same exact-target discipline with an `oauth` protocol namespace
and an endpoint/client/reference hash. No native-store enumeration is added.

Local inputs must be regular files of at most 64 MiB, admitted before/after
opening one handle. Empty files are valid. Stream only that opened handle's
initial readable length using a 64 KiB buffer; growth cannot extend the upload,
shrinkage fails. Concurrent in-place writes remain caller responsibility. No
full encoded buffer, plaintext temporary file or secret-bearing config file.

`provider-credentials store` reads bounded JSON only from nonterminal stdin,
never arguments. Password credentials contain username/password; SFTP key
credentials may explicitly name a private-key path and optional passphrase.
Validate types/lengths before writing a separate native-secret namespace scoped
to protocol, canonical host, port and reference. Native secrets are absent from
profile JSON, backups, argv, stderr and success results. Only the explicit
operation accesses the named entry; no enumeration or guessed user-secret path.
Use a versioned compact payload and zeroizing allocations. Windows payloads
respect the actual Credential Manager blob cap; reject rather than truncate.

## Real curl transport and process bounds

FTP/FTPS use the separately installed curl executable. SFTP uses an isolated
private `provider-engine` mode of the application, calling raw `curl-sys` only
inside that child. The CLI cannot express an authentication mask that disables
all SSH agents across libssh/libssh2/Windows backends; password uploads must
also work when the server advertises public-key authentication first. The
worker therefore sets `SSH_AUTH_TYPES` to exactly PASSWORD or PUBLICKEY, never
AGENT or keyboard-interactive. Explicit private keys supply their own public
key derivation; the public-key-file option is explicitly empty. Host identity
uses the required SHA256 pin before authentication or source transfer.

Raw libcurl has no eager Rust wrapper constructor: global initialization happens
only inside an invoked private worker. Linux links the separately installed
system libcurl; that library must have SFTP and SHA256 host-pin support. Native
macOS/Windows packaging must provide its configured SSH-capable libcurl build;
the linked-library SFTP self-check runs on every native OS. Windows CI uses
pinned vcpkg static curl with SSH and TLS. Native linkage and its licenses are
part of packaging; an arbitrary installed CLI is not the SFTP backend.

Start the FTP curl executable with `-q` first to disable ambient configuration.
Supply fixed direct-connection/no-proxy, FTP-only protocol allowlist, no-retry,
no-redirect, silent arguments and five-second connect/sixty-second operation
limits. Remove proxy, SSH-agent, TLS session-key logging and debug/config
variables from every engine environment. FTPS requires TLS; passive FTP ignores
server-supplied PASV IP addresses so the control endpoint determines the data
host. No provider performs ambient agent/key discovery or credential fallback.

Credentials travel through a one-operation pipe carrying a bounded FTP curl
config or strict SFTP JSON;
source bytes travel independently through child stdin. On Unix the config pipe
is inherited only by that exact child at a fixed descriptor, referenced through
`/dev/fd`; no secret is written to a filesystem file. On Windows use a random,
first-instance, current-user-ACL named pipe rejecting remote clients. Connect
and writes use nonblocking pipe state, not an unbounded writer thread; verify
the client PID before releasing config bytes. Close the pipe after its bounded
config is delivered. Test that EOF is received without dropping buffered bytes.

Reuse existing process-group/parent-death/core-dump isolation on Unix and
suspended-start Job Object on Windows. A maximum sixty-second active operation
owns one child and three bounded operation-scoped drain/source workers with
256 KiB requested stacks. Diagnostics are counted/discarded at 16 KiB, result
stdout is capped at 4 KiB, config at 8 KiB. Wait/kill/reap/join on failure;
source read/native filesystem stalls retain the existing hard-cancellation
limitation and must not be hidden as a guaranteed deadline. There is no idle
watcher or permanent timer. Linux parent-death protection applies to the direct
child, not a malicious descendant escaping its managed group.

Accept success only from engine exit zero and bounded fixed result fields
confirming protocol success and exactly the admitted upload byte count. Error
diagnostics never include raw curl output, device/server responses or secrets.
After stdout failure, explicitly say the remote object remains; never retry.

## OAuth authorization and refresh

OAuth profiles name authorization/token HTTPS endpoints, public client ID,
bounded scopes, a credential reference and an explicit existing bearer-upload
destination. Bind an operation-local callback listener to 127.0.0.1 on an
ephemeral port, use `/oauth/callback`, a cryptographically random state, and
PKCE S256 with a 43–128-character high-entropy verifier. Redirect URI exactly
matches that ephemeral origin/path. Print a bounded authorization event for
the explicit login command; opening a browser is a separate explicit flag.
Real user consent cannot happen unattended; mock consent must never be called
real account acceptance.

Wait only during the explicit operation, at most 180 seconds. Accept bounded
local GET callbacks with exact Host/path, one code/state and constant-time
state comparison. Reject duplicates, wrong state, malformed percent encoding,
server error and oversized input without echoing query tokens. The browser
response is fixed success/failure text, never the authorization code or tokens.
The token POST has five-second connect/sixty-second operation limits, no proxy,
redirect or retry, a bounded 32 KiB body and strict bearer-token/expiry fields.
Loopback HTTP endpoints are allowed only through an explicit test/development
flag and only literal loopback addresses; normal accounts require HTTPS.

Store access tokens using the existing origin-scoped bearer credential entry,
so existing HTTP upload/task commands use them without a new secret argument.
Store refresh tokens and expiry metadata in separate scoped native entries.
`oauth refresh` uses the stored refresh token, replaces rotated tokens and
updates the bearer entry; no token appears in output. Providers lacking refresh
tokens support only their returned access-token lifetime, stated explicitly.
Storage partial failures are truthful and preserve successfully stored new
credentials instead of automatically deleting an unrelated previous entry.
Removal affects only the configured connection's exact entries.

### Concrete OAuth implementation contract (next delivery)

Add `crates/app-core/src/oauth_provider.rs` and a small explicit desktop CLI
adapter. No dependency or persistent runtime is needed: use existing reqwest,
Tokio, UUID randomness, SHA256, base64 and zeroizing allocations. Root wires the
module/exports and `oauth login|refresh|remove|target PROFILE` command. Login
prints an authorization event; a caller opens the URL externally. No automatic
browser launch, real consent, discovery, account access or background refresh.

Strict version-1 regular profile (16 KiB, one opened frozen extent) fields:
`flow:authorization_code|device_code`, `client_id`, `token_endpoint`,
`authorization_endpoint` (code flow only) or `device_authorization_endpoint`
(device flow only), `scopes` (at most 32 printable OAuth scope tokens of 128
bytes), `credential` (same 1–64 safe reference as the existing destination),
`upload_destination` (an explicit existing bearer profile), and optional bounded
`authorization_parameters`/`token_parameters` maps for provider audience/resource
parameters. Reserved protocol parameters and client secrets are rejected.
Endpoints are canonical HTTPS, no userinfo/query/fragment; an explicit CLI
`--allow-loopback-http` permits literal-loopback mock token/auth endpoints only.
The upload destination itself always retains existing HTTPS credential policy.
This implements native public-client OAuth; it does not pretend a desktop
embedded client secret is confidential.

Preflight freezes the existing destination, requires Bearer authentication and
matching credential reference, and derives its exact existing upload target.
Refresh metadata lives under `Screenshots/provider/oauth/SHA256/REFERENCE`,
where the hash binds the flow/endpoints, client ID, scopes, parameter maps and
upload target. Access tokens are stored through the existing private upload
credential API from app-core; refresh metadata is a separate versioned,
zeroizing native-store payload (at most 2560 bytes). Validate the complete new
response/payload before any write. Save refresh state first, then access token;
a failed second write reports the partial state explicitly. Refresh preserves
an omitted old refresh token and persists rotations. Removal always attempts
both exact entries and reports partial failure. No enumeration or plaintext
secret/profile updates occur.

Code flow binds only `127.0.0.1:0`, uses an exact origin/Host/path callback,
64-character high-entropy verifier/state and S256. Callback headers/request
line are capped at 8 KiB, at most 16 callback connections, short per-connection
I/O timeout and 180-second total login deadline. Reject duplicate/reserved
parameters, malformed percent encoding, wrong state, OAuth error and invalid
code without echoing them. Device flow uses RFC 8628: bounded device/user codes,
HTTPS verification URI, server interval (default 5 seconds), authorization-
pending and slow-down (+5 seconds), and a total deadline no longer than 180
seconds. Polling exists only during an explicitly invoked device login.

Every token POST has five-second connect/sixty-second request deadlines within
the remaining operation budget, no proxy/retry/redirect/decompression, a 16 KiB
encoded request and 32 KiB response cap. Active requests/callback waits observe
the caller cancellation closure; CLI uses the existing Unix cancellation
lifecycle through result output. Access tokens are bounded ASCII Bearer values
(at most 1024 bytes, fits Windows native storage); refresh tokens are at most
1024 printable ASCII bytes. `token_type` must be Bearer; optional `expires_in`
is a bounded positive integer. Return only expiry/refresh-availability and
storage status, never tokens or server diagnostics.

Focused tests cover the RFC PKCE example, profile/namespace admission, callback
ambiguities/state, token bounds, failed/rotated refresh storage, denied device
codes and pending/slow-down timing. An opt-in actual loopback identity fixture
runs the real CLI PKCE and device flows, validates request verifiers, writes
unique native credentials, performs refresh and removes both entries. Synthetic
identity tests are evidence of protocol execution, not real provider consent.

## Custom-uploader execution completion

Implement a separate bounded syntax/request layer, then route `.sxcu` execution
through it without weakening existing native-JSON validation. Preserve current
image/file admission and explicit upload semantics. Support official request
methods and body modes for their corresponding image/file/text/URL input type;
reject an invalid input/body combination before networking. Support string Data
templates, dynamic URL/query/header/form values, nested functions and multiple
response tokens. Validate the final URL/header/body after expansion and bind
credentials to the final canonical origin. Preserve link protocol/userinfo
checks and response/input/expansion limits. Authentication material is supplied
through references in native storage, not accepted into remembered plaintext.

Parse templates into a bounded tree: at most 16 nesting levels, 128 calls and
64 KiB expanded text, with existing response cap 1 MiB. JSON/XML selector and
regex work needs explicit node/operation/backtracking budgets; never substitute
an unbounded .NET-compatible engine for the linear one without isolation. Keep
unsupported constructs explicit, including any remaining .NET-only regex or
JPath/XPath feature. For headless interactive functions, use a separate bounded
answer-map argument; outputbox yields an explicit result event. Never silently
select the first choice or execute clipboard/file-read NameParser actions.
Configured ErrorMessage is evaluated only on unsuccessful HTTP responses and
returned through a sanitized bounded error result, not raw engine logging.
Old invalid/unsupported versions follow the official migration/rejection rules.

The official schema and function inventory above is the compatibility checklist.
Do not label a remaining restricted selector/regex implementation “full .sxcu
compatibility” until its defined supported behavior and rejection boundary are
tested against that inventory.

## Exact implementation boundary and order

1. Add `crates/app-core/src/provider_credentials.rs` and a crate-private native
   opaque-byte helper only if required to reuse existing Secret Service,
   Keychain and Windows Credential Manager handling. Root registers exports.
2. Add `apps/desktop/src/sharing_provider.rs`,
   `sharing_provider_unix.rs`, `sharing_provider_windows.rs`,
   `sharing_provider_worker.rs`, `apps/desktop/tests/sharing_provider.rs` and
   `scripts/test-provider-upload.py`. Root wires command/Task actions.
   Keep protocol/profile and credential validation separate from process work.
   Native library admission additionally reaches desktop/root Cargo manifests
   and lockfile, `vendor/curl-sys/build.rs` and `LOCAL-PATCH.md`,
   `packaging/windows/vcpkg.json`, `.github/workflows/rust.yml`, and native
   notices in `scripts/package.py` / `scripts/test-package.py`. The common
   native worker enforces SFTP authentication types on Unix and Windows;
   FTP/FTPS use external curl. These are operation-scoped processes and native
   build dependencies, not another resident runtime.
3. Add `apps/desktop/src/oauth_provider.rs`,
   `crates/app-core/src/oauth_provider.rs` and focused core/executable tests.
   Reuse existing HTTP runtime and native-secret adapters; no second resident
   runtime. Root coordinates the small upload-destination credential API reach.
4. After transport/OAuth pass, coordinate ownership of
   `crates/app-core/src/upload.rs` and add a dedicated
   `crates/app-core/src/sharex_syntax.rs` plus tests for the compatibility
   inventory; do not edit shared upload code concurrently with root.
5. Run focused optimized tests and isolated actual transports before full
   workspace/release/package checks. Record actual commands/results separately
   from design targets. Update only this document, relevant provider/upload
   documentation, README, delivery/status and review logs with completed facts.

Actual transport tests use private ephemeral SSH/FTP/FTPS servers and synthetic
credentials/keys/certificates, never a user's service or existing SSH key. Verify
exact binary/empty bytes, SFTP wrong-host-key refusal, FTPS certificate refusal,
plaintext-FTP permission, no config/password in argv/logs/disk, source extent,
deadline/cancel/descendant cleanup and ambiguous-success reporting. Mock OAuth
tests verify state/PKCE/code exchange, no redirect, expiry/refresh rotation,
native-store scoping and rejected callbacks. Custom-uploader fixtures cover the
entire published schema/function/body inventory and no secret persistence.

Primary contracts: [curl options](https://curl.se/docs/manpage.html),
[OAuth 2.0](https://www.rfc-editor.org/rfc/rfc6749),
[PKCE](https://www.rfc-editor.org/rfc/rfc7636),
[ShareX schema](https://github.com/ShareX/ShareX/blob/develop/ShareX.UploadersLib/CustomUploader/CustomUploaderItem.cs),
[ShareX syntax](https://github.com/ShareX/ShareX/blob/develop/ShareX.UploadersLib/CustomUploader/ShareXSyntaxParser.cs),
[ShareX functions](https://github.com/ShareX/ShareX/tree/develop/ShareX.UploadersLib/CustomUploader/Functions).

## Completed transport verification

Four provider contract units, two scoped native-credential units and four
actual CLI admission tests passed. The Unix config-pipe subprocess fixture
proves that only the child receives the synthetic config, no secret occurs in
argv, and a source grown after admission uploads only its original extent.
Windows GNU compilation passed. A minimal Windows-target harness includes the
actual unmodified `sharing_provider_windows.rs` module and its two pipe tests;
both passed under Wine, checking complete delivery/EOF and refusal before any
bytes reach a mismatched client PID. This is pipe emulation evidence, not
native Windows provider acceptance. The redundant full-app Windows test
build was stopped after this isolated proof; it also reported an unrelated
missing generated GPUI `shaders_bytes.rs` in the cross-target build directory.
That full link is not counted as passed. Native CI and the shader toolchain
remain separate from the successful metadata-only Windows compilation check.

The explicit `scripts/test-provider-upload.py` check passed against private
FTP/TLS FTP, OpenSSH key authentication, and a Paramiko 4.0 in-memory password
SFTP service. The latter advertises `publickey,password`, accepts only a
synthetic password and counts every key-authentication attempt. Eight exact
binary/empty uploads passed; valid password transfers made zero key attempts.
Wrong password, untrusted TLS, wrong SSH pin and missing plaintext permission
were refused. Each unique native credential entry was removed; source/profile
bytes and isolated user-state paths remained unchanged. A cleanup self-check
injects a credential-removal and server-close failure and verifies that every
other resource is still cleaned up. Paramiko is an opt-in test dependency and
is not loaded or installed by the application or regular Cargo tests.

The same check passed synthetic source PNG → opaque redaction recipe → saved
PNG → FTPS upload → Markdown link → QR PNG. Remote bytes equaled the saved
finalized PNG; independent FFmpeg decoding confirmed exact redacted RGBA pixels,
and QR scanning recovered the generated public link. The unredacted original
was preserved and was not uploaded.

Thirteen focused optimized provider/worker units passed, including an injected
outer cancellation during credential lookup, fixed source extent, native SFTP
capability/auth-mask checks, and an actual blocked-success-stdout child killed
by SIGTERM after operation cleanup. Standalone Unix operations reuse the
recorder's retired-operation signal lifecycle; Task uploads retain the caller's
same cancellation closure, checked immediately after native lookup and before
starting any transfer. No transfer-local replacement flag loses a pending
Task cancellation.

A second minimal Windows harness includes the unchanged production raw-worker
source and links a checksum-verified official curl 8.23.0_9 DLL. Under Wine it
passed binary/empty key uploads, wrong pin, password denial and invalid-key
refusal. A private hidden Pageant mock received zero requests. A deliberately
unsafe AUTH_ANY mutation with an invalid explicit key did contact the mock,
proving the negative control detects agent access. All windows were confined
to private authenticated Xvfb. This is Wine/official-DLL evidence; the production
native Windows vcpkg build and native account-free CI remain separate gates.

Current focused evidence: `target/provider-transport-review-unit.log`,
`target/provider-transport-review-e2e.log`,
`target/verification/provider-transport-review-e2e.json`,
`target/verification/provider-native-worker-harness/windows/actual-sftp.json`,
and `target/verification/provider-pipe-harness/{build,wine}.log`.
No full-workspace, frozen-release, real-account or reference-hardware gate is
closed by these focused checks; the root coordinates those separately.
