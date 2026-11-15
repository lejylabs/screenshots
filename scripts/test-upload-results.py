#!/usr/bin/env python3
"""Verify inert link output through an independent CommonMark/HTML parser.

Requires Python, markdown-it-py, zbarimg and a built screenshots executable. Only
synthetic URLs and generated QR pixels are used; no network or desktop access.
"""

import argparse
import hashlib
from html.parser import HTMLParser
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from markdown_it import MarkdownIt


class Anchor(HTMLParser):
    def __init__(self):
        super().__init__()
        self.hrefs = []
        self.text = []

    def handle_starttag(self, tag, attrs):
        if tag == "a":
            self.hrefs.append(dict(attrs).get("href"))

    def handle_data(self, data):
        self.text.append(data)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/screenshots"))
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve()
    zbar = shutil.which("zbarimg")
    if not zbar:
        raise SystemExit("zbarimg is required for the independent maximum-length QR check")
    with tempfile.TemporaryDirectory(prefix="screenshots-links-") as temporary:
        root = Path(temporary)
        env = dict(os.environ)
        for key in ("HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"):
            env[key] = str(root / key)
        for key in ("DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS"):
            env.pop(key, None)

        def run(arguments, value):
            data = value if isinstance(value, bytes) else json.dumps(value).encode()
            result = subprocess.run([str(binary), *map(str, arguments)], input=data,
                                    capture_output=True, env=env, timeout=10)
            assert result.returncode == 0, result.stderr.decode(errors="replace")
            assert not result.stderr
            return json.loads(result.stdout)

        label = "A [picture] & &#65; <tag> ' \" \\ café"
        urls = ["https://example.invalid/a%20b?token=a&amp;b#part",
                "https://example.invalid/?value=&#123;",
                "https://example.invalid/?value=&#x41;",
                "https://example.invalid/a(b)?x='quoted'&y=%2F"]
        for url in urls:
            canonical = url.replace("'", "%27")
            for mode in ("markdown", "html"):
                value = run(["link", "format", mode, "--label", label], {"url": url})
                rendered = MarkdownIt("commonmark").render(value["text"]) if mode == "markdown" else value["text"]
                anchor = Anchor()
                anchor.feed(rendered)
                assert anchor.hrefs == [canonical], (mode, canonical, anchor.hrefs)
                assert "".join(anchor.text).strip() == label

        url = "https://example.invalid/inert-upload-result"
        destination = root / "link.png"
        value = run(["link", "qr", destination], {"url": url})
        assert value["format"] == "png" and value["error_correction"] == "M"
        scanned = run(["qr", "scan", destination], b"")
        assert [code["text"] for code in scanned["codes"]] == [url]
        maximum_url = "https://example.invalid/"
        maximum_url += "x" * (2048 - len(maximum_url))
        maximum = root / "maximum.png"
        run(["link", "qr", maximum], {"url": maximum_url})
        decoded = subprocess.run([zbar, "--quiet", "--raw", "--nodbus", str(maximum)],
                                 capture_output=True, env=env, timeout=10)
        assert decoded.returncode == 0 and not decoded.stderr
        assert decoded.stdout == maximum_url.encode() + b"\n"
        assert {p.name for p in root.iterdir()} == {"link.png", "maximum.png"}
    report = {"passed": True, "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "commonmark_html_href_cases": len(urls) * 2, "label_exact": True,
              "qr_roundtrip": True, "maximum_2048_byte_qr_zbar_exact": True,
              "isolated_user_state_unchanged": True}
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report))


if __name__ == "__main__":
    main()
