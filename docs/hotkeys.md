# Configurable Windows global shortcuts

Implemented 2026-11-12 on base `bc56f2d209f65fae9666131ba42b230c56183190`.
This records a feature slice, not full Windows or ShareX-parity acceptance.

Follow-up: Active window is now a fourth command, disabled by default. Legacy
three-binding settings retain all values and gain the disabled slot. See PLAN.md
for active-window scope and current checks. The original measurements below
describe the earlier three-command build, not a fresh four-command benchmark.

## Behavior and ownership

Settings exposes four named Kit inputs for Primary display, Quick region,
Capture and annotate, and Active window. Save shortcuts applies all four; an empty value disables
that command. Restore defaults saves Ctrl+Shift+S/R/E and disables Active window. Input supports Ctrl, Alt,
Shift and Super with letters, digits, F1–F24 or PrintScreen. Letters/digits need
Ctrl, Alt or Super. Each string is limited to 64 bytes; normalization precedes
duplicate detection. Arbitrary command assignment/task profiles remain future work.

Settings version 1 gains an optional hotkeys array. Older files retain the original
defaults. Folder/theme writes preserve bindings, and validation precedes settings
publication. Settings IO stays on the existing worker path.

At startup the manager waits for the existing asynchronous settings load, then
registers each configured command independently. Conflicts are reported against
the affected command; disabled/conflicting commands never acquire substitute
default bindings. Live edits reserve new keys while existing keys still route to
their original commands. Successful persistence commits the new mapping and
releases obsolete keys. Failed persistence releases reservations and keeps old
bindings. A failed reservation rolls back the newly reserved keys. Reassigning an
already owned key to another command requires no duplicate OS registration.

Native registration/unregistration stays on the foreground event-loop thread.
Callbacks use the existing bounded event bridge. A native cleanup failure retains
ownership/routing information, reports remaining bindings and blocks further edits
until restart, bounding ownership to at most eight keys. Drop retries release.

There is no new dependency, runtime, thread, polling timer, cache or startup UI.
Retained configuration is four strings of at most 64 bytes plus small native
registration metadata. Four Kit inputs exist only on Settings and release on
page exit/window close. Their text is bounded before insertion; Kit's existing
bounded input undo engine is reused. Capture, source pixels and output choices
are unchanged. Non-Windows settings explain that this integration is unavailable.

## Original verification

- Optimized application build passes with the locked dependency graph.
- Optimized core suite: 63 passed, 25 opt-in tests skipped in the regular run.
  Settings tests cover normalization, duplicate/invalid/oversized combinations,
  legacy defaults, persistence and preservation after failed saves.
- Focused final hotkey checks pass: native parser/visible-label agreement and event
  routing. The explicitly invoked Windows registration fixture also passes. It
  checks reservations, rollback, a conflict owned by a second native manager,
  commit/remapping, disable/release, and startup with a disabled command plus a
  conflict while an independent configured command remains active. It captures
  no desktop pixels and does not inject input or change clipboard data.
- Eight optimized Kit UI tests pass, including default field values, disabled
  controls without a native manager and input release when leaving Settings.
  These are headless component checks, not physical mouse/keyboard acceptance.

## Optimized resident observations

The same Windows development PC ran one 30-second sample per build after five
seconds of warmup, with real tray/hotkey services and no captures. The before
artifact was built from a clean git archive of the base commit; the after artifact
contains this feature. Raw samples remain local under the ignored
`target/hotkey-verification/` directory. Only the measurement summary is retained
here; machine identifiers and environment details are excluded. Both processes
exited on their diagnostic deadlines.

| Observation | Before | After |
| --- | ---: | ---: |
| Peak Private Bytes | 6.961 MiB | 6.977 MiB |
| Peak working set | 18.289 MiB | 33.547 MiB |
| Additional process CPU time | 0 measured ticks | 0 measured ticks |
| Peak threads | 8 | 8 |
| First application log to hotkey-ready log, one sample | 59.196 ms | 37.020 ms |

Private memory differs by 16 KiB; the much larger working-set difference is also
reported and is not interchangeable with private committed memory. These isolated
runs do not establish the cause of working-set variation or a startup speedup.
The readiness spans exclude process creation and initialization before the first
log. The new configured-ready message follows asynchronous settings loading;
its manager-ready message alone does not indicate active bindings.

These were Windows development-machine observations. Power mode, display/DPI,
GPU allocations, wakeups and energy were not measured. An earlier
sandbox run failed cursor/CIM access and is excluded. Zero measured CPU ticks
over 30 seconds does not prove zero work or the five-minute idle gate.

Remaining acceptance: configure/restart through the real Settings UI, capture
while another application has focus, keyboard/IME/accessibility and mixed-DPI
interaction, native unregistration fault injection, five-minute/reference-machine
resource checks, physical input/presentation and post-UI/soak behavior. No new
claim is made for non-Windows hotkeys or complete capture/editor acceptance.
