# Local ashpd 0.14.0 patch

Source: crates.io `ashpd` 0.14.0, unchanged package source and MIT license retained.

The ScreenCast v6 response specifies the stream property `pipewire-serial`.
Add only its explicit serde field name to `StreamProperties::pipewire_serial`;
the original underscore Rust field name otherwise misses the standard key and
discards the identity needed for serial-based PipeWire targeting. No underscore
alias is accepted. The desktop private D-Bus/PipeWire capture fixture sends the
standard hyphenated key independently of ashpd's serializer.

Specification: https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html
