#!/usr/bin/env python3
"""Explicit owned X11 actual-main resident observation; never builds or captures."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import secrets
import selectors
import shutil
import signal
import subprocess
import sys
import time
from xml.sax.saxutils import escape

ROOT = Path(__file__).resolve().parents[1]
PROC = Path('/proc')
HELPER = b'__native-desktop-services-worker'
MEMORY = ('Private_Clean', 'Private_Dirty', 'Private_Hugetlb', 'Pss', 'Rss', 'Swap', 'SwapPss', 'Shared_Clean', 'Shared_Dirty', 'Shared_Hugetlb')
SOURCE_TREES = ('apps/desktop/src', 'crates/app-ui/src', 'crates/app-core/src', 'vendor/gpui-pre-linux/src')


def load_owner():
    spec = importlib.util.spec_from_file_location('owned_native_services', ROOT / 'scripts/test-unix-native-services.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def source_digest():
    aggregate = hashlib.sha256()
    paths = [ROOT / name for name in ('Cargo.toml', 'Cargo.lock', 'scripts/fixtures/unix_native_services.rs', 'scripts/test-unix-native-services.py', 'scripts/test-unix-resident-resources.py')]
    for tree in SOURCE_TREES:
        paths.extend((ROOT / tree).rglob('*.rs'))
    for path in sorted(paths):
        aggregate.update(str(path.relative_to(ROOT)).encode() + b'\0')
        aggregate.update(bytes.fromhex(digest(path)))
    return aggregate.hexdigest()


def stat_fields(text):
    fields = text[text.rfind(')') + 2:].split()
    if len(fields) < 22:
        raise RuntimeError('Invalid process stat')
    return {'starttime': int(fields[19]), 'cpu_ticks': int(fields[11]) + int(fields[12]), 'parent': int(fields[1]), 'state': fields[0]}


def memory_fields(text):
    memory = {}
    for line in text.splitlines():
        pieces = line.split()
        if pieces and pieces[0].rstrip(':') in MEMORY:
            if len(pieces) != 3 or pieces[2] != 'kB' or not pieces[1].isdecimal():
                raise RuntimeError('Unexpected smaps units')
            memory[pieces[0][:-1]] = int(pieces[1]) * 1024
    if not all(name in memory for name in ('Private_Clean', 'Private_Dirty', 'Pss', 'Rss', 'Swap')):
        raise RuntimeError('Missing required smaps counters')
    return {name: memory.get(name, 0) for name in MEMORY}


def named_numbers(text, names):
    result = {}
    for line in text.splitlines():
        pieces = line.split(':', 1)
        if len(pieces) == 2 and pieces[0].strip() in names:
            result[pieces[0].strip()] = int(pieces[1].strip())
    return result


def tasks(pid):
    values = sorted((PROC / str(pid) / 'task').iterdir(), key=lambda path: int(path.name))
    if not 1 <= len(values) <= 256:
        raise RuntimeError('Thread admission exceeded')
    return values


def family_pids(pid):
    pending, found = [pid], set()
    while pending:
        member = pending.pop()
        if member in found:
            continue
        if len(found) >= 8:
            raise RuntimeError('Application family admission exceeded')
        directory = PROC / str(member)
        if directory.stat().st_uid != os.geteuid():
            raise RuntimeError('Application child has unexpected UID')
        found.add(member)
        for task in tasks(member):
            pending.extend(int(value) for value in (task / 'children').read_text().split())
    return found


def identify(pid, binary):
    members = family_pids(pid)
    if len(members) != 2:
        raise RuntimeError('Expected exactly actual main and actual native helper')
    helper = (members - {pid}).pop()
    for member in members:
        if not (PROC / str(member) / 'exe').samefile(binary):
            raise RuntimeError('Unexpected application family executable')
    arguments = (PROC / str(helper) / 'cmdline').read_bytes().split(b'\0')
    if len(arguments) != 3 or arguments[1] != HELPER or arguments[2] != b'':
        raise RuntimeError('Unexpected native helper invocation')
    if stat_fields((PROC / str(helper) / 'stat').read_text())['parent'] != pid:
        raise RuntimeError('Native helper is not a direct child of the actual main')
    return helper


def process_snapshot(pid):
    directory = PROC / str(pid)
    initial = stat_fields((directory / 'stat').read_text())
    if initial['state'] in ('Z', 'X'):
        raise RuntimeError('Application process is no longer live')
    memory = memory_fields((directory / 'smaps_rollup').read_text())
    per_thread = {}
    for task in tasks(pid):
        status = named_numbers((task / 'status').read_text(), {'voluntary_ctxt_switches', 'nonvoluntary_ctxt_switches'})
        if len(status) != 2:
            raise RuntimeError('Missing per-thread context counters')
        wakeups = named_numbers((task / 'sched').read_text(), {'se.statistics.nr_wakeups'})
        per_thread[task.name] = {**status, 'starttime': stat_fields((task / 'stat').read_text())['starttime'], 'scheduler_wakeups': wakeups.get('se.statistics.nr_wakeups')}
    io = named_numbers((directory / 'io').read_text(), {'rchar', 'wchar', 'syscr', 'syscw', 'read_bytes', 'write_bytes', 'cancelled_write_bytes'})
    current = stat_fields((directory / 'stat').read_text())
    if current['starttime'] != initial['starttime']:
        raise RuntimeError('PID identity changed during snapshot')
    return {**current, 'memory_bytes': memory, 'fds': len(list((directory / 'fd').iterdir())), 'threads': per_thread, 'io': io}


def snapshot(pid, helper):
    if family_pids(pid) != {pid, helper}:
        raise RuntimeError('Application family changed')
    return {str(member): process_snapshot(member) for member in (pid, helper)}


def aggregate(sample):
    memory = {name: sum(value['memory_bytes'][name] for value in sample.values()) for name in MEMORY}
    return {'memory_bytes': memory, 'private_bytes': sum(memory[name] for name in ('Private_Clean', 'Private_Dirty', 'Private_Hugetlb')), 'threads': sum(len(value['threads']) for value in sample.values()), 'fds': sum(value['fds'] for value in sample.values())}


def compare(before, after, elapsed, clock_ticks):
    stable = before.keys() == after.keys() and all(before[pid]['starttime'] == after[pid]['starttime'] for pid in before)
    if not stable or elapsed <= 0:
        raise RuntimeError('Unstable measurement process identities')
    ticks = sum(after[pid]['cpu_ticks'] - before[pid]['cpu_ticks'] for pid in before)
    if ticks < 0:
        raise RuntimeError('CPU counters moved backwards')
    thread_stable = all(before[pid]['threads'].keys() == after[pid]['threads'].keys() and all(before[pid]['threads'][tid]['starttime'] == after[pid]['threads'][tid]['starttime'] for tid in before[pid]['threads']) for pid in before)
    thread_delta = {}
    for name in ('voluntary_ctxt_switches', 'nonvoluntary_ctxt_switches', 'scheduler_wakeups'):
        available = thread_stable and all(value[name] is not None for sample in (before, after) for process in sample.values() for value in process['threads'].values())
        thread_delta[name] = sum(after[pid]['threads'][tid][name] - before[pid]['threads'][tid][name] for pid in before for tid in before[pid]['threads']) if available else None
    io = {name: sum(after[pid]['io'][name] - before[pid]['io'][name] for pid in before) for name in before[next(iter(before))]['io']}
    return {'elapsed_seconds': elapsed, 'stable_family': stable, 'stable_endpoint_threads': thread_stable, 'joint_cpu_ticks': ticks, 'one_core_cpu_percent': ticks / clock_ticks / elapsed * 100, 'thread_counter_delta': thread_delta, 'joint_io_counter_delta': io}


class Supervisor:
    def __init__(self, family, interrupts):
        self.family, self.interrupts = family, interrupts
        self.deadline = time.monotonic() + 900
        self.watched = {}
        self.expected_exits = set()
        self.buffer = bytearray()

    def watch(self, process):
        descriptor = os.pidfd_open(process.pid)
        self.watched[descriptor] = process.pid
        return descriptor

    def check(self):
        self.interrupts.check()
        if time.monotonic() >= self.deadline:
            raise RuntimeError('Whole observation deadline exceeded')
        with selectors.DefaultSelector() as selector:
            for descriptor in self.watched:
                if descriptor not in self.expected_exits:
                    selector.register(descriptor, selectors.EVENT_READ)
            if selector.select(0):
                raise RuntimeError('Required private service or application exited early')

    def pause(self, seconds):
        self.check()
        with selectors.DefaultSelector() as selector:
            for descriptor in self.watched:
                if descriptor not in self.expected_exits:
                    selector.register(descriptor, selectors.EVENT_READ)
            if self.interrupts.wait(selector, max(0, min(seconds, self.deadline - time.monotonic()))):
                raise RuntimeError('Required private service or application exited early')
        self.check()

    def line(self, stream, seconds=5):
        until = min(self.deadline, time.monotonic() + seconds)
        while b'\n' not in self.buffer:
            self.check()
            with selectors.DefaultSelector() as selector:
                for descriptor in self.watched:
                    if descriptor not in self.expected_exits:
                        selector.register(descriptor, selectors.EVENT_READ, 'exit')
                selector.register(stream, selectors.EVENT_READ, 'line')
                events = self.interrupts.wait(selector, max(0, until - time.monotonic()))
            if not events:
                raise RuntimeError('Private protocol deadline exceeded')
            for key, _ in events:
                if key.data == 'exit':
                    raise RuntimeError('Private service exited during protocol step')
                data = os.read(stream.fileno(), 256 - len(self.buffer))
                if not data:
                    raise RuntimeError('Private protocol closed')
                self.buffer.extend(data)
                if len(self.buffer) >= 256:
                    raise RuntimeError('Private protocol output exceeds admission')
        value, _, remaining = self.buffer.partition(b'\n')
        self.buffer = bytearray(remaining)
        return value.decode('ascii')

    def command(self, host, command):
        self.check()
        if len(command) > 64 or '\n' in command:
            raise RuntimeError('Invalid supervisor command')
        host.stdin.write(command.encode('ascii') + b'\n')
        host.stdin.flush()
        return self.line(host.stdout)

    def state(self, host, pid):
        parts = self.command(host, f'PROBE {pid}').split()
        if len(parts) != 4 or parts[0] != 'STATE' or not all(value.isdecimal() for value in parts[1:]):
            raise RuntimeError('Invalid private window state')
        return tuple(map(int, parts[1:]))

    def window(self, host, pid, helper, visible):
        until = min(self.deadline, time.monotonic() + 15)
        wanted = (1, 1, helper) if visible else (0, 0, helper)
        while True:
            if self.state(host, pid) == wanted:
                return
            if time.monotonic() >= until:
                raise RuntimeError('Actual control/tray state deadline exceeded')
            self.pause(0.1)

    def exited(self, process, descriptor, seconds=15):
        self.expected_exits.add(descriptor)
        try:
            until = min(self.deadline, time.monotonic() + seconds)
            with selectors.DefaultSelector() as selector:
                selector.register(descriptor, selectors.EVENT_READ)
                if not self.interrupts.wait(selector, max(0, until - time.monotonic())):
                    raise RuntimeError('Owned process exit deadline exceeded')
            if process.wait():
                raise RuntimeError('Owned process returned a failure')
        finally:
            self.watched.pop(descriptor)
            self.expected_exits.discard(descriptor)
            os.close(descriptor)

    def close_descriptors(self):
        for descriptor in self.watched:
            os.close(descriptor)
        self.watched.clear()


def exercise(args, owner, family, interrupts, report):
    supervisor = Supervisor(family, interrupts)
    runtime = family.create_runtime()
    report['phase'] = 'private_session_startup'
    environment = owner.isolated_environment(runtime)
    environment.update({'XDG_SESSION_TYPE': 'x11', 'LIBGL_ALWAYS_SOFTWARE': '1', 'VK_DRIVER_FILES': str(args.software_vulkan_icd), 'VK_ICD_FILENAMES': str(args.software_vulkan_icd)})
    try:
        # Install the same cookie before Xvfb starts and for its chosen display.
        cookie = secrets.token_hex(16)
        owner.run([args.xauth, '-f', environment['XAUTHORITY'], 'add', ':0', '.', cookie], 5, family, interrupts, env=environment, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        xserver = family.spawn([args.xvfb, '-displayfd', '1', '-screen', '0', '1280x800x24', '-auth', environment['XAUTHORITY'], '-nolisten', 'tcp', '-noreset'], env=environment, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        supervisor.watch(xserver)
        display = supervisor.line(xserver.stdout)
        if not display.isdecimal():
            raise RuntimeError('Invalid private Xvfb display')
        environment['DISPLAY'] = ':' + display
        owner.run([args.xauth, '-f', environment['XAUTHORITY'], 'add', environment['DISPLAY'], '.', cookie], 5, family, interrupts, env=environment, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        del cookie
        address = f'unix:path={runtime}/bus'
        environment['DBUS_SESSION_BUS_ADDRESS'] = address
        service_dir = runtime / 'services'
        service_dir.mkdir(mode=0o700)
        config = runtime / 'bus.conf'
        config.write_text('<busconfig><type>session</type><listen>' + escape(address) + '</listen><auth>EXTERNAL</auth><servicedir>' + escape(str(service_dir)) + '</servicedir><policy context="default"><allow own="*"/><allow send_destination="*"/><allow receive_sender="*"/></policy></busconfig>')
        daemon = family.spawn([args.dbus_daemon, '--config-file=' + str(config), '--nofork', '--print-address=1'], env=environment, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        supervisor.watch(daemon)
        actual = supervisor.line(daemon.stdout)
        if actual.split(',', 1)[0] != address:
            raise RuntimeError('Unexpected private D-Bus authority')
        environment['DBUS_SESSION_BUS_ADDRESS'] = actual
        host = family.spawn([str(args.fixture), 'resident_host'], env=environment, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        supervisor.watch(host)
        if supervisor.line(host.stdout) != 'RESIDENT_HOST_READY':
            raise RuntimeError('Private host did not become ready')
        report['phase'] = 'actual_main_startup'
        main = family.spawn([str(args.binary)], env=environment, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        main_descriptor = supervisor.watch(main)
        until = time.monotonic() + 20
        while True:
            supervisor.check()
            try:
                helper = identify(main.pid, args.binary)
                break
            except (FileNotFoundError, RuntimeError):
                if time.monotonic() >= until:
                    raise RuntimeError('Actual main/helper startup deadline exceeded') from None
                supervisor.pause(0.1)
        helper_descriptor = os.pidfd_open(helper)
        supervisor.watched[helper_descriptor] = helper
        supervisor.window(host, main.pid, helper, True)
        initial = snapshot(main.pid, helper)
        report['visible_initial'] = {'processes': initial, 'joint': aggregate(initial)}
        report['phase'] = 'close_show_cycles'
        cycles = []
        for index in range(args.cycles):
            supervisor.window(host, main.pid, helper, True)
            if supervisor.command(host, f'CLOSE {main.pid}') != 'CLOSE_SENT':
                raise RuntimeError('Actual native close was not sent')
            supervisor.window(host, main.pid, helper, False)
            hidden = snapshot(main.pid, helper)
            # A normal second invocation must forward Show to this retained owner.
            secondary = family.spawn([str(args.binary)], env=environment, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            descriptor = os.pidfd_open(secondary.pid)
            try:
                with selectors.DefaultSelector() as selector:
                    selector.register(descriptor, selectors.EVENT_READ)
                    if not interrupts.wait(selector, 10):
                        raise RuntimeError('Actual secondary Show deadline exceeded')
                if secondary.wait() != 0:
                    raise RuntimeError('Actual secondary Show failed')
            finally:
                os.close(descriptor)
            supervisor.window(host, main.pid, helper, True)
            if identify(main.pid, args.binary) != helper:
                raise RuntimeError('Actual main/helper changed after Show')
            cycles.append({'cycle': index + 1, 'windowless_alive': True, 'reopened': True, 'joint_hidden': aggregate(hidden)})
        report['cycles'] = cycles
        if supervisor.command(host, f'CLOSE {main.pid}') != 'CLOSE_SENT':
            raise RuntimeError('Final actual native close failed')
        supervisor.window(host, main.pid, helper, False)
        report['closed_immediate'] = {'processes': snapshot(main.pid, helper)}
        report['phase'] = 'windowless_warmup'
        warmup_until = time.monotonic() + 30
        while time.monotonic() < warmup_until:
            supervisor.pause(min(1, warmup_until - time.monotonic()))
        supervisor.window(host, main.pid, helper, False)
        report['phase'] = 'five_minute_idle'
        before = snapshot(main.pid, helper)
        start = time.monotonic()
        until = start + 300
        # No X11/D-Bus requests or Show during the measurement interval.
        topology_checks = 0
        while time.monotonic() < until:
            supervisor.pause(min(1, until - time.monotonic()))
            if family_pids(main.pid) != {main.pid, helper}:
                raise RuntimeError('Application family changed during idle')
            topology_checks += 1
        after = snapshot(main.pid, helper)
        elapsed = time.monotonic() - start
        supervisor.window(host, main.pid, helper, False)
        measured = compare(before, after, elapsed, os.sysconf('SC_CLK_TCK'))
        joint_before, joint_after = aggregate(before), aggregate(after)
        measured.update({'warmup_seconds': 30, 'topology_checks': topology_checks, 'before': before, 'after': after, 'joint_before': joint_before, 'joint_after': joint_after})
        measured['private_budget_observed'] = max(joint_before['private_bytes'], joint_after['private_bytes']) <= 50 * 1024 * 1024 and joint_after['memory_bytes']['Swap'] == 0 and joint_before['memory_bytes']['Swap'] == 0
        measured['cpu_budget_observed'] = measured['one_core_cpu_percent'] <= 0.1
        report['idle'] = measured
        report['phase'] = 'post_idle_show'
        secondary = family.spawn([str(args.binary)], env=environment, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        descriptor = supervisor.watch(secondary)
        supervisor.exited(secondary, descriptor, 10)
        supervisor.window(host, main.pid, helper, True)
        report['post_idle_reopen'] = True
        report['phase'] = 'actual_tray_quit'
        supervisor.expected_exits.update((main_descriptor, helper_descriptor))
        if supervisor.command(host, f'QUIT {helper}') != 'QUIT_SENT':
            raise RuntimeError('Actual tray Quit was not sent')
        supervisor.watched.pop(helper_descriptor)
        os.close(helper_descriptor)
        supervisor.exited(main, main_descriptor)
        report['graceful_main_quit'] = True
        report['status'] = 'observed'
        report['phase'] = 'finished'
    finally:
        supervisor.close_descriptors()


def self_test():
    fields = ['S', '7'] + ['0'] * 9 + ['11', '13'] + ['0'] * 6 + ['99'] + ['0'] * 2
    assert stat_fields('20 (a tricky ) process) ' + ' '.join(fields)) == {'starttime': 99, 'cpu_ticks': 24, 'parent': 7, 'state': 'S'}
    memory = memory_fields('Private_Clean: 1 kB\nPrivate_Dirty: 2 kB\nPss: 4 kB\nRss: 5 kB\nSwap: 0 kB\n')
    assert memory['Private_Clean'] == 1024 and memory['SwapPss'] == 0
    try:
        memory_fields('Private_Clean: 1 MB\n')
    except RuntimeError:
        pass
    else:
        raise AssertionError('Invalid memory units accepted')
    def endpoint(ticks):
        return {'20': {'starttime': 99, 'cpu_ticks': ticks, 'threads': {'20': {'starttime': 99, 'voluntary_ctxt_switches': ticks, 'nonvoluntary_ctxt_switches': 0, 'scheduler_wakeups': None}}, 'io': {'read_bytes': 0}}}
    result = compare(endpoint(1), endpoint(31), 300, 100)
    assert abs(result['one_core_cpu_percent'] - 0.1) < 1e-12
    assert result['thread_counter_delta']['scheduler_wakeups'] is None
    assert result['thread_counter_delta']['voluntary_ctxt_switches'] == 30
    changed = endpoint(31)
    changed['20']['starttime'] = 100
    try:
        compare(endpoint(1), changed, 300, 100)
    except RuntimeError:
        pass
    else:
        raise AssertionError('PID identity change accepted')
    print('PASS pure metric guards; no native or process checks')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--self-test', action='store_true')
    parser.add_argument('--allow-private-fixtures', action='store_true')
    parser.add_argument('--binary', type=Path)
    parser.add_argument('--fixture', type=Path)
    parser.add_argument('--software-vulkan-icd', type=Path)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--label', default='current')
    parser.add_argument('--cycles', type=int, default=8)
    parser.add_argument('--xvfb', default='Xvfb')
    parser.add_argument('--xauth', default='xauth')
    parser.add_argument('--dbus-daemon', default='dbus-daemon')
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    if sys.platform != 'linux' or not args.allow_private_fixtures:
        parser.error('Linux and explicit --allow-private-fixtures are required')
    if not all((args.binary, args.fixture, args.software_vulkan_icd, args.output)) or not 1 <= args.cycles <= 8 or not re.fullmatch(r'[A-Za-z0-9_-]{1,48}', args.label):
        parser.error('Supply explicit binary, fixture, software ICD, output and a safe label; cycles must be 1–8')
    for name in ('binary', 'fixture', 'software_vulkan_icd'):
        setattr(args, name, getattr(args, name).resolve(strict=True))
    if args.binary == args.fixture or args.binary.name != 'screenshots':
        parser.error('Measure the normal screenshots executable, separate from the fixture')
    for name in ('xvfb', 'xauth', 'dbus_daemon'):
        executable = shutil.which(getattr(args, name))
        if not executable:
            parser.error('Required private fixture executable is unavailable')
        setattr(args, name, executable)
    report = {'schema': 1, 'label': args.label, 'status': 'failed', 'binary_sha256': digest(args.binary), 'fixture_sha256': digest(args.fixture), 'source_tree_sha256_at_observation': source_digest(), 'software_icd_sha256': digest(args.software_vulkan_icd), 'clock_ticks_per_second': os.sysconf('SC_CLK_TCK'), 'scope': 'actual normal main and actual Linux native helper, authenticated private software Xvfb, private D-Bus/SNI host, no capture or clipboard read/write', 'limits': 'no GPU memory, physical display/input, reference hardware, wakeup inference from context switches, peak between endpoints or old/new equivalence claim', 'targets': {'joint_private_bytes': 50 * 1024 * 1024, 'one_core_cpu_percent': 0.1}}
    owner = load_owner()
    failed = False
    try:
        with owner.Interrupts() as interrupts:
            family = owner.OwnedChildren()
            try:
                exercise(args, owner, family, interrupts, report)
            finally:
                # Preserve pidfds/subreaper/runtime authority until kernel-confirmed exit/reap.
                family.close()
                report['cleanup_reaped_all_owned_children'] = not family.handles and family.runtime is None and not Path(f'/proc/self/task/{os.getpid()}/children').read_text().split()
                if not report['cleanup_reaped_all_owned_children']:
                    raise RuntimeError('Owned cleanup did not finish')
    except BaseException as error:
        report['status'] = 'failed'
        report['failure_type'] = type(error).__name__  # Never retain native stderr, paths or secrets.
        failed = True
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open('x', encoding='utf-8') as output:
        json.dump(report, output, indent=2)
        output.write('\n')
    budgets = report.get('idle', {})
    print('Private resident observation complete; inspect aggregate JSON and scope limits')
    if failed or not budgets.get('private_budget_observed') or not budgets.get('cpu_budget_observed'):
        raise SystemExit(2)


if __name__ == '__main__':
    main()
