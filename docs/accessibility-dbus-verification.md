# Maintained Linux accessibility transport verification

2026-11-15. The zbus 6 migration is exercised by
`apps/desktop/tests/accessibility_dbus.rs`. This fixture verifies the actual
vendored `accesskit_unix` adapter and its maintained connection runtime. It does
not require a display, the user's D-Bus session or a screen reader.

The parent starts two private `dbus-daemon` processes, with sockets under a new
0700 temporary directory. One serves the session accessibility Bus/Status API;
the other serves the AT-SPI Registry Socket API. A fresh test child receives
only the private session address, removes any inherited AT-SPI address and
display variables, and redirects its configuration/runtime paths to the fixture.
Only that child constructs the real AccessKit adapter and window/button tree.
The independent observer and mock services use libdbus, rather than the same
zbus wire types being checked.

The optimized fixture verifies:

- `org.a11y.Status.IsEnabled` activates the adapter, which obtains the separate
  AT-SPI address through `org.a11y.Bus.GetAddress` and calls Registry Socket Embed.
- The actual Application and Accessible interfaces publish the toolkit name,
  writable/readable application ID, registry parent and window/button children.
- The button publishes the expected name, role, interface list, focus states,
  action count and `au`, `as`, `(so)` introspection signatures.
- A real Action DoAction call reaches the application's Click handler, and an
  incremental AccessKit tree update changes the independently read button name.
- Disabling accessibility calls the deactivation handler, skips inactive tree
  generation and releases the AT-SPI connection. Re-enabling embeds a fresh
  connection and publishes the initial tree again.
- StateSet, InterfaceSet and borrowed/owned/null ObjectRef retain their expected
  signatures and exact little/big-endian wire round trips. A malformed state
  array is rejected; array/tuple encodings are checked independently of their
  wrapper serializers.
- A new zbus ObjectServer interface insertion returns `None`. A duplicate returns
  the rejected interface as `Some` and leaves the original remotely served value
  unchanged.

Run:

```sh
CARGO_BUILD_JOBS=2 cargo test --locked --profile release-fast -p screenshots --test accessibility_dbus
```

`target/accessibility-dbus-final.log` records four passing tests (three checks
plus the child entry helper), with no ignored checks. The first observed test
execution took 0.05 seconds after compilation; this is fixture duration, not a
production latency or performance acceptance result. Observer requests,
startup/phase waits and child teardown have deadlines of at most three seconds.
Child and daemon guards kill/reap owned processes, and mock threads stop/join on
teardown.
The fixture's short mock dispatch/retry timers exist only during the test.

The added dependencies are Linux-only test access to packages already present
in the maintained graph. No production runtime, timer, service, image buffer or
worker was added by this verification. Native screen-reader behavior, platform
accessibility acceptance, hardware presentation and process resource budgets
remain separate gates; this fixture does not establish them.
