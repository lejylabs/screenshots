# Local verification artifact retention

The November 15 cleanup removed only reproducible artifacts below this
repository's ignored `target/` after all builders and test services were stopped.
The directory shrank from 257 GiB to 44 GiB; filesystem free space rose from
approximately 51 to 144 GiB. Shared/COW storage means apparent removed bytes are
not identical to newly free physical bytes. The exact removed directories are
recorded locally in `target/verification/artifact-cleanup.json`.

Current application binaries, verification JSON/logs, the latest verified
portable archive, extracted Windows/Xvfb toolchains and isolated Wine prefix
were preserved. Source, documents, user captures and other projects' temporary
directories were untouched. Missing test executables can be rebuilt from the
locked sources; historical results remain in the delivery evidence.

`release-fast` now retains limited debug information instead of full type debug
information, matching the normal release's debug level while keeping its
existing optimization/LTO/codegen settings. This reduces future disk growth;
it is not a runtime performance or memory claim. Do not automatically delete
artifacts while Cargo, native fixtures or a benchmark is still using them.
