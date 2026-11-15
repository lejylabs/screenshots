# Local GLib macros diagnostic dependency patch

Base: the published `glib-macros` 0.18.5 crate from crates.io. Its VCS metadata
records commit `42b9caf98e03ded086362d9653ca58fe94dc8658`, path `glib-macros`, in
<https://github.com/gtk-rs/gtk-rs-core>. Original package version, MIT license and
copyright notices are retained. Cargo cache metadata and the redundant original
manifest are omitted.

The unmaintained `proc-macro-error` dependency is replaced with
`proc-macro-error3` 3.1.1. Source changes only update the crate namespace;
existing attributes, abort messages, spans and generated application code are
unchanged. Default features are disabled and `syn2-error` is selected explicitly
to match this crate's Syn 2 parser. The `nightly` feature is not enabled; stable
Rust 1.100 retains the `compile_error!` diagnostic path. Automatic nightly
diagnostic selection from the old dependency is not retained.

This is a build-time helper migration with no new application runtime, threads,
timers, cache or idle work. `proc-macro-error2` is also unmaintained and is not
used. Remove this override after upgrading the parent GLib stack to an upstream
release using a maintained diagnostic helper.
