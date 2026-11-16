#!/usr/bin/env python3
"""Actual full-SXCU CLI E2E against private HTTPS services, using synthetic data only."""
import argparse
import base64
import email.parser
import email.policy
import hashlib
import shutil
import signal
import sys
import tarfile
import time
import tomllib
import http.server
import json
import os
import re
from pathlib import Path
import ssl
import struct
import subprocess
import tempfile
import threading
import urllib.parse
import uuid
import zlib


class Server(http.server.HTTPServer):
    context = None
    def get_request(self):
        client, address = super().get_request()
        client.settimeout(5)
        try:
            return (self.context.wrap_socket(client, server_side=True) if self.context else client), address
        except BaseException:
            client.close()
            raise


def handler(records, token):
    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"
        def log_message(self, *_):
            pass
        def handle_request(self):
            length = int(self.headers.get("Content-Length", "0"))
            assert 0 <= length <= 2 * 1024 * 1024
            body = self.rfile.read(length)
            assert len(body) == length
            content_type = self.headers.get("Content-Type", "")
            multipart = {}
            if content_type.startswith("multipart/form-data"):
                message = email.parser.BytesParser(policy=email.policy.default).parsebytes(b"Content-Type: " + content_type.encode() + b"\r\nMIME-Version: 1.0\r\n\r\n" + body)
                for part in message.iter_parts():
                    multipart[part.get_param("name", header="content-disposition")] = {"filename": part.get_filename(), "data": part.get_payload(decode=True)}
            record = {"method": self.command, "path": self.path, "body": body, "headers": dict(self.headers), "multipart": multipart}
            records.append(record)
            if self.path.startswith("/auth"):
                assert self.headers.get("Authorization") == "Bearer " + token
            parameters = urllib.parse.parse_qs(urllib.parse.urlsplit(self.path).query)
            reflected = [value for name, values in parameters.items() if name in ("key", "auth", "access_key", "access_key_id") for value in values]
            if self.headers.get("X-Auth"):
                reflected.append(self.headers["X-Auth"])
            for pair in self.headers.get("Cookie", "").split(";"):
                if "=" in pair:
                    reflected.append(pair.split("=", 1)[1].strip().strip('"'))
            if body and not content_type.startswith("multipart/form-data"):
                def is_auth(name):
                    name = name.split("}")[-1].lower().replace("_", "").replace("-", "")
                    return name in ("key", "auth", "accesskey", "credentials", "cookie") or any(value in name for value in ("password", "secret", "token"))
                raw_body = body.decode("utf-8", errors="replace")
                if content_type.lower().split(";",1)[0].strip() == "application/x-www-form-urlencoded":
                    reflected.extend(value for name, values in urllib.parse.parse_qs(raw_body).items() if is_auth(name) for value in values)
                elif raw_body.lstrip().startswith("<"):
                    import xml.etree.ElementTree as ET
                    for element in ET.fromstring(raw_body).iter():
                        if is_auth(element.tag):
                            reflected.append("".join(element.itertext()))
                        reflected.extend(value for name, value in element.attrib.items() if is_auth(name))
                elif raw_body.lstrip().startswith(("{", "[")):
                    def body_values(value, auth=False):
                        if isinstance(value, dict):
                            for name, child in value.items():
                                if name.lower() == "cookie" and isinstance(child, str):
                                    reflected.extend(pair.split("=", 1)[1].strip().strip('"') for pair in child.split(";") if "=" in pair)
                                else:
                                    body_values(child, auth or is_auth(name))
                        elif isinstance(value, list):
                            for child in value:
                                body_values(child, auth)
                        elif auth and value is not None:
                            reflected.append(str(value))
                    body_values(json.loads(raw_body))
                else:
                    reflected.extend(value for name, values in urllib.parse.parse_qs(raw_body).items() if is_auth(name) for value in values)
            raw_projection = urllib.parse.urlsplit(self.path).query.split("=", 1)[1] if "=" in urllib.parse.urlsplit(self.path).query else ""
            projections = [raw_projection, re.sub(r"%[0-9a-fA-F]{2}", lambda value: value.group(0).upper(), raw_projection)]
            if reflected:
                projections.append("".join(f"%{value:02x}" for value in reflected[0].encode("utf-8")))
            encoded_message = "denied " + " | ".join(projections)
            if self.path.startswith("/encoded-error"):
                status, value, kind = 422, {"message": encoded_message}, "application/json"
            elif self.path.startswith("/encoded-output"):
                status, value, kind = 200, {"message": encoded_message, "files": [{"id": 2, "url": "https://example.invalid/public/safe"}], "thumbnail": "", "deletion": ""}, "application/json"
            elif self.path.startswith("/reflect-error"):
                status, value, kind = 422, {"message": "denied " + " ".join(reflected)}, "application/json"
            elif self.path.startswith("/reflect-link"):
                status, value, kind = 200, {"files": [{"id": 2, "url": "https://example.invalid/share?value=" + reflected[0]}], "thumbnail": "", "deletion": ""}, "application/json"
            elif self.path.startswith("/error"):
                status, value, kind = 422, {"message": "denied " + token}, "application/json"
            elif self.path.startswith("/plain"):
                status, value, kind = 200, b"https://example.invalid/raw-response", "text/plain"
            elif self.path.startswith("/xml-response"):
                status, value, kind = 200, b"<links><link type='public'>https://example.invalid/xml</link></links>", "application/xml"
            elif self.path.startswith("/regex-response"):
                status, value, kind = 200, b'prefix href="https://example.invalid/regex" suffix', "text/plain"
            elif self.path.startswith("/no-link"):
                status, value, kind = 200, b"acknowledged", "text/plain"
            else:
                status, value, kind = 200, {"files": [{"id": 1, "url": "https://example.invalid/private"}, {"id": 2, "url": "https://example.invalid/public/" + uuid.uuid4().hex}], "thumbnail": "https://example.invalid/thumb", "deletion": "https://example.invalid/delete", "function": "inputbox"}, "application/json"
            payload = json.dumps(value).encode() if isinstance(value, dict) else value
            self.send_response(status)
            self.send_header("Content-Type", kind)
            self.send_header("Content-Length", str(len(payload)))
            self.send_header("X-Public-Link", "https://example.invalid/header")
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(payload)
        do_GET = do_POST = do_PUT = do_PATCH = do_DELETE = handle_request
    return Handler


def text_section_hash(path):
    with path.open("rb") as source:
        header = source.read(64)
        assert header[:6] == b"\x7fELF\x02\x01"
        table = struct.unpack_from("<Q", header, 40)[0]
        entry_size, count, names_index = struct.unpack_from("<HHH", header, 58)
        source.seek(table + names_index * entry_size)
        names_header = source.read(entry_size)
        names_offset, names_size = struct.unpack_from("<QQ", names_header, 24)
        source.seek(names_offset)
        names = source.read(names_size)
        for index in range(count):
            source.seek(table + index * entry_size)
            section = source.read(entry_size)
            name = struct.unpack_from("<I", section)[0]
            if names[name:].split(b"\0", 1)[0] == b".text":
                offset, length = struct.unpack_from("<QQ", section, 24)
                source.seek(offset)
                digest = hashlib.sha256()
                while length:
                    block = source.read(min(length, 65536))
                    assert block
                    digest.update(block)
                    length -= len(block)
                return digest.hexdigest()
    raise RuntimeError("ELF executable text section is missing")


def command(binary, *args, env, input=None):
    return subprocess.run([str(binary), *map(str, args)], env=env, input=input, capture_output=True, timeout=20)


def checked(result, token, label):
    assert token.encode() not in result.stdout + result.stderr, "Authentication material escaped into CLI output"
    if result.returncode or result.stderr:
        raise RuntimeError(f"{label} failed (exit {result.returncode}): " + result.stderr.decode(errors="replace"))
    return [json.loads(line) for line in result.stdout.splitlines()]


def install_lifecycle(root, binary, env, profile, options, source, token):
    """Exercise actual CLI bundles and a running old CLI across a private install upgrade."""
    repository = Path(__file__).resolve().parents[1]
    version = tomllib.loads((repository / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    pieces = list(map(int, version.split(".")))
    newer = ".".join(map(str, [pieces[0], pieces[1], pieces[2] + 1]))
    native_target = "aarch64-unknown-linux-gnu" if os.uname().machine == "aarch64" else "x86_64-unknown-linux-gnu"
    portable = root / "copied-portable"
    portable.mkdir()
    shutil.copy2(binary, portable / "screenshots")
    (portable / "lib").mkdir()
    shutil.copytree(binary.parent / "lib/screenshots-sxcu", portable / "lib/screenshots-sxcu")
    for name in ("LICENSE", "README.md", "Cargo.lock", "THIRD_PARTY_NOTICES.md"):
        shutil.copyfile(repository / name, portable / name)
    info = {"version": version, "target": native_target, "binary_sha256": hashlib.sha256((portable / "screenshots").read_bytes()).hexdigest()}
    checked(command(portable / "screenshots", "sharex", "inspect", profile, "--options", options, env=env), token, "Copied portable CLI inspect")
    def package(release):
        info["version"] = release
        (portable / "BUILD-INFO.json").write_text(json.dumps(info))
        archive = root / ("fixture-" + release + ".tar.gz")
        with tarfile.open(archive, "w:gz", compresslevel=1) as stream:
            for path in sorted(portable.rglob("*")):
                if path.is_file():
                    stream.add(path, arcname="fixture/" + path.relative_to(portable).as_posix(), recursive=False)
        archive.with_name(archive.name + ".sha256").write_text(hashlib.sha256(archive.read_bytes()).hexdigest() + "  " + archive.name + "\n")
        return archive
    def install(action, *arguments):
        result = subprocess.run([sys.executable, str(repository / "scripts/install-user.py"), action, *map(str, arguments)], env=env, capture_output=True, timeout=40)
        checked(result, token, "Private user-install " + action)
    install_root = root / "installed"
    child = None
    changed_manifest = None
    original_manifest = None
    try:
        install("install", package(version), install_root)
        checked(command(install_root / "screenshots", "sharex", "inspect", profile, "--options", options, env=env), token, "Installed CLI inspect")
        ready, go = root / "old-process-ready", root / "old-process-go"
        task = root / "installed-task.json"
        task.write_text(json.dumps({"version": 1, "source": {"type": "raw_file", "path": str(source)}, "actions": [
            {"type": "command", "program": "/bin/sh", "args": ["-c", 'printf ready > "$1"; while [ ! -f "$2" ]; do /bin/sleep 0.02; done', "fixture", str(ready), str(go)], "timeout_ms": 20000},
            {"type": "sharex_upload", "kind": "file", "profile": str(profile), "options": str(options)}]}))
        child = subprocess.Popen([str(install_root / "screenshots"), "task", "run", str(task)], env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
        deadline = time.monotonic() + 5
        while not ready.exists():
            if child.poll() is not None or time.monotonic() >= deadline:
                raise RuntimeError("Installed old CLI did not reach its explicit upgrade checkpoint")
            time.sleep(.02)
        # Both fixture packages intentionally reuse the same compiled old binary.
        # This tests retained-runtime selection; it is not new-release code acceptance.
        install("install", package(newer), install_root)
        changed_manifest = install_root / "versions" / (newer + "-" + native_target) / "lib/screenshots-sxcu/RUNTIME-MANIFEST.json"
        original_manifest = changed_manifest.read_bytes()
        changed_manifest.write_bytes(b"new-version-runtime-deliberately-unavailable")
        go.touch()
        out, err = child.communicate(timeout=15)
        checked(subprocess.CompletedProcess(child.args, child.returncode, out, err), token, "Running old installed CLI after upgrade")
        changed_manifest.write_bytes(original_manifest)
        original_manifest = None
        checked(command(install_root / "screenshots", "sharex", "inspect", profile, "--options", options, env=env), token, "Compiled old CLI ignores mutable current runtime")
    finally:
        if child and child.poll() is None:
            child.terminate()
            try:
                child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=3)
        if original_manifest is not None:
            changed_manifest.write_bytes(original_manifest)
        if (install_root / ".screenshots-install.json").exists():
            install("uninstall", install_root)
    return {"copied_cli_inspect": True, "installed_cli_inspect": True, "running_old_cli_retained_runtime_after_upgrade": True,
            "fixture_reuses_old_compiled_binary_not_new_release_acceptance": True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release-fast/screenshots"))
    parser.add_argument("--report", type=Path)
    parser.add_argument("--skip-install", action="store_true", help="Development-only isolation; final acceptance requires install lifecycle")
    parser.add_argument("--skip-task", action="store_true", help="Development-only transport isolation; final acceptance requires Task")
    args = parser.parse_args()
    source_binary = args.binary.resolve()
    binary = source_binary
    if not os.environ.get("DBUS_SESSION_BUS_ADDRESS", "").startswith("unix:"):
        raise RuntimeError("An already-unlocked local Secret Service is required")
    with tempfile.TemporaryDirectory(prefix="screenshots-sxcu-e2e-") as temporary:
        root = Path(temporary)
        # Freeze the tested app/runtime so concurrent root Cargo checks cannot swap them.
        frozen = root / "frozen-app"
        frozen.mkdir()
        binary = frozen / "screenshots"
        shutil.copy2(source_binary, binary)
        before = text_section_hash(binary)
        subprocess.run(["strip", "--strip-debug", str(binary)], check=True, capture_output=True, timeout=30)
        assert text_section_hash(binary) == before
        (frozen / "lib").mkdir()
        shutil.copytree(source_binary.parent / "lib/screenshots-sxcu", frozen / "lib/screenshots-sxcu")
        env = dict(os.environ)
        for name in ("HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"):
            env[name] = str(root / name)
        for name in ("DISPLAY", "WAYLAND_DISPLAY"):
            env.pop(name, None)
        env["SSLKEYLOGFILE"] = str(root / "must-not-create-tls-keylog")
        ca, ca_key, key, cert, csr, extensions = [root / name for name in ("ca.crt", "ca.key", "tls.key", "tls.crt", "server.csr", "extensions")]
        extensions.write_text("basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=IP:127.0.0.1,DNS:localhost\n")
        for arguments in [
            ["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", str(ca_key), "-out", str(ca), "-days", "1", "-subj", "/CN=Private synthetic SXCU CA", "-addext", "basicConstraints=critical,CA:TRUE"],
            ["openssl", "req", "-new", "-newkey", "rsa:2048", "-nodes", "-keyout", str(key), "-out", str(csr), "-subj", "/CN=localhost"],
            ["openssl", "x509", "-req", "-in", str(csr), "-CA", str(ca), "-CAkey", str(ca_key), "-CAcreateserial", "-out", str(cert), "-days", "1", "-extfile", str(extensions)]]:
            assert subprocess.run(arguments, capture_output=True, timeout=20).returncode == 0
        env["SSL_CERT_FILE"] = str(ca)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.minimum_version = ssl.TLSVersion.TLSv1_2
        context.load_cert_chain(cert, key)
        records, other_records, plain_records = [], [], []
        token = "synthetic-sxcu-token-" + uuid.uuid4().hex
        servers, threads, cleanup = [], [], []
        snapshots = {}

        def write(name, value):
            path = root / name
            path.write_text(json.dumps(value))
            snapshots[path] = path.read_bytes()
            return path

        try:
            for record in (records, other_records, plain_records):
                server = Server(("127.0.0.1", 0), handler(record, token))
                server.context = None if record is plain_records else context
                servers.append(server)
                thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": .1})
                thread.start()
                threads.append((server, thread))
            origin = f"https://127.0.0.1:{servers[0].server_port}"
            other = f"https://127.0.0.1:{servers[1].server_port}"
            source = root / "private-original-name.bin"
            original = b"synthetic unchanged binary\0\xff\r\n"
            source.write_bytes(original)
            text = root / "text.txt"
            text.write_text('synthetic "text" & <xml>\n', encoding="utf-8")
            url_input = root / "url.txt"
            url_input.write_text("https://example.invalid/original?q=a+b\n")
            native = write("native.json", {"url": origin + "/native", "method": "POST", "field": "file", "json_pointer": "/url", "credential": "sxcu-" + uuid.uuid4().hex, "authentication": "bearer"})
            cleanup.append(native)
            checked(command(binary, "credentials", "store", native, env=env, input=token.encode()), token, "Synthetic native credential store")
            options = write("options.json", {"version": 1, "filename": "public file.bin", "credential_profile": str(native),
                                            "answers": {"Headers.X-Variant#0": "beta", "Arguments.caption#0": "caption"}, "name_values": {"now": "2026-11-15T12:34:56"}})

            def profile(body="Binary", method="POST", path="/auth", destination="FileUploader", **extra):
                value = {"Version": "17.0.0", "Name": "Synthetic fixture", "DestinationType": destination, "RequestMethod": method,
                         "RequestURL": origin + path, "Body": body, "FileFormName": "file", "URL": "{json:files[?(@.id == 2)].url}",
                         "ThumbnailURL": "{json:thumbnail}", "DeletionURL": "{json:deletion}"}
                value.update(extra)
                return value

            def execute(name, value, kind, item=source, opts=options):
                path = write(name + ".sxcu", value)
                arguments = ["sharex", kind, path, item]
                if opts:
                    arguments.extend(("--options", opts))
                output = checked(command(binary, *arguments, env=env), token, "Actual SXCU upload")[-1]
                assert output["success"]
                return path, output

            # Every supported method uses the same admitted binary body; no retry is hidden.
            for method in ("GET", "POST", "PUT", "PATCH", "DELETE"):
                _, outcome = execute("binary-" + method, profile(method=method), "file")
                assert records[-1]["method"] == method and records[-1]["body"] == original
                assert outcome["thumbnail"] == "https://example.invalid/thumb" and outcome["deletion"] == "https://example.invalid/delete"
            multipart = profile(body="MultipartFormData", path="/auth/{filename}?q=old&empty", Parameters={"q": "overridden", "filename": "{filename}"}, Headers={"X-Variant": "{select:alpha|beta}", "X-Encoded": "{base64:{filename}}"}, Arguments={"caption": "{inputbox:Caption|default}", "date": "%y-%mo-%d"})
            path, _ = execute("multipart", multipart, "file")
            record = records[-1]
            assert record["multipart"]["file"] == {"filename": "public file.bin", "data": original}
            assert record["multipart"]["caption"]["data"] == b"caption" and record["multipart"]["date"]["data"] == b"2026-11-15"
            assert record["headers"]["x-variant"] == "beta" and base64.b64decode(record["headers"]["x-encoded"]) == b"public file.bin"
            query = urllib.parse.parse_qs(urllib.parse.urlsplit(record["path"]).query, keep_blank_values=True)
            assert query == {"q": ["overridden"], "filename": ["public file.bin"], "empty": [""]}
            inspected = checked(command(binary, "sharex", "inspect", path, "--options", options, env=env), token, "SXCU inspect")[-1]
            assert any(item["id"] == "Arguments.caption#0" for item in inspected["answers"])
            # All text modes, including no-file multipart and exact JSON/XML escaping.
            for body in ("None", "MultipartFormData", "FormURLEncoded", "JSON", "XML", "Binary"):
                extra = {"FileFormName": "", "Arguments": {"value": "{input}"}}
                if body == "JSON":
                    extra["Data"] = '{"text":"{input}"}'
                if body == "XML":
                    extra["Data"] = "<text>{input}</text>"
                _, _ = execute("text-" + body, profile(body=body, destination="TextUploader", **extra), "text", text)
                record = records[-1]
                if body == "None":
                    assert not record["body"]
                elif body == "MultipartFormData":
                    assert record["multipart"]["value"]["data"] == text.read_bytes()
                elif body == "FormURLEncoded":
                    assert urllib.parse.parse_qs(record["body"].decode())["value"] == [text.read_text()]
                elif body == "JSON":
                    assert json.loads(record["body"])["text"] == text.read_text()
                elif body == "XML":
                    import xml.etree.ElementTree as ET
                    assert ET.fromstring(record["body"]).text == text.read_text()
                else:
                    assert record["body"] == text.read_bytes()
            for label, path_value, template, expected in [
                ("xml", "/xml-response", "{xml:/links/link[@type='public']}", "https://example.invalid/xml"),
                ("regex", "/regex-response", r'{regex:(?<=href=").+(?=")}', "https://example.invalid/regex"),
                ("header", "/auth", "{header:X-Public-Link}", "https://example.invalid/header"),
                ("plain", "/plain", "", "https://example.invalid/raw-response")]:
                _, outcome = execute(label, profile(path=path_value, URL=template, ThumbnailURL="", DeletionURL=""), "file")
                assert outcome["url"] == expected
            # URL sharing permits no returned link; shortening uses the same explicit URL input.
            _, outcome = execute("url", profile(body="None", path="/plain?target={input}", destination="URLShortener", URL="", ThumbnailURL="", DeletionURL=""), "url", url_input)
            assert outcome["url"] == "https://example.invalid/raw-response"
            _, outcome = execute("url-sharing", profile(body="None", path="/no-link", destination="URLSharingService", URL="", ThumbnailURL="", DeletionURL=""), "url-sharing", url_input)
            assert outcome["url"] is None
            # Explicit outputbox is a result event, without a fake link or clipboard side effect.
            _, outcome = execute("output", profile(URL="{outputbox:Result|uploaded}", ThumbnailURL="", DeletionURL=""), "file")
            assert outcome["url"] is None and outcome["events"] == [{"title": "Result", "text": "uploaded"}]
            # Failure functions work and redact known authentication material from explicit output.
            error = write("error.sxcu", profile(path="/error", ErrorMessage="{json:message}"))
            result = command(binary, "sharex", "file", error, source, "--options", options, env=env)
            assert result.returncode != 0 and token.encode() not in result.stdout + result.stderr
            assert json.loads(result.stdout.splitlines()[-1])["error_message"] == "denied [redacted]"
            count = len(records)
            bad_options = write("missing-answer.json", {"version": 1})
            refused = command(binary, "sharex", "file", root / "multipart.sxcu", source, "--options", bad_options, env=env)
            assert refused.returncode != 0 and not refused.stdout and len(records) == count
            wrong = write("wrong-origin.sxcu", profile(RequestURL=other + "/auth"))
            refused = command(binary, "sharex", "file", wrong, source, "--options", options, env=env)
            assert refused.returncode != 0 and not refused.stdout and not other_records
            # Review regressions: known auth aliases fail before helper/keyring/network.
            missing_native = write("unreadable-native.json", {"url": origin + "/unused", "method": "POST", "field": "file", "json_pointer": "/url", "credential": "not-stored-" + uuid.uuid4().hex, "authentication": "bearer"})
            refusal_options = write("refusal-options.json", {"version": 1, "allow_http": True, "credential_profile": str(missing_native), "credential_origin": origin})
            no_runtime = root / "without-runtime"
            no_runtime.mkdir()
            shutil.copy2(binary, no_runtime / "screenshots")
            plain_origin = f"http://127.0.0.1:{servers[2].server_port}"
            for key_name in ("key", "auth", "access_key", "access_key_id"):
                secret = "synthetic-alias-" + uuid.uuid4().hex
                rejected_profile = write("plain-" + key_name + ".sxcu", profile(RequestURL=plain_origin + "/refuse?" + urllib.parse.urlencode({key_name: secret})))
                absent_keyring = dict(env)
                absent_keyring.pop("DBUS_SESSION_BUS_ADDRESS", None)
                refused = command(no_runtime / "screenshots", "sharex", "file", rejected_profile, source, "--options", refusal_options, env=absent_keyring)
                assert refused.returncode != 0 and not refused.stdout and b"requires HTTPS" in refused.stderr and secret.encode() not in refused.stderr and not plain_records
                reflected_profile = write("alias-reflection-" + key_name + ".sxcu", profile(path="/reflect-error?" + urllib.parse.urlencode({key_name: secret}), ErrorMessage="{json:message}"))
                result = command(binary, "sharex", "file", reflected_profile, source, env=env)
                assert result.returncode != 0 and secret.encode() not in result.stdout + result.stderr
                assert json.loads(result.stdout.splitlines()[-1])["error_message"] == "denied [redacted]"
            alias_secret = "synthetic-header-" + uuid.uuid4().hex
            alias_profile = write("x-auth-reflection.sxcu", profile(path="/reflect-error", Headers={"X-Auth": alias_secret}, ErrorMessage="{json:message}"))
            result = command(binary, "sharex", "file", alias_profile, source, env=env)
            assert result.returncode != 0 and alias_secret.encode() not in result.stdout + result.stderr and json.loads(result.stdout.splitlines()[-1])["error_message"] == "denied [redacted]"
            alias_http = write("plain-x-auth.sxcu", profile(RequestURL=plain_origin + "/refuse", Headers={"X-Auth": alias_secret}))
            refused = command(no_runtime / "screenshots", "sharex", "file", alias_http, source, "--options", refusal_options, env=absent_keyring)
            assert refused.returncode != 0 and not refused.stdout and b"requires HTTPS" in refused.stderr and not plain_records
            dynamic_secret = "synthetic-dynamic-" + uuid.uuid4().hex
            dynamic_origin_profile = write("dynamic-auth.sxcu", profile(RequestURL="{inputbox:Endpoint}", Parameters={"key": dynamic_secret}))
            dynamic_options = write("dynamic-auth-options.json", {"version": 1, "answers": {"RequestURL#0": other + "/reflect-error"}, "credential_origin": origin, "credential_profile": str(missing_native)})
            refused = command(binary, "sharex", "file", dynamic_origin_profile, source, "--options", dynamic_options, env=absent_keyring)
            assert refused.returncode != 0 and not refused.stdout and b"origin does not match" in refused.stderr and not other_records
            cookie_values = ["synthetic-cookie-" + uuid.uuid4().hex + "==", "synthetic-cookie-" + uuid.uuid4().hex + "=a=b", "synthetic-cookie-" + uuid.uuid4().hex + "=="]
            cookie_header = "sid=" + cookie_values[0] + "; access=" + cookie_values[1] + '; quoted="' + cookie_values[2] + '"'
            cookie_profile = write("cookie-reflection.sxcu", profile(path="/reflect-error", Headers={"Cookie": cookie_header}, ErrorMessage="{json:message}"))
            result = command(binary, "sharex", "file", cookie_profile, source, env=env)
            assert result.returncode != 0 and all(value.encode() not in result.stdout + result.stderr for value in cookie_values)
            assert json.loads(result.stdout.splitlines()[-1])["error_message"] == "denied [redacted] [redacted] [redacted]"
            cookie_link = write("cookie-link.sxcu", profile(path="/reflect-link", Headers={"Cookie": cookie_header}))
            result = command(binary, "sharex", "file", cookie_link, source, env=env)
            assert result.returncode != 0 and not result.stdout and all(value.encode() not in result.stderr for value in cookie_values)
            for field in ("URL", "ThumbnailURL", "DeletionURL", "ErrorMessage"):
                for form in ("{inputbox:Title|default}", "{prompt:Title}", "{select:first|second}"):
                    count = len(records)
                    unanswered = write("response-answer.sxcu", profile(path="/plain", **{field: form}))
                    refused = command(binary, "sharex", "file", unanswered, source, "--options", refusal_options, env=absent_keyring)
                    assert refused.returncode != 0 and not refused.stdout and b"answer_required" in refused.stderr and len(records) == count
            indexed_profile = write("indexed.sxcu", profile(path="/values", URL="{select:{json:files[0].url}|{json:files[1].url}}"))
            indexed_options = write("indexed-options.json", {"version": 1, "answer_indices": {"URL#0": 1}})
            result = checked(command(binary, "sharex", "file", indexed_profile, source, "--options", indexed_options, env=env), token, "Precommitted response index")[-1]
            assert result["success"] and result["url"].startswith("https://example.invalid/public/")
            count = len(records)
            refused = command(binary, "sharex", "file", indexed_profile, source, "--options", refusal_options, env=absent_keyring)
            assert refused.returncode != 0 and not refused.stdout and b"selection_index_required" in refused.stderr and len(records) == count
            computed_profile = write("computed-response.sxcu", profile(path="/values", URL="{{json:function}:Caption}"))
            count = len(records)
            refused = command(binary, "sharex", "file", computed_profile, source, "--options", refusal_options, env=absent_keyring)
            assert refused.returncode != 0 and not refused.stdout and b"function_binding_required" in refused.stderr and len(records) == count
            bound_options = write("computed-options.json", {"version": 1, "function_names": {"URL#0": "inputbox"}, "answers": {"URL#0": "https://example.invalid/explicit"}})
            bound = checked(command(binary, "sharex", "file", computed_profile, source, "--options", bound_options, env=env), token, "Bound computed response prompt")[-1]
            assert bound["url"] == "https://example.invalid/explicit"
            static_profile = write("computed-static.sxcu", profile(path="/values", URL="{re{select:sponse|sponse}url}"))
            static_options = write("computed-static-options.json", {"version": 1, "answers": {"URL#1": "sponse"}})
            resolved = checked(command(binary, "sharex", "file", static_profile, source, "--options", static_options, env=env), token, "Statically resolved computed response name")[-1]
            assert resolved["url"] == origin + "/values"
            # Literal query names retain upstream wire spelling but security uses delivered names.
            for encoded_name in ("a%75th", "%6bey", "access%5fkey", "%61ccess%5Fkey%5Fid"):
                encoded_value = "synthetic-projected-" + uuid.uuid4().hex
                cleartext = write("encoded-name-http.sxcu", profile(RequestURL=plain_origin + "/refuse", Parameters={encoded_name: encoded_value}))
                refused = command(no_runtime / "screenshots", "sharex", "file", cleartext, source, "--options", refusal_options, env=absent_keyring)
                assert refused.returncode != 0 and not refused.stdout and b"requires HTTPS" in refused.stderr and not plain_records
                computed = write("encoded-name-dynamic.sxcu", profile(RequestURL="{inputbox:Endpoint}", Parameters={encoded_name: encoded_value}))
                unbound_options = write("encoded-unbound-options.json", {"version": 1, "answers": {"RequestURL#0": other + "/reflect-error"}})
                refused = command(no_runtime / "screenshots", "sharex", "file", computed, source, "--options", unbound_options, env=absent_keyring)
                assert refused.returncode != 0 and not refused.stdout and b"fixed HTTPS origin" in refused.stderr and not other_records
                refused = command(binary, "sharex", "file", computed, source, "--options", dynamic_options, env=absent_keyring)
                assert refused.returncode != 0 and not refused.stdout and b"origin does not match" in refused.stderr and not other_records
            double_embedded = write("embedded-encoded-http.sxcu", profile(RequestURL=plain_origin + "/refuse?a%2575th=synthetic-private"))
            refused = command(no_runtime / "screenshots", "sharex", "file", double_embedded, source, "--options", refusal_options, env=absent_keyring)
            assert refused.returncode != 0 and not refused.stdout and b"requires HTTPS" in refused.stderr and not plain_records
            for projected_secret in ("synthetic+private", "synthetic private", "p%q&=/é"):
                projections = [urllib.parse.quote_plus(projected_secret), urllib.parse.quote(projected_secret, safe=""), "".join(f"%{value:02x}" for value in projected_secret.encode())]
                projection_profile = write("encoded-secret-error.sxcu", profile(path="/encoded-error", Parameters={"key": projected_secret}, ErrorMessage="{json:message}"))
                result = command(binary, "sharex", "file", projection_profile, source, env=env)
                assert result.returncode != 0 and json.loads(result.stdout.splitlines()[-1])["error_message"] == "denied [redacted] | [redacted] | [redacted]"
                assert all(value.encode() not in result.stdout + result.stderr for value in [projected_secret, *projections])
                projection_output = write("encoded-secret-output.sxcu", profile(path="/encoded-output", Parameters={"key": projected_secret}, URL="{outputbox:Encoded|{json:message}}{json:files[0].url}", ThumbnailURL="", DeletionURL=""))
                output = checked(command(binary, "sharex", "file", projection_output, source, env=env), token, "Encoded known-secret output redaction")[-1]
                assert output["events"] == [{"title": "Encoded", "text": "denied [redacted] | [redacted] | [redacted]"}]
                response_url_profile = write("encoded-secret-link.sxcu", profile(path="/values", Parameters={"key": projected_secret}, URL="{responseurl}"))
                rejected_link = command(binary, "sharex", "file", response_url_profile, source, env=env)
                assert rejected_link.returncode != 0 and not rejected_link.stdout and all(value.encode() not in rejected_link.stderr for value in [projected_secret, *projections])
                forbidden_qr = root / "must-not-create-secret-qr.png"
                rejected_task = write("encoded-secret-task.json", {"version": 1, "source": {"type": "raw_file", "path": str(source)}, "actions": [
                    {"type": "upload", "profile": str(response_url_profile)}, {"type": "link", "format": "markdown", "label": "Private"}, {"type": "qr", "path": str(forbidden_qr)}]})
                rejected = command(binary, "task", "run", rejected_task, env=env)
                assert rejected.returncode != 0 and not forbidden_qr.exists() and all(value.encode() not in rejected.stdout + rejected.stderr for value in [projected_secret, *projections])
            public_token_profile = write("public-token.sxcu", profile(path="/values", Parameters={"key": "known-private-only"}, URL="https://example.invalid/share?token=unrecognized-public-token", ThumbnailURL="", DeletionURL=""))
            public = checked(command(binary, "sharex", "file", public_token_profile, source, env=env), token, "Unrecognized public share token remains valid")[-1]
            assert public["url"] == "https://example.invalid/share?token=unrecognized-public-token"
            # Raw spellings remain known credentials even if URL decoding changes their meaning.
            for raw_secret in ("synthetic+private", "private%2Fcredential"):
                for reflected in (False, True):
                    raw_profile = write("raw-known-link.sxcu", profile(path="/reflect-link" if reflected else "/values", Parameters={"key": raw_secret}, URL="{json:files[0].url}" if reflected else "https://example.invalid/x?key=" + raw_secret, ThumbnailURL="", DeletionURL=""))
                    rejected = command(binary, "sharex", "file", raw_profile, source, env=env)
                    assert rejected.returncode != 0 and not rejected.stdout and raw_secret.encode() not in rejected.stderr
                    forbidden = root / "raw-known-qr.png"
                    raw_task = write("raw-known-task.json", {"version": 1, "source": {"type": "raw_file", "path": str(source)}, "actions": [
                        {"type": "upload", "profile": str(raw_profile)}, {"type": "link", "format": "markdown", "label": "Private"}, {"type": "qr", "path": str(forbidden)}]})
                    rejected = command(binary, "task", "run", raw_task, env=env)
                    assert rejected.returncode != 0 and not rejected.stdout and not forbidden.exists() and raw_secret.encode() not in rejected.stderr
            # Inspect masks only known reflected auth values, leaving useful public metadata intact.
            inspect_known = "opaquevalue+plus"
            inspect_cookie = "cookieopaque=="
            inspected_profile = write("known-inspect.sxcu", profile(path="/values", Headers={"Authorization": "Bearer " + inspect_known, "Cookie": "sid=" + inspect_cookie + "; second=secondopaque", "X-Api-Key": "private%2Fcredential"}, Parameters={"a%75th": "paramopaque"}, URL="{inputbox:Caption|" + inspect_known + "}", ThumbnailURL="{inputbox:secondopaque|public fallback}", DeletionURL="{select:public-first|" + inspect_cookie + "|opaquevalue%2bplus|unrelated-public-token}", ErrorMessage="{inputbox:Other|private%252Fcredential}"))
            inspect_options = write("known-inspect-options.json", {"version": 1, "credential_profile": str(missing_native)})
            count = len(records)
            inspection = checked(command(binary, "sharex", "inspect", inspected_profile, "--options", inspect_options, env=absent_keyring), token, "Known auth reflection in inspect")[-1]
            by_id = {value["id"]: value for value in inspection["answers"]}
            assert by_id["URL#0"]["title"] == "Caption" and by_id["URL#0"]["default_text"] == "[redacted]"
            assert by_id["ThumbnailURL#0"]["title"] == "[redacted]" and by_id["ThumbnailURL#0"]["default_text"] == "public fallback"
            assert by_id["DeletionURL#0"]["choices"] == ["public-first", "[redacted]", "[redacted]", "unrelated-public-token"]
            assert by_id["ErrorMessage#0"]["default_text"] == "[redacted]"
            assert any(value["id"] == "URL#0" and value["answer_kind"] == "value" for value in inspection["answer_contracts"])
            assert len(records) == count and all(value not in json.dumps(inspection, ensure_ascii=False) for value in (inspect_known, inspect_cookie, "secondopaque", "private%2Fcredential", "private%252Fcredential", "opaquevalue%2bplus"))
            # Configured and resolved body auth share the same guard/mask across JSON, XML and raw forms.
            body_cases = [("XML", "<request><password>opaque-xml-42</password><caption>public</caption></request>", "opaque-xml-42", {}),
                          ("XML", "<request password='opaque-attribute-42'><caption>public</caption></request>", "opaque-attribute-42", {}),
                          ("JSON", '{"credentials":{"password":"opaque-json-42"},"caption":"public"}', "opaque-json-42", {}),
                          ("JSON", "password=opaque%2Bform&caption=public", "opaque+form", {"Content-Type": "application/x-www-form-urlencoded"}),
                          ("JSON", '{"Cookie":"sid=opaque-body-cookie=="}', "opaque-body-cookie==", {})]
            body_cases.extend(("JSON", prefix+"=public&password=opaque-prefix-51", "opaque-prefix-51", {"Content-Type":"application/x-www-form-urlencoded"})
                              for prefix in ["{caption}","[caption]","<caption>",'{"public":true}'])
            for body_mode, literal_body, body_secret, body_headers in body_cases:
                clear_body = write("body-http.sxcu", profile(body=body_mode, destination="TextUploader", RequestURL=plain_origin + "/refuse", Data=literal_body, Headers=body_headers))
                denied = command(no_runtime / "screenshots", "sharex", "text", clear_body, text, "--options", refusal_options, env=absent_keyring)
                assert denied.returncode != 0 and not denied.stdout and b"requires HTTPS" in denied.stderr and not plain_records
                unbound_body = write("body-dynamic.sxcu", profile(body=body_mode, destination="TextUploader", RequestURL="{inputbox:Endpoint}", Data=literal_body, Headers=body_headers))
                denied = command(no_runtime / "screenshots", "sharex", "text", unbound_body, text, "--options", unbound_options, env=absent_keyring)
                assert denied.returncode != 0 and not denied.stdout and b"fixed HTTPS origin" in denied.stderr and not other_records
                denied = command(binary, "sharex", "text", unbound_body, text, "--options", dynamic_options, env=absent_keyring)
                assert denied.returncode != 0 and not denied.stdout and b"origin does not match" in denied.stderr and not other_records
                body_failure = write("body-failure.sxcu", profile(body=body_mode, destination="TextUploader", path="/reflect-error", Data=literal_body, Headers=body_headers, ErrorMessage="{outputbox:Failure|{json:message}}{json:message}"))
                failure = command(binary, "sharex", "text", body_failure, text, env=env)
                assert failure.returncode != 0 and body_secret.encode() not in failure.stdout + failure.stderr
                failure_value = json.loads(failure.stdout.splitlines()[-1])
                assert failure_value["error_message"] == "denied [redacted]" and failure_value["events"] == [{"title": "Failure", "text": "denied [redacted]"}]
                body_link = write("body-link.sxcu", profile(body=body_mode, destination="TextUploader", path="/reflect-link", Data=literal_body, Headers=body_headers))
                failed_link = command(binary, "sharex", "text", body_link, text, env=env)
                assert failed_link.returncode != 0 and not failed_link.stdout and body_secret.encode() not in failed_link.stderr
                no_qr = root / "body-must-not-share.png"
                body_task = write("body-task.json", {"version": 1, "source": {"type": "raw_file", "path": str(text)}, "actions": [
                    {"type": "sharex_upload", "kind": "text", "profile": str(body_link)}, {"type": "link", "format": "markdown", "label": "Private"}, {"type": "qr", "path": str(no_qr)}]})
                failed_task = command(binary, "task", "run", body_task, env=env)
                assert failed_task.returncode != 0 and not failed_task.stdout and not no_qr.exists() and body_secret.encode() not in failed_task.stderr
                body_event = write("body-event.sxcu", profile(body=body_mode, destination="TextUploader", path="/values", Data=literal_body, Headers=body_headers, URL="{outputbox:Echo|" + body_secret + "}{json:files[1].url}"))
                event = checked(command(binary, "sharex", "text", body_event, text, env=env), token, "Known body auth event redaction")[-1]
                assert event["events"] == [{"title": "Echo", "text": "[redacted]"}]
                assert body_secret.encode() in records[-1]["body"] or body_secret == "opaque+form"
            for prefix in ["{caption}","[caption]","<caption>",'{"public":true}']:
                prefix_profile=write("prefix-inspect.sxcu",profile(body="JSON",destination="TextUploader",Data=prefix+"=public&password=opaque-prefix-51",URL="{inputbox:Caption|opaque-prefix-51}"))
                inspected=checked(command(binary,"sharex","inspect",prefix_profile,env=absent_keyring),token,"Prefix-form inspect")[-1]
                assert "opaque-prefix-51" not in json.dumps(inspected,ensure_ascii=False)
            for body_mode, frozen_data in [("JSON", '{"password":"opaque%y","caption":"public"}'), ("XML", "<r password='opaque%y'><caption>public</caption></r>")]:
                frozen_profile = write("body-frozen.sxcu", profile(body=body_mode, destination="TextUploader", path="/values", Data=frozen_data, URL="{inputbox:Caption|opaque2020}"))
                frozen_options = write("body-frozen-options.json", {"version": 1, "name_values": {"now": "2020-01-02T03:04:05"}, "credential_profile": str(missing_native)})
                count = len(records)
                inspected = checked(command(binary, "sharex", "inspect", frozen_profile, "--options", frozen_options, env=absent_keyring), token, "Frozen body auth inspect")[-1]
                assert len(records) == count and "opaque2020" not in json.dumps(inspected, ensure_ascii=False)
                assert next(value for value in inspected["answers"] if value["id"] == "URL#0")["default_text"] == "[redacted]"
            unused_data = write("unused-form-data.sxcu", profile(body="FormURLEncoded", destination="TextUploader", path="/values", Data="password=unused-private", Arguments={"caption": "public"}))
            ignored = checked(command(binary, "sharex", "text", unused_data, text, env=env), token, "Unused upstream Data remains ignored")[-1]
            assert ignored["success"] and urllib.parse.parse_qs(records[-1]["body"].decode()) == {"caption": ["public"]}
            innocent_data = write("unrelated-data.sxcu", profile(body="JSON", destination="TextUploader", path="/values", Data='{"caption":"password=public"}'))
            assert checked(command(binary, "sharex", "text", innocent_data, text, env=env), token, "Unrelated Data remains data")[-1]["success"]
            dtd_profile = write("body-dtd.sxcu", profile(body="XML", destination="TextUploader", path="/values", Data="<!DOCTYPE r [<!ENTITY x SYSTEM 'file:///must-not-read'>]><r>&x;</r>"))
            count = len(records)
            denied = command(no_runtime / "screenshots", "sharex", "text", dtd_profile, text, env=absent_keyring)
            assert denied.returncode != 0 and not denied.stdout and b"DTD" in denied.stderr and len(records) == count
            # HTTP failure retains sanitized details/events and stops either Task upload branch.
            for generic in (False, True):
                failure_value = profile(path="/error", ErrorMessage="{outputbox:Failure|{json:message}}{json:message}")
                if generic:
                    failure_value["Headers"] = {"Authorization": "Bearer " + token}
                failure_profile = write("task-failure.sxcu", failure_value)
                forbidden = root / ("generic-later-action" if generic else "explicit-later-action")
                upload_action = {"type": "upload", "profile": str(failure_profile)} if generic else {"type": "sharex_upload", "kind": "file", "profile": str(failure_profile), "options": str(options)}
                failure_task = write("task-failure.json", {"version": 1, "source": {"type": "raw_file", "path": str(source)}, "actions": [upload_action,
                    {"type": "command", "program": "/bin/sh", "args": ["-c", 'printf ran > "$1"', "fixture", str(forbidden)], "timeout_ms": 1000}]})
                failed = command(binary, "task", "run", failure_task, env=env)
                assert failed.returncode != 0 and token.encode() not in failed.stdout + failed.stderr and not forbidden.exists()
                failures = [json.loads(line) for line in failed.stdout.splitlines()]
                assert len(failures) == 1 and failures[0]["type"] == "action_failed" and failures[0]["index"] == 1
                assert failures[0]["result"]["error_message"] == "denied [redacted]"
                assert failures[0]["result"]["events"] == [{"title": "Failure", "text": "denied [redacted]"}]
            # Original filenames/configs/source bytes are not rewritten or leaked by default.
            assert source.read_bytes() == original
            assert all(path.read_bytes() == original for path, original in snapshots.items())
            assert not (root / "must-not-create-tls-keylog").exists()
            if not args.skip_task:
                def chunk(kind, payload):
                    return struct.pack(">I", len(payload)) + kind + payload + struct.pack(">I", zlib.crc32(kind + payload) & 0xFFFFFFFF)
                pixels = bytes(channel for y in range(3) for x in range(4) for channel in (x * 50, y * 70, 13, 255))
                rows = b"".join(b"\0" + pixels[y * 16:(y + 1) * 16] for y in range(3))
                png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 4, 3, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b"")
                image = root / "unredacted-source.png"
                image.write_bytes(png)
                recipe = write("recipe.json", {"version": 1, "steps": [{"op": "redact", "x": 1, "y": 1, "width": 2, "height": 1}]})
                sxcu = write("task.sxcu", profile(body="MultipartFormData", destination="ImageUploader"))
                for save in (False, True):
                    qr = root / ("saved-qr.png" if save else "memory-qr.png")
                    actions = [{"type": "save", "path": str(root / "final.png")}] if save else []
                    actions += [{"type": "sharex_upload", "kind": "image", "profile": str(sxcu), "options": str(options)}, {"type": "link", "format": "markdown", "label": "Redacted"}, {"type": "qr", "path": str(qr)}]
                    task = write("task.json", {"version": 1, "source": {"type": "file", "path": str(image)}, "recipe": str(recipe), "actions": actions})
                    events = checked(command(binary, "task", "run", task, env=env), token, "Finalized SXCU Task")
                    assert events[-1]["type"] == "complete"
                    uploaded = records[-1]["multipart"]["file"]["data"]
                    received = root / "received.png"
                    received.write_bytes(uploaded)
                    decoded = command(Path("ffmpeg"), "-hide_banner", "-loglevel", "error", "-nostdin", "-i", received, "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "-", env=env)
                    assert decoded.returncode == 0 and not decoded.stderr
                    expected = bytearray(pixels)
                    for x in (1, 2):
                        expected[(4 + x) * 4:(4 + x + 1) * 4] = bytes((0, 0, 0, 255))
                    assert decoded.stdout == expected and uploaded != png and image.read_bytes() == png
                    scanned = checked(command(binary, "qr", "scan", qr, env=env), token, "Task QR decode")[-1]
                    upload_result = next(event["result"]["upload"] for event in events if event.get("result", {}).get("action") == "sharex_upload")
                    assert [code["text"] for code in scanned["codes"]] == [upload_result["url"]]
                    if not save:
                        assert not (root / "final.png").exists()
                    else:
                        assert uploaded == (root / "final.png").read_bytes()
            lifecycle = {} if args.skip_install else install_lifecycle(root, binary, env, root / "binary-POST.sxcu", options, source, token)
        finally:
            errors = []
            try:
                for profile in cleanup:
                    try:
                        result = command(binary, "credentials", "remove", profile, env=env)
                        if result.returncode:
                            errors.append("synthetic credential cleanup")
                    except Exception:
                        errors.append("synthetic credential cleanup")
            finally:
                for server in servers:
                    try:
                        if any(owned is server and thread.is_alive() for owned, thread in threads):
                            server.shutdown()
                        server.server_close()
                    except Exception:
                        errors.append("private server cleanup")
                for _, thread in threads:
                    thread.join(6)
                    if thread.is_alive():
                        errors.append("private server thread cleanup")
            if errors:
                raise RuntimeError("SXCU fixture cleanup failed: " + ", ".join(errors))
        report = {"passed": True, "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(), "https_requests": len(records),
                  "methods": ["GET", "POST", "PUT", "PATCH", "DELETE"], "text_body_modes": 6,
                  "native_auth_origin_binding_and_redaction": True, "templates_selectors_prompts_events": True,
                  "no_tls_keylog": True, "auth_alias_cookie_and_response_answer_regressions": True, "delivered_query_names_and_known_encoding_projections": True, "raw_known_links_and_inspect_metadata_redaction": True, "configured_body_auth_and_frozen_data_consistency": True, "task_failure_events_and_later_action_suppression": True, "finalized_memory_and_saved_task_pixels": not args.skip_task,
                  "real_account_acceptance": False, "installed_runtime_lifecycle": lifecycle, "frozen_debug_stripped_copy_text_section_unchanged": True}
        if args.report:
            args.report.parent.mkdir(parents=True, exist_ok=True)
            args.report.write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report))


if __name__ == "__main__":
    main()
