#!/usr/bin/env python3
"""Probe the actual curl build script against a synthetic Windows install tree.

This checks vcpkg discovery metadata; native CI must still link/run real SFTP.
Requires already compiled release-fast Rust build dependencies, not a Windows SDK.
"""
import argparse
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--artifacts', type=Path, default=ROOT / 'target/release-fast')
    args = parser.parse_args()
    artifacts = args.artifacts.resolve()
    # Build output is trusted compiler data; don't require an extra search binary on CI.
    libraries = list(artifacts.rglob('*.rlib'))
    dependencies = sorted({path.parent for path in libraries})
    with tempfile.TemporaryDirectory(prefix='screenshots-native-curl-') as temporary:
        root = Path(temporary)
        installed = root / 'vcpkg/installed'
        metadata = installed / 'vcpkg'
        (metadata / 'info').mkdir(parents=True)
        (metadata / 'updates').mkdir()
        (root / 'vcpkg/.vcpkg-root').touch()
        triplet = 'x64-windows-static-md'
        native = installed / triplet
        (native / 'lib').mkdir(parents=True)
        (native / 'include').mkdir()
        ports = [('curl', ['libcurl'], 'libssh2, openssl, zlib'),
                 ('libssh2', ['libssh2'], 'openssl, zlib'),
                 ('openssl', ['libssl', 'libcrypto'], ''), ('zlib', ['zlib'], '')]
        status = []
        for name, libs, requires in ports:
            status.append(f'Package: {name}\nVersion: 1.0\nArchitecture: {triplet}\nStatus: install ok installed\nDepends: {requires}\n\n')
            paths = []
            for library in libs:
                (native / 'lib' / (library + '.lib')).touch()
                paths.append(f'{triplet}/lib/{library}.lib\n')
            (metadata / 'info' / f'{name}_1.0_{triplet}.list').write_text(''.join(paths))
        (metadata / 'status').write_text(''.join(status))
        executable = root / 'probe'
        command = ['rustc', '--edition=2018', '--crate-name', 'curl_discovery_fixture',
                   '--cfg', 'target_env="msvc"', '-Aexplicit_builtin_cfgs_in_flags',
                   str(ROOT / 'vendor/curl-sys/build.rs'), '-o', str(executable)]
        for crate in ['cc', 'pkg_config', 'vcpkg']:
            candidates = [path for path in libraries if path.name.startswith('lib' + crate + '-')]
            if not candidates:
                raise RuntimeError('Compile release-fast build dependencies before this fixture')
            command += ['--extern', crate + '=' + str(max(candidates, key=lambda path:path.stat().st_mtime_ns))]
        for directory in dependencies:
            command += ['-L', 'dependency=' + str(directory)]
        subprocess.run(command, check=True, cwd=ROOT, timeout=60, capture_output=True)
        environment = dict(os.environ, TARGET='x86_64-pc-windows-msvc',
                           VCPKG_ROOT=str(root / 'vcpkg'), VCPKGRS_TRIPLET=triplet,
                           SCREENSHOTS_REQUIRE_NATIVE_CURL='1', CARGO_CFG_TARGET_FEATURE='',
                           OUT_DIR=str(root))
        for variable in ['VCPKGRS_DISABLE', 'NO_VCPKG', 'VCPKGRS_NO_CURL', 'CURL_NO_VCPKG']:
            environment.pop(variable, None)
        result = subprocess.run([str(executable)], env=environment, cwd=ROOT,
                                capture_output=True, text=True, timeout=10, check=True)
        emitted = set(result.stdout.splitlines())
        for library in ['libcurl', 'libssh2', 'libssl', 'libcrypto', 'zlib']:
            assert 'cargo:rustc-link-lib=' + library in emitted, library
        assert 'cargo:rustc-cfg=libcurl_vendored' not in emitted
        print('Actual build-script discovery passed: curl, SSH, modern OpenSSL and zlib; native linking remains a separate gate.')


if __name__ == '__main__':
    main()
