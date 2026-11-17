#!/usr/bin/env python3
"""Actual owned Xdnd source -> production native worker -> existing GPUI/editor.

Requires an optimized desktop binary and desktop test binary built with
native-drop-probe. Creates an authenticated Xvfb and isolated XDG directories;
never connects to the user's display, clipboard, input devices or account.
"""
import argparse
import ctypes as c
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import queue
import secrets
import select
import shutil
import signal
import socket
import struct
import subprocess
import tempfile
import threading
import time
import urllib.parse
import zlib


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def image(pixels):
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data) & 0xffffffff)
    return (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', 2, 1, 8, 6, 0, 0, 0))
            + chunk(b'sRGB', b'\0') + chunk(b'IDAT', zlib.compress(b'\0' + pixels)) + chunk(b'IEND', b''))


def pixel_hash(pixels):
    value = 0xcbf29ce484222325
    for byte in pixels:
        value = ((value ^ byte) * 0x100000001b3) & ((1 << 64) - 1)
    return f'{value:016x}'


def load_owner(repository):
    spec = importlib.util.spec_from_file_location('owned_clipboard_fixture', repository / 'scripts/test-clipboard-input.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    class Message(c.Structure):
        _fields_ = [('kind', c.c_int), ('serial', c.c_ulong), ('send', c.c_int), ('display', c.c_void_p),
                    ('window', c.c_ulong), ('message_type', c.c_ulong), ('format', c.c_int), ('data', c.c_long * 5)]
    class Event(c.Union):
        _fields_ = module.Event._fields_ + [('message', Message)]
    module.Event = Event
    return module, Message, Event


class Source:
    def __init__(self, module, Message, Event, display, authority):
        self.owner = owner = module.Owner(display, authority)
        self.Event, self.Message = Event, Message
        self.x, self.connection = owner.x, owner.connection
        self.messages, self.requests, self.clocks = [], [], []
        self.guard = threading.Lock()
        self.mutate_owner = None
        self.receiver = None
        self.active_target, self.pending_position, self.position_count = None, None, 0
        self.status_history, self.status_failures = [], []
        self.original_clipboard_atom = owner.atoms['CLIPBOARD']
        for name in ['XdndSelection', 'XdndTypeList', 'XdndEnter', 'XdndPosition', 'XdndDrop',
                     'XdndLeave', 'XdndStatus', 'XdndFinished', 'XdndActionCopy', '_FIXTURE_CLOCK']:
            owner.atoms[name] = self.x.XInternAtom(self.connection, name.encode(), 0)
        owner.atoms['CLIPBOARD'] = owner.atoms['XdndSelection']
        self.x.XSelectInput(self.connection, owner.window, 1 << 22)
        self.x.XSetSelectionOwner(self.connection, owner.atoms['XdndSelection'], owner.window, 0)
        next_event = self.x.XNextEvent
        def observed(connection, pointer):
            result = next_event(connection, pointer)
            event = c.cast(pointer, c.POINTER(Event)).contents
            with self.guard:
                if event.kind == 33:
                    data = tuple(int(v) & 0xffffffff for v in event.message.data)
                    self.messages.append((event.message.message_type, data))
                    if event.message.message_type == owner.atoms['XdndStatus'] and event.message.window == owner.window:
                        if data[0] == self.active_target:
                            if self.pending_position is None:
                                if not self.status_failures:
                                    self.status_failures.append('Actual receiver sent an unsolicited/duplicate Status')
                            elif self.pending_position[1] != data[0]:
                                self.status_failures.append('Actual receiver replied to a different Position owner')
                            else:
                                self.status_history.append((self.pending_position, data))
                                self.pending_position = None
                        # XDND requires sources to ignore replies from an old target after Leave.

                elif event.kind == 30:
                    request = event.request
                    require(request.selection == owner.atoms['XdndSelection'], 'Production receiver requested another selection')
                    self.requests.append((request.requestor, request.selection, request.target, request.property, request.time))
                elif event.kind == 28 and event.property.atom == owner.atoms['_FIXTURE_CLOCK']:
                    self.clocks.append(event.property.time)
            if self.receiver is not None:
                self.receiver.event(event)
            return result
        self.x.XNextEvent = observed
        original_property = owner.property
        def property_with_mutation(window, atom, kind, data, bits=8):
            if kind in (owner.atoms['image/png'], owner.atoms['text/uri-list']) and self.mutate_owner:
                mode, self.mutate_owner = self.mutate_owner, None
                if mode == 'aba':
                    self.x.XSetSelectionOwner(self.connection, owner.atoms['XdndSelection'], owner.alternate_window, 0)
                self.x.XSetSelectionOwner(self.connection, owner.atoms['XdndSelection'], owner.window, 0)
                self.x.XFlush(self.connection)
            original_property(window, atom, kind, data, bits)
        owner.property = property_with_mutation
        self.x.XMapWindow.argtypes = [c.c_void_p, c.c_ulong]
        self.x.XUnmapWindow.argtypes = [c.c_void_p, c.c_ulong]
        self.x.XMoveWindow.argtypes = [c.c_void_p, c.c_ulong, c.c_int, c.c_int]
        self.x.XTranslateCoordinates.argtypes = [c.c_void_p, c.c_ulong, c.c_ulong, c.c_int, c.c_int,
                                                c.POINTER(c.c_int), c.POINTER(c.c_int), c.POINTER(c.c_ulong)]
        self.x.XTranslateCoordinates.restype = c.c_int
        self.clock()

    def clock(self):
        with self.guard:
            before = len(self.clocks)
        self.owner.property(self.owner.window, self.owner.atoms['_FIXTURE_CLOCK'], 31, b'x')
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            with self.guard:
                if len(self.clocks) > before:
                    require(self.clocks[-1] != 0, 'Private X server returned CurrentTime')
                    return self.clocks[-1]
            time.sleep(.002)
        raise RuntimeError('Private native clock event was not delivered')

    def audit_status(self):
        with self.guard: require(not self.status_failures, '; '.join(self.status_failures))

    def send(self, target, kind, data):
        if kind == 'XdndPosition' and data[0] == self.owner.window:
            with self.guard:
                require(self.pending_position is None, 'Fixture source sent Position before native Status')
                require(target == self.active_target, 'Fixture source changed its Position target without Enter')
                require(self.position_count < 32, 'Fixture Position ledger exceeds its finite case bound')
                self.position_count += 1
                self.pending_position = (self.position_count, target, data[2], data[3], data[4])
        event = self.Event()
        event.message = self.Message(33, 0, 1, self.connection, target, self.owner.atoms[kind], 32,
                                     (c.c_long * 5)(*data))
        require(self.x.XSendEvent(self.connection, target, 0, 0, c.byref(event)), 'Cannot send owned Xdnd message')
        self.x.XFlush(self.connection)

    def offer(self, target, mime, body, mode='direct', extended=False):
        self.owner.set({mime: body}, mode=mode, chunk=3)
        self.x.XSetSelectionOwner(self.connection, self.owner.atoms['XdndSelection'], self.owner.window, 0)
        self.audit_status()
        with self.guard:
            self.messages.clear()
            self.requests.clear()
            self.active_target, self.pending_position, self.position_count = target, None, 0
            self.status_history.clear()
        if extended:
            self.owner.property(self.owner.window, self.owner.atoms['XdndTypeList'], 4,
                                [self.owner.atoms[mime]], 32)
        self.send(target, 'XdndEnter', [self.owner.window, (5 << 24) | int(extended), self.owner.atoms[mime], 0, 0])
        x, y, child = c.c_int(), c.c_int(), c.c_ulong()
        root = self.x.XDefaultRootWindow(self.connection)
        require(self.x.XTranslateCoordinates(self.connection, target, root, 450, 300,
                                             c.byref(x), c.byref(y), c.byref(child)), 'Owned target has no root position')
        packed = ((x.value & 0xffff) << 16) | (y.value & 0xffff)
        stamp = self.clock()
        self.send(target, 'XdndPosition', [self.owner.window, 0, packed, stamp, self.owner.atoms['XdndActionCopy']])
        return stamp

    def wait_status(self, target, observe, timeout=5, accepted=True):
        with self.guard: number = self.position_count
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            observe(); self.audit_status()
            with self.guard:
                matches = [data for position, data in self.status_history
                           if position[0] == number and position[1] == target]
                if matches:
                    require(len(matches) == 1, 'A Position received more than one actual native Status')
                    data = matches[0]
                    actual = bool(data[1] & 1)
                    require(actual == accepted, 'Actual first Status decision differs; refusal was not skipped')
                    require(data[4] == (self.owner.atoms['XdndActionCopy'] if accepted else 0),
                            'Actual Status action differs from its decision')
                    return data
            time.sleep(.005)
        raise RuntimeError('Actual native receiver did not acknowledge the outstanding Position')

    def position(self, target, x, y, action=None):
        root = self.x.XDefaultRootWindow(self.connection)
        rx, ry, child = c.c_int(), c.c_int(), c.c_ulong()
        require(self.x.XTranslateCoordinates(self.connection, target, root, x, y,
                                             c.byref(rx), c.byref(ry), c.byref(child)), 'Owned target has no root position')
        stamp = self.clock()
        if action is None: action = self.owner.atoms['XdndActionCopy']
        self.send(target, 'XdndPosition', [self.owner.window, 0,
                  ((rx.value & 0xffff) << 16) | (ry.value & 0xffff), stamp, action])
        return stamp

    def assert_status_count(self, count, observe, duration=.15):
        deadline = time.monotonic() + duration
        while time.monotonic() < deadline:
            observe(); self.audit_status()
            with self.guard:
                require(len(self.status_history) == count, 'Actual receiver emitted Status without a new Position')
            time.sleep(.005)

    def leave(self, target):
        self.send(target, 'XdndLeave', [self.owner.window, 0, 0, 0, 0])
        with self.guard:
            self.active_target, self.pending_position = None, None

    def drop(self, target):
        self.audit_status()
        with self.guard:
            require(self.pending_position is None and self.status_history,
                    'Fixture source dropped before the actual native Status')
            position, status = self.status_history[-1]
            require(position[0] == self.position_count and position[1] == target and status[1] & 1,
                    'Fixture source dropped an unacknowledged/refused Position')
        stamp = self.clock()
        self.send(target, 'XdndDrop', [self.owner.window, 0, stamp, 0, 0])
        return stamp

    def wait_request(self, mime, observe, timeout=3):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            observe(); self.audit_status()
            with self.guard:
                requests = [request for request in self.requests if request[2] == self.owner.atoms[mime]]
                if requests:
                    return requests[-1]
            time.sleep(.005)
        raise RuntimeError('Production worker did not request the offered native representation')

    def late_reply(self, request, body):
        requestor, selection, target, prop, stamp = request
        prop = prop or target
        self.owner.property(requestor, prop, target, body)
        reply = self.Event()
        # Only a fixture-owned delayed or obsolete helper requestor, never another app's window.
        from_module = self.selection_type
        reply.selection = from_module(31, 0, 1, self.connection, requestor, selection, target, prop, stamp)
        self.x.XSendEvent(self.connection, requestor, 0, 0, c.byref(reply))
        self.x.XFlush(self.connection)

    def close(self):
        self.owner.close()


class Probe:
    def __init__(self, child, binary):
        self.child, self.binary = child, binary
        self.lines, self.helpers = queue.Queue(), {}
        self.all_stdout = []
        self.url_routes = 0
        def read():
            for raw in child.stdout:
                line = raw.decode('utf-8', errors='replace').strip()
                self.all_stdout.append(line)
                if 'DROP_PROBE ' in line:
                    self.lines.put(line.split('DROP_PROBE ', 1)[1].split())
        self.reader = threading.Thread(target=read, daemon=True)
        self.reader.start()
        self.target = int(self.wait('target')[1])

    def observe(self):
        try:
            children = Path(f'/proc/{self.child.pid}/task/{self.child.pid}/children').read_text().split()
        except FileNotFoundError:
            raise RuntimeError('Owned GPUI fixture exited')
        for value in children:
            pid = int(value)
            if pid in self.helpers:
                continue
            try:
                command = Path(f'/proc/{pid}/cmdline').read_bytes().split(b'\0')
                if len(command) < 2 or command[1] != b'__native-capture-worker':
                    continue
                require(Path(f'/proc/{pid}/exe').resolve() == self.binary, 'Unexpected native helper executable')
                descriptor = os.pidfd_open(pid)
                # Recheck executable/lineage after acquiring the identity-stable handle.
                require(Path(f'/proc/{pid}/exe').resolve() == self.binary
                        and value in Path(f'/proc/{self.child.pid}/task/{self.child.pid}/children').read_text().split(),
                        'Native helper lineage changed during observation')
                self.helpers[pid] = descriptor
            except (FileNotFoundError, ProcessLookupError):
                continue

    def wait(self, kind, timeout=8):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.observe()
            try:
                value = self.lines.get(timeout=.02)
                if value[0] == 'url-route':
                    self.url_routes += 1
                    continue
                require(value[0] == kind, f'Expected owned probe {kind}, got {value[0]}')
                return value
            except queue.Empty:
                require(self.child.poll() is None, 'Owned GPUI fixture exited before reply')
        raise RuntimeError(f'Owned GPUI fixture did not reply: {kind}')

    def command(self, command, reply):
        self.child.stdin.write((command + '\n').encode())
        self.child.stdin.flush()
        return self.wait(reply)

    def state(self):
        value = self.command('state', 'state')
        return tuple(value[1:])

    def wait_state(self, width, height, expected=None, timeout=7):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            state = self.state()
            if state[1:3] == (str(width), str(height)) and (expected is None or state[3] == expected):
                return state
            time.sleep(.015)
        raise RuntimeError('Actual editor pixels/state did not reach the expected private oracle')

    def fresh(self):
        self.command('close', 'closed')
        self.target = int(self.command('new', 'target')[1])
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if self.state()[0] == '1':
                return
            time.sleep(.02)
        raise RuntimeError('Fresh existing control window did not become ready')

    def assert_no_editor(self, duration=.2):
        deadline = time.monotonic() + duration
        while time.monotonic() < deadline:
            require(self.state()[1:3] == ('0', '0'), 'Canceled/invalid native drop opened an editor')
            time.sleep(.015)

    def all_helpers_exited(self):
        poller = select.poll()
        for descriptor in self.helpers.values():
            poller.register(descriptor, select.POLLIN)
        ended = {descriptor for descriptor, _ in poller.poll(0)}
        deadline = time.monotonic() + 2
        while len(ended) < len(self.helpers) and time.monotonic() < deadline:
            ended.update(descriptor for descriptor, _ in poller.poll(20))
        require(len(ended) == len(self.helpers), 'An observed native acquisition helper remained live')
        return len(ended)


class Receiver:
    """Tiny owned native Xdnd target; XTEST addresses only this private Xvfb."""
    def __init__(self, source):
        self.source, self.x, self.connection = source, source.x, source.connection
        self.guard = threading.Lock()
        self.helper, self.body, self.phase, self.stamp = None, None, 0, None
        self.accept, self.auto_finish, self.enters = True, True, 0
        self.formats, self.failures = [], []
        self.positions, self.pending_status, self.drops = [], [], 0
        self.hold_status = False
        owner = source.owner
        self.atoms = dict(owner.atoms)
        for name in ['XdndAware', 'TARGETS', 'TIMESTAMP', '_OUTGOING_RECEIVER_DATA']:
            self.atoms[name] = self.x.XInternAtom(self.connection, name.encode(), 0)
        self.x.XConvertSelection.argtypes = [c.c_void_p, c.c_ulong, c.c_ulong, c.c_ulong, c.c_ulong, c.c_ulong]
        self.x.XGetWindowProperty.argtypes = [c.c_void_p, c.c_ulong, c.c_ulong, c.c_long, c.c_long, c.c_int, c.c_ulong,
                                             c.POINTER(c.c_ulong), c.POINTER(c.c_int), c.POINTER(c.c_ulong),
                                             c.POINTER(c.c_ulong), c.POINTER(c.POINTER(c.c_ubyte))]
        self.x.XGetWindowProperty.restype = c.c_int
        self.x.XFree.argtypes = [c.c_void_p]
        self.x.XMapRaised.argtypes = [c.c_void_p, c.c_ulong]
        self.x.XRaiseWindow.argtypes = [c.c_void_p, c.c_ulong]
        self.x.XSetInputFocus.argtypes = [c.c_void_p, c.c_ulong, c.c_int, c.c_ulong]
        self.x.XStringToKeysym.argtypes, self.x.XStringToKeysym.restype = [c.c_char_p], c.c_ulong
        self.x.XKeysymToKeycode.argtypes, self.x.XKeysymToKeycode.restype = [c.c_void_p, c.c_ulong], c.c_ubyte
        root = self.x.XDefaultRootWindow(self.connection)
        self.window = self.x.XCreateSimpleWindow(self.connection, root, 700, 100, 300, 220, 0, 0, 0)
        owner.property(self.window, self.atoms['XdndAware'], 4, [5], 32)
        self.x.XMapRaised(self.connection, self.window); self.x.XFlush(self.connection)
        self.xt = c.CDLL('libXtst.so.6')
        self.xt.XTestFakeMotionEvent.argtypes = [c.c_void_p, c.c_int, c.c_int, c.c_int, c.c_ulong]
        self.xt.XTestFakeMotionEvent.restype = c.c_int
        self.xt.XTestFakeButtonEvent.argtypes = [c.c_void_p, c.c_uint, c.c_int, c.c_ulong]
        self.xt.XTestFakeButtonEvent.restype = c.c_int
        self.xt.XTestFakeKeyEvent.argtypes = [c.c_void_p, c.c_uint, c.c_int, c.c_ulong]
        self.xt.XTestFakeKeyEvent.restype = c.c_int
        source.receiver = self

    def read_property(self, target):
        kind, bits, count, after = c.c_ulong(), c.c_int(), c.c_ulong(), c.c_ulong()
        pointer = c.POINTER(c.c_ubyte)()
        status = self.x.XGetWindowProperty(self.connection, self.window, self.atoms['_OUTGOING_RECEIVER_DATA'],
                                          0, 16385, 1, 0, c.byref(kind), c.byref(bits), c.byref(count),
                                          c.byref(after), c.byref(pointer))
        try:
            require(status == 0 and after.value == 0 and count.value <= 65536, 'Outgoing metadata exceeded its native extent')
            if target == self.atoms['text/uri-list']:
                require(bits.value == 8 and kind.value == target, 'Outgoing URI response has wrong type')
                return c.string_at(pointer, count.value)
            require(bits.value == 32 and count.value <= 3, 'Outgoing finite scalar response has wrong extent')
            return tuple(c.cast(pointer, c.POINTER(c.c_ulong))[i] & 0xffffffff for i in range(count.value))
        finally:
            if pointer: self.x.XFree(pointer)

    def request(self, target):
        self.x.XConvertSelection(self.connection, self.atoms['XdndSelection'], target,
                                 self.atoms['_OUTGOING_RECEIVER_DATA'], self.window, self.stamp)
        self.x.XFlush(self.connection)

    def finish(self, correct=True):
        with self.guard: helper = self.helper
        require(helper, 'No current native outgoing source')
        self.source.send(helper, 'XdndFinished', [self.window if correct else self.window + 1,
                                                1, self.atoms['XdndActionCopy'], 0, 0])

    def acknowledge(self, packet, accepted):
        self.source.send(packet[0], 'XdndStatus',
                         [self.window, 3 if accepted else 2, 0, 0,
                          self.atoms['XdndActionCopy'] if accepted else 0])

    def acknowledge_held(self, accepted):
        with self.guard:
            require(len(self.pending_status) == 1, 'Expected exactly one outstanding native Position')
            packet = self.pending_status.pop()
        self.acknowledge(packet, accepted)
        return packet

    def event(self, event):
        try:
            if event.kind == 33 and event.message.window == self.window:
                kind = event.message.message_type
                data = [int(value) & 0xffffffff for value in event.message.data]
                if kind == self.atoms['XdndEnter']:
                    require(data[2] == self.atoms['text/uri-list'] and data[1] >> 24 >= 3,
                            'Native source did not offer the existing file-URI API')
                    with self.guard: self.helper, self.enters = data[0], self.enters + 1
                elif kind == self.atoms['XdndPosition']:
                    require(data[4] == self.atoms['XdndActionCopy'], 'Native source advertised a destructive action')
                    with self.guard:
                        require(len(self.positions) < 16, 'Native fixture Position observation bound exceeded')
                        self.positions.append(tuple(data))
                        if self.hold_status:
                            require(not self.pending_status,
                                    'Source sent a second Position before the first native Status')
                            self.pending_status.append(tuple(data))
                            acknowledge = False
                        else:
                            acknowledge = True
                    if acknowledge: self.acknowledge(data, self.accept)
                elif kind == self.atoms['XdndDrop']:
                    require(data[2] != 0, 'Outgoing native drop omitted its server timestamp')
                    with self.guard:
                        require(not self.pending_status, 'Source dropped before its outstanding Status was acknowledged')
                        self.drops += 1
                        require(self.positions and data[2] == self.positions[-1][3],
                                'Drop does not use the acknowledged final Position timestamp')
                    self.stamp, self.phase = data[2], 0
                    self.request(self.atoms['TARGETS'])
            elif event.kind == 31 and event.selection.requestor == self.window:
                require(event.selection.property != 0, 'Native outgoing source refused an advertised representation')
                target = event.selection.target
                value = self.read_property(target)
                if target == self.atoms['TARGETS']:
                    require(set(value) == {self.atoms['TARGETS'], self.atoms['TIMESTAMP'], self.atoms['text/uri-list']},
                            'Outgoing advertised unbounded/unimplemented native formats')
                    self.formats = list(value)
                    self.request(self.atoms['TIMESTAMP'])
                elif target == self.atoms['TIMESTAMP']:
                    require(len(value) == 1 and value[0] != 0, 'Native source ownership timestamp is absent')
                    self.request(self.atoms['text/uri-list'])
                elif target == self.atoms['text/uri-list']:
                    with self.guard: self.body = value
                    if self.auto_finish: self.finish()
        except BaseException as error:
            with self.guard: self.failures.append(str(error))

    def reset(self, accept=True, auto_finish=True, hold_status=False):
        with self.guard:
            self.helper, self.body, self.enters = None, None, 0
            self.accept, self.auto_finish, self.hold_status = accept, auto_finish, hold_status
            self.positions.clear(); self.pending_status.clear(); self.drops = 0
            self.failures.clear()

    def wait(self, predicate, probe, timeout=5):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            probe.observe()
            with self.guard:
                require(not self.failures, '; '.join(self.failures))
                if predicate(): return
            time.sleep(.005)
        raise RuntimeError('Owned native outgoing target oracle timed out')

    def owner(self):
        return self.x.XGetSelectionOwner(self.connection, self.atoms['XdndSelection'])

    def start(self, probe):
        origin = int(probe.command('outgoing', 'outgoing-source')[1])
        require(origin != self.window and origin != probe.target, 'Outgoing fixture reused a foreign window')
        self.x.XRaiseWindow(self.connection, origin)
        self.x.XSetInputFocus(self.connection, origin, 2, 0)
        self.x.XMapRaised(self.connection, self.window)
        self.x.XFlush(self.connection)
        time.sleep(.15)  # Finite fixture layout/focus settling, never a production timer.
        x, y, child = c.c_int(), c.c_int(), c.c_ulong()
        root = self.x.XDefaultRootWindow(self.connection)
        require(self.x.XTranslateCoordinates(self.connection, origin, root, 120, 100,
                                             c.byref(x), c.byref(y), c.byref(child)), 'Owned drag source has no root position')
        require(self.xt.XTestFakeMotionEvent(self.connection, -1, x.value, y.value, 0), 'Private XTEST motion failed')
        require(self.xt.XTestFakeButtonEvent(self.connection, 1, 1, 0), 'Private XTEST press failed')
        self.x.XFlush(self.connection); time.sleep(.025)
        self.xt.XTestFakeMotionEvent(self.connection, -1, x.value + 10, y.value + 10, 0)
        self.x.XFlush(self.connection); time.sleep(.025)
        self.xt.XTestFakeMotionEvent(self.connection, -1, 850, 200, 0)
        self.x.XFlush(self.connection)
        self.wait(lambda: self.enters > 0, probe)
        with self.guard: helper = self.helper
        require(self.owner() == helper and helper != origin, 'Native source does not own its one helper selection')
        return origin

    def release(self):
        require(self.xt.XTestFakeButtonEvent(self.connection, 1, 0, 0), 'Private XTEST release failed')
        self.x.XFlush(self.connection)

    def escape(self):
        code = self.x.XKeysymToKeycode(self.connection, self.x.XStringToKeysym(b'Escape'))
        require(code != 0, 'Private keymap has no Escape key')
        self.xt.XTestFakeKeyEvent(self.connection, code, 1, 0)
        self.xt.XTestFakeKeyEvent(self.connection, code, 0, 0)
        self.x.XFlush(self.connection)

    def released_selection(self, probe, timeout=5):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            probe.observe()
            with self.guard: require(not self.failures, '; '.join(self.failures))
            if self.owner() == 0: return
            time.sleep(.005)
        raise RuntimeError('Outgoing helper selection did not retire')

    def assert_waiting(self, probe, position_count, duration=.12):
        deadline = time.monotonic() + duration
        while time.monotonic() < deadline:
            probe.observe()
            with self.guard:
                require(not self.failures, '; '.join(self.failures))
                require(len(self.positions) == position_count and len(self.pending_status) == 1,
                        'Native Position/Status exchange was not serialized')
                require(self.drops == 0 and self.body is None, 'Hover/unsolicited acceptance caused a premature Drop')
                helper = self.helper
            require(self.owner() == helper, 'A delayed hover refusal canceled the final gesture')
            time.sleep(.005)

    def controlled_final_case(self, probe, expected, hover_accept, final_accept, move_after_release=False):
        self.reset(auto_finish=True, hold_status=True)
        self.start(probe)
        self.wait(lambda: len(self.positions) == 1 and len(self.pending_status) == 1, probe)
        # Coalesced movement remains one desired point while the hover ACK is withheld.
        self.xt.XTestFakeMotionEvent(self.connection, -1, 855, 205, 0)
        self.xt.XTestFakeMotionEvent(self.connection, -1, 860, 210, 0)
        self.release()
        if move_after_release:
            require(self.xt.XTestFakeMotionEvent(self.connection, -1, 1100, 700, 0),
                    'Private post-release XTEST motion failed')
            self.x.XFlush(self.connection)
        # The server processes release/movement before delivering the old hover ACK.
        self.source.clock()
        if move_after_release:
            self.x.XQueryPointer.argtypes = [c.c_void_p, c.c_ulong, c.POINTER(c.c_ulong), c.POINTER(c.c_ulong),
                                            c.POINTER(c.c_int), c.POINTER(c.c_int), c.POINTER(c.c_int),
                                            c.POINTER(c.c_int), c.POINTER(c.c_uint)]
            self.x.XQueryPointer.restype = c.c_int
            root = self.x.XDefaultRootWindow(self.connection)
            returned, child, rx, ry, wx, wy, mask = c.c_ulong(), c.c_ulong(), c.c_int(), c.c_int(), c.c_int(), c.c_int(), c.c_uint()
            require(self.x.XQueryPointer(self.connection, root, c.byref(returned), c.byref(child),
                                         c.byref(rx), c.byref(ry), c.byref(wx), c.byref(wy), c.byref(mask)),
                    'Private pointer proof failed')
            require((rx.value, ry.value) == (1100, 700) and not mask.value & (1 << 8),
                    'Native pointer did not move after releasing the button')
        self.assert_waiting(probe, 1)
        old = self.acknowledge_held(hover_accept)
        self.wait(lambda: len(self.positions) == 2 and len(self.pending_status) == 1, probe)
        self.assert_waiting(probe, 2)
        with self.guard:
            final = self.positions[-1]
            require(final[2] == (860 << 16) | 210,
                    'Final native Position follows the cursor instead of the saved release point')
            require(final != old, 'Final Position reused the old hover exchange')
        self.acknowledge_held(final_accept)
        if final_accept:
            self.wait(lambda: self.body is not None, probe)
            with self.guard:
                require(self.body == expected and self.drops == 1,
                        'Actual final acceptance did not deliver exactly one original URI offer')
        self.released_selection(probe)
        with self.guard:
            require(self.drops == int(final_accept), 'The hover decision overrode the final native Status')
            if not final_accept: require(self.body is None, 'Refused final native Position transferred a file')
        probe.command('outgoing-close', 'outgoing-closed')

    def run_cases(self, probe, expected, original_path, report):
        original = Path(os.fsdecode(original_path)).read_bytes()
        for mode in ['copy', 'stale-finish', 'escape', 'close', 'reject', 'missing-finish']:
            self.reset(accept=mode != 'reject', auto_finish=mode == 'copy')
            self.start(probe)
            if mode == 'escape':
                self.escape(); self.released_selection(probe); self.release()
            elif mode == 'close':
                probe.command('outgoing-close', 'outgoing-closed')
                self.released_selection(probe); self.release()
            else:
                self.release()
                if mode != 'reject':
                    self.wait(lambda: self.body is not None, probe)
                    with self.guard: require(self.body == expected, 'Outgoing native URI changed original path bytes')
                    if mode == 'stale-finish':
                        self.finish(correct=False); time.sleep(.1)
                        with self.guard: require(self.owner() == self.helper, 'Stale native finish retired the newer/source gesture')
                        self.finish()
                self.released_selection(probe, timeout=5.5)
            require(Path(os.fsdecode(original_path)).read_bytes() == original, 'Native drag-out changed or deleted its original file')
            if mode != 'close': probe.command('outgoing-close', 'outgoing-closed')
            report['checks'].append('outgoing-native-owned-' + mode + '-original-preserved')
        for name, hover, final, moved in [
            ('delayed-hover-accept-final-refusal', True, False, False),
            ('delayed-hover-refusal-final-accept', False, True, False),
            ('saved-release-point-after-native-cursor-move', True, True, True),
        ]:
            self.controlled_final_case(probe, expected, hover, final, moved)
            require(Path(os.fsdecode(original_path)).read_bytes() == original,
                    'Controlled native negotiation changed or deleted its original file')
            report['checks'].append('outgoing-native-owned-' + name)
        self.source.receiver = None


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--repository-root', type=Path, required=True)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--test-binary', type=Path, required=True)
    parser.add_argument('--xvfb', default=shutil.which('Xvfb'))
    parser.add_argument('--library-dir')
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    binary, test_binary = args.binary.resolve(strict=True), args.test_binary.resolve(strict=True)
    require(args.xvfb and shutil.which('xauth'), 'Authenticated private Xvfb tools are required')
    module, Message, Event = load_owner(args.repository_root.resolve(strict=True))
    report = {'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
              'test_binary_sha256': hashlib.sha256(test_binary.read_bytes()).hexdigest(), 'checks': []}
    source = child = server = probe = None
    with tempfile.TemporaryDirectory(prefix='screenshots-native-drop-') as temporary:
        root = Path(temporary)
        display = next((f':{value}' for value in range(260, 330)
                        if not Path(f'/tmp/.X{value}-lock').exists() and not Path(f'/tmp/.X11-unix/X{value}').exists()), None)
        require(display, 'No unused private X11 display')
        decoy = next((f':{value}' for value in range(65000, 65020)
                      if not Path(f'/tmp/.X{value}-lock').exists() and not Path(f'/tmp/.X11-unix/X{value}').exists()), None)
        require(decoy, 'No unavailable private decoy display')
        authority = root / 'authority'
        env = dict(os.environ, DISPLAY=display, XAUTHORITY=str(authority), XDG_SESSION_TYPE='x11',
                   SCREENSHOTS_NATIVE_DROP_PRIVATE='1', SCREENSHOTS_NATIVE_DROP_ROOT=str(root),
                   SCREENSHOTS_NATIVE_DROP_BINARY=str(binary), SCREENSHOTS_NATIVE_DROP_DECOY_DISPLAY=decoy,
                   LIBGL_ALWAYS_SOFTWARE='1')
        for name in ['WAYLAND_DISPLAY', 'WAYLAND_SOCKET', 'DBUS_SESSION_BUS_ADDRESS']:
            env.pop(name, None)
        for name in ['XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_CACHE_HOME']:
            env[name] = str(root / name)
        if args.library_dir:
            env['LD_LIBRARY_PATH'] = args.library_dir
        subprocess.run(['xauth', '-f', str(authority), 'add', display, '.', secrets.token_hex(16)],
                       check=True, capture_output=True, timeout=5)
        pixels_a, pixels_b = bytes([11, 23, 37, 255, 43, 59, 71, 128]), bytes([91, 83, 77, 255, 63, 59, 41, 255])
        png_a, png_b = image(pixels_a), image(pixels_b)
        (root / 'new-input.png').write_bytes(png_b)
        raw_path = os.fsencode(root) + b'/synthetic-\xff.png'
        with open(raw_path, 'wb') as output:
            output.write(png_a)
        uri_a = ('file://' + urllib.parse.quote_from_bytes(raw_path) + '\r\n').encode()
        uri_b = (root / 'new-input.png').as_uri().encode() + b'\r\n'
        try:
            server = subprocess.Popen([args.xvfb, display, '-screen', '0', '1280x800x24', '-auth', str(authority),
                                       '-nolisten', 'tcp', '-noreset'], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            for _ in range(100):
                require(server.poll() is None, 'Owned Xvfb exited')
                if subprocess.run(['xdpyinfo', '-display', display], env=env, capture_output=True, timeout=2).returncode == 0:
                    break
                time.sleep(.02)
            else:
                raise RuntimeError('Owned Xvfb did not become ready')
            source = Source(module, Message, Event, display, authority)
            source.selection_type = module.Selection
            stderr_path = root / 'probe.stderr'
            with stderr_path.open('wb') as stderr:
                child = subprocess.Popen([str(test_binary), 'capture_worker::native_drop_probe_tests::native_x11_drop_end_to_end',
                                          '--exact', '--ignored', '--nocapture', '--test-threads=1'],
                                         env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=stderr, start_new_session=True)
                probe = Probe(child, binary)
                probe.fresh()
                def positive(name, mime, body, expected, mode='direct', extended=False):
                    target = probe.target
                    stamp = source.offer(target, mime, body, mode, extended)
                    source.wait_status(target, probe.observe)
                    if mime == 'image/png':
                        with source.guard:
                            require(not source.requests, 'Raster bytes were requested on hover')
                    dropped = source.drop(target)
                    request = source.wait_request(mime, probe.observe)
                    require(request[4] == dropped, 'XConvertSelection did not use the actual final Drop timestamp')
                    require(dropped >= stamp or stamp - dropped > 0x80000000, 'Private server timestamp order is invalid')
                    probe.wait_state(2, 1, pixel_hash(expected))
                    report['checks'].append(name)
                    probe.fresh()
                positive('raw-direct-no-hover-pixels-captured-display-final-timestamp', 'image/png', png_a, pixels_a)
                positive('raw-INCR-bounded-chunks-extended-type-list', 'image/png', png_a, pixels_a, 'incr', True)
                # Native non-UTF8 URI survives all the way to the existing file decoder/editor.
                source.offer(probe.target, 'text/uri-list', uri_a)
                source.wait_status(probe.target, probe.observe)
                with source.guard:
                    require(len(source.requests) == 1, 'URI hover metadata was not acquired once')
                source.owner.set({'text/uri-list': uri_b})
                dropped = source.drop(probe.target)
                deadline = time.monotonic() + 3
                while time.monotonic() < deadline:
                    with source.guard:
                        if len(source.requests) >= 2:
                            require(source.requests[-1][4] == dropped, 'Final URI reacquisition used a hover timestamp')
                            break
                    probe.observe(); time.sleep(.005)
                else:
                    raise RuntimeError('Drop reused URI hover data without reacquisition')
                probe.wait_state(2, 1, pixel_hash(pixels_b))
                report['checks'].append('URI-final-reacquisition-replaces-hover-source')
                probe.fresh()
                # Confirm non-UTF8 path action itself, separately from the final-change oracle.
                source.offer(probe.target, 'text/uri-list', uri_a)
                source.wait_status(probe.target, probe.observe); source.drop(probe.target)
                probe.wait_state(2, 1, pixel_hash(pixels_a))
                report['checks'].append('native-non-UTF8-file-URI-editor-pixels')
                probe.fresh()
                with socket.socket() as private_endpoint:
                    private_endpoint.bind(('127.0.0.1', 0)); private_endpoint.listen(1); private_endpoint.setblocking(False)
                    url = f'http://127.0.0.1:{private_endpoint.getsockname()[1]}/image?private-query-token'.encode()
                    previous = probe.url_routes
                    source.offer(probe.target, 'text/uri-list', url)
                    source.wait_status(probe.target, probe.observe)
                    # Hover has no HTTP acquisition, including against an owned endpoint.
                    try: accepted, _ = private_endpoint.accept()
                    except BlockingIOError: pass
                    else: accepted.close(); raise RuntimeError('Hover contacted a private image endpoint')
                    source.drop(probe.target)
                    deadline = time.monotonic() + 6
                    while time.monotonic() < deadline:
                        state = probe.state()
                        if probe.url_routes > previous and state[0] == '1': break
                        time.sleep(.01)
                    else: raise RuntimeError('Native URL did not reach the existing application URL handler')
                    probe.assert_no_editor()
                    try: accepted, _ = private_endpoint.accept()
                    except BlockingIOError: pass
                    else: accepted.close(); raise RuntimeError('Existing private-network image policy was bypassed')
                    report['checks'].append('URL-native-worker-existing-handler-no-hover-fetch-private-network-refusal')
                probe.fresh()
                previous = probe.url_routes
                source.offer(probe.target, 'text/uri-list', b'https://user:private-query-token@example.invalid/image')
                source.wait_request('text/uri-list', probe.observe)
                probe.assert_no_editor(.3)
                require(probe.url_routes == previous, 'Credential-bearing URL reached image acquisition')
                report['checks'].append('credential-URL-refused-before-application-acquisition')
                probe.fresh()
                for mutation in ['same', 'aba']:
                    source.offer(probe.target, 'image/png', png_a)
                    source.wait_status(probe.target, probe.observe)
                    source.mutate_owner = mutation
                    source.drop(probe.target)
                    source.wait_request('image/png', probe.observe)
                    probe.assert_no_editor(.5)
                    report['checks'].append('XFixes-owner-' + mutation + '-refused')
                    probe.fresh()
                for action in ['cancel', 'unmap', 'move', 'close', 'new-input']:
                    target = probe.target
                    source.offer(target, 'image/png', png_a, 'stall')
                    source.wait_status(target, probe.observe); source.drop(target)
                    old = source.wait_request('image/png', probe.observe)
                    if action == 'unmap':
                        source.x.XUnmapWindow(source.connection, target); source.x.XFlush(source.connection)
                        probe.state()  # Native invalidation is observed before the obsolete reply.
                    elif action == 'move':
                        source.x.XMoveWindow(source.connection, target, 145, 125); source.x.XFlush(source.connection)
                        probe.state()
                    elif action == 'close':
                        probe.command('close', 'closed')
                        probe.target = int(probe.command('new', 'target')[1])
                    elif action == 'new-input':
                        probe.command('new-input', 'new-input-admitted')
                        probe.wait_state(2, 1, pixel_hash(pixels_b))
                    else:
                        probe.command('cancel', 'cancelled')
                    source.late_reply(old, png_a)
                    if action == 'new-input':
                        time.sleep(.2); probe.wait_state(2, 1, pixel_hash(pixels_b))
                    else:
                        probe.assert_no_editor(.3)
                    report['checks'].append('blocked-provider-' + action + '-late-result-refused')
                    if action == 'unmap':
                        source.x.XMapWindow(source.connection, target); source.x.XFlush(source.connection)
                    probe.fresh()
                # An unresponsive promised-data source is reaped by the real five-second parent watchdog.
                source.offer(probe.target, 'image/png', png_a, 'stall')
                source.wait_status(probe.target, probe.observe)
                started = time.monotonic(); source.drop(probe.target); source.wait_request('image/png', probe.observe)
                deadline = started + 6.5
                while time.monotonic() < deadline:
                    probe.observe()
                    with source.guard:
                        if any(kind == source.owner.atoms['XdndFinished'] and data[0] == probe.target and data[1] == 0
                               for kind, data in source.messages):
                            break
                    time.sleep(.01)
                else:
                    raise RuntimeError('Blocked provider was not refused within the native watchdog budget')
                probe.assert_no_editor()
                report['checks'].append('blocked-provider-five-second-watchdog-no-editor')
                report['blocked_provider_observation_ms'] = (time.monotonic() - started) * 1000
                # Actual GPUI receiver + real isolated URI worker, not a synthetic Status producer.
                for final_accept in [True, False]:
                    probe.fresh()
                    target = probe.target
                    source.offer(target, 'text/uri-list', uri_a, 'stall')
                    hover_request = source.wait_request('text/uri-list', probe.observe)
                    source.assert_status_count(0, probe.observe)
                    with source.guard:
                        require(source.pending_position is not None and source.position_count == 1,
                                'Actual receiver prematurely consumed the blocked hover Position')
                    # User release is cached at the source until its one actual hover ACK.
                    source.late_reply(hover_request, uri_a)
                    source.wait_status(target, probe.observe)
                    source.assert_status_count(1, probe.observe)
                    source.owner.set({'text/uri-list': uri_b})
                    # Atom None is a mandatory malformed-action refusal control; no Status is forged.
                    source.position(target, 460, 310,
                                    action=source.owner.atoms['XdndActionCopy'] if final_accept else 0)
                    source.wait_status(target, probe.observe, accepted=final_accept)
                    source.assert_status_count(2, probe.observe)
                    if final_accept:
                        source.drop(target)
                        probe.wait_state(2, 1, pixel_hash(pixels_b))
                        source.assert_status_count(2, probe.observe)
                        report['checks'].append('actual-receiver-delayed-hover-final-accept-one-Status-per-Position')
                    else:
                        probe.assert_no_editor()
                        source.assert_status_count(2, probe.observe)
                        source.leave(target)
                        report['checks'].append('actual-receiver-delayed-hover-invalid-final-action-refusal-no-late-accept')
                source.audit_status()
                receiver = Receiver(source)
                receiver.run_cases(probe, uri_a, raw_path, report)
                child.stdin.write(b'quit\n'); child.stdin.flush()
                child.wait(timeout=8)
                require(child.returncode == 0, 'Owned native test process failed')
                report['observed_native_helpers_reaped'] = probe.all_helpers_exited()
                require(report['observed_native_helpers_reaped'] > 0, 'No actual native worker was observed')
                source.audit_status()
            diagnostics = stderr_path.read_bytes() + '\n'.join(probe.all_stdout).encode()
            require(b'synthetic-\xff' not in diagnostics and b'private-query-token' not in diagnostics,
                    'Native diagnostic output exposed a source payload')
            report['scope'] = 'Owned Xvfb native protocol/worker/GPUI/editor software fixture; no physical input, presentation, user-session or PLAN performance acceptance.'
            args.report.parent.mkdir(parents=True, exist_ok=True)
            args.report.write_text(json.dumps(report, indent=2) + '\n')
        finally:
            if child is not None and child.poll() is None:
                child.terminate()
                try: child.wait(timeout=3)
                except subprocess.TimeoutExpired: child.kill(); child.wait(timeout=3)
            if probe is not None:
                for descriptor in probe.helpers.values():
                    try: signal.pidfd_send_signal(descriptor, signal.SIGKILL)
                    except ProcessLookupError: pass
                    finally: os.close(descriptor)
            if source is not None: source.close()
            if server is not None:
                server.terminate()
                try: server.wait(timeout=3)
                except subprocess.TimeoutExpired: server.kill(); server.wait(timeout=3)


if __name__ == '__main__':
    main()
