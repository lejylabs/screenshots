"""Explicit Unix E2E fixture: synthetic pixels -> isolated CLI helper -> private IPP.

Uses an ephemeral 127.0.0.1 listener and no CUPS client, daemon, queues or printer device.
A missing queue must fail after one exact query, without discovery or provisioning.
Run only after activation/build: python3 virtual_ipp.py /absolute/screenshots
"""
import argparse
import binascii
import http.server
import json
import os
from pathlib import Path
import selectors
import socket
import struct
import subprocess
import tempfile
import threading
import time
import zlib

LIMIT = 1024 * 1024


def attr(tag, name, value):
    name = name.encode("ascii")
    if isinstance(value, str):
        value = value.encode("utf-8")
    return bytes([tag]) + struct.pack(">H", len(name)) + name + struct.pack(">H", len(value)) + value


def integer(value):
    return struct.pack(">i", value)


def parse_ipp(data):
    assert 8 <= len(data) <= LIMIT
    operation = struct.unpack(">H", data[2:4])[0]
    position = 8
    names = {}
    previous = ""
    while position < len(data):
        tag = data[position]
        position += 1
        if tag == 3:
            return operation, names, data[position:]
        if tag < 0x10:
            continue
        length = struct.unpack(">H", data[position:position + 2])[0]
        position += 2
        name = data[position:position + length].decode("ascii") if length else previous
        position += length
        length = struct.unpack(">H", data[position:position + 2])[0]
        position += 2
        assert position + length <= len(data)
        names.setdefault(name, []).append(data[position:position + length])
        position += length
        previous = name
    raise AssertionError("IPP request lacks end-of-attributes")


class PrivateIPP(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_):
        pass

    def body(self):
        data = bytearray()
        if self.headers.get("Transfer-Encoding", "").lower() == "chunked":
            while True:
                line = self.rfile.readline(100)
                assert line.endswith(b"\r\n")
                length = int(line.split(b";", 1)[0].strip(), 16)
                assert 0 <= length <= LIMIT - len(data)
                if not length:
                    assert self.rfile.readline(100) == b"\r\n"
                    return bytes(data)
                part = self.rfile.read(length)
                assert len(part) == length
                data.extend(part)
                assert self.rfile.read(2) == b"\r\n"
        length = int(self.headers.get("Content-Length", "0"))
        assert 0 < length <= LIMIT
        data = self.rfile.read(length)
        assert len(data) == length
        return data

    def do_POST(self):
        self.connection.settimeout(3)
        assert self.client_address[0] == "127.0.0.1"
        assert self.path == "/printers/private-fixture"
        data = self.body()
        operation, names, document = parse_ipp(data)
        self.server.requests.append((operation, names, document))
        if len(self.server.requests) > 16:
            raise AssertionError("Unexpected IPP request loop")
        status = b"\x04\x06" if self.server.missing else b"\0\0"
        result = data[:2] + status + data[4:8] + b"\x01"
        result += attr(0x47, "attributes-charset", "utf-8")
        result += attr(0x48, "attributes-natural-language", "en")
        base = f"ipp://127.0.0.1:{self.server.server_port}"
        if self.server.missing:
            assert operation == 0x000B and not document
        elif operation == 0x000B:
            assert not document
            result += b"\x04"
            for tag, name, value in [
                (0x45, "printer-uri-supported", base + "/printers/private-fixture"),
                (0x42, "printer-name", "private-fixture"),
                (0x41, "printer-info", "Owned synthetic printer fixture"),
                (0x23, "printer-state", integer(3)),
                (0x22, "printer-is-accepting-jobs", b"\x01"),
                (0x22, "printer-is-temporary", b"\x00"),
                (0x21, "printer-type", integer(0)),
                (0x22, "printer-is-shared", b"\x00"),
                (0x49, "document-format-supported", "application/postscript"),
                (0x49, "document-format-default", "application/postscript"),
                (0x44, "uri-security-supported", "none"),
                (0x44, "uri-authentication-supported", "none"),
                (0x44, "ipp-versions-supported", "1.1"),
                (0x44, "compression-supported", "none"),
                (0x44, "media-supported", "iso_a4_210x297mm"),
                (0x44, "media-default", "iso_a4_210x297mm"),
                (0x21, "copies-default", integer(1)),
                (0x33, "copies-supported", integer(1) + integer(1)),
                (0x44, "sides-supported", "one-sided"),
            ]:
                result += attr(tag, name, value)
            for index, value in enumerate([2, 11]):
                result += attr(0x23, "operations-supported" if index == 0 else "", integer(value))
        elif operation == 2:
            assert document.startswith(b"%!PS-Adobe-3.0")
            assert names.get("ipp-attribute-fidelity") == [b"\x01"]
            assert names.get("requesting-user-name") == [b"screenshots"]
            assert names.get("copies") == [integer(1)]
            assert names.get("number-up") == [integer(1)]
            assert names.get("orientation-requested") == [integer(3)]
            assert names.get("media") == [b"iso_a4_210x297mm"]
            assert names.get("sides") == [b"one-sided"]
            assert names.get("print-scaling") == [b"none"]
            assert names.get("job-sheets") == [b"none"]
            assert names.get("page-ranges") == [integer(1) + integer(1)]
            result += b"\x02"
            result += attr(0x21, "job-id", integer(37))
            result += attr(0x45, "job-uri", base + "/jobs/37")
            result += attr(0x23, "job-state", integer(3))
        else:
            raise AssertionError(f"Unexpected IPP operation: {operation}")
        result += b"\x03"
        self.send_response(200)
        self.send_header("Content-Type", "application/ipp")
        self.send_header("Content-Length", str(len(result)))
        self.end_headers()
        self.wfile.write(result)


def png():
    def chunk(name, data):
        return struct.pack(">I", len(data)) + name + data + struct.pack(">I", binascii.crc32(name + data) & 0xFFFFFFFF)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 2, 1, 8, 6, 0, 0, 0))
            + chunk(b"tEXt", b"Comment\x00PRIVATE_FIXTURE_METADATA_MUST_NOT_PRINT")
            + chunk(b"IDAT", zlib.compress(b"\x00\x01\x02\x03\xff\x00\x00\x00\x00")) + chunk(b"IEND", b""))


def bounded_run(command, close_stdout=False):
    process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                               stderr=subprocess.DEVNULL if close_stdout else subprocess.STDOUT, start_new_session=True)
    output = bytearray()
    try:
        if close_stdout:
            process.stdout.close()
            return process.wait(timeout=15), b""
        with selectors.DefaultSelector() as selector:
            selector.register(process.stdout, selectors.EVENT_READ)
            until = time.monotonic() + 15
            while selector.get_map():
                assert time.monotonic() < until, "Private CLI fixture timed out"
                for key, _ in selector.select(0.1):
                    part = os.read(key.fileobj.fileno(), 4096)
                    if not part:
                        selector.unregister(key.fileobj)
                    else:
                        output.extend(part)
                        assert len(output) <= 8192, "Private CLI fixture diagnostics exceeded 8 KiB"
        return process.wait(timeout=2), bytes(output)
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=2)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    options = parser.parse_args()
    assert os.name == "posix" and options.binary.is_absolute() and options.binary.is_file()
    with tempfile.TemporaryDirectory(prefix="screenshots-private-ipp-") as directory:
        source = Path(directory) / "synthetic.png"
        original = png()
        source.write_bytes(original)
        server = http.server.HTTPServer(("127.0.0.1", 0), PrivateIPP)
        server.requests = []
        server.missing = False
        thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True)
        thread.start()
        try:
            invalid_code, _ = bounded_run([str(options.binary), "print-image", str(source), "--paper", "a4"])
            assert invalid_code != 0 and not server.requests
            command = [str(options.binary), "print-image", str(source),
                "--printer", "private-fixture", "--paper", "a4", "--server", f"127.0.0.1:{server.server_port}"]
            server.missing = True
            before = len(server.requests)
            missing_code, missing_output = bounded_run(command)
            assert missing_code != 0 and b"before submission" in missing_output
            assert len(server.requests) == before + 1
            assert server.requests[-1][0] == 0x000B and not server.requests[-1][2]
            server.missing = False
            code, output = bounded_run(command)
            assert code == 0, output.decode("utf-8", errors="replace")
            result = json.loads(output)
            assert result == {"status": "accepted_by_spooler", "physical_delivery": "unverified", "copies": 1, "pages": 1}
            documents = [document for operation, _, document in server.requests if operation == 2 and document]
            assert len(documents) == 1
            document = documents[0]
            assert document.startswith(b"%!PS-Adobe-3.0") and document.endswith(b"%%EOF\n")
            assert b"PRIVATE_FIXTURE_METADATA" not in document
            assert b"010203ffffff" in document
            assert document.count(b"showpage") == 1
            assert source.read_bytes() == original
            job_options = [names for operation, names, _ in server.requests if operation == 2]
            assert len(job_options) == 1
            assert job_options[0].get("copies") == [integer(1)]
            assert job_options[0].get("job-name") == [b"Screenshot"]
            # A second explicitly requested job reaches only this virtual spooler.
            # A public stdout failure must be nonzero and must not duplicate it.
            failed_code, _ = bounded_run(command, close_stdout=True)
            assert failed_code != 0
            documents_after = [document for operation, _, document in server.requests if operation == 2 and document]
            assert len(documents_after) == 2 and documents_after[1] == document
            assert source.read_bytes() == original
            print("PASS: exact missing-queue refusal without discovery/provisioning, private IPP acceptance, metadata stripping, source preservation, stdout failure without retry; no physical printer or user queue")
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)
            assert not thread.is_alive()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
