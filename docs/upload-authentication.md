# Windows Bearer authentication

Anonymous uploads continue to work without a credential. Authenticated profiles currently require Windows Credential Manager and HTTPS; macOS/Linux secret-store integration is pending. The app refuses unsupported authenticated profiles instead of sending them anonymously.

1. Copy [the Bearer profile example](upload-destination.bearer.example.json), set your actual HTTPS endpoint and response format, and choose a non-secret `credential` reference such as `main`.
2. Load the profile in the app. Click **Copy credential target**. The target includes the canonical HTTPS origin (including non-default port) and reference name.
3. Open **Control Panel → Credential Manager → Windows Credentials → Add a generic credential**. Paste that exact target as the Internet/network address, use `token` as the username, and enter your Bearer token as the password. Do not put the token in JSON, a URL, a shell command or an application log. Username is not sent.
4. Click **Upload saved PNG** or **Upload PNG** in capture history. The app reads the matching credential only for this explicit action and sends `Authorization: Bearer …` over HTTPS. Missing/invalid credentials stop the request before sending it.

The reference accepts 1–64 ASCII letters, digits, underscores or hyphens. Tokens must use Bearer token characters (letters, digits, `-._~+/=`), without spaces/control characters. Windows Generic Credentials store a bounded UTF-16 password blob; the supported blob limit is Windows' `CRED_MAX_CREDENTIAL_BLOB_SIZE` (2,560 bytes, including any terminator), not an unlimited token store.

Changing HTTPS origin, port or reference selects a different credential target. Paths on the same origin share the configured reference. Redirects and retries remain disabled. **Forget destination** removes the app's profile, not the OS credential; manage/rotate/remove the secret in Credential Manager. Profile files contain only the credential reference. Upload history stores provider-returned sharing links, never an application-generated Authorization field. Those links may independently contain access parameters and are retained locally as documented.

Temporary native/UTF-16/UTF-8 token copies are wiped after header creation, and the HTTP header is marked sensitive. HTTP/TLS libraries necessarily hold request data during transmission; this does not promise forensic erasure of every transport buffer. Native credential lookup and real HTTPS service interoperability still need manual acceptance on the deployment machine.
