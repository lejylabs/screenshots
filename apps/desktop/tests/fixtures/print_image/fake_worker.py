"""Owned process-boundary fixture. No printer API or network operation."""
import os
import signal
import socket
import sys
import uuid

version, descriptor, parent, nonce = os.environ["SCREENSHOTS_NATIVE_CAPTURE_PIPE"].split(":")
assert version == "v1" and int(parent) == os.getppid()
assert os.getpid() == os.getpgrp()
channel = socket.socket(fileno=int(descriptor))
channel.settimeout(5)
received = b""
while len(received) < 24:
    part = channel.recv(24 - len(received))
    assert part
    received += part
assert received == b"PRINTv1\n" + uuid.UUID(nonce).bytes
mode = sys.argv[1]
if mode == "accept":
    channel.sendall(b"ACCEPT1\n")
    sys.exit(0)
if mode == "reject":
    sys.exit(2)
if mode == "unknown":
    sys.exit(3)
if mode == "missing-ack":
    sys.exit(0)
if mode == "wrong-ack":
    channel.sendall(b"WRONG!!\n")
    sys.exit(0)
if mode == "extra-descendant":
    if os.fork() == 0:
        signal.pause()
    # A rejected outcome plus a retained descendant must be reaped as one family.
    sys.exit(3)
assert mode == "hang"
signal.pause()
