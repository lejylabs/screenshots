# Private native Linux desktop-services fixture

This is a functional adapter/controller/process fixture. It uses the current
production `app-core` and includes the actual desktop early-child-entry,
authenticated transport and process-family owner. It does not start GPUI, request
frames, access the clipboard, exercise a real compositor or establish any PLAN
CPU/private-memory/GPU/physical-input/presentation budget.

The six modes are `activation`, `stalled_bus`, `close_stalled_bus`, `x11`,
`wayland` and `close_consent`. Their assertions cover installed-idle portal
activation, stalled authentication and shutdown, real X11 registration/rollback,
modifier aliases, directed-signal forgeries, delayed consent/old-input retirement,
tray/menu changes and owned native teardown. X11 delivery injects Ctrl+Alt+G into
the blank owned server and verifies the current-epoch command through
`services.action`; it does not count a mocked callback or accept a raw packet as
proof of admission. The separate Ctrl+Alt+F8 conflict remains intentional. After failed staging, a
second registrar must acquire the actual proposed first binding (Ctrl+Alt+G),
proving the earlier successful grab was released during rollback.

## Run

Use Linux, the pinned repository Rust toolchain, Python 3.12+ with Linux pidfd
support, the existing application build dependencies, and system `Xvfb` plus
`dbus-daemon`. CI installs those native prerequisites explicitly. The runner does
not install packages or use root. A locally extracted Xvfb is acceptable only
after its publisher checksum/signature and runtime dependencies have been verified;
pass that executable using `--xvfb /path/to/verified/Xvfb`.

From the repository, run preparation guards first:

```sh
python3 scripts/test-unix-native-services.py --self-test
```

Run the Python-only owned-process fault lane next (no Cargo, Xvfb or D-Bus):

```sh
python3 scripts/test-unix-native-services.py --allow-private-fixtures --self-test-cleanup
```

This lane creates a long-lived Python parent and a descendant which calls
`setsid`, exercises a command timeout, and deliberately postpones reaping to
check pending ownership. It validates adoption of the separate-group descendant,
open authenticated pidfds and retained authority files across repeated real
SIGINT/SIGTERM delivery to the runner. It then restores actual reaping, verifies
an empty child set and deleted runtime, and reports the recorded interruption.
The deliberate delay tests ownership while pending; it does not simulate a
kernel D-state or claim that SIGKILL can release one.

Then, serially with other repository Cargo commands:

```sh
python3 scripts/test-unix-native-services.py --allow-private-fixtures
```

The runner builds a standalone fixture project in the stable
`target/verification/unix-native-services-fixture/project` directory and reuses
`target/release-fast`. The direct versions come from the current root lock;
patch paths and optimized profiles come from the current root manifest. The
initial full `cargo metadata` can fetch public locked manifests on a fresh
cache. Its resulting dependencies must be a subset of the root lock (including
source/checksum); compilation then uses `--locked --offline`, profile
`release-fast`, and two jobs. The repository manifest and lock are not modified.
There is no extra workspace member, copied dependency tree, backend or committed
fixture lock. The fixture project and dependency outputs have stable identities;
reruns do not accumulate random package/build directories. Normal Cargo retention
applies within the existing target/cache directories.

Every native run creates fresh private `/tmp/screenshots-native-*` HOME, XDG,
authority and temp directories. Native child environments allow only locale/PATH
plus those owned paths: inherited host display/bus/Wayland sockets, preload and
keyring authority are absent. Xvfb disables TCP; the session bus authenticates
with EXTERNAL only and searches only the runner's single owned activation service
directory. The Wayland socket provides singleton identity only. It is no
compositor and has no capture interface.

Each mode has a finite 15/30-second active-work deadline; startup has a ten-second
bounded-line deadline. The native fixture retains its three-second current-input
wait and production transport deadlines. Interrupt handlers record a signal and
wake active waits through an owned pipe; they never raise from the signal handler.
All owned children are retained by one Linux subreaper owner. Cleanup validates
each direct child's pidfd using waitid before signaling, retains that pidfd until
actual exit/reap, and discovers newly adopted descendants after parent exit. This
covers helpers which made separate process groups and bus-activated services;
there is no global PID or group kill.

A ten-second cleanup wait is a reporting interval, not an ownership timeout.
If exit/reaping or kernel inspection remains pending, the owner retains its
handles and private runtime and continues teardown. Ordinary repeated
SIGINT/SIGTERM only update the recorded interruption through this entire phase;
previous handlers and interruption reporting resume after the family is empty
and the private runtime has been removed. Authority files are owned explicitly,
not by a temporary-directory finalizer that can delete them while a child lives.
No system service/keyring changes are made.

No hard bound is claimed for uninterruptible kernel waits or failed inspection.
The owner may remain in pending cleanup indefinitely. SIGKILL, process/host death
or an external CI controller forcing termination after its grace period bypass
Python cleanup; retained files and descendants then require that controller's
process containment/disposable runner. The fixture does not prove that an
external controller kills separate process groups and cannot guarantee recovery
after its own forced death. Deliberately unreapable kernel faults require external
containment; the included deterministic pending test restores real reaping.

## Provenance and pending verification

The precursor fixture actually passed all six modes against Unix integration
commit `67a160964fc3e44a19b555b0614139a8f77addba`. Its final source SHA256 was
`02139b0b73494986fdfec932684cafe4eb1c2ec47fa91bb0ff08c13604cebe5a`, runner SHA256
`584f3ae2ce4ba4f2f300f3df34a1f8106fce64b8293ccc836db948bb7e6d154f`, and native log
SHA256 `751a81d9c4e7dc206c4e96dd31a103676c579c59867fd0d246a7584ae27f2bc8`.
The source exercised reservation-before-publication, consumed-control lost-wake
and quiet-readiness acknowledgment corrections. Its current-epoch native X11
capture command was admitted; no capture was performed. This is prior evidence,
not an execution result for the tracked portable runner.

The portable source keeps that fixture's assertions, changes its production
module path for the tracked layout, corrects two stale F7 diagnostic labels to
Ctrl+Alt+G, and fixes the rollback probe to acquire the actual proposed first
binding instead of an unrelated F7. The original six-mode pass did not establish
that corrected G rollback assertion. The preparation/build/isolation runner and
Linux CI step are new. Review 1 found the earlier cleanup deadline could abandon
ownership and repeated signals could interrupt teardown; the current owner and
record-only handlers address both findings, with the explicit fault lane above.
At revision-2 authoring handoff, none of the portable runner's modes had been
executed. Root subsequently corrected Unicode module-path generation and ran
preparation guards, the Python timeout/setsid/pending-owner/repeated-signal fault
lane, the optimized fixture build and all six native modes with the corrected G
rollback assertion successfully on Linux x86_64. The owned Xvfb was the verified
private extraction described above; no user display or bus was used. Logs are in
`target/verification/unix-fixture-ci-integration/`. The successful native run
reported pending cleanup twice and retained its child pidfds/runtime until reaped.
Fresh-cache manifest resolution and the actual CI invocation on both Linux
architectures remain pending. Root workspace checks remain separate.
A standalone fixture measurement must never be described as full GUI/session
performance acceptance.
