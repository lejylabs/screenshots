#!/usr/bin/env python3
"""Build production Unix services, then exercise six modes on owned Linux services."""
import argparse
import ctypes
import json
import os
from pathlib import Path
import selectors
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import tomllib
from xml.sax.saxutils import escape

ROOT = Path(__file__).resolve().parents[1]
PACKAGE = "unix-native-services-private-fixtures"
MODULE_PATH = '#[path = "../../apps/desktop/src/desktop_services_process.rs"]'
MODES = ("activation", "stalled_bus", "close_stalled_bus", "x11", "wayland", "close_consent")


def literal(value):
    if isinstance(value, dict):
        return "{" + ",".join(json.dumps(k) + "=" + literal(v) for k, v in value.items()) + "}"
    return json.dumps(value, ensure_ascii=False)


def fixture_manifest(root, locked):
    application = tomllib.loads((root / "Cargo.toml").read_text())
    def version(name):
        versions = {p["version"] for p in locked if p["name"] == name}
        assert len(versions) == 1, f"Expected one locked {name}, found {versions}"
        return "=" + versions.pop()
    dependencies = {
        "app-core": {"path": str(root / "crates/app-core")},
        **{name: version(name) for name in ("anyhow", "dbus", "futures-lite", "libc", "async-io")},
        "x11rb": {"version": version("x11rb"), "features": ["xkb", "xtest"]},
        "uuid": {"version": version("uuid"), "features": ["v4"]},
    }
    text = f'[package]\nname="{PACKAGE}"\nversion="0.0.0"\nedition="2024"\npublish=false\n[workspace]\n[dependencies]\n'
    text += "".join(json.dumps(k) + "=" + literal(v) + "\n" for k, v in dependencies.items())
    text += "\n[patch.crates-io]\n"
    for name, original in application["patch"]["crates-io"].items():
        patch = dict(original)
        if "path" in patch:
            patch["path"] = str((root / patch["path"]).resolve())
        text += json.dumps(name) + "=" + literal(patch) + "\n"
    for profile in ("release", "release-fast"):
        text += f"\n[profile.{profile}]\n"
        text += "".join(json.dumps(k) + "=" + literal(v) + "\n"
                        for k, v in application["profile"][profile].items())
    return text


def validate_lock(original, selected):
    packages = {(p["name"], p["version"], p.get("source"), p.get("checksum")) for p in original}
    assert all(p["name"] == PACKAGE or
               (p["name"], p["version"], p.get("source"), p.get("checksum")) in packages
               for p in selected), "Fixture resolution introduced a dependency outside the application lock"


class Interrupted(RuntimeError):
    pass


class Interrupts:
    """Signal handlers only record/wake; teardown never checks interruption."""
    def __enter__(self):
        self.reader, self.writer = os.pipe2(os.O_NONBLOCK | os.O_CLOEXEC)
        self.pending = None
        self.count = 0
        self.previous = {number: signal.signal(number, self.record)
                         for number in (signal.SIGTERM, signal.SIGINT)}
        return self

    def record(self, number, _frame):
        self.pending = number
        self.count += 1
        try:
            os.write(self.writer, b"!")
        except BlockingIOError:
            pass  # One existing wake is sufficient; retain only the latest signal/count.

    def check(self):
        if self.pending is not None:
            raise Interrupted(f"Fixture interrupted by signal {self.pending}")

    def wait(self, selector, timeout):
        self.check()
        selector.register(self.reader, selectors.EVENT_READ)
        try:
            events = selector.select(timeout)
        finally:
            selector.unregister(self.reader)
        self.check()
        return events

    def __exit__(self, kind, _error, _traceback):
        # The caller must finish family/runtime teardown before leaving this scope.
        for number, handler in self.previous.items():
            signal.signal(number, handler)
        os.close(self.reader)
        os.close(self.writer)
        if self.pending is not None:
            print(f"PRIVATE interruption reported after cleanup: signal={self.pending}, count={self.count}",
                  file=sys.stderr, flush=True)
            if kind is None:
                self.check()


class OwnedChildren:
    """Retain authenticated child identities and runtime until actual exit/reap."""
    def __init__(self):
        libc = ctypes.CDLL(None, use_errno=True)
        if libc.prctl(36, 1, 0, 0, 0) != 0:  # PR_SET_CHILD_SUBREAPER
            raise OSError(ctypes.get_errno(), "Could not own fixture descendants")
        for name in ("pidfd_open", "P_PIDFD"):
            if not hasattr(os, name):
                raise RuntimeError(f"Linux Python pidfd support required: {name}")
        if not hasattr(signal, "pidfd_send_signal"):
            raise RuntimeError("Python pidfd_send_signal support required")
        self.handles = {}
        self.processes = []
        self.runtime = None

    def spawn(self, command, **options):
        process = subprocess.Popen(command, start_new_session=True, **options)
        self.processes.append(process)  # Prevent Popen destructors racing our authenticated reap.
        return process

    def create_runtime(self):
        assert self.runtime is None
        self.runtime = Path(tempfile.mkdtemp(prefix="screenshots-native-", dir="/tmp"))
        self.runtime.chmod(0o700)
        return self.runtime

    def discover(self):
        children = Path(f"/proc/self/task/{os.getpid()}/children").read_text().split()
        for child in children:
            pid = int(child)
            if pid in self.handles:
                continue
            try:
                descriptor = os.pidfd_open(pid)
            except ProcessLookupError:
                continue
            try:
                # ECHILD rejects reused/unrelated PIDs before any signal. WNOWAIT reserves zombies.
                os.waitid(os.P_PIDFD, descriptor, os.WEXITED | os.WNOHANG | os.WNOWAIT)
            except ChildProcessError:
                os.close(descriptor)
                continue
            except BaseException:
                os.close(descriptor)
                raise
            self.handles[pid] = descriptor

    def reap_ready(self, events):
        for key, _ in events:
            pid, descriptor = key.data, key.fd
            result = os.waitid(os.P_PIDFD, descriptor, os.WEXITED | os.WNOHANG)
            if result is not None:
                os.close(descriptor)
                del self.handles[pid]

    def close_step(self, timeout=10):
        self.discover()
        if not self.handles:
            return True
        with selectors.DefaultSelector() as selector:
            for pid, descriptor in self.handles.items():
                try:
                    signal.pidfd_send_signal(descriptor, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                selector.register(descriptor, selectors.EVENT_READ, pid)
            self.reap_ready(selector.select(timeout))
        self.discover()  # Exited parents may have handed us separately grouped descendants.
        return not self.handles

    def close(self):
        while True:
            try:
                if self.close_step():
                    if self.runtime is not None:
                        shutil.rmtree(self.runtime)
                        self.runtime = None
                    self.processes.clear()
                    return
                print("PRIVATE cleanup pending; retaining child pidfds and private runtime", file=sys.stderr, flush=True)
            except Exception as error:
                # Kernel/inspection failures do not release identities or delete authority files.
                print(f"PRIVATE cleanup pending: {error}; ownership retained", file=sys.stderr, flush=True)
                threading.Event().wait(0.1)


def run(command, timeout, family, interrupts, **options):
    process = family.spawn(command, cwd=ROOT, **options)
    descriptor = os.pidfd_open(process.pid)
    try:
        with selectors.DefaultSelector() as selector:
            selector.register(descriptor, selectors.EVENT_READ)
            if not interrupts.wait(selector, timeout):
                raise subprocess.TimeoutExpired(command, timeout)
        code = process.wait()
    finally:
        os.close(descriptor)  # The retained family owner discovers/reaps unfinished children.
    if code:
        raise subprocess.CalledProcessError(code, command)


def rust_module_path(root):
    # JSON ASCII escapes are not Rust Unicode escapes; preserve Unicode path characters.
    return "#[path = " + json.dumps(str(root / "apps/desktop/src/desktop_services_process.rs"), ensure_ascii=False) + "]"


def build(family, interrupts):
    # Stable package and target paths avoid a new dependency/binary cache for each run.
    project = ROOT / "target/verification/unix-native-services-fixture/project"
    project.mkdir(parents=True, exist_ok=True)
    (project / "src").mkdir(exist_ok=True)
    root_lock = (ROOT / "Cargo.lock").read_bytes()
    original = tomllib.loads(root_lock.decode())["package"]
    (project / "Cargo.toml").write_text(fixture_manifest(ROOT, original))
    (project / "Cargo.lock").write_bytes(root_lock)
    source = (ROOT / "scripts/fixtures/unix_native_services.rs").read_text()
    assert source.count(MODULE_PATH) == 1, "Production early-entry module path changed"
    source = source.replace(MODULE_PATH, rust_module_path(ROOT))
    (project / "src/main.rs").write_text(source)
    manifest = str(project / "Cargo.toml")
    environment = {**os.environ, "CARGO_TARGET_DIR": str(ROOT / "target"), "CARGO_BUILD_JOBS": "2"}
    try:
        with (project / "metadata.json").open("wb") as output:
            # Fresh caches may fetch public locked manifests here; no root lock mutation.
            run(["cargo", "metadata", "--format-version", "1", "--manifest-path", manifest],
                600, family, interrupts, env=environment, stdout=output)
        validate_lock(original, tomllib.loads((project / "Cargo.lock").read_text())["package"])
        run(["cargo", "build", "--manifest-path", manifest, "--profile", "release-fast",
             "--locked", "--offline", "-j", "2"], 900, family, interrupts, env=environment)
    finally:
        assert (ROOT / "Cargo.lock").read_bytes() == root_lock, "Application lock changed during fixture build"
    return ROOT / "target/release-fast" / PACKAGE


def startup_line(stream, interrupts):
    until = time.monotonic() + 10
    value = bytearray()
    with selectors.DefaultSelector() as selector:
        selector.register(stream, selectors.EVENT_READ)
        while b"\n" not in value:
            if not interrupts.wait(selector, max(0, until - time.monotonic())):
                raise RuntimeError("Private fixture startup timed out")
            chunk = os.read(stream.fileno(), 4096 - len(value))
            if not chunk:
                raise RuntimeError("Private service exited before readiness")
            value.extend(chunk)
            if len(value) == 4096 and b"\n" not in value:
                raise RuntimeError("Oversized private startup line")
    return value.split(b"\n", 1)[0].decode()


def isolated_environment(runtime):
    # Allowlist excludes inherited desktop, keyring, activation and preload authority.
    environment = {name: os.environ[name] for name in ("PATH", "LANG", "LC_ALL", "LC_CTYPE") if name in os.environ}
    authority = runtime / "Xauthority"
    authority.touch(mode=0o600)
    for name in ("home", "data-shared", "config", "cache", "data", "tmp"):
        (runtime / name).mkdir()
    environment.update({
        "HOME": str(runtime / "home"), "XDG_DATA_DIRS": str(runtime / "data-shared"),
        "XDG_RUNTIME_DIR": str(runtime), "XAUTHORITY": str(authority),
        "XDG_CONFIG_HOME": str(runtime / "config"), "XDG_CACHE_HOME": str(runtime / "cache"),
        "XDG_DATA_HOME": str(runtime / "data"), "TMPDIR": str(runtime / "tmp"),
        "SCREENSHOTS_PRIVATE_NATIVE_FIXTURE": "1",
    })
    return environment


def exercise(binary, xvfb, daemon, family, interrupts):
    runtime = family.create_runtime()
    environment = isolated_environment(runtime)
    stopped = threading.Event()
    authority = None
    waiter = None
    try:
        # Blank owned server. The fixture never requests pixels or clipboard content.
        xserver = family.spawn([xvfb, "-displayfd", "1", "-nolisten", "tcp", "-screen", "0", "800x600x24"],
                                   env=environment, stdout=subprocess.PIPE)
        display = startup_line(xserver.stdout, interrupts)
        if not display.isdecimal():
            raise RuntimeError("Xvfb did not provide a private display number")
        environment["DISPLAY"] = f":{display}"
        service_dir = runtime / "data/dbus-1/services"
        service_dir.mkdir(parents=True)
        executable = '"' + str(binary).replace('\\', '\\\\').replace('"', '\\"') + '"'
        (service_dir / "org.freedesktop.portal.Desktop.service").write_text(
            "[D-BUS Service]\nName=org.freedesktop.portal.Desktop\nExec=" + executable + " --activated-portal-mock\n")
        address = f"unix:path={runtime}/bus"
        environment["DBUS_SESSION_BUS_ADDRESS"] = address
        config = runtime / "bus.conf"
        config.write_text('<busconfig><type>session</type><listen>' + escape(address) +
                          '</listen><auth>EXTERNAL</auth><servicedir>' + escape(str(service_dir)) +
                          '</servicedir><policy context="default"><allow own="*"/>'
                          '<allow send_destination="*"/><allow receive_sender="*"/></policy></busconfig>')
        bus = family.spawn([daemon, "--config-file=" + str(config), "--nofork", "--print-address=1"],
                               env=environment, stdout=subprocess.PIPE)
        actual = startup_line(bus.stdout, interrupts)
        if actual.split(",", 1)[0] != address:
            raise RuntimeError("D-Bus did not provide the requested private bus")
        environment["DBUS_SESSION_BUS_ADDRESS"] = actual
        for mode in MODES[:4]:
            print(f"PRIVATE mode={mode}", flush=True)
            selected = {**environment, "XDG_SESSION_TYPE": "x11"}
            if mode in ("stalled_bus", "close_stalled_bus"):
                selected["DBUS_SESSION_BUS_ADDRESS"] = f"unix:path={runtime}/bus-stalled"
            run([str(binary), mode], 30 if mode == "x11" else 15, family, interrupts, env=selected)
            (runtime / "bus-stalled").unlink(missing_ok=True)
        # Authority for singleton identity only; this is no compositor simulation.
        authority = socket.socket(socket.AF_UNIX)
        authority.bind(str(runtime / "wayland-private"))
        authority.listen(4)
        authority.settimeout(0.1)
        def accept_authority():
            while not stopped.is_set():
                try:
                    client, _ = authority.accept()
                    client.close()
                except socket.timeout:
                    pass
        waiter = threading.Thread(target=accept_authority)
        waiter.start()
        for mode in MODES[4:]:
            print(f"PRIVATE mode={mode}", flush=True)
            run([str(binary), mode, "wayland"], 30, family, interrupts,
                env={**environment, "XDG_SESSION_TYPE": "wayland", "WAYLAND_DISPLAY": "wayland-private"})
    finally:
        stopped.set()
        if waiter:
            waiter.join()
        if authority:
            authority.close()


def self_test():
    unicode_path = rust_module_path(Path("/tmp/prüfen/截图"))
    assert "prüfen/截图" in unicode_path and "\\u00" not in unicode_path
    locked = tomllib.loads((ROOT / "Cargo.lock").read_text())["package"]
    manifest = tomllib.loads(fixture_manifest(ROOT, locked))
    assert manifest["dependencies"]["app-core"]["path"] == str(ROOT / "crates/app-core")
    assert manifest["dependencies"]["x11rb"]["features"] == ["xkb", "xtest"]
    validate_lock(locked, locked)
    try:
        validate_lock(locked, [{"name": "unexpected-fixture-dependency", "version": "0"}])
    except AssertionError:
        pass
    else:
        raise AssertionError("Lock-subset guard accepted an unrelated dependency")
    with tempfile.TemporaryDirectory() as directory:
        environment = isolated_environment(Path(directory))
        assert not ({"DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "DBUS_SESSION_BUS_ADDRESS", "LD_PRELOAD"} & environment.keys())
        assert environment["HOME"].startswith(directory + "/")
    source = (ROOT / "scripts/fixtures/unix_native_services.rs").read_text()
    assert source.count(MODULE_PATH) == 1
    assert "services.action(event)" in source and '"Ctrl+Alt+G"' in source
    assert 'register_shortcut("Ctrl+Alt+F8")' in source
    assert 'probe.register_shortcut(&second.0[0])' in source
    print("Manifest, lock rejection, isolation and source-path self-check passed; no Cargo/native checks run")


def cleanup_child():
    # Fault lane only: an owned parent with a long-lived, separately grouped descendant.
    if os.fork() == 0:
        os.setsid()
        print(json.dumps({"pid": os.getpid(), "parent": os.getppid(), "group": os.getpgrp()}), flush=True)
        while True:
            signal.pause()
    while True:
        signal.pause()


def cleanup_self_test(family, interrupts):
    from unittest.mock import patch
    runtime = family.create_runtime()
    marker = runtime / "authority-retained"
    marker.write_text("owned")
    process = family.spawn([sys.executable, str(Path(__file__).resolve()), "--cleanup-child"],
                           stdout=subprocess.PIPE, env={"PATH": os.defpath})
    descendant = json.loads(startup_line(process.stdout, interrupts))
    assert descendant["parent"] == process.pid
    assert descendant["group"] == descendant["pid"] != process.pid
    try:
        run([sys.executable, "-c", "import signal; signal.pause()"], 0.05, family, interrupts)
    except subprocess.TimeoutExpired:
        pass
    else:
        raise AssertionError("Long-lived owned child did not time out")
    # Defer only reaping deterministically, not SIGKILL delivery. The patch restores
    # on assertion/error too, so the outer real teardown can always finish.
    with patch.object(family, "reap_ready"):
        assert not family.close_step(0.05), "Deferred reap must remain pending"
        until = time.monotonic() + 3
        while descendant["pid"] not in family.handles and time.monotonic() < until:
            family.close_step(0.05)
            threading.Event().wait(0.01)
        assert descendant["pid"] in family.handles, "Separately grouped descendant was not adopted"
        retained = dict(family.handles)
        assert retained and marker.exists()
        with patch.object(family, "discover", side_effect=OSError("injected inspection failure")):
            try:
                family.close_step(0)
            except OSError:
                pass
            else:
                raise AssertionError("Injected inspection failure was lost")
        assert retained == family.handles and marker.exists()
        for descriptor in retained.values():
            os.fstat(descriptor)  # Pending/error paths must retain authenticated identities.
    real_reap = family.reap_ready
    observations = []
    def interrupted_reap(events):
        if not observations:
            for _ in range(2):
                signal.raise_signal(signal.SIGINT)
                signal.raise_signal(signal.SIGTERM)
            valid = marker.exists() and retained.items() <= family.handles.items()
            try:
                for descriptor in retained.values():
                    os.fstat(descriptor)
            except OSError:
                valid = False
            observations.append(valid)
        real_reap(events)
    # Deliver repeated real signals from inside the production close/reap path.
    with patch.object(family, "reap_ready", side_effect=interrupted_reap):
        family.close()
    assert observations == [True] and interrupts.count == 4
    assert not family.handles and family.runtime is None and not runtime.exists()
    assert not Path(f"/proc/self/task/{os.getpid()}/children").read_text().split()
    print("PASS cleanup fault checks: timeout, setsid adoption, retained pending ownership, repeated signals", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--allow-private-fixtures", action="store_true")
    parser.add_argument("--xvfb", default="Xvfb", help="system Xvfb or an already verified private executable")
    parser.add_argument("--self-test", action="store_true", help="pure preparation guards; no Cargo or native children")
    parser.add_argument("--self-test-cleanup", action="store_true", help="owned Python process/signal faults; no Cargo/display/bus")
    parser.add_argument("--cleanup-child", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.cleanup_child:
        cleanup_child()
        return
    if args.self_test:
        self_test()
        return
    if sys.platform != "linux" or not args.allow_private_fixtures:
        parser.error("Linux and explicit --allow-private-fixtures are required")
    if args.self_test_cleanup:
        # The fault deliberately records interruption; success requires reporting it after cleanup.
        checks_passed = False
        try:
            with Interrupts() as interrupts:
                family = OwnedChildren()
                try:
                    cleanup_self_test(family, interrupts)
                    checks_passed = True
                finally:
                    family.close()
        except Interrupted:
            if checks_passed:
                return
            raise
        raise AssertionError("Recorded interrupts were not reported after cleanup")
    xvfb = shutil.which(args.xvfb)
    daemon = shutil.which("dbus-daemon")
    if not xvfb or not daemon:
        parser.error("Install Xvfb and dbus-daemon first; this runner performs no system installation")
    with Interrupts() as interrupts:
        family = OwnedChildren()
        try:
            binary = build(family, interrupts).resolve(strict=True)
            exercise(binary, xvfb, daemon, family, interrupts)
        finally:
            family.close()  # Handlers remain record-only through all reaping and runtime removal.
        interrupts.check()
        print("PASS private native six modes; no GUI/performance acceptance claimed", flush=True)


if __name__ == "__main__":
    main()
