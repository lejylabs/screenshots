# Explicit native macOS SFTP library selection

Published curl-sys 0.4.91+curl-8.22.0 source, original licenses retained.
Its build script normally bypasses pkg-config on Apple and links system libcurl.
When PKG_CONFIG_PATH is explicitly configured, probe that library first instead;
this permits the pinned static curl/libssh2/OpenSSL/zlib build used by native CI.
No curl initialization runs in the desktop process; only the isolated explicit
SFTP worker calls curl_global_init and owns cleanup. System-library fallback and
other platform selection retain upstream behavior. See docs/sharing-completion.md.
