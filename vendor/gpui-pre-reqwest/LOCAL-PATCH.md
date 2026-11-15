# Local gpui-pre-reqwest patch

Base: the published `gpui-pre-reqwest` 0.12.15 crate from crates.io, as cached in
the Cargo registry. Its packaged VCS metadata records commit
`68a26f46bb6ed5131f56517a574041d2c4d2aec8` with `dirty: true`; this is the
published package, not a claim that it matches a clean upstream Git revision.
The original MIT and Apache-2.0 licenses are retained.

This patch removes the unmaintained `rustls-pemfile` dependency and uses the
existing `rustls-pki-types` PEM APIs. The minimum version is 1.9.0, which supplies
`pem::PemObject`, its reader iterator and `pem::SectionKind`.

Certificate and CRL bundles continue to filter other sections and map parser
errors to the existing public error messages. Identity parsing keeps certificate
order and the last private key; it accepts PKCS#1, PKCS#8 and SEC1 keys, rejects
recognized public-key/CSR/CRL sections, and skips other section kinds as the old
`rustls-pemfile::Item` mapping did. TLS backends and default features are unchanged.
`rustls-pki-types` is enabled by `__tls` so certificate bundle parsing also works
with native TLS alone. There is no new runtime, worker, cache or background work;
PEM decoding reuses the parser that `rustls-pemfile` already wrapped.
The existing `alloc` feature is retained and `std` is explicit because PEM reader
APIs must also compile when no Rustls dependency enables it indirectly.

Focused tests in `src/tls.rs` cover these compatibility rules and existing TLS
tests remain in place. Cargo registry metadata, the redundant original manifest,
the packaged lockfile and repository-only CI files were omitted from vendoring.

API reference: <https://docs.rs/rustls-pki-types/1.9.0/rustls_pki_types/pem/trait.PemObject.html>.
Remove this patch when the GPUI dependency chain ships this migration upstream.
