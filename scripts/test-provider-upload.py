#!/usr/bin/env python3
"""Run explicit provider uploads against private FTP/FTPS and SSH servers.

Linux-only native integration: Python, Paramiko 4.0, OpenSSL, OpenSSH, installed curl
and an already-unlocked Secret Service. Creates/removes only unique synthetic
credential entries. No existing credential/key, desktop, clipboard or account.
"""

import argparse
import base64
import getpass
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import ssl
import struct
import subprocess
import tempfile
import threading
import time
import zlib


class PasswordSftpServer:
    """Private in-memory SSH service advertising both key and password auth."""
    def __init__(self):
        import paramiko
        import logging
        import stat
        logging.getLogger("paramiko").addHandler(logging.NullHandler())
        self.files, self.failures = {}, []
        self.public_key_attempts = 0
        self.password_attempts = 0
        self.transport = None
        self.stop = threading.Event()
        self.socket = socket.socket()
        self.socket.bind(("127.0.0.1", 0))
        self.socket.listen(4)
        self.socket.settimeout(.2)
        self.port = self.socket.getsockname()[1]
        key = paramiko.RSAKey.generate(2048)
        self.pin = base64.b64encode(hashlib.sha256(key.asbytes()).digest()).decode()
        owner = self

        class Authentication(paramiko.ServerInterface):
            def get_allowed_auths(self, username):
                return "publickey,password"
            def check_auth_publickey(self, username, public_key):
                owner.public_key_attempts += 1
                return paramiko.AUTH_FAILED
            def check_auth_password(self, username, password):
                owner.password_attempts += 1
                return (paramiko.AUTH_SUCCESSFUL if username == "synthetic"
                        and password == "synthetic-provider-password" else paramiko.AUTH_FAILED)
            def check_channel_request(self, kind, channel):
                return paramiko.OPEN_SUCCEEDED if kind == "session" else paramiko.OPEN_FAILED_ADMINISTRATIVELY_PROHIBITED

        class Handle(paramiko.SFTPHandle):
            def __init__(self, path, flags):
                super().__init__(flags)
                self.path, self.data = path, bytearray()
            def write(self, offset, data):
                if offset != len(self.data) or len(self.data) + len(data) > 64 * 1024 * 1024:
                    owner.failures.append("unexpected or oversized SFTP write")
                    return paramiko.SFTP_FAILURE
                self.data.extend(data)
                return paramiko.SFTP_OK
            def close(self):
                owner.files[self.path] = bytes(self.data)
                return paramiko.SFTP_OK

        class Files(paramiko.SFTPServerInterface):
            def open(self, path, flags, attributes):
                if not re.fullmatch(r"/uploads/[0-9a-f]{32}\.bin", path) or path in owner.files:
                    owner.failures.append("unexpected SFTP destination")
                    return paramiko.SFTP_PERMISSION_DENIED
                return Handle(path, flags)
            def stat(self, path):
                attributes = paramiko.SFTPAttributes()
                if path in ("/", "/uploads"):
                    attributes.st_mode = stat.S_IFDIR | 0o700
                elif path in owner.files:
                    attributes.st_mode = stat.S_IFREG | 0o600
                    attributes.st_size = len(owner.files[path])
                else:
                    return paramiko.SFTP_NO_SUCH_FILE
                return attributes
            lstat = stat

        def run():
            while not self.stop.is_set():
                try:
                    client, _ = self.socket.accept()
                except socket.timeout:
                    continue
                except OSError:
                    return
                try:
                    client.settimeout(5)
                    self.transport = paramiko.Transport(client)
                    self.transport.add_server_key(key)
                    self.transport.set_subsystem_handler("sftp", paramiko.SFTPServer, Files)
                    self.transport.start_server(server=Authentication())
                    while self.transport.is_active() and not self.stop.wait(.02):
                        pass
                except (EOFError, OSError, paramiko.SSHException):
                    pass  # Expected when host-pin/password negative cases disconnect.
                finally:
                    if self.transport:
                        self.transport.close()
                        self.transport.join(5)
                    client.close()
        self.thread = threading.Thread(target=run)
        try:
            self.thread.start()
        except BaseException:
            self.socket.close()
            raise

    def close(self):
        self.stop.set()
        self.socket.close()
        if self.transport:
            self.transport.close()
        self.thread.join(6)
        assert not self.thread.is_alive()
        assert not self.failures, self.failures


class FtpServer:
    def __init__(self, context=None):
        self.context = context
        self.socket = socket.socket()
        self.socket.bind(("127.0.0.1", 0))
        self.socket.listen(8)
        self.socket.settimeout(0.2)
        self.port = self.socket.getsockname()[1]
        self.stop = threading.Event()
        self.files = {}
        self.failures = []
        self.tls_uploads = 0
        self.last_operation = None
        self.thread = threading.Thread(target=self.run)
        try:
            self.thread.start()
        except BaseException:
            self.socket.close()
            raise

    def run(self):
        while not self.stop.is_set():
            try:
                client, _ = self.socket.accept()
            except socket.timeout:
                continue
            except OSError:
                return
            try:
                self.session(client)
            except (ConnectionError, ssl.SSLError, OSError):
                client.close()  # Expected for a deliberately rejected TLS certificate.
            except Exception as error:
                self.failures.append((type(error).__name__, self.last_operation))
                client.close()

    def session(self, client):
        client.settimeout(5)
        reader = client.makefile("rb")
        listener, tls, protected, authenticated = None, False, False, False

        def reply(line):
            client.sendall(line.encode() + b"\r\n")

        reply("220 private synthetic provider fixture")
        try:
            while not self.stop.is_set():
                line = reader.readline(4097)
                if not line:
                    return
                assert len(line) <= 4096
                operation, _, argument = line.decode("utf-8").rstrip("\r\n").partition(" ")
                operation = operation.upper()
                self.last_operation = operation
                if operation == "AUTH":
                    assert self.context and not tls and argument.upper() in ("TLS", "SSL")
                    reply("234 start TLS")
                    reader.close()
                    client = self.context.wrap_socket(client, server_side=True)
                    reader = client.makefile("rb")
                    tls = True
                elif operation == "USER":
                    assert argument == "synthetic"
                    reply("331 password required")
                elif operation == "PASS":
                    assert argument == "synthetic-provider-password"
                    authenticated = True
                    reply("230 logged in")
                elif operation == "PBSZ":
                    assert tls and argument == "0"
                    reply("200 PBSZ accepted")
                elif operation == "PROT":
                    assert tls and argument == "P"
                    protected = True
                    reply("200 private data channel")
                elif operation == "PWD":
                    reply('257 "/"')
                elif operation in ("SYST", "FEAT"):
                    reply("215 UNIX Type: L8" if operation == "SYST" else "211 no additional extensions")
                elif operation in ("TYPE", "OPTS"):
                    reply("200 accepted")
                elif operation == "CWD":
                    assert authenticated and argument in ("/", "uploads", "/uploads")
                    reply("250 changed directory")
                elif operation in ("EPSV", "PASV"):
                    assert authenticated
                    if listener:
                        listener.close()
                    listener = socket.socket()
                    listener.bind(("127.0.0.1", 0))
                    listener.listen(1)
                    listener.settimeout(5)
                    port = listener.getsockname()[1]
                    reply(f"229 Entering Extended Passive Mode (|||{port}|)" if operation == "EPSV"
                          else f"227 Entering Passive Mode (127,0,0,1,{port // 256},{port % 256})")
                elif operation == "STOR":
                    assert authenticated and listener
                    name = argument.rsplit("/", 1)[-1]
                    assert re.fullmatch(r"[0-9a-f]{32}\.bin", name)
                    assert name not in self.files
                    if self.context:
                        assert tls and protected
                    reply("150 opening data connection")
                    data, _ = listener.accept()
                    data.settimeout(5)
                    if protected:
                        data = self.context.wrap_socket(data, server_side=True)
                    output = bytearray()
                    while True:
                        block = data.recv(65536)
                        if not block:
                            break
                        output.extend(block)
                        assert len(output) <= 64 * 1024 * 1024
                    data.close()
                    listener.close()
                    listener = None
                    self.files[name] = bytes(output)
                    self.tls_uploads += int(protected)
                    reply("226 upload complete")
                elif operation == "QUIT":
                    reply("221 goodbye")
                    return
                else:
                    raise AssertionError("Unexpected FTP operation")
        finally:
            if listener:
                listener.close()
            reader.close()
            client.close()

    def close(self):
        self.stop.set()
        self.socket.close()
        self.thread.join(6)
        assert not self.thread.is_alive()
        assert not self.failures, self.failures


def command(argv, **kwargs):
    return subprocess.run(list(map(str, argv)), capture_output=True, timeout=20, **kwargs)


def require(result, label):
    if result.returncode:
        message = result.stderr.decode(errors="replace")
        assert "synthetic-provider-password" not in message
        raise RuntimeError(f"{label} failed (exit {result.returncode}): {message}")
    assert not result.stderr, f"{label} emitted diagnostics"
    return json.loads(result.stdout)


def cleanup_resources(paths, remove, servers, stop_ssh, close_log):
    errors = []
    try:
        for path in paths:
            try:
                remove(path)
            except Exception:
                errors.append("synthetic credential removal")
    finally:
        try:
            for server in servers:
                try:
                    server.close()
                except Exception:
                    errors.append("private FTP server cleanup")
        finally:
            try:
                stop_ssh()
            except Exception:
                errors.append("private SSH server cleanup")
            finally:
                try:
                    close_log()
                except Exception:
                    errors.append("private SSH log cleanup")
    if errors:
        raise RuntimeError("Provider fixture cleanup failed: " + ", ".join(errors))


def cleanup_self_check():
    events = []

    def remove(path):
        events.append(path)
        if path == "first":
            raise RuntimeError("deliberate removal failure")

    class Server:
        def __init__(self, name):
            self.name = name
        def close(self):
            events.append(self.name)
            if self.name == "ftp":
                raise RuntimeError("deliberate server failure")

    try:
        cleanup_resources(["first", "second"], remove, [Server("ftp"), Server("ftps")],
                          lambda: events.append("ssh"), lambda: events.append("log"))
    except RuntimeError as error:
        assert "credential removal" in str(error) and "FTP server cleanup" in str(error)
    else:
        raise AssertionError("Cleanup failures were hidden")
    assert events == ["first", "second", "ftp", "ftps", "ssh", "log"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/screenshots"))
    parser.add_argument("--report", type=Path)
    parser.add_argument("--self-test-cleanup", action="store_true")
    args = parser.parse_args()
    cleanup_self_check()
    if args.self_test_cleanup:
        print(json.dumps({"cleanup_self_check": True}))
        return
    binary = args.binary.resolve()
    for tool in ("curl", "openssl", "ssh-keygen", "sshd", "ffmpeg"):
        if not shutil.which(tool):
            raise RuntimeError(f"Required local test tool is missing: {tool}")
    if not os.environ.get("DBUS_SESSION_BUS_ADDRESS", "").startswith("unix:"):
        raise RuntimeError("An already-unlocked local Secret Service is required")
    with tempfile.TemporaryDirectory(prefix="screenshots-provider-e2e-") as temporary:
        root = Path(temporary)
        env = dict(os.environ)
        for key in ("HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"):
            env[key] = str(root / key)
        for key in ("DISPLAY", "WAYLAND_DISPLAY", "SSH_AUTH_SOCK"):
            env.pop(key, None)
        key, certificate = root / "tls.key", root / "tls.crt"
        result = command(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", key, "-out", certificate,
                          "-days", "1", "-subj", "/CN=localhost", "-addext", "subjectAltName=IP:127.0.0.1,DNS:localhost"])
        assert result.returncode == 0
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.minimum_version = ssl.TLSVersion.TLSv1_2
        context.load_cert_chain(certificate, key)
        servers = []
        profiles, cleanup = [], []
        ssh = None
        log = None
        source = root / "binary-input.bin"
        source_bytes = b"synthetic provider source\x00\xff\r\n"
        source.write_bytes(source_bytes)
        empty = root / "empty.bin"
        empty.write_bytes(b"")

        def profile(name, value):
            path = root / f"{name}.json"
            path.write_text(json.dumps(value))
            profiles.append((path, path.read_bytes()))
            return path

        def store(path, value):
            # A failed result write/native reply can follow a committed store.
            cleanup.append(path)
            require(command([binary, "provider-credentials", "store", path], input=json.dumps(value).encode(), env=env), "Synthetic credential store")

        try:
            servers.append(FtpServer())
            servers.append(FtpServer(context))
            for mode, server in zip(("ftp", "ftps"), servers):
                value = {"version": 1, "protocol": mode, "host": "127.0.0.1", "port": server.port,
                         "remote_directory": "/uploads", "credential": "fixture-" + root.name[-8:],
                         "public_base": "https://example.invalid/uploads/"}
                if mode == "ftp":
                    value["allow_plaintext"] = True
                else:
                    value["ca_file"] = str(certificate)
                path = profile(mode, value)
                store(path, {"kind": "password", "username": "synthetic", "password": "synthetic-provider-password"})
                for item in (source, empty):
                    uploaded = require(command([binary, "provider-upload", path, item], env=env), "Native provider upload")
                    filename = uploaded["key"].rsplit("/", 1)[-1]
                    assert server.files[filename] == item.read_bytes()
                    assert uploaded["bytes"] == item.stat().st_size
                    assert uploaded["url"] == "https://example.invalid/uploads/" + filename
                if mode == "ftp":
                    invalid = dict(value)
                    invalid.pop("allow_plaintext")
                    refused = command([binary, "provider-upload", profile("ftp-refused", invalid), source], env=env)
                    assert refused.returncode != 0 and not refused.stdout
                else:
                    assert server.tls_uploads == 2
                    invalid = dict(value)
                    invalid.pop("ca_file")
                    refused = command([binary, "provider-upload", profile("ftps-untrusted", invalid), source], env=env)
                    assert refused.returncode != 0 and not refused.stdout
                    assert len(server.files) == 2

                    # The provider task action must send the finalized saved pixels.
                    def png_chunk(kind, payload):
                        return (struct.pack(">I", len(payload)) + kind + payload
                                + struct.pack(">I", zlib.crc32(kind + payload) & 0xFFFFFFFF))

                    pixels = bytes(channel for y in range(3) for x in range(4)
                                   for channel in (x * 50, y * 70, 13, 255))
                    rows = b"".join(b"\0" + pixels[y * 16:(y + 1) * 16] for y in range(3))
                    original_png = (b"\x89PNG\r\n\x1a\n"
                                    + png_chunk(b"IHDR", struct.pack(">IIBBBBB", 4, 3, 8, 6, 0, 0, 0))
                                    + png_chunk(b"IDAT", zlib.compress(rows)) + png_chunk(b"IEND", b""))
                    image = root / "task-source.png"
                    image.write_bytes(original_png)
                    recipe = root / "redaction.json"
                    recipe.write_text(json.dumps({"version": 1, "steps": [
                        {"op": "redact", "x": 1, "y": 1, "width": 2, "height": 1}]}))
                    saved, qr = root / "final.png", root / "share.png"
                    task = root / "provider-task.json"
                    task.write_text(json.dumps({"version": 1, "source": {"type": "file", "path": str(image)},
                                                "recipe": str(recipe), "actions": [
                                                    {"type": "save", "path": str(saved)},
                                                    {"type": "provider_upload", "profile": str(path)},
                                                    {"type": "link", "format": "markdown", "label": "Redacted"},
                                                    {"type": "qr", "path": str(qr)}]}))
                    result = command([binary, "task", "run", task], env=env)
                    assert result.returncode == 0 and not result.stderr
                    events = [json.loads(line) for line in result.stdout.splitlines()]
                    assert events[-1]["type"] == "complete"
                    transferred = events[1]["result"]["upload"]
                    name = transferred["key"].rsplit("/", 1)[-1]
                    assert server.files[name] == saved.read_bytes()
                    decoded = command(["ffmpeg", "-hide_banner", "-loglevel", "error", "-nostdin", "-i", saved,
                                       "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "-"], env=env)
                    assert decoded.returncode == 0 and not decoded.stderr
                    expected = bytearray(pixels)
                    for x in (1, 2):
                        expected[(4 + x) * 4:(4 + x + 1) * 4] = bytes((0, 0, 0, 255))
                    assert decoded.stdout == expected
                    assert server.files[name] != original_png
                    scanned = require(command([binary, "qr", "scan", qr], env=env), "Provider task QR decode")
                    assert [code["text"] for code in scanned["codes"]] == [transferred["url"]]
                    assert image.read_bytes() == original_png

            host_key, identity = root / "host_key", root / "identity"
            for path in (host_key, identity):
                result = command(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", path])
                assert result.returncode == 0
            authorized = root / "authorized_keys"
            authorized.write_bytes(identity.with_suffix(".pub").read_bytes())
            remote = root / "remote"
            remote.mkdir()
            listener = socket.socket()
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
            listener.close()
            config = root / "sshd_config"
            config.write_text(f"Port {port}\nListenAddress 127.0.0.1\nHostKey {host_key}\nPidFile {root / 'sshd.pid'}\nAuthorizedKeysFile {authorized}\nStrictModes no\nUsePAM no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPubkeyAuthentication yes\nAuthenticationMethods publickey\nAllowUsers {getpass.getuser()}\nForceCommand internal-sftp\nSubsystem sftp internal-sftp\nAllowTcpForwarding no\nX11Forwarding no\nPermitTTY no\nPrintMotd no\nUseDNS no\nLogLevel ERROR\n")
            log = (root / "sshd.log").open("wb")
            ssh = subprocess.Popen([shutil.which("sshd"), "-D", "-e", "-f", str(config)], stdin=subprocess.DEVNULL,
                                   stdout=subprocess.DEVNULL, stderr=log, start_new_session=True)
            deadline = time.monotonic() + 5
            while True:
                if ssh.poll() is not None:
                    raise RuntimeError("Private SSH fixture could not start")
                try:
                    connection = socket.create_connection(("127.0.0.1", port), timeout=0.2)
                    connection.close()
                    break
                except OSError:
                    if time.monotonic() >= deadline:
                        raise RuntimeError("Private SSH fixture did not become ready")
                    time.sleep(0.02)
            public_key = base64.b64decode(host_key.with_suffix(".pub").read_text().split()[1])
            pin = base64.b64encode(hashlib.sha256(public_key).digest()).decode()
            value = {"version": 1, "protocol": "sftp", "host": "127.0.0.1", "port": port,
                     "remote_directory": str(remote), "credential": "fixture-" + root.name[-8:], "host_sha256": pin}
            path = profile("sftp", value)
            store(path, {"kind": "ssh_key", "username": getpass.getuser(), "private_key": str(identity), "passphrase": ""})
            for item in (source, empty):
                uploaded = require(command([binary, "provider-upload", path, item], env=env), "Native SFTP upload")
                assert Path(uploaded["key"]).read_bytes() == item.read_bytes()
            wrong = dict(value)
            wrong["host_sha256"] = base64.b64encode(bytes(32)).decode()
            refused = command([binary, "provider-upload", profile("sftp-wrong-host", wrong), source], env=env)
            assert refused.returncode != 0 and not refused.stdout
            assert len(list(remote.iterdir())) == 2
            password_server = PasswordSftpServer()
            servers.append(password_server)
            value = {"version": 1, "protocol": "sftp", "host": "127.0.0.1", "port": password_server.port,
                     "remote_directory": "/uploads", "credential": "password-" + root.name[-8:],
                     "host_sha256": password_server.pin}
            path = profile("sftp-password", value)
            store(path, {"kind": "password", "username": "synthetic", "password": "synthetic-provider-password"})
            for item in (source, empty):
                uploaded = require(command([binary, "provider-upload", path, item], env=env), "Native password SFTP upload")
                assert password_server.files[uploaded["key"]] == item.read_bytes()
            value["credential"] = "wrong-password-" + root.name[-8:]
            path = profile("sftp-password-refused", value)
            store(path, {"kind": "password", "username": "synthetic", "password": "synthetic-invalid-password"})
            refused = command([binary, "provider-upload", path, source], env=env)
            assert refused.returncode != 0 and not refused.stdout
            assert len(password_server.files) == 2 and password_server.public_key_attempts == 0
            assert password_server.password_attempts == 3
            assert source.read_bytes() == source_bytes and not empty.read_bytes()
            assert all(path.read_bytes() == original for path, original in profiles)
            for key_name in ("HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"):
                assert not (root / key_name).exists()
        finally:
            def remove_credential(path):
                result = command([binary, "provider-credentials", "remove", path], env=env)
                if result.returncode:
                    raise RuntimeError("Synthetic provider credential removal failed")

            def stop_ssh():
                if ssh:
                    try:
                        os.killpg(ssh.pid, signal.SIGTERM)
                    except ProcessLookupError:
                        pass
                    try:
                        ssh.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        os.killpg(ssh.pid, signal.SIGKILL)
                        ssh.wait(timeout=5)

            cleanup_resources(cleanup, remove_credential, servers, stop_ssh,
                              lambda: log.close() if log else None)
        report = {"passed": True, "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                  "protocols": ["ftp", "ftps", "sftp"], "exact_binary_empty_uploads": 8,
                  "valid_sftp_password_with_publickey_advertised": True, "sftp_password_no_publickey_fallback": True,
                  "wrong_sftp_password_refusal": True,
                  "plaintext_permission_refusal": True, "untrusted_tls_refusal": True,
                  "wrong_ssh_host_key_refusal": True, "unique_native_credentials_removed": True,
                  "task_final_redacted_pixels_only": True, "task_link_qr_chain": True,
                  "source_profiles_user_state_unchanged": True}
        if args.report:
            args.report.parent.mkdir(parents=True, exist_ok=True)
            args.report.write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report))


if __name__ == "__main__":
    main()
