# File hashes

The explicit headless command computes a SHA-256 digest of one regular file:

```sh
screenshots hash-file FILE
screenshots hash-file FILE --expect HEX
```

Successful computation prints one JSON object, then exits with status 0:

```json
{"algorithm":"sha256","digest":"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad","bytes":3}
```

`digest` is lowercase hexadecimal and `bytes` is the number of bytes hashed.
`--expect` must contain exactly 64 ASCII hexadecimal digits, with no prefix,
spaces or separators. Comparison accepts either letter case. A check adds
`"matches":true` or `"matches":false` to the JSON. A mismatch still prints the
computed digest, flushes stdout, prints a fixed error on stderr, and exits with
status 1. Invalid arguments, unreadable inputs and hashing failures exit with
status 1 without result JSON. An output-stream failure may leave partial output.

Arguments are validated before inspecting/opening the file. Native file paths,
including non-UTF-8 paths on Unix, are supported. Symlinks to regular files are
accepted; directories, FIFOs and devices are rejected. Input is limited to
1 GiB, checked before opening and again on the opened handle. Reading uses that
handle's initial length as a fixed extent, so concurrent growth cannot create
an unbounded read. Early EOF and a changed final length fail. This does not
provide an atomic snapshot or detect all concurrent edits: in-place writes or
a length change restored before the final check can produce a digest of mixed
contents. Hash files that other programs are not editing.

A cooperative 30-second deadline is checked around each read and before
reporting success. It cannot interrupt a native open, metadata or read call
that has stalled, for example on a remote filesystem; Ctrl+C terminates this
explicit process. On Unix, nonblocking open also prevents a FIFO substituted
after the initial type check from waiting for a writer. There is no background
worker or watchdog thread.

Resource cost is one open file, a 64 KiB reusable stack buffer, fixed-size
SHA-256 state and a small JSON result. The command streams bytes without image
decoding, memory mapping or loading the whole file. The existing SHA-256
dependency is used directly. It starts no desktop UI, logging subscriber,
network runtime, thread, cache or timer and does not read or write the
clipboard, upload data, create history/configuration or modify the source.
Operating-system file-access metadata may still change when a file is read.

Executable fixtures cover empty/`abc` known digests, 1 MiB binary input,
uppercase expectations, mismatch JSON/exit behavior, invalid syntax, native
paths, a sparse oversized file, FIFO rejection and absent user-state writes.
Focused unit fixtures verify the fixed read extent, short-read failure and
deadline check. These bounds are implementation limits; optimized resource
measurements and platform results must be recorded separately before making
performance-acceptance claims. This utility does not complete the remaining
ShareX utility backlog.
