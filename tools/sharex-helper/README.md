# Private SXCU interpreter

This original helper implements the published custom-uploader function contract
using Json.NET, .NET regular expressions and .NET XPath. It contains no copied
ShareX implementation or GPL resources. The reference inventory, security
boundaries, runtime costs and integration plan are in
[the runtime design](../../docs/sharex-runtime.md).

The app starts this self-contained helper only for an explicit prepare, inspect
or response stage. It does not load .NET into the desktop, discover another
runtime, access credentials, upload, open a browser, read arbitrary files or
change the clipboard. Headless inputbox/select answers must be supplied;
outputbox yields a bounded result event. Environment identity names require
explicit caller context or opt-in. Random word/emoji vocabularies are original,
so identical random vocabulary/distribution is not claimed.

Build with the exact SDK 10.0.404 and runtime 10.0.15:

```text
python3 scripts/build-sharex-helper.py --rid linux-x64 --output target/release-fast/lib/screenshots-sxcu
```

The script uses the committed NuGet lock (Newtonsoft.Json 13.0.4), rejects a
changed SDK, keeps build/cache/output state under ignored target, disables
telemetry, certificate/tool first-use, compiler servers and node reuse, and
writes a complete SHA256 runtime manifest plus notices. `--update-lock` is an
explicit maintainer operation. Native RID builds run the helper's self-test;
cross-publication alone is not a native execution result. Required RIDs are
win-x64, linux-x64, linux-arm64, osx-arm64 and osx-x64.

The process protocol is one UTF-8 JSON request on stdin and one JSON result on
stdout. No profile/source data travels in argv or a temporary config file.
Requests contain protocol version, stage, frozen profile_json, input kind,
public filename/text, explicit answers/context and an optional base64 response
body/header snapshot. Configuration JSON does not infer date objects from
strings. Response JSONPath retains Json.NET's ordinary value semantics.
Inspection returns stable field/call IDs and available static prompt metadata;
sensitive defaults/choices are redacted and computed metadata is identified.

Stages have a five-second parent deadline and a child watchdog, 4 MiB input,
2 MiB output and 64 MiB managed-heap limits. The parent verifies the private
runtime manifest, closes processes/pipes on cancellation and discards raw
helper diagnostics. Direct and JSONPath regex matches have 100 ms timeouts;
XML forbids DTD/external resolution. See the design for expansion, input,
source-stream and native overhead limits.

Run focused native checks with:

```text
target/release-fast/lib/screenshots-sxcu/screenshots-sxcu --self-test
python3 scripts/test-sharex-upload.py --binary target/release-fast/screenshots --report target/verification/sharex-e2e.json
```

The explicit E2E fixture uses private HTTPS services, synthetic native secrets
and image/text/binary data. It verifies finalized in-memory and saved Task
uploads, then copied/installed CLI runtime resolution across an upgrade. It
preserves source files and removes its temporary credentials/installations.
No real account or physical desktop input is involved.
