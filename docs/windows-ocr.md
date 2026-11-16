# Windows file OCR

`screenshots.exe ocr INPUT [--language LANG]` uses a separately installed
`tesseract.exe` from the executable search path. The default model is `eng`;
`--language deu+eng`, for example, requires both installed models. The application
does not install an engine, fetch models or upload pixels. A missing engine or
model returns an error. The shared [OCR contract](ocr.md) describes JSON output,
input validation and byte limits.

The command bypasses GPUI and resident startup. It decodes only the explicitly
supplied file, sends an in-memory PNG through stdin, and prints exactly one JSON
result after successful recognition. The original, history, settings and clipboard
are untouched. Engine stderr is counted and discarded. The external process is a
trusted local engine, not a sandbox for arbitrary executables.

The Windows launcher creates a non-inheritable Job Object with
`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. The shared native Windows launcher attaches
that Job Object during process creation with `PROC_THREAD_ATTRIBUTE_JOB_LIST`,
alongside an explicit inherited-handle list and hidden process creation. Startup
fails closed if atomic containment is unavailable. No engine code runs through
a suspended-create/late-assignment interval. The process receives the same direct
Tesseract arguments and `OMP_THREAD_LIMIT=2` as Unix; no shell interprets the path
or language.

The job permits no child-process breakaway. Closing the last job handle kills the
engine and its descendants, including descendants retaining pipe ends. Cleanup
closes the job, waits for the direct child, then joins pipe workers. Parent
termination closes its Job handle through native process cleanup. Actual native
process/handle checks remain distinct from emulated or metadata-only evidence.

The active operation uses at most three pipe workers with requested 256 KiB
stacks, a three-message channel, at most 32 MiB PNG input, 256 KiB recognized
output, and 8 KiB read chunks. Stderr is capped at 16 KiB and never retained.
Decoded RGBA is capped at 16 MiB and encoded source input at 32 MiB. A 30-second
deadline and 25 ms active-operation completion checks bound normal engine work;
there is no idle timer, OCR service, runtime, image cache or background stream.
Native spawn, pipe and process-cleanup calls still depend on the OS.
Job bookkeeping, native process-creation attributes, pipe allocations, thread-stack
commitment and the external engine's own memory are not measured or hard-capped by
these application limits.

The Windows-only unit fixtures exercise successful output with discarded
diagnostics, invalid UTF-8, nonzero exits, both stream caps, and a short deadline
that must kill a descendant holding inherited pipes. CLI integration fixtures
exercise engine lookup, language injection rejection, image admission, source
preservation and absent user-state directories using synthetic images. Run them
in an optimized native Windows checkout:

```powershell
cargo test --profile release-fast -p screenshots ocr_windows::tests -- --test-threads=1
cargo test --profile release-fast -p screenshots --test ocr_windows -- --test-threads=1
```

An isolated `cargo check --target x86_64-pc-windows-gnu --tests` compiled the actual
Windows engine module and CLI test source with their minimal dependencies from
Linux. The whole desktop binary and its test paths also passed GNU Windows
`cargo check`. The isolated optimized Windows test binary ran under Wine:
three lifecycle tests passed and its internal child fixture stayed ignored.
This checks emulated job/process behavior, not native Windows integration.
These tests were not executed on native Windows during this development pass.
Native Tesseract accuracy, Ctrl+C/console teardown,
restrictive parent-job policies, abrupt process termination and reference-machine
CPU/private-memory/latency acceptance remain open. A cross-compile or Linux test
pass is not native Windows evidence.

The nonexistent-engine unit test uses a unique absolute path. Windows searches
the application and system directories as well as `PATH`, so the CLI lookup
fixture accepts either recognition by a preinstalled engine or an explicit
unavailable/model error. It never installs an engine or changes those directories.

API behavior follows Microsoft's documentation for
[Job Object limits](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_basic_limit_information),
[atomic child containment](windows-child-startup.md),
[process creation flags](https://learn.microsoft.com/en-us/windows/win32/procthread/process-creation-flags)
and [process creation attributes](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute).
