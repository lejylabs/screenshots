#!/usr/bin/env python3
"""Verify upload-files against a bounded local HTTP fixture, using synthetic bytes.

Requires Python 3 and a built screenshots binary. Makes no external requests and
uses no credentials. Temporary profiles explicitly opt into loopback HTTP.
"""
import argparse
import hashlib
import http.server
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading

REPO = Path(__file__).resolve().parents[1]
MAX_PAYLOAD = 64 * 1024


def file_hash(path):
    value = hashlib.sha256()
    with open(path, "rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default=str(REPO / "target/release/screenshots"))
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    binary = shutil.which(args.binary)
    if not binary:
        raise SystemExit("screenshots binary is unavailable; use --binary PATH")
    binary = str(Path(binary).resolve())
    bodies = [b"synthetic\0binary\xff\xfe\r\n", b"", b"plain synthetic text\n"]
    reports = []
    with tempfile.TemporaryDirectory(prefix="screenshots-upload-files-") as temporary:
        root = Path(temporary)
        env = {key: value for key, value in os.environ.items()
               if key not in ["DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS"]}
        state = ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME",
                 "XDG_DATA_HOME", "XDG_CACHE_HOME"]
        env.update({name: str(root / name.lower()) for name in state})
        paths = []
        for index, body in enumerate(bodies, 1):
            path = root / f"private-fixture-{index}.bin"
            path.write_bytes(body)
            paths.append(path)
        original = [file_hash(path) for path in paths]
        for case, statuses in [("success", [200, 200, 200]), ("second_failure", [200, 500])]:
            requests = []
            errors = []

            class Handler(http.server.BaseHTTPRequestHandler):
                def setup(self):
                    super().setup()
                    self.connection.settimeout(5)

                def log_message(self, *_):
                    pass

                def do_PUT(self):
                    try:
                        index = len(requests)
                        if self.path != "/file" or index >= len(statuses):
                            raise ValueError("Unexpected request path or extra upload")
                        if self.headers.get("Authorization") or self.headers.get("Cookie"):
                            raise ValueError("Unexpected credentials")
                        if self.headers.get("Content-Type") != "application/octet-stream":
                            raise ValueError("Unexpected content type")
                        if self.headers.get("If-None-Match") != "*":
                            raise ValueError("Missing create-only precondition")
                        if self.headers.get("Transfer-Encoding"):
                            raise ValueError("Unexpected transfer encoding")
                        length = int(self.headers["Content-Length"])
                        if not 0 <= length <= MAX_PAYLOAD:
                            raise ValueError("Payload exceeds fixture limit")
                        body = self.rfile.read(length)
                        if len(body) != length or body != bodies[index]:
                            raise ValueError("Request bytes differ from source")
                        requests.append({"index": index + 1, "bytes": length,
                                         "sha256": hashlib.sha256(body).hexdigest()})
                        response = f"https://example.invalid/result/{index + 1}".encode()
                        self.send_response(statuses[index])
                        self.send_header("Content-Length", str(len(response)))
                        self.send_header("Connection", "close")
                        self.end_headers()
                        self.wfile.write(response)
                    except Exception as error:
                        errors.append(str(error))
                        self.close_connection = True

            server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
            server.daemon_threads = True
            worker = threading.Thread(target=server.serve_forever,
                                      kwargs={"poll_interval": 0.05}, daemon=True)
            profile = root / f"{case}.json"
            profile.write_text(json.dumps({
                "url": f"http://127.0.0.1:{server.server_port}/file",
                "method": "PUT", "allow_http": True, "response": {"type": "text"},
            }))
            profile_hash = file_hash(profile)
            worker.start()
            try:
                result = subprocess.run([binary, "upload-files", str(profile), *(str(p) for p in paths)],
                                        env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                        timeout=15)
            finally:
                server.shutdown()
                server.server_close()
                worker.join(timeout=5)
                if worker.is_alive():
                    raise RuntimeError("Fixture server did not shut down")
            if errors:
                raise RuntimeError(f"Loopback fixture failed: {errors}")
            lines = [json.loads(line) for line in result.stdout.splitlines()]
            if case == "success":
                if result.returncode or result.stderr or len(requests) != 3 or len(lines) != 4:
                    raise RuntimeError("Expected three successful uploads and a completion row")
                for index, line in enumerate(lines[:3], 1):
                    expected = {"type": "uploaded", "index": index, "total": 3,
                                "url": f"https://example.invalid/result/{index}",
                                "thumbnail": None, "deletion": None}
                    if line != expected:
                        raise RuntimeError("Unexpected upload NDJSON row")
                if lines[3] != {"type": "complete", "uploaded": 3}:
                    raise RuntimeError("Unexpected completion NDJSON row")
            else:
                if result.returncode == 0 or len(requests) != 2 or len(lines) != 1:
                    raise RuntimeError("Failed second upload did not stop before the third")
                if lines[0] != {"type": "uploaded", "index": 1, "total": 3,
                                "url": "https://example.invalid/result/1",
                                "thumbnail": None, "deletion": None}:
                    raise RuntimeError("Failed batch did not retain exactly the first confirmation")
                if b"2 of 3" not in result.stderr or b"1 confirmed" not in result.stderr:
                    raise RuntimeError("Failed batch did not report the failed index/count")
            if [file_hash(path) for path in paths] != original or file_hash(profile) != profile_hash:
                raise RuntimeError("Upload changed a source or profile")
            if any(Path(env[name]).exists() for name in state):
                raise RuntimeError("Upload created user-state files")
            reports.append({"case": case, "exit_success": result.returncode == 0,
                            "requests": requests, "ndjson_rows": len(lines),
                            "sources_unchanged": True, "profile_unchanged": True,
                            "user_state_created": False})
    report = {"scope": "synthetic loopback HTTP only; no account/provider/TLS acceptance",
              "binary_sha256": file_hash(binary), "cases": reports}
    rendered = json.dumps(report, indent=2) + "\n"
    if args.report:
        args.report.write_text(rendered)
    print(rendered, end="")


if __name__ == "__main__":
    main()
