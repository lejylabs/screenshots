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
