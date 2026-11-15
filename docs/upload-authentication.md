# Native upload credentials

Anonymous uploads continue to work without a credential. Authenticated uploads require HTTPS and an origin/reference-scoped native secret: Windows Credential Manager, Linux Secret Service or macOS Keychain. Missing, locked or unavailable stores stop the upload before sending; there is no anonymous or plaintext-file fallback. The existing credential-entry UI remains Windows-only. The native core save/read/remove APIs are implemented for all three platforms; no non-Windows credential-entry UI is claimed here.

## Windows setup

1. Set up or load an HTTPS destination in **Uploads**. Click **Save credential**, choose Bearer token, API key or Basic, enter a non-secret reference such as `main`, and for Basic a username. Enter the token, API key or password in the masked field. **Save credential (session destination)** writes the secret to Credential Manager for that HTTPS origin and reference, and attaches only the reference to this session. Click **Remember** if the profile should persist; the JSON still contains no secret.
2. JSON profiles remain supported. Copy [the Bearer profile example](upload-destination.bearer.example.json), [the API-key profile example](upload-destination.api-key.example.json), or [the Basic profile example](upload-destination.basic.example.json), set your actual HTTPS endpoint and response format, and choose a non-secret `credential` reference such as `main`. Load the profile, then use **Save credential** or continue with Credential Manager as below.
3. **Copy credential target** shows the canonical HTTPS origin (including non-default port) and reference name. You can still open **Control Panel → Credential Manager → Windows Credentials → Add a generic credential**, paste that exact target as the Internet/network address, use your actual account username for Basic (or `token` for Bearer/API-key), and enter your password, Bearer token or API key as the password. Do not put passwords or tokens in JSON, a URL, a shell command or an application log. Username is sent only in Basic mode.
4. Click **Test destination** to send a throwaway 1×1 PNG through the same authenticated path; it is not recorded in upload history. Click **Upload image** or **Upload PNG** in capture history for a real screenshot. The app reads the matching credential only for this explicit action and sends `Authorization: Bearer …` by default, or the raw `X-API-Key` value when `authentication` is `api_key`, or `Authorization: Basic …` when it is `basic`, over HTTPS. Explicit `authentication: bearer` is also supported; an explicit authentication mode requires a credential reference. Missing/invalid credentials stop the request before sending it.

The reference accepts 1–64 ASCII letters, digits, underscores or hyphens. Bearer tokens must use token characters (letters, digits, `-._~+/=`). API keys may use printable ASCII punctuation as well. Bearer/API-key modes reject spaces/control characters. Basic uses UTF-8 for a nonempty username/password, allows spaces and colons in passwords, and rejects control characters and colons in usernames. The combined UTF-8 username, colon and password must fit 4,096 bytes; the native username is also bounded. Servers requiring legacy Basic character encodings need separate interoperability validation. No arbitrary secret header name is configurable. Destination JSON may include up to 8 non-secret custom request headers; Authorization, X-API-Key, Cookie and other transport names stay reserved for this credential path. Windows Generic Credentials store a bounded UTF-16 password blob; the supported blob limit is Windows' `CRED_MAX_CREDENTIAL_BLOB_SIZE` (2,560 bytes, including any terminator), not an unlimited token store.

Changing HTTPS origin or port selects a different credential target. Windows reference names are case-insensitive: `main` and `MAIN` access the same OS credential. Preserve existing spelling and use distinct names rather than case-only differences; Linux/macOS references retain exact matching. Paths on the same origin share the configured reference. Redirects and retries remain disabled. **Remove credential** deletes the OS secret for the attached HTTPS origin/reference and returns an anonymous session destination; click **Remember** to persist that. **Forget destination** removes the app's profile and still leaves the OS credential in place. Saving a credential overwrites the OS secret for that exact target and leaves any previous reference's secret in place. Profile files contain only the credential reference. Upload history stores provider-returned sharing links, never an application-generated Authorization field. Those links may independently contain access parameters and are retained locally as documented.

Temporary password/token and decoded username copies, including Basic formatting/Base64 strings, are wiped after header creation, and the HTTP header is marked sensitive. HTTP/TLS libraries necessarily hold request data during transmission; this does not promise forensic erasure of every transport buffer. Native credential lookup and real HTTPS service interoperability still need manual acceptance on the deployment machine.

## Linux Secret Service and macOS Keychain

Linux uses `dbus-secret-service` 4.1.0 without its optional cryptography features.
The dependency's [SecretService API](https://docs.rs/dbus-secret-service/4.1.0/dbus_secret_service/struct.SecretService.html)
documents the zero-prompt-timeout behavior.
Operations open a user-session D-Bus connection only when explicitly requested.
`DBUS_SESSION_BUS_ADDRESS` must explicitly contain only local `unix:` addresses;
TCP, autolaunch, mixed/network addresses and missing addresses are rejected before
connecting, so the Plain session cannot carry secrets to a network bus.
The address is limited to 4 KiB and four alternatives;
the dependency bounds individual service method calls to two seconds. The native
Secret Service protects persistent secrets. The supported `Plain` session does
not add a separate encryption layer to the local D-Bus transport. The app does
not claim transport-buffer erasure or an end-to-end deadline for a series of
native calls. The dependency's optional DH implementation uses a noncryptographic
random generator, so it is deliberately not selected.

Entries use attributes `application=me.lejy.screenshots` and `target` equal to
the canonical `Screenshots/upload/{HTTPS origin}/{reference}`. Only exact scoped
items are requested. Locked matches, ambiguous duplicate matches and a locked
default collection are errors; the adapter never calls unlock. Prompt timeout
zero prevents create/delete authorization prompts from opening. Saving updates
one exact unlocked match or creates one item in the unlocked default collection;
it does not create a collection or fall back to a transient session collection.
Deletion is idempotent when no matching item exists, and never removes unrelated
entries. Missing D-Bus/provider access is an error.

macOS uses Security.framework generic passwords with service
`me.lejy.screenshots.upload`, account equal to the same canonical target, and
cloud synchronization explicitly disabled through
[PasswordOptions](https://docs.rs/security-framework/3.7.0/security_framework/passwords/struct.PasswordOptions.html).
Each request explicitly selects the Data Protection Keychain and supplies
`kSecUseAuthenticationUIFail`; legacy file-keychain fallback is disabled. This
requires a supported signed identity/entitlements; an unsigned CLI can fail
closed with an authorization error. The adapter does not change process-wide Keychain policy.
If access requires authorization, unlock/grant access outside this operation and
retry using the same signed application identity. Deletion treats `errSecItemNotFound` as already removed. The calling signed
application identity, login keychain permissions and macOS runtime behavior still
require native acceptance.

Both new stores keep a versioned binary payload containing username and secret
inside the OS secret value. No username/password is added to destination JSON.
Input validation precedes native writes; stored payloads are limited to 8 KiB
before interpretation, Basic username/password together to 4,096 UTF-8 bytes and
Bearer/API-key values to 2,560 bytes. Native libraries allocate returned secret
data before the application can check its length; this limit does not claim a
hard bound on malicious OS-service response allocations. Application-owned payload
vectors and temporary header strings use `Zeroizing`; native/library transport
copies remain outside that erasure guarantee. No shell command, command-line
secret, resident connection, worker thread, polling loop or cache is added.

Regular tests cover bounded payloads, UTF-8/injection rejection, canonical target
validation, fake-store origin separation and sensitive headers without contacting
the real secret store. The opt-in
`native_credential_round_trip_is_scoped_and_removed` fixture creates, reads,
overwrites and deletes one uniquely named synthetic native entry, checks another
origin cannot find it and retries deletion. It requires an already accessible
store and never unlocks or prompts. On 2026-11-15 the Linux fixture passed against
the accessible local Secret Service using only its unique synthetic entry. It
verified create/read/overwrite, origin separation and idempotent deletion; the
entry was removed. No existing account secret was read or changed. macOS runtime,
provider interoperability and native resource/latency acceptance remain open.

## Headless credential setup

`screenshots credentials target PROFILE_JSON` prints `{"target":"..."}` without
contacting the store. The HTTPS profile must already contain a credential
reference and may specify bearer/api_key/basic authentication.

`screenshots credentials store PROFILE_JSON [--username NAME]` reads the secret
from nonterminal stdin only; no secret argument or echo prompt is accepted. Input
is bounded to 8,192 bytes and must be UTF-8. One terminal LF or CRLF is removed;
other controls remain subject to credential validation. Basic requires a username,
which is trimmed on save. Other modes store the fixed nonsecret username `token`.
Successful output is `{"stored":true,"profile_file_changed":false}`.

`screenshots credentials remove PROFILE_JSON` explicitly deletes the scoped OS
entry and prints `{"removed":true,"profile_file_changed":false}`. The profile
file is unchanged, so later uploads fail until a credential is restored or the
reference is deliberately removed from the profile. No command uploads data.
Failures go to stderr with exit 1; payloads are never included in diagnostics.

The Linux/macOS native value is exactly the bytes `screenshots-upload-v1\0`,
followed by UTF-8 username, one NUL separator, then UTF-8 secret. Unsupported
prefixes, missing separators, malformed UTF-8 and invalid header credentials fail
closed. Only the native secret value contains this envelope; profile JSON stores
no part of it. This format is distinct from the existing Windows native fields.
