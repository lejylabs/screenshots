#!/usr/bin/env python3
"""Opt-in Linux CLI/SigV4 check using isolated loopback TLS and synthetic credentials.

Requires OpenSSL, an unlocked local Secret Service and a built screenshots binary.
Touches only its unique native credential target and private temporary directory.
"""

import argparse
import hashlib
import hmac
import http.client
import http.server
import json
import os
from pathlib import Path
import re
import shutil
import ssl
import subprocess
import sys
import tempfile
import threading
import time
import uuid

REPO = Path(__file__).resolve().parents[1]
ACCESS_KEY = "SYNTHETICACCESSKEY"
SECRET_KEY = "SyntheticSecretKeyForLocalTestOnly0123456789"
REGION = "test-region-1"
BUCKET = "synthetic-bucket"
PREFIX = "fixture/~objects"
SIGNED_HEADERS = "content-type;host;if-none-match;x-amz-content-sha256;x-amz-date"
STATE_DIRS = ("HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME",
              "XDG_DATA_HOME", "XDG_CACHE_HOME")


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def resolve_executable(value, label):
    resolved = shutil.which(value)
    require(resolved is not None, f"{label} is unavailable")
    return str(Path(resolved).resolve())


def run(binary, arguments, env, payload=None):
    # Never include command arguments, stdout or stderr in errors: they can
    # contain a credential target or authorization data if a regression occurs.
    try:
        result = subprocess.run([binary, *map(str, arguments)], env=env,
                                input=payload, capture_output=True, timeout=20)
    except subprocess.TimeoutExpired as error:
        raise RuntimeError("CLI operation exceeded its 20-second test deadline") from error
    require(len(result.stdout) <= 65536 and len(result.stderr) <= 65536,
            "CLI output exceeded the test bound")
    for output in (result.stdout, result.stderr):
        require(ACCESS_KEY.encode() not in output and SECRET_KEY.encode() not in output,
                "CLI output exposed synthetic credential material")
    return result


def success(result, label):
    require(result.returncode == 0, f"{label} failed")
    require(not result.stderr, f"{label} emitted unexpected diagnostics")
    try:
        value = json.loads(result.stdout)
    except (UnicodeError, json.JSONDecodeError) as error:
        raise RuntimeError(f"{label} did not emit one JSON result") from error
    return value


def sign(key, message):
    return hmac.new(key, message.encode("ascii"), hashlib.sha256).digest()


class LocalServer(http.server.HTTPServer):
    def __init__(self, context):
        super().__init__(("127.0.0.1", 0), Handler)
        self.context = context
        self.records = []
        self.errors = []
        self.lock = threading.Lock()
        self.timeout = 0.2
        self.stopping = threading.Event()

    def get_request(self):
        raw, address = self.socket.accept()
        raw.settimeout(5)
        try:
            return self.context.wrap_socket(raw, server_side=True), address
        except BaseException:
            raw.close()
            raise

    def handle_error(self, request, client_address):
        with self.lock:
            self.errors.append("local TLS request failed")

    def serve(self):
        while not self.stopping.is_set():
            self.handle_request()


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_):
        pass

    def parse_request(self):
        class HeaderReader:
            def __init__(self, source):
                self.source = source
                self.remaining = 16384

            def readline(self, limit=-1):
                value = self.source.readline(min(4097, self.remaining + 1,
                                                 limit if limit >= 0 else 4097))
                self.remaining -= len(value)
                if len(value) > 4096 or self.remaining < 0:
                    raise http.client.LineTooLong("test header limit")
                return value

        original = self.rfile
        self.rfile = HeaderReader(original)
        try:
            return super().parse_request()
        finally:
            self.rfile = original

    def do_PUT(self):
        try:
            self.verify_and_reply()
        except BaseException:
            with self.server.lock:
                self.server.errors.append("independent request verification failed")
            self.send_response(400)
            self.send_header("Content-Length", "0")
            self.send_header("Connection", "close")
            self.end_headers()
        self.close_connection = True

    def verify_and_reply(self):
        require(len(self.raw_requestline) <= 4096, "request line too large")
        require(sum(len(name) + len(value) + 4 for name, value in self.headers.items()) <= 16384,
                "request headers too large")
        require(self.headers.get("Transfer-Encoding") is None, "unexpected transfer encoding")
        lengths = self.headers.get_all("Content-Length", [])
        require(len(lengths) == 1 and lengths[0].isdigit(), "invalid body length")
        length = int(lengths[0])
        require(length <= 65536, "body exceeded 64 KiB test bound")
        body = self.rfile.read(length)
        require(len(body) == length, "short request body")
        expected_prefix = f"/{BUCKET}/{PREFIX}/"
        require(self.path.startswith(expected_prefix), "unexpected object prefix")
        key = self.path[len(f"/{BUCKET}/"):]
        suffix = key[len(PREFIX) + 1:]
        require(re.fullmatch(r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}\.bin", suffix),
                "object name did not use a UUID")
        require("?" not in self.path and "%" not in self.path, "unexpected URI encoding")
        headers = {}
        for name in SIGNED_HEADERS.split(";"):
            values = self.headers.get_all(name, [])
            require(len(values) == 1, "missing or duplicate signed header")
            headers[name] = " ".join(values[0].split())
        require(headers["host"] == f"127.0.0.1:{self.server.server_port}", "wrong host")
        require(headers["content-type"] == "application/octet-stream", "wrong MIME type")
        require(headers["if-none-match"] == "*", "missing creation precondition")
        digest = hashlib.sha256(body).hexdigest()
        require(headers["x-amz-content-sha256"] == digest, "payload hash mismatch")
        date = headers["x-amz-date"]
        require(re.fullmatch(r"\d{8}T\d{6}Z", date), "invalid signing timestamp")
        canonical_headers = "".join(f"{name}:{headers[name]}\n" for name in sorted(headers))
        canonical_request = "\n".join(("PUT", self.path, "", canonical_headers,
                                       SIGNED_HEADERS, digest))
        scope = f"{date[:8]}/{REGION}/s3/aws4_request"
        to_sign = "\n".join(("AWS4-HMAC-SHA256", date, scope,
                              hashlib.sha256(canonical_request.encode("ascii")).hexdigest()))
        signing_key = sign(("AWS4" + SECRET_KEY).encode("ascii"), date[:8])
        for component in (REGION, "s3", "aws4_request"):
            signing_key = sign(signing_key, component)
        signature = hmac.new(signing_key, to_sign.encode("ascii"), hashlib.sha256).hexdigest()
        expected_auth = (f"AWS4-HMAC-SHA256 Credential={ACCESS_KEY}/{scope},"
                         f"SignedHeaders={SIGNED_HEADERS},Signature={signature}")
        authorization = self.headers.get_all("Authorization", [])
        require(len(authorization) == 1 and hmac.compare_digest(authorization[0], expected_auth),
                "signature mismatch")
        with self.server.lock:
            ordinal = len(self.server.records)
            require(ordinal < 3, "unexpected retry or extra request")
            status = (200, 200, 412)[ordinal]
            self.server.records.append({"body": body, "key": key, "status": status})
        self.send_response(status)
        self.send_header("Content-Length", "0")
        self.send_header("Connection", "close")
        if status == 200:
            self.send_header("ETag", '"synthetic-etag"')
        self.end_headers()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default=str(REPO / "target/release/screenshots"))
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    require(sys.platform.startswith("linux"), "This opt-in native credential fixture is Linux-only")
    require(os.environ.get("DBUS_SESSION_BUS_ADDRESS"), "An explicit local Secret Service session is required")
    binary = resolve_executable(args.binary, "screenshots binary")
    openssl = resolve_executable("openssl", "OpenSSL")
    started = time.monotonic()
    report = None
    with tempfile.TemporaryDirectory(prefix="screenshots-s3-test-") as temporary:
        root = Path(temporary)
        cert, key = root / "cert.pem", root / "key.pem"
        generated = subprocess.run([
            openssl, "req", "-x509", "-newkey", "rsa:2048", "-sha256", "-nodes",
            "-days", "2", "-subj", "/CN=localhost",
            "-addext", "subjectAltName=IP:127.0.0.1,DNS:localhost",
            "-addext", "basicConstraints=critical,CA:FALSE",
            "-addext", "extendedKeyUsage=serverAuth",
            "-keyout", str(key), "-out", str(cert),
        ], capture_output=True, timeout=20)
        require(generated.returncode == 0, "Test TLS certificate generation failed")
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cert, key)
        server = LocalServer(context)
        worker = threading.Thread(target=server.serve, daemon=True)
        worker.start()
        env = dict(os.environ)
        env.update({name: str(root / name.lower()) for name in STATE_DIRS})
        env.update(SSL_CERT_FILE=str(cert), NO_PROXY="*", no_proxy="*")
        env.pop("DISPLAY", None)
        env.pop("WAYLAND_DISPLAY", None)
        profile = root / "profile.json"
        profile_bytes = json.dumps({
            "endpoint": f"https://127.0.0.1:{server.server_port}", "region": REGION,
            "bucket": BUCKET, "key_prefix": PREFIX, "credential": f"test-{uuid.uuid4()}",
        }).encode("ascii")
        profile.write_bytes(profile_bytes)
        source = root / "private-source.bin"
        original = b"synthetic\x00binary\xff\xfe\r\nS3 payload"
        source.write_bytes(original)
        empty = root / "empty.bin"
        empty.write_bytes(b"")
        attempted_store = False
        cleanup_ok = False
        try:
            target = success(run(binary, ["s3-credentials", "target", profile], env), "Target query")
            require(target.get("target", "").startswith("Screenshots/s3/https://127.0.0.1:"),
                    "Credential target has the wrong namespace")
            attempted_store = True
            credentials = json.dumps({"access_key_id": ACCESS_KEY,
                                      "secret_access_key": SECRET_KEY}).encode("ascii")
            stored = success(run(binary, ["s3-credentials", "store", profile], env, credentials), "Credential store")
            require(stored == {"stored": True, "profile_file_changed": False}, "Unexpected store result")
            for path, body in ((source, original), (empty, b"")):
                result = success(run(binary, ["s3-upload", profile, path], env), "S3 upload")
                with server.lock:
                    require(not server.errors, "Local signature verification failed")
                    record = server.records[-1]
                require(record["body"] == body, "Uploaded bytes changed")
                require(result == {"bucket": BUCKET, "key": record["key"], "etag": '"synthetic-etag"'},
                        "S3 upload result did not match the verified request")
            rejected = run(binary, ["s3-upload", profile, source], env)
            require(rejected.returncode != 0 and not rejected.stdout, "HTTP 412 was reported as success")
            require(b"412" in rejected.stderr, "HTTP 412 failure omitted its status")
            with server.lock:
                require(not server.errors and len(server.records) == 3, "Unexpected retry or server failure")
                require([r["status"] for r in server.records] == [200, 200, 412], "Unexpected status sequence")
                require(server.records[-1]["body"] == original, "Rejected upload bytes changed")
                require(len({r["key"] for r in server.records}) == 3, "Object keys were reused")
            require(profile.read_bytes() == profile_bytes and source.read_bytes() == original
                    and empty.read_bytes() == b"", "Local source or profile changed")
            for name in STATE_DIRS:
                require(not Path(env[name]).exists(), "Unexpected user-state files were created")
            removed = success(run(binary, ["s3-credentials", "remove", profile], env), "Credential removal")
            require(removed == {"removed": True, "profile_file_changed": False}, "Unexpected removal result")
            cleanup_ok = True
            missing = run(binary, ["s3-upload", profile, source], env)
            require(missing.returncode != 0 and not missing.stdout, "Removed credential remained readable")
            require(b"credential" in missing.stderr.lower(), "Removed credential did not fail native lookup")
            # The missing-credential command has exited; any synchronous attempt
            # to send a request would already have been observed by the server.
            with server.lock:
                require(not server.errors and len(server.records) == 3, "Missing credential attempted networking")
            report = {"passed": True, "requests_verified": 3, "binary_and_empty_exact": True,
                      "sigv4_independently_verified": True, "http_412_no_retry": True,
                      "synthetic_credential_removed": True, "source_profile_unchanged": True,
                      "user_state_unchanged": True, "elapsed_seconds": round(time.monotonic() - started, 3)}
        finally:
            try:
                if attempted_store and not cleanup_ok:
                    result = run(binary, ["s3-credentials", "remove", profile], env)
                    cleanup_ok = result.returncode == 0
            finally:
                server.stopping.set()
                worker.join(timeout=7)
                server.server_close()
            require(not worker.is_alive(), "Local TLS server failed to stop")
            require(not attempted_store or cleanup_ok, "Synthetic credential cleanup failed")
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report))


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, subprocess.SubprocessError) as error:
        print(f"S3 fixture failed: {type(error).__name__}; no credential details emitted", file=sys.stderr)
        raise SystemExit(1)
