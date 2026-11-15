# Local GTK 3 macros diagnostic dependency patch

Base: the published `gtk3-macros` 0.18.2 crate from crates.io. Its VCS metadata
records commit `00133512bfc8bbb8e9be59b20e93406c6a3eeb21` with `dirty: true`,
path `gtk3-macros`, in <https://github.com/gtk-rs/gtk3-rs>. This is the published
package, not a claim of an exact clean upstream commit. Original package version,
MIT license and copyright notices are retained. Cargo cache metadata and the
redundant original manifest are omitted.

The unmaintained `proc-macro-error` dependency is replaced with
`proc-macro-error3` 3.1.1. Source changes only update the crate namespace;
existing attributes, abort messages, spans and generated application code are
unchanged. Default features are disabled and `syn2-error` is selected explicitly
to match this crate's Syn 2 parser. The `nightly` feature is not enabled; stable
Rust 1.100 retains the `compile_error!` diagnostic path. Automatic nightly
diagnostic selection from the old dependency is not retained.

This build-time helper migration adds no application runtime, threads, timers,
cache or idle work. It removes one unmaintained transitive helper but does not
make the GTK 3 stack maintained. `proc-macro-error2` is also unmaintained and is
not used. Remove this override when the application moves to a maintained parent
toolkit stack.
