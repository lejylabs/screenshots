# Local zbus 6 migration

Published `gpui-pre-linux 0.3.12` source, with original license files.
Adapt dependency manifests and native D-Bus consumer APIs to maintained zbus 6.
Preserve existing accessibility data model, portal and keyring behavior.
See `docs/dependency-maintenance.md` for verification and runtime costs.

## Native clipboard and drop integration

The local backend also preserves effective Wayland descriptor authority across
connection setup, reads a focused image offer asynchronously through bounded
transfers, and transports raw-image/URI drop payloads with role-specific pending
ownership. Offers and admission leases survive background work through foreground
delivery/discard; obsolete requests cancel. Source scope and private native verification are documented
in `docs/clipboard-import-output.md`; these paths are local changes to upstream.

## Resident graphics lifetime

Wayland and X11 release the existing lazy shared GPU context after their last
native window destroys its renderer. The display connection and clipboard stay
owned, so a descriptor-only Wayland launch can reopen without inventing a named
authority or losing clipboard content. The next native window initializes the
same context slot on demand. No new runtime, cache, worker, poll or connection
is introduced. Actual process/driver/private memory and recreation latency must
be measured; dropping owned GPU objects alone does not establish PLAN budgets.

The compositor GPU hint remains copied metadata across all window creations and
GPU-context retirements. It is not consumed by the first window: later contexts
must retain the same compositor adapter preference. The opt-in native probe
checks both this metadata lifetime and actual selected adapter identity across
recreation; it does not establish hardware power/latency acceptance.
