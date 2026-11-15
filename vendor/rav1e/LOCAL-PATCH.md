# Local rav1e token-pasting dependency patch

Base: published `rav1e` 0.8.1 from crates.io, VCS commit
`38806721f718e7ae6919e72385bc305bf824ef81` in
<https://github.com/xiph/rav1e>. Original version, BSD-2-Clause license, PATENTS,
build script, Rust sources and assembly are retained. Cargo cache metadata,
the redundant original manifest and packaged lockfile are omitted.

The only upstream-file change replaces `paste` with `pastey` 0.2.3 under the
same dependency alias. `pastey` retains the `paste!` and `item!` exports used by
rav1e. Identifier construction, CPU dispatch, assembly declarations, codecs,
features and default features are unchanged. No AVIF capability is removed.
The upstream development branch has also migrated to `pastey`; this local
patch retains released 0.8.1 rather than adopting unrelated development changes.

This build-time macro substitution adds no application runtime, worker, timer,
cache or idle activity. No encoder performance change is claimed. Remove the
override when a compatible upstream release uses a maintained helper.
