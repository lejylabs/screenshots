# Local VariantStrIter output-pointer fix

Source: crates.io `glib` 0.18.5, MIT. The crate keeps its real upstream version.
`src/variant_iter.rs` contains the exact two-line backport of upstream commit
[b5a4071e439bef2b5eea76c3aa25e5ae84839e34](https://github.com/gtk-rs/gtk-rs-core/commit/b5a4071e439bef2b5eea76c3aa25e5ae84839e34).
Cargo cache metadata and the redundant original manifest are omitted.

`g_variant_get_child` writes the returned string pointer into its last supplied
output argument. The local pointer is now mutable and C receives `&mut p`, rather
than writing through a shared Rust reference. This removes that undefined
behavior without changing the iterator API, allocation or runtime behavior.

This local backport does not pretend to be glib 0.20 or suppress a version-based
advisory. Replace the override when the GTK3/GPUI dependency family can use a
maintained release containing the fix. Optimized upstream `variant_iter::tests`
are the targeted regression checks; normal workspace tests cover integration.
