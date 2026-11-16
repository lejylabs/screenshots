# Explicit native macOS SFTP library selection

Published curl-sys 0.4.91+curl-8.22.0 source, original licenses retained.
Its build script normally bypasses pkg-config on Apple and links system libcurl.
When PKG_CONFIG_PATH is explicitly configured, probe that library first instead;
this permits the pinned static curl/libssh2/OpenSSL/zlib build used by native CI.
No curl initialization runs in the desktop process; only the isolated explicit
SFTP worker calls curl_global_init and owns cleanup. System-library fallback and
other platform selection retain upstream behavior. See docs/sharing-completion.md.

When SCREENSHOTS_REQUIRE_NATIVE_CURL is set for the Windows native build,
failure to discover the pinned vcpkg library is an immediate build error rather
than a silent bundled fallback without SFTP. vcpkg-rs 0.2.15 expects
VCPKG_ROOT/installed; CI exposes its separate cached install tree there through
an owned directory junction.

The MSVC probe now uses vcpkg's installed port metadata without library-name
overrides. Those overrides disable dependency traversal in vcpkg-rs 0.2.15;
the old OpenSSL 1.x names also omit modern libssl/libcrypto and static zlib.
The metadata probe supplies the full installed static closure. Windows system
libraries used by curl/SSPI/OpenSSL remain explicit. Native CI validates linking
and the linked engine's SFTP capability; Linux cross-compilation is not proof.
