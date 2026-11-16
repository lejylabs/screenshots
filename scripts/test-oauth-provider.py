#!/usr/bin/env python3
"""Opt-in Linux OAuth E2E with private identity/upload servers and unique native secrets.

Requires an already-unlocked local Secret Service, Python stdlib and OpenSSL.
Does not open a browser or contact a real account. All identities are synthetic.
"""
import argparse
import base64
import hashlib
import http.client
import http.server
import json
import os
from pathlib import Path
import selectors
import signal
import ssl
import subprocess
import tempfile
import threading
import time
import urllib.parse
import uuid


class Identity:
    def __init__(self):
        self.codes = {}
        self.refresh = {}
        self.current_access = None
        self.uploads = []
        self.device_polls = 0
        self.errors = []
        self.origin = None

    def token(self, refresh=True):
        access = "synthetic-access-" + uuid.uuid4().hex
        result = {"access_token": access, "token_type": "Bearer", "expires_in": 3600}
        self.current_access = access
        if refresh:
            value = "synthetic-refresh-" + uuid.uuid4().hex
            self.refresh[value] = 0
            result["refresh_token"] = value
        return result


class BoundedServer(http.server.HTTPServer):
    tls = None
    def get_request(self):
        client, address = super().get_request()
        client.settimeout(5)
        if self.tls:
            try:
                client = self.tls.wrap_socket(client, server_side=True)
            except BaseException:
                client.close()
                raise
        return client, address


def handler(identity):
    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"
        def log_message(self, *_):
            pass
        def response(self, status, value):
            body = json.dumps(value).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(body)
        def do_GET(self):
            try:
                url = urllib.parse.urlsplit(self.path)
                assert url.path == "/authorize"
                values = urllib.parse.parse_qs(url.query, strict_parsing=True)
                assert all(len(value) == 1 for value in values.values())
                values = {key: value[0] for key, value in values.items()}
                assert values["response_type"] == "code" and values["code_challenge_method"] == "S256"
                assert values["client_id"] == "synthetic-client"
                redirect = urllib.parse.urlsplit(values["redirect_uri"])
                assert redirect.hostname == "127.0.0.1" and redirect.path == "/oauth/callback" and not redirect.query
                code = "synthetic-code-" + uuid.uuid4().hex
                identity.codes[code] = (values["code_challenge"], values["redirect_uri"])
                location = values["redirect_uri"] + "?" + urllib.parse.urlencode({"code": code, "state": values["state"]})
                self.send_response(302)
                self.send_header("Location", location)
                self.send_header("Content-Length", "0")
                self.send_header("Connection", "close")
                self.end_headers()
            except Exception as error:
                identity.errors.append(type(error).__name__)
                self.response(500, {"error": "fixture_failure"})
        def do_POST(self):
            try:
                self.connection.settimeout(5)
                length = int(self.headers["Content-Length"])
                assert 0 <= length <= 65536
                body = self.rfile.read(length)
                assert len(body) == length
                if self.path == "/upload":
                    assert self.headers["Authorization"] == "Bearer " + identity.current_access
                    assert b"synthetic oauth upload\0\xff" in body
                    identity.uploads.append(hashlib.sha256(body).hexdigest())
                    self.response(200, {"url": "https://example.invalid/shared/" + uuid.uuid4().hex})
                    return
                assert self.headers["Content-Type"] == "application/x-www-form-urlencoded"
                values = urllib.parse.parse_qs(body.decode(), keep_blank_values=True, strict_parsing=True)
                assert all(len(value) == 1 for value in values.values())
                values = {key: value[0] for key, value in values.items()}
                assert values["client_id"] == "synthetic-client"
                if self.path == "/device":
                    assert "grant_type" not in values
                    self.response(200, {"device_code": "synthetic-device-secret", "user_code": "SYNTHETIC-CODE",
                                        "verification_uri": identity.origin + "/device-verify", "expires_in": 180, "interval": 1})
                elif self.path == "/token":
                    grant = values["grant_type"]
                    if grant == "authorization_code":
                        challenge, redirect = identity.codes.pop(values["code"])
                        actual = base64.urlsafe_b64encode(hashlib.sha256(values["code_verifier"].encode()).digest()).rstrip(b"=").decode()
                        assert actual == challenge and values["redirect_uri"] == redirect
                        self.response(200, identity.token())
                    elif grant == "refresh_token":
                        old = values["refresh_token"]
                        count = identity.refresh[old]
                        identity.refresh[old] += 1
                        if count == 0:
                            del identity.refresh[old]
                            result = identity.token()
                            # This new refresh token is used twice when the server omits rotation.
                            identity.refresh[result["refresh_token"]] = 1
                        else:
                            result = identity.token(refresh=False)
                        self.response(200, result)
                    elif grant == "urn:ietf:params:oauth:grant-type:device_code":
                        assert values["device_code"] == "synthetic-device-secret"
                        identity.device_polls += 1
                        if identity.device_polls < 3:
                            self.response(400, {"error": "authorization_pending" if identity.device_polls == 1 else "slow_down"})
                        else:
                            self.response(200, identity.token())
                    else:
                        raise AssertionError("Unknown synthetic grant")
                else:
                    raise AssertionError("Unknown synthetic endpoint")
            except Exception as error:
                identity.errors.append(type(error).__name__)
                self.response(500, {"error": "fixture_failure"})
    return Handler


def request(url):
    parsed = urllib.parse.urlsplit(url)
    assert parsed.scheme == "http" and parsed.hostname == "127.0.0.1"
    connection = http.client.HTTPConnection(parsed.hostname, parsed.port, timeout=5)
    try:
        connection.request("GET", parsed.path + ("?" + parsed.query if parsed.query else ""))
        response = connection.getresponse()
        return response.status, response.getheader("Location"), response.read(8193)
    finally:
        connection.close()


def command(binary, *args, env):
    return subprocess.run([str(binary), *map(str, args)], env=env, stdin=subprocess.DEVNULL,
                          capture_output=True, timeout=20)


def checked(result, label):
    assert b"synthetic-access-" not in result.stdout + result.stderr
    assert b"synthetic-refresh-" not in result.stdout + result.stderr
    assert b"synthetic-device-secret" not in result.stdout + result.stderr
    if result.returncode or result.stderr:
        raise RuntimeError(f"{label} failed (exit {result.returncode}): " + result.stderr.decode(errors="replace"))
    return [json.loads(line) for line in result.stdout.splitlines()]


def login(binary, profile, env, device=False):
    child = subprocess.Popen([str(binary), "oauth", "login", str(profile), "--allow-loopback-http"],
                             env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                             start_new_session=True)
    selector = selectors.DefaultSelector()
    try:
        selector.register(child.stdout, selectors.EVENT_READ)
        if not selector.select(10):
            raise RuntimeError("OAuth CLI did not emit its explicit authorization event")
        first = child.stdout.readline(16385)
        assert len(first) <= 16384
        event = json.loads(first)
        if device:
            assert event["type"] == "device_authorization" and event["user_code"] == "SYNTHETIC-CODE"
        else:
            assert event["type"] == "authorization"
            status, callback, _ = request(event["url"])
            assert status == 302
            parsed = urllib.parse.urlsplit(callback)
            values = urllib.parse.parse_qs(parsed.query)
            wrong = urllib.parse.urlunsplit(parsed._replace(query=urllib.parse.urlencode({"code": values["code"][0], "state": "deliberately-wrong"})))
            assert request(wrong)[0] == 400
            assert request(callback)[0] == 200
        out, err = child.communicate(timeout=20)
        result = subprocess.CompletedProcess(child.args, child.returncode, first + out, err)
        records = checked(result, "OAuth CLI login")
        assert records[-1]["type"] == "complete" and records[-1]["result"]["access_stored"]
    finally:
        selector.close()
        if child.poll() is None:
            os.killpg(child.pid, signal.SIGKILL)
        child.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release-fast/screenshots"))
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve()
    if not os.environ.get("DBUS_SESSION_BUS_ADDRESS", "").startswith("unix:"):
        raise RuntimeError("An already-unlocked local Secret Service is required")
    with tempfile.TemporaryDirectory(prefix="screenshots-oauth-e2e-") as temporary:
        root = Path(temporary)
        env = dict(os.environ)
        for name in ("HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"):
            env[name] = str(root / name)
        for name in ("DISPLAY", "WAYLAND_DISPLAY", "SSLKEYLOGFILE"):
            env.pop(name, None)
        key, cert = root / "tls.key", root / "tls.crt"
        ca, ca_key, request_file = root / "ca.crt", root / "ca.key", root / "server.csr"
        extensions = root / "extensions"
        extensions.write_text("basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=IP:127.0.0.1,DNS:localhost\n")
        commands = [
            ["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", str(ca_key), "-out", str(ca), "-days", "1", "-subj", "/CN=Private synthetic OAuth CA", "-addext", "basicConstraints=critical,CA:TRUE"],
            ["openssl", "req", "-new", "-newkey", "rsa:2048", "-nodes", "-keyout", str(key), "-out", str(request_file), "-subj", "/CN=localhost"],
            ["openssl", "x509", "-req", "-in", str(request_file), "-CA", str(ca), "-CAkey", str(ca_key), "-CAcreateserial", "-out", str(cert), "-days", "1", "-extfile", str(extensions)],
        ]
        for arguments in commands:
            result = subprocess.run(arguments, capture_output=True, timeout=20)
            assert result.returncode == 0
        env["SSL_CERT_FILE"] = str(ca)
        identity = Identity()
        servers, threads, cleanup, profiles = [], [], [], {}
        try:
            server = BoundedServer(("127.0.0.1", 0), handler(identity))
            servers.append(server)
            identity.origin = f"http://127.0.0.1:{server.server_port}"
            upload = BoundedServer(("127.0.0.1", 0), handler(identity))
            servers.append(upload)
            tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            tls.minimum_version = ssl.TLSVersion.TLSv1_2
            tls.load_cert_chain(cert, key)
            upload.tls = tls
            for server in servers:
                thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": .1})
                thread.start()
                threads.append((server, thread))
            source = root / "input.bin"
            source.write_bytes(b"synthetic oauth upload\0\xff")
            for device in (False, True):
                reference = "oauth-" + uuid.uuid4().hex
                destination = root / (reference + "-upload.json")
                destination.write_text(json.dumps({"url": f"https://127.0.0.1:{upload.server_port}/upload", "method": "POST", "field": "file", "json_pointer": "/url", "credential": reference, "authentication": "bearer"}))
                profile = root / (reference + ".json")
                value = {"version": 1, "flow": "device_code" if device else "authorization_code", "client_id": "synthetic-client",
                         "token_endpoint": f"https://127.0.0.1:{upload.server_port}/token", "scopes": ["upload", "offline_access"], "credential": reference,
                         "upload_destination": str(destination)}
                value["device_authorization_endpoint" if device else "authorization_endpoint"] = identity.origin + ("/device" if device else "/authorize")
                profile.write_text(json.dumps(value))
                profiles.update({profile: profile.read_bytes(), destination: destination.read_bytes()})
                cleanup.append(profile)
                refused = command(binary, "oauth", "target", profile, env=env)
                assert refused.returncode != 0 and not refused.stdout
                login(binary, profile, env, device)
                for _ in range(3):
                    outcome = checked(command(binary, "oauth", "refresh", profile, "--allow-loopback-http", env=env), "OAuth refresh")[-1]
                    assert outcome["result"]["access_stored"] and outcome["result"]["refresh_available"]
                checked(command(binary, "upload-file", destination, source, env=env), "Actual bearer upload")
                checked(command(binary, "oauth", "remove", profile, "--allow-loopback-http", env=env), "OAuth removal")
                refused = command(binary, "oauth", "refresh", profile, "--allow-loopback-http", env=env)
                assert refused.returncode != 0 and not refused.stdout
                refused = command(binary, "upload-file", destination, source, env=env)
                assert refused.returncode != 0 and not refused.stdout
            assert all(path.read_bytes() == original for path, original in profiles.items())
            assert len(identity.uploads) == 2 and not identity.errors
            assert source.read_bytes() == b"synthetic oauth upload\0\xff"
            for name in ("HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"):
                assert not (root / name).exists()
        finally:
            errors = []
            try:
                for profile in cleanup:
                    try:
                        result = command(binary, "oauth", "remove", profile, "--allow-loopback-http", env=env)
                        if result.returncode:
                            errors.append("native credential cleanup")
                    except Exception:
                        errors.append("native credential cleanup")
            finally:
                for server in servers:
                    try:
                        if any(owned is server and thread.is_alive() for owned, thread in threads):
                            server.shutdown()
                        server.server_close()
                    except Exception:
                        errors.append("private server cleanup")
                for _, thread in threads:
                    thread.join(5)
                    if thread.is_alive():
                        errors.append("private server thread cleanup")
            if errors:
                raise RuntimeError("OAuth fixture cleanup failed: " + ", ".join(errors))
        report = {"passed": True, "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(), "pkce_code_and_device_flows": True,
                  "wrong_state_refused_before_valid_callback": True, "pending_slow_down_observed": True,
                  "native_refresh_rotation_and_omission": True, "actual_https_bearer_uploads": 2, "oauth_token_posts_verified_https": True,
                  "unique_native_credentials_removed": True, "real_account_acceptance": False}
        if args.report:
            args.report.parent.mkdir(parents=True, exist_ok=True)
            args.report.write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report))


if __name__ == "__main__":
    main()
