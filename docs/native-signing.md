# Explicit native signing and notarization

Status: implementation and synthetic checks complete; fresh source review pending,
2026-11-16. No production signing identity,
certificate/private key or Apple account was supplied. Tooling can be completed
and tested with synthetic command fixtures; real signatures and notarization
remain separate release inputs. Nothing signs, submits or changes a keychain
at normal application startup or in ordinary CI.

## Requested behavior

Extend native packaging with an explicit `--sign-profile FILE` containing public
identity selectors only. Windows uses a certificate thumbprint, exact MSIX
publisher and HTTPS timestamp endpoint. macOS uses stable application/team and
installer identities, with an optional existing notary keychain profile supplied
separately. Passwords, PFX contents, private keys and account tokens do not belong
in profile JSON, process arguments or logs. Unknown fields/types, nonnative
profiles and invalid identities fail before signer invocation. Inputs are bounded
private snapshots and only package staging is mutated.

Sign nested executables before the enclosing package. The private .NET apphost
is a separate stable signing identity; its macOS entitlement is only allow-jit,
without debugger/task access or blanket library-validation exceptions. Sign all
bundled Mach-O native libraries with the application team, then regenerate each
changed runtime member hash and the outer runtime-manifest anchor. Existing
managed assemblies/notices remain covered by that manifest and package seal.
Preserve SDK/runtime versions and exact file inventory; never accept an unsigned
extra loader file to make signing pass.

Code signing changes executable bytes. Windows may record the final signed EXE
hash before MakeAppx packages it. A macOS bundle signature also seals resources,
so its internal BUILD-INFO cannot contain the final whole signed main-binary hash
without a circular signature dependency. Retain the admitted pre-bundle-signing
binary hash with an explicit hash-scope field; the final external artifact digest
and native signature cover the published result. Runtime integrity still uses
its regenerated manifest. Do not describe the provenance hash as the final
signed executable hash.

Verify each requested signature and expected publisher/team before publication.
macOS installer signing follows app signing; optional notarytool submission is
explicit, finite and must report Accepted before stapling/verification. A failed
signer, timeout, rejection, missing identity or verification failure publishes no
new production-marked artifact. Do not overwrite an existing artifact/checksum
or alter the caller's portable input. There is no signing fallback to ad-hoc,
self-signed trust or a test certificate.

## Files and checks

- `scripts/native-signing-windows.py`: atomic owned Job/stdio native tool runner.
- `scripts/native-signing.py`: strict public profile admission, bounded native
  tool calls, nested-code signing, runtime hash refresh and native verification.
- `scripts/package-app.py`: optional profile/public publisher routing inside its
  existing owned staging; unsigned development mode remains explicit.
- `scripts/test-native-signing.py`: independent synthetic signer mutation,
  manifest/anchor agreement, publisher XML escaping, entitlement scope, rejected
  signature/notarization and no-publication/source-preservation checks.
- This contract and packaging guide: record actual checks and missing real
  identity/native execution separately. Root owns these files.

Native command waits are operation-scoped, bounded and silent by default.
Optional notary submission is network/account access only when explicitly
requested with an already accessible identity. Unattended implementation tests
never call a real signer or submit a real artifact.

## Primary contracts

Microsoft documents the apphost's stable identity and allow-jit requirement in
[.NET macOS publishing](https://learn.microsoft.com/en-us/dotnet/core/deploying/macos).
Apple's [notarization workflow](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow)
describes notarytool and stapling. Windows package certificate selection,
file/timestamp SHA-256 and publisher agreement follow
[SignTool](https://learn.microsoft.com/en-us/windows/desktop/seccrypto/signtool) and
[MSIX signing](https://learn.microsoft.com/en-us/windows/msix/package/sign-app-package-using-signtool).
These are tool contracts, not evidence that this project's signatures exist.


The earlier synthetic snapshot passed seventeen signing-flow fixtures, three native layout
fixtures and eleven portable-package regressions. They use deterministic fake
signer mutations and owned Python children for output/timeout checks. Real
platform signing/notary execution remains unperformed; no production identity,
keychain state, native trust or account access was modified.


The first fresh source review found untrusted native tool lookup, missing macOS
installer identity comparison and rollback ownership gaps. Corrections use
protected absolute macOS tools and the registered Windows SDK root, a private
working directory and cleaned tool environment; signed MakeAppx/pkgbuild share
that resolver. Installer verification compares the trusted native leaf identity
with the exact configured Developer ID Installer/team. That earlier publication
snapshot retained artifact/checksum handles and attempted identity-checked
rollback; a later review identified the pathname race described below. Its
seventeen-case synthetic snapshot was historical, not the current race guarantee.
No real signature, keychain or notarization has been performed.


A second source review found nonregular foreign replacements could stop group
rollback, and a normally exited tool could leave a pipe-holding descendant.
The intermediate rollback skipped foreign object types; current publication
performs no output-name deletion, as described below. POSIX tool control observes an unreaped leader, signals its reserved
group and reaps afterward. Windows release tools use an owned kill-on-close job
attached atomically with an explicit handle list at native creation; result pipes
and waits remain bounded. New synthetic regressions cover foreign symlink/
directory replacement and an exited parent with a live pipe-holding child. The
Windows ABI path still requires native CI execution; no real signing is inferred.

## Public profile examples

These selectors are illustrative and are not usable test/production identities:

```json
{"version":1,"platform":"macos","application_identity":"Developer ID Application: Example Company (ABCDEFGHIJ)","installer_identity":"Developer ID Installer: Example Company (ABCDEFGHIJ)","team_id":"ABCDEFGHIJ"}
```

```json
{"version":1,"platform":"windows","thumbprint":"0123456789abcdef0123456789abcdef01234567","publisher":"CN=Example Company","timestamp_url":"https://timestamp.example.invalid/"}
```

Use exact existing Developer ID certificate names matching the team. Windows
thumbprint selects the existing certificate; publisher must match its subject.
The release owner supplies accessible identities and a real timestamp endpoint.
No example imports a certificate or authorizes an account operation by itself.


Round three found reader-thread admission happened before the POSIX cleanup
scope. All post-spawn reader admission is now guarded; exceptional startup kills
and reaps the reserved owned group and closes the result pipe. An injected
Thread.start failure regression confirms the owned tool has a recorded terminal
status after the exception. Windows reader admission is inside its atomic Job
cleanup scope. That earlier synthetic snapshot passed seventeen cases on Linux;
the POSIX-specific case is explicitly excluded on Windows, not counted as native
coverage. This correction is author runtime evidence, not a clean extra review.


Native CI exposed macOS Python without os.waitid/os.WNOWAIT. The release-tool
runner now uses libSystem waitid with XNU's documented nonreaping flags on that
platform; it still kills the owned group while the leader is unreaped, then
waits. The aligned child-info buffer checks both SIGCHLD and the requested PID.
The Linux native-ABI regression proves nonreaping observation on its own host;
Darwin execution remains an exact native CI gate. Synthetic tool/path assertions
compare resolved paths, and the controlled environment fixture uses a protected
Python executable plus an explicit private script, so Windows does not depend on
POSIX shebang execution. Twenty-three synthetic fixtures pass locally; no real
certificate, signer, trust store or notarization was accessed. Primary contracts:
[XNU wait flags](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/wait.h)
and [child-info layout](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/signal.h).

The portable runner also marks the reaping phase before calling Popen.wait.
A wait may reap its child and then re-raise KeyboardInterrupt, so cleanup never
signals a numeric process group after any such wait started. The injected
reap-then-interrupt regression checks exactly one pre-wait signal; the live
pipe-holding descendant test handles disappearance of /proc state atomically.
These are synthetic/native-process lifecycle tests, not real release signing.

The first real Darwin portability run reached the signing fixtures and exposed
XNU's zombie-only group behavior: killpg can return EPERM after the sole leader
exits. The runner accepts that result only after public libproc reports exactly
our still-unreaped leader as the group's sole member; errors, extra members or
truncation remain failures. The leader reservation is checked before and after
that bounded two-slot snapshot. This is distinct from ignoring permission errors
for a live family. Primary implementation contracts: [XNU group signalling](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_sig.c)
and [libproc group enumeration](https://github.com/apple-oss-distributions/xnu/blob/main/libsyscall/wrappers/libproc/libproc.c).

Current output publication performs all signing/verification before any final
name is linked, and never replaces an existing name. It does not automatically
roll back shared output names after a filesystem publication failure: POSIX
stat-then-unlink cannot prevent deletion of a concurrent foreign replacement.
Already created, individually verified artifact/checksum parts therefore remain
for review; an explicit exception note identifies an incomplete set and asks for
a new output directory before retry. Source staging still cleans up privately.
This preserves data rather than pretending that a multi-file set is atomic.
Tests inject late failures and foreign replacements and prohibit output unlinks.
The incoming runtime's member hashes and BUILD-INFO manifest anchor must also
agree before any signer mutation; this is consistency admission, not independent
provenance authentication of a caller-provided unsigned archive.

The native MSIX manifest is written as UTF-8 with LF, including Unicode publisher
names, independent of the Windows ANSI code page. Signed JSON uses the same
explicit newline policy. Installer verification accepts the exact legacy trusted
status or the Developer ID distribution status demonstrated in the
[AWS SAM installer verification example](https://docs.aws.amazon.com/serverless-application-model/latest/developerguide/reference-sam-cli-install-verify.html),
while retaining the exact requested leaf identity and native verifier exit check.
Fixtures exercise all admitted statuses, refusal cases and actual Unicode XML
file bytes; no real certificate or trust-store acceptance is inferred.

Fresh EGM source reviews 5 and 6 found no Blocker, Major or Nit. Current local
checks pass 23 signing/process, 12 portable-package and 4 native-layout fixtures.
The earlier Windows native snapshot's two stdout-newline fixture failures were
corrected with explicit binary fixture output; the production tool runner still
returns the native bytes unchanged. The corrected current source has not run on
Darwin/Windows CI: GitHub currently denies pushes for the authenticated account.
This source-review convergence does not close real signing/notarization gates.

The first resumed Windows private run passed owned topology/physical-mode,
bitmap/history-alias, atomic Job-limit and sent-message-flood fixtures, plus the
23 signing-process fixtures (four platform-specific cases skipped). It then
stopped in native package-layout verification because Windows does not preserve
POSIX executable bits on synthetic extensionless helper files. The macOS/Debian
mode-layout fixtures now explicitly require a Unix host; Windows continues to
run both MSIX XML/Unicode-byte fixtures. This corrects host-specific test scope,
without treating skipped Unix layouts as Windows passes. The later persistence,
colour and logger step did not run in that failed job and remains pending.
