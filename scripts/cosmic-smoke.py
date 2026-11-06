#!/usr/bin/env python3
"""Real, isolated nested COSMIC smoke; no build, install, host config or restart.

Run after building both binaries:
  /usr/bin/python3 scripts/cosmic-smoke.py --parent-wayland /run/user/1000/wayland-1

Requires Python GI (Gio, GTK4), libxkbcommon, a working parent Wayland display,
Mesa/EGL and the bridge-enabled cosmic-comp binary from build-cosmic.sh.
Only cosmic-comp connects to the parent socket. All clients, custom shortcuts,
HOME, XDG directories and D-Bus are private. Evidence is always retained.
The verifier can pass an absolute parent socket through
WINDOW_ZONES_SMOKE_COSMIC_PARENT_DISPLAY; WINDOW_ZONES_COSMIC_COMPOSITOR selects
the local custom binary. Neither variable is forwarded to application clients.
"""

import argparse
import array
import ctypes
import ctypes.util
import hashlib
import json
import mmap
import os
from pathlib import Path
import select
import shlex
import shutil
import signal
import socket
import stat
import struct
import subprocess
import sys
import tempfile
import time
import traceback
import uuid
import zlib

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / 'scripts/smoke'))
import owned_process


class Failure(RuntimeError):
    pass


def until(predicate, description, timeout=12, pump=None):
    deadline = time.monotonic() + timeout
    while True:
        value = predicate()
        if value:
            return value
        if time.monotonic() >= deadline:
            raise Failure('Timed out: ' + description)
        if pump:
            pump(min(0.05, max(0, deadline - time.monotonic())))
        else:
            time.sleep(0.05)


def text(path):
    return path.read_text(errors='replace') if path.exists() else ''


def u32(*values):
    return struct.pack('=' + 'I' * len(values), *values)


def wire_string(value):
    data = value.encode() + b'\0'
    return u32(len(data)) + data + b'\0' * (-len(data) % 4)


def get_string(data, offset=0):
    length, = struct.unpack_from('=I', data, offset)
    return data[offset + 4:offset + 4 + length - 1].decode(), offset + 4 + (length + 3) // 4 * 4


class Wayland:
    """Small wire client for public observation, capture and native keyboard input.

    No bridge methods are used here. Opcodes follow cosmic-toplevel-info v2,
    ext-foreign-toplevel-list v1, xdg-output v3, ext-image-copy-capture v1,
    and virtual-keyboard-unstable v1. This client never requests window moves.
    """

    def __init__(self, path, log):
        self.socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.socket.settimeout(3)
        self.socket.connect(str(path))
        self.buffer = b''
        self.next_id = 2
        self.handlers = {1: self.display_event}
        self.globals = {}
        self.outputs = {}
        self.windows = {}
        self.log = log.open('x')
        self.registry = self.new(self.registry_event)
        self.send(1, 1, u32(self.registry))
        self.roundtrip()
        self.info = self.bind('zcosmic_toplevel_info_v1', 2, self.info_event)
        self.xdg_manager = self.bind('zxdg_output_manager_v1', 3)
        for name, interface, version in self.globals.values():
            if interface != 'wl_output':
                continue
            output = self.bind_name(name, interface, min(version, 4))
            self.outputs[output] = {}
            xdg = self.new(lambda opcode, data, output=output: self.output_event(output, opcode, data))
            self.send(self.xdg_manager, 1, u32(xdg, output))
        self.bind('ext_foreign_toplevel_list_v1', 1, self.list_event)
        self.roundtrip()
        self.roundtrip()

    def new(self, handler=None):
        result = self.next_id
        self.next_id += 1
        self.handlers[result] = handler or (lambda _opcode, _data: None)
        return result

    def send(self, obj, opcode, data=b'', fd=None):
        message = u32(obj, (len(data) + 8) << 16 | opcode) + data
        if fd is None:
            self.socket.sendall(message)
        else:
            count = self.socket.sendmsg([message], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array('i', [fd]))])
            if count < len(message):
                self.socket.sendall(message[count:])

    def destroy(self, obj, opcode=0):
        self.send(obj, opcode)
        # Match a destroyed libwayland proxy: queued events are no longer
        # delivered to its old listener, including teardown's stopped event.
        self.handlers[obj] = lambda _opcode, _data: None

    def pump(self, timeout=0.05):
        if select.select([self.socket], [], [], timeout)[0]:
            data = self.socket.recv(65536)
            if not data:
                raise Failure('Nested compositor closed the independent observer connection')
            self.buffer += data
        while len(self.buffer) >= 8:
            obj, word = struct.unpack_from('=II', self.buffer)
            size, opcode = word >> 16, word & 65535
            if size < 8 or size % 4:
                raise Failure('Malformed Wayland event')
            if len(self.buffer) < size:
                break
            data, self.buffer = self.buffer[8:size], self.buffer[size:]
            self.log.write(json.dumps({'time': time.monotonic(), 'object': obj,
                                       'opcode': opcode, 'payload': data.hex()}) + '\n')
            self.log.flush()
            if obj not in self.handlers:
                raise Failure(f'Unknown Wayland object {obj}')
            self.handlers[obj](opcode, data)

    def display_event(self, opcode, data):
        if opcode == 0:
            obj, code = struct.unpack_from('=II', data)
            message, _ = get_string(data, 8)
            raise Failure(f'Wayland protocol error {obj}/{code}: {message}')
        elif opcode == 1:
            self.handlers.pop(struct.unpack('=I', data)[0], None)

    def roundtrip(self):
        done = []
        callback = self.new(lambda _opcode, _data: done.append(True))
        self.send(1, 0, u32(callback))
        until(lambda: done, 'Wayland roundtrip', 4, self.pump)

    def registry_event(self, opcode, data):
        if opcode == 0:
            name, = struct.unpack_from('=I', data)
            interface, offset = get_string(data, 4)
            version, = struct.unpack_from('=I', data, offset)
            self.globals[name] = (name, interface, version)
        elif opcode == 1:
            self.globals.pop(struct.unpack('=I', data)[0], None)

    def bind_name(self, name, interface, version, handler=None):
        obj = self.new(handler)
        self.send(self.registry, 0, u32(name) + wire_string(interface) + u32(version, obj))
        return obj

    def bind(self, interface, version, handler=None):
        matches = [entry for entry in self.globals.values() if entry[1] == interface]
        if len(matches) != 1 or matches[0][2] < version:
            raise Failure(f'Required public Wayland global unavailable: {interface} v{version}')
        return self.bind_name(matches[0][0], interface, version, handler)

    def output_event(self, output, opcode, data):
        if opcode == 0:
            self.outputs[output]['origin'] = list(struct.unpack('=ii', data))
        elif opcode == 1:
            self.outputs[output]['size'] = list(struct.unpack('=ii', data))
        elif opcode == 3:
            self.outputs[output]['name'] = get_string(data)[0]

    def list_event(self, opcode, data):
        if opcode == 0:
            foreign, = struct.unpack('=I', data)
            state = {'foreign': foreign, 'closed': False, 'frames': {}, 'states': []}
            self.windows[foreign] = state
            self.handlers[foreign] = lambda op, payload: self.foreign_event(state, op, payload)
            cosmic = self.new(lambda op, payload: self.cosmic_event(state, op, payload))
            self.send(self.info, 1, u32(cosmic, foreign))
        elif opcode == 1:
            raise Failure('Compositor stopped public toplevel observation')

    def foreign_event(self, state, opcode, data):
        if opcode == 0:
            state['closed'] = True
        elif opcode == 2:
            state['title'] = get_string(data)[0]
        elif opcode == 3:
            state['app_id'] = get_string(data)[0]
        elif opcode == 4:
            state['identifier'] = get_string(data)[0]

    def cosmic_event(self, state, opcode, data):
        if opcode == 8:
            count, = struct.unpack_from('=I', data)
            state['states'] = list(struct.unpack('=' + 'I' * (count // 4), data[4:4 + count]))
        elif opcode == 9:
            output, x, y, width, height = struct.unpack('=Iiiii', data)
            state['frames'][output] = [x, y, width, height]
        elif opcode == 5:
            state['frames'].pop(struct.unpack('=I', data)[0], None)

    def info_event(self, opcode, _data):
        if opcode == 2:
            # Log public state at the protocol's atomic batch boundary.
            self.log.write(json.dumps({'snapshot': list(self.windows.values()),
                                       'outputs': self.outputs, 'time': time.monotonic()}) + '\n')
            self.log.flush()

    def window(self, title):
        matches = [window for window in self.windows.values()
                   if window.get('title') == title and not window['closed']]
        return matches[0] if len(matches) == 1 else None

    def frame(self, title):
        state = self.window(title)
        if not state or len(state['frames']) != 1:
            return None
        output, frame = next(iter(state['frames'].items()))
        origin = self.outputs.get(output, {}).get('origin')
        return [frame[0] + origin[0], frame[1] + origin[1], *frame[2:]] if origin else None

    def keyboard(self):
        manager = self.bind('zwp_virtual_keyboard_manager_v1', 1)
        seat = self.bind('wl_seat', 1)
        keyboard = self.new()
        self.send(manager, 0, u32(seat, keyboard))
        library = ctypes.CDLL(ctypes.util.find_library('xkbcommon') or 'libxkbcommon.so.0')
        library.xkb_context_new.argtypes = [ctypes.c_int]
        library.xkb_context_new.restype = ctypes.c_void_p
        library.xkb_keymap_new_from_names.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_int]
        library.xkb_keymap_new_from_names.restype = ctypes.c_void_p
        library.xkb_keymap_get_as_string.argtypes = [ctypes.c_void_p, ctypes.c_int]
        library.xkb_keymap_get_as_string.restype = ctypes.c_void_p
        library.xkb_keymap_unref.argtypes = [ctypes.c_void_p]
        library.xkb_context_unref.argtypes = [ctypes.c_void_p]
        context = library.xkb_context_new(0)
        if not context:
            raise Failure('xkbcommon context creation failed')
        keymap = None
        pointer = None
        try:
            keymap = library.xkb_keymap_new_from_names(context, None, 0)
            if not keymap:
                raise Failure('xkbcommon default keymap creation failed')
            pointer = library.xkb_keymap_get_as_string(keymap, 1)
            if not pointer:
                raise Failure('xkbcommon keymap serialization failed')
            data = ctypes.string_at(pointer) + b'\0'
            fd = os.memfd_create('window-zones-cosmic-keymap', os.MFD_CLOEXEC)
            try:
                os.write(fd, data)
                self.send(keyboard, 0, u32(1, len(data)), fd)
            finally:
                os.close(fd)
        finally:
            if pointer:
                libc = ctypes.CDLL(None)
                libc.free.argtypes = [ctypes.c_void_p]
                libc.free(pointer)
            if keymap:
                library.xkb_keymap_unref(keymap)
            library.xkb_context_unref(context)
        self.roundtrip()
        return keyboard

    def shortcut(self, keyboard):
        # Use an arrow: evdev extended F-keys may map to multimedia keysyms.
        stamp = int(time.monotonic() * 1000) & 0xffffffff
        # Press the modifier keys too: COSMIC's shortcut filter uses the seat's
        # held-key state, not only the virtual keyboard's serialized mask.
        for offset, (key, pressed) in enumerate(
                ((29, 1), (56, 1), (106, 1), (106, 0), (56, 0), (29, 0))):
            self.send(keyboard, 1, u32((stamp + offset) & 0xffffffff, key, pressed))
        self.send(keyboard, 2, u32(0, 0, 0, 0))
        self.roundtrip()

    def capture(self, path):
        manager = self.bind('ext_image_copy_capture_manager_v1', 1)
        sources = self.bind('ext_output_image_capture_source_manager_v1', 1)
        shm = self.bind('wl_shm', 1)
        source = self.new()
        self.send(sources, 0, u32(source, next(iter(self.outputs))))
        constraints = {'formats': []}
        def session_event(opcode, data):
            if opcode == 0:
                constraints['size'] = struct.unpack('=II', data)
            elif opcode == 1:
                constraints['formats'].append(struct.unpack('=I', data)[0])
            elif opcode == 4:
                constraints['done'] = True
            elif opcode == 5:
                raise Failure('Compositor stopped output capture')
        session = self.new(session_event)
        self.send(manager, 0, u32(session, source, 0))
        until(lambda: constraints.get('done'), 'capture constraints', 5, self.pump)
        formats = constraints['formats']
        supported = [value for value in (1, 0, 0x34324258, 0x34324241) if value in formats]
        if not supported:
            raise Failure(f'No supported 32-bit shared-memory capture format: {formats}')
        fmt = supported[0]
        width, height = constraints['size']
        if not (0 < width <= 16384 and 0 < height <= 16384):
            raise Failure(f'Invalid output capture size: {width}x{height}')
        size = width * height * 4
        fd = os.memfd_create('window-zones-cosmic-capture', os.MFD_CLOEXEC)
        memory = None
        try:
            os.ftruncate(fd, size)
            memory = mmap.mmap(fd, size)
            pool = self.new()
            self.send(shm, 0, u32(pool, size), fd)
            buffer = self.new()
            self.send(pool, 0, u32(buffer, 0, width, height, width * 4, fmt))
            result = {}
            def frame_event(opcode, data):
                if opcode == 0:
                    result['transform'] = struct.unpack('=I', data)[0]
                elif opcode == 3:
                    result['ready'] = True
                elif opcode == 4:
                    raise Failure('Output capture failed: ' + str(struct.unpack('=I', data)[0]))
            frame = self.new(frame_event)
            self.send(session, 0, u32(frame))
            self.send(frame, 1, u32(buffer))
            self.send(frame, 2, struct.pack('=iiii', 0, 0, width, height))
            self.send(frame, 3)
            until(lambda: result.get('ready'), 'native output capture', 8, self.pump)
            pixels = memory[:]
            rows = bytearray()
            # ARGB/XRGB have B,G,R,X byte order on little-endian Linux;
            # ABGR/XBGR have R,G,B,X. PNG retains raw capture orientation.
            bgr = fmt in (0, 1)
            if sys.byteorder != 'little':
                raise Failure('Capture conversion currently requires little-endian Linux')
            rgb = bytearray(width * height * 3)
            rgb[0::3] = pixels[2 if bgr else 0::4]
            rgb[1::3] = pixels[1::4]
            rgb[2::3] = pixels[0 if bgr else 2::4]
            for offset in range(0, len(rgb), width * 3):
                rows += b'\0' + rgb[offset:offset + width * 3]
            def chunk(kind, payload):
                return struct.pack('!I', len(payload)) + kind + payload + struct.pack('!I', zlib.crc32(kind + payload))
            path.write_bytes(b'\x89PNG\r\n\x1a\n' +
                             chunk(b'IHDR', struct.pack('!IIBBBBB', width, height, 8, 2, 0, 0, 0)) +
                             chunk(b'IDAT', zlib.compress(rows)) + chunk(b'IEND', b''))
            path.with_suffix('.json').write_text(json.dumps({'width': width, 'height': height,
                'wl_shm_format': fmt, 'buffer_transform': result.get('transform'),
                'protocol': 'ext-image-copy-capture-v1', 'raw_orientation': True}, indent=2) + '\n')
            self.destroy(frame)
            self.destroy(buffer)
            self.destroy(pool, 1)
            self.destroy(session, 1)
            self.destroy(source)
            self.roundtrip()
        finally:
            if memory is not None:
                memory.close()
            os.close(fd)

    def close(self):
        self.socket.close()
        self.log.close()


class Smoke:
    def __init__(self, args):
        self.args = args
        self.work = args.output_dir or ROOT / 'target/verification' / ('cosmic-' + time.strftime('%Y%m%d-%H%M%S') + '-' + uuid.uuid4().hex[:6])
        self.work = self.work.resolve()
        self.work.mkdir(mode=0o700, parents=True, exist_ok=False)
        self.private = Path(tempfile.mkdtemp(prefix='wz-cosmic-'))
        self.owner = uuid.uuid4().hex
        self.registry = self.work / 'pids'
        self.registry.touch()
        self.children = []
        self.streams = []
        self.checks = []
        self.actions = (self.work / 'actions.jsonl').open('x')
        self.observer = None
        self.parent_socket = args.parent_wayland
        if self.parent_socket is None:
            selected = os.environ.get('WINDOW_ZONES_SMOKE_COSMIC_PARENT_DISPLAY') or os.environ.get('WAYLAND_DISPLAY')
            if selected:
                self.parent_socket = Path(selected)
                if not self.parent_socket.is_absolute():
                    self.parent_socket = Path(os.environ.get('XDG_RUNTIME_DIR', '/nonexistent')) / selected
        self.base_env = {name: os.environ[name] for name in ('PATH', 'LANG', 'LC_ALL', 'LC_CTYPE', 'TZ') if name in os.environ}
        self.base_env.update(WINDOW_ZONES_SMOKE_PRIVATE_OWNER=self.owner,
                             WINDOW_ZONES_SMOKE_OWNER=str(self.work),
                             XDG_CURRENT_DESKTOP='COSMIC', XDG_SESSION_DESKTOP='COSMIC',
                             DESKTOP_SESSION='cosmic', XDG_SESSION_TYPE='wayland',
                             GDK_BACKEND='wayland', GTK_A11Y='none', NO_AT_BRIDGE='1',
                             XDG_DATA_DIRS='/usr/local/share:/usr/share', RUST_LOG='info',
                             COSMIC_BACKEND='winit', WINIT_UNIX_BACKEND='wayland')
        for variable, directory in (('HOME', 'home'), ('XDG_CONFIG_HOME', 'config'),
                                    ('XDG_DATA_HOME', 'data'), ('XDG_CACHE_HOME', 'cache'),
                                    ('XDG_STATE_HOME', 'state')):
            path = self.private / directory
            path.mkdir(mode=0o700)
            self.base_env[variable] = str(path)
        self.config = Path(self.base_env['XDG_CONFIG_HOME']) / 'window_zones/config.toml'
        self.config.parent.mkdir()
        self.config.write_text(self.configuration())
        self.original_config = self.config.read_bytes()

    @staticmethod
    def configuration():
        bindings = [('Left', 'move-to-zone', 'left-half'), ('Right', 'move-to-zone', 'right-half'),
                    ('Down', 'move-to-zone', 'minimum-probe'), ('C', 'center', None),
                    ('Backspace', 'restore', None)]
        bindings = '\n'.join('[[bindings]]\nhotkey = "Ctrl+Alt+' + key + '"\naction = { type = "' + action + '"' +
                            (', zone = "' + zone + '"' if zone else '') + ' }\n' for key, action, zone in bindings)
        return bindings + '\n[zones]\nminimum-probe = { x = 0, y = 0, width = 1, height = 1 }\n'

    def event(self, kind, **fields):
        self.actions.write(json.dumps({'time': time.monotonic(), 'kind': kind, **fields}) + '\n')
        self.actions.flush()

    def check(self, name, condition, detail=None):
        self.checks.append({'name': name, 'passed': bool(condition), 'detail': detail})
        self.event('assertion', **self.checks[-1])
        if not condition:
            raise Failure(name + ': ' + str(detail))

    def start(self, command, label, env, stdin=False):
        log = self.work / (label + '.log')
        stream = log.open('xb')
        self.streams.append(stream)
        child = subprocess.Popen([str(part) for part in command], env=env, cwd=ROOT,
                                 stdin=subprocess.PIPE if stdin else subprocess.DEVNULL,
                                 stdout=stream, stderr=subprocess.STDOUT)
        self.children.append(child)
        owned_process.register(self.registry, self.owner, os.getpid(), child.pid)
        self.event('spawn', label=label, pid=child.pid, argv=list(map(str, command)))
        return child, log

    def send(self, child, line):
        self.event('stdin', pid=child.pid, line=line)
        child.stdin.write((line + '\n').encode())
        child.stdin.flush()

    def stop(self, child, graceful=False):
        if child.poll() is None:
            if graceful:
                self.send(child, 'quit')
            else:
                identity = owned_process.records(self.registry).get(child.pid)
                if identity is not None:
                    owned_process.signal_identity(child.pid, identity, signal.SIGTERM)
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                identity = owned_process.records(self.registry).get(child.pid)
                if identity is not None:
                    owned_process.signal_identity(child.pid, identity, signal.SIGKILL)
                child.wait(timeout=5)
        if graceful:
            self.check('runtime clean quit', child.returncode == 0, child.returncode)

    def cli(self, env, label, command, backend='auto', explicit=True, success=True):
        argv = [str(self.args.binary), '--backend', backend]
        if explicit:
            argv += ['--config', str(self.config)]
        argv += command
        result = subprocess.run(argv, env=env, cwd=ROOT, stdout=subprocess.PIPE,
                                stderr=subprocess.STDOUT, timeout=8)
        output = result.stdout.decode(errors='replace')
        (self.work / (label + '.log')).write_text(output)
        self.event('cli', label=label, argv=argv, exit=result.returncode, output=output)
        if success:
            self.check(label + ' exits successfully', result.returncode == 0, output)
        return result.returncode, output

    def environment(self, phase):
        runtime = self.private / ('runtime-' + phase)
        runtime.mkdir(mode=0o700)
        env = dict(self.base_env, XDG_RUNTIME_DIR=str(runtime))
        bus, log = self.start(['dbus-daemon', '--session', '--nofork', '--nopidfile', '--print-address=1'], phase + '-dbus', env)
        address = until(lambda: next((line for line in text(log).splitlines() if line.startswith('unix:')), None),
                        'private session D-Bus address', self.args.startup_timeout)
        env['DBUS_SESSION_BUS_ADDRESS'] = address
        return env, bus

    def compositor(self, env, phase, enabled):
        launch = dict(env, WAYLAND_DISPLAY=str(self.parent_socket))
        if enabled:
            launch['WINDOW_ZONES_COSMIC_BRIDGE'] = '1'
        child, log = self.start([self.args.cosmic_binary, '--no-xwayland'], phase + '-compositor', launch)
        runtime = Path(env['XDG_RUNTIME_DIR'])
        def socket_path():
            if child.poll() is not None:
                raise Failure('Nested COSMIC exited during startup: ' + text(log))
            matches = [path for path in runtime.iterdir() if path.name.startswith('wayland-') and stat.S_ISSOCK(path.stat().st_mode)]
            return matches[0] if len(matches) == 1 else None
        path = until(socket_path, 'nested compositor socket', self.args.startup_timeout)
        return child, dict(env, WAYLAND_DISPLAY=str(path)), path

    def bridge(self, env):
        from gi.repository import Gio
        self.connection = Gio.DBusConnection.new_for_address_sync(env['DBUS_SESSION_BUS_ADDRESS'],
            Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION, None, None)
        owner = self.connection.call_sync('org.freedesktop.DBus', '/org/freedesktop/DBus',
            'org.freedesktop.DBus', 'GetNameOwner', self.variant('(s)', ('org.window_zones.Cosmic',)),
            None, Gio.DBusCallFlags.NONE, 3000, None).unpack()[0]
        self.bus_owner = owner
        return self.call('GetSession')[0]

    @staticmethod
    def variant(signature, values):
        from gi.repository import GLib
        return GLib.Variant(signature, values)

    def call(self, method, signature=None, values=()):
        from gi.repository import Gio
        result = self.connection.call_sync(self.bus_owner, '/org/window_zones/Cosmic',
            'org.window_zones.Cosmic1', method, self.variant(signature, values) if signature else None,
            None, Gio.DBusCallFlags.NONE, 3000, None).unpack()
        self.event('bridge-observation' if method != 'MoveWindow' else 'bridge-error-probe', method=method, result=result)
        return result

    @staticmethod
    def fixture_state(log):
        lines = [line[len('WINDOW_STATE '):] for line in text(log).splitlines() if line.startswith('WINDOW_STATE ')]
        return json.loads(lines[-1]) if lines else None

    def observe(self, label, expected, fixture_log, active=True, title='Window Zones test: COSMIC'):
        def matches():
            frame = self.observer.frame(title)
            state = self.fixture_state(fixture_log)
            return frame == expected and state and state[:2] == expected[2:] and state[2] == active
        until(matches, label + ' public geometry and GTK committed size', 8, self.observer.pump)
        self.check(label + ' independently observed frame', True,
                   {'expected': expected, 'cosmic_toplevel_info': self.observer.frame(title),
                    'native_gtk': self.fixture_state(fixture_log)})
        self.observer.capture(self.work / (label + '.png'))
        self.check(label + ' native capture', True)

    def dispatch(self, env, key, label, expected, fixture_log, backend='auto', explicit=True):
        _, output = self.cli(env, label, ['dispatch', 'Ctrl+Alt+' + key], backend, explicit)
        self.check(label + ' completed response', 'Dispatch state: Succeeded' in output, output)
        self.observe(label, expected, fixture_log)

    def runtime(self, env, mode, label=None):
        label = label or mode
        argv = [self.args.binary, '--backend', 'auto' if mode == 'run' else 'wayland',
                '--perf-log', self.work / (label + '-perf.jsonl')]
        if mode == 'run':
            argv += ['--config', self.config]
        argv += [mode]
        child, log = self.start(argv, label, env, stdin=True)
        ready = 'Interactive session started' if mode == 'run' else 'Window Zones TUI'
        until(lambda: ready in text(log) or (child.poll() is not None and self.runtime_failed(log)),
              mode + ' ready', 12, self.observer.pump)
        self.check(mode + ' uses COSMIC', 'Window backend: cosmic-wayland' in text(log), text(log))
        return child, log

    @staticmethod
    def runtime_failed(log):
        raise Failure('Window Zones runtime exited: ' + text(log))

    def run(self):
        self.check('parent Wayland socket exists', self.parent_socket is not None and self.parent_socket.is_absolute()
                   and self.parent_socket.exists() and stat.S_ISSOCK(self.parent_socket.stat().st_mode), str(self.parent_socket))
        for binary in (self.args.binary, self.args.cosmic_binary):
            self.check('executable exists: ' + str(binary), binary.is_file() and os.access(binary, os.X_OK))
        import gi
        gi.require_version('Gtk', '4.0')
        from gi.repository import Gtk  # Import only: the driver never opens a host display.
        self.check('GTK4 fixture dependency', Gtk.get_major_version() == 4)
        self.check('libxkbcommon available', ctypes.util.find_library('xkbcommon') is not None)
        fd = os.pidfd_open(os.getpid())
        os.close(fd)
        metadata = {'binary': str(self.args.binary), 'cosmic_binary': str(self.args.cosmic_binary),
                    'binary_sha256': hashlib.sha256(self.args.binary.read_bytes()).hexdigest(),
                    'cosmic_sha256': hashlib.sha256(self.args.cosmic_binary.read_bytes()).hexdigest(),
                    'parent_wayland': str(self.parent_socket), 'private_root': str(self.private),
                    'owner': self.owner, 'argv': sys.argv,
                    'limits': ['one nested output, scale 1', 'native undecorated GTK4 client; no Xwayland',
                               'not a seated desktop/physical keyboard release gate',
                               'captures preserve raw buffer orientation; transform sidecars retained']}
        stamp = self.args.cosmic_binary.with_name(self.args.cosmic_binary.name + '.window-zones.json')
        self.check('custom compositor has source provenance', stamp.is_file(),
                   'Build the current bridge with scripts/build-cosmic.sh')
        build = json.loads(stamp.read_text())
        overlay = ROOT / 'cosmic-comp'
        inputs = {name: hashlib.sha256((overlay / name).read_bytes()).hexdigest()
                  for name in ('revision', 'bridge.patch', 'window_zones.rs')}
        self.check('custom compositor matches current bridge sources',
                   build.get('revision') == (overlay / 'revision').read_text().strip()
                   and build.get('inputs') == inputs
                   and build.get('binary_sha256') == metadata['cosmic_sha256'],
                   {'stamp': str(stamp), 'rebuild': 'scripts/build-cosmic.sh'})
        metadata['cosmic_build'] = build
        (self.work / 'launch.json').write_text(json.dumps(metadata, indent=2) + '\n')
        # Verify the unmodified behavior of the opt-in binary first, on a private bus.
        env, bus = self.environment('disabled')
        comp, disabled, _ = self.compositor(env, 'disabled', False)
        for backend in ('auto', 'wayland'):
            _, output = self.cli(disabled, 'missing-bridge-' + backend, ['status'], backend, success=False)
            self.check('missing bridge ' + backend + ' is explicit',
                       'COSMIC companion status:' in output and 'COSMIC companion capabilities:' not in output
                       and 'Using runtime window backend: cosmic-wayland' in output, output)
        self.stop(comp)
        self.stop(bus)
        env, _ = self.environment('enabled')
        # Install a REAL compositor custom-command shortcut, before starting it.
        # This is a fresh private config, never a replacement for host shortcuts.
        command = shlex.join([str(self.args.binary), '--backend', 'wayland', '--config', str(self.config),
                              'dispatch', 'Ctrl+Alt+Right']) + ' > ' + shlex.quote(str(self.work / 'native-hotkey-client.log')) + ' 2>&1'
        shortcut = Path(env['XDG_CONFIG_HOME']) / 'cosmic/com.system76.CosmicSettings.Shortcuts/v1/custom'
        shortcut.parent.mkdir(parents=True, exist_ok=True)
        shortcut.write_text('{\n (modifiers: [Ctrl, Alt], key: "Right", description: Some("Window Zones smoke")):\n  Spawn(' + json.dumps(command) + '),\n}\n')
        (self.work / 'custom-shortcuts.ron').write_bytes(shortcut.read_bytes())
        comp_config = Path(env['XDG_CONFIG_HOME']) / 'cosmic/com.system76.CosmicComp/v1'
        comp_config.mkdir(parents=True, exist_ok=True)
        (comp_config / 'autotile').write_text('true\n')
        comp, env, nested_socket = self.compositor(env, 'enabled', True)
        # Bus ownership may be registered after the listening socket. Bounded, no placement retries.
        def bridge_ready():
            try:
                return self.bridge(env)
            except Exception as error:
                from gi.repository import GLib
                if isinstance(error, GLib.Error):
                    return None
                raise
        session = until(bridge_ready, 'opt-in bridge ownership', self.args.startup_timeout)
        session_path = Path(session)
        self.check('bridge session is this private compositor', session_path.is_absolute()
                   and session_path.stat().st_ino == nested_socket.stat().st_ino
                   and session_path.stat().st_dev == nested_socket.stat().st_dev, session)
        self.check('bridge capability contract', set(self.call('GetCapabilities')[0]) == {'focused-window', 'displays', 'move-resize'})
        self.observer = Wayland(nested_socket, self.work / 'toplevel-observer.jsonl')
        self.check('single independent nested output', len(self.observer.outputs) == 1, self.observer.outputs)
        output = next(iter(self.observer.outputs.values()))
        until(lambda: output.get('size') and output.get('origin') is not None and output.get('name'),
              'independent xdg-output layout', 5, self.observer.pump)
        self.check('nested output origin', output['origin'] == [0, 0], output)
        displays = self.call('GetDisplays')[0]
        width, height = output['size']
        self.check('authoritative usable area agrees with panel-free public output',
                   [list(display) for display in displays] == [[output['name'], 0, 0, width, height]], displays)
        area = [0, 0, width, height]
        left = [0, 0, width // 2, height]
        right = [width // 2, 0, width // 2, height]
        for backend, explicit in (('auto', True), ('wayland', False)):
            _, status = self.cli(env, 'status-' + backend, ['status'], backend, explicit)
            self.check('status ' + backend + ' capabilities/config', 'Using runtime window backend: cosmic-wayland' in status
                       and 'COSMIC companion capabilities:' in status and 'config state: Loaded' in status
                       and str(self.config) in status, status)
        fixture, fixture_log = self.start([sys.executable, ROOT / 'scripts/gnome-smoke.py', '--test-window', 'COSMIC'],
                                          'native-gtk-window', env, stdin=True)
        until(lambda: self.observer.window('Window Zones test: COSMIC') and
              self.observer.frame('Window Zones test: COSMIC') and
              self.fixture_state(fixture_log) and self.fixture_state(fixture_log)[2],
              'native GTK window mapped and focused', 15, self.observer.pump)
        baseline = self.observer.frame('Window Zones test: COSMIC')[:]
        self.observe('initial-native', baseline, fixture_log)
        wrong_path = self.private / 'wrong-wayland.sock'
        wrong_socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            wrong_socket.bind(str(wrong_path))
            wrong_env = dict(env, WAYLAND_DISPLAY=str(wrong_path))
            for backend in ('auto', 'wayland'):
                _, mismatch = self.cli(wrong_env, 'wrong-socket-' + backend, ['status'],
                                       backend=backend, success=False)
                self.check('wrong socket ' + backend + ' bridge session mismatch',
                           'COSMIC companion status:' in mismatch
                           and str(wrong_path) in mismatch and str(nested_socket) in mismatch
                           and 'COSMIC companion capabilities:' not in mismatch, mismatch)
            self.observer.roundtrip()
            self.observe('wrong-socket-does-not-move', baseline, fixture_log)
        finally:
            wrong_socket.close()
            wrong_path.unlink(missing_ok=True)
        keyboard = self.observer.keyboard()
        run, _ = self.runtime(env, 'run')
        self.dispatch(env, 'Left', 'run-left-half', left, fixture_log)
        centered = [(width - left[2]) // 2, (height - left[3]) // 2, *left[2:]]
        self.dispatch(env, 'C', 'run-center', centered, fixture_log, 'wayland', False)
        self.dispatch(env, 'Backspace', 'run-restore-center', baseline, fixture_log)
        # Real custom shortcut starts a real CLI dispatch client bound to this runtime.
        before = len(text(self.work / 'run-perf.jsonl'))
        self.event('native-keyboard', protocol='zwp_virtual_keyboard_v1', chord='Ctrl+Alt+Right')
        self.observer.shortcut(keyboard)
        native_log = self.work / 'native-hotkey-client.log'
        until(lambda: 'Dispatch state: Succeeded' in text(native_log), 'COSMIC custom-command dispatch response', 8, self.observer.pump)
        def native_receipt():
            data = text(self.work / 'run-perf.jsonl')[before:]
            if not data.endswith('\n'):
                return None
            rows = [json.loads(line) for line in data.splitlines()]
            return next((row for row in rows if row.get('hotkey') == 'alt+ctrl+right'
                         and row.get('result') == 'ok'), None)
        receipt = until(native_receipt, 'matching native dispatch runtime perf receipt', 5, self.observer.pump)
        self.check('native accelerator reaches persistent runtime', True, receipt)
        self.observe('native-hotkey-right-half', right, fixture_log)
        _, partial = self.cli(env, 'constrained-placement', ['dispatch', 'Ctrl+Alt+Down'], success=False)
        self.check('constrained placement reports failure', 'Dispatch state: Error' in partial, partial)
        def constrained_frame():
            frame = self.observer.frame('Window Zones test: COSMIC')
            state = self.fixture_state(fixture_log)
            return frame if frame and state and state[:2] == frame[2:] else None
        partial_frame = until(constrained_frame, 'committed constrained frame', 5, self.observer.pump)
        self.check('constrained placement exposes actual partial frame',
                   partial_frame != right and partial_frame[2] > width // 100
                   and partial_frame[3] > height // 100, partial_frame)
        self.observer.capture(self.work / 'constrained-placement.png')
        self.dispatch(env, 'Backspace', 'restore-after-partial', baseline, fixture_log)
        # Native state requests are fixture user controls, not compositor test endpoints.
        self.send(fixture, 'maximize')
        until(lambda: self.fixture_state(fixture_log) and self.fixture_state(fixture_log)[3],
              'fixture maximized commit', 8, self.observer.pump)
        self.dispatch(env, 'Left', 'maximized-to-left-half', left, fixture_log)
        self.send(fixture, 'fullscreen')
        until(lambda: self.fixture_state(fixture_log) and self.fixture_state(fixture_log)[4],
              'fixture fullscreen commit', 8, self.observer.pump)
        until(lambda: 3 in self.observer.window('Window Zones test: COSMIC')['states'],
              'public fullscreen state', 5, self.observer.pump)
        full_before = self.observer.frame('Window Zones test: COSMIC')[:]
        _, rejected = self.cli(env, 'fullscreen-rejection', ['dispatch', 'Ctrl+Alt+Left'], success=False)
        self.check('fullscreen dispatch explicitly rejected', 'Dispatch state: Error' in rejected, rejected)
        self.observe('fullscreen-rejection-unchanged', full_before, fixture_log)
        self.send(fixture, 'restore')
        until(lambda: self.fixture_state(fixture_log) and not self.fixture_state(fixture_log)[4],
              'fixture exits fullscreen', 8, self.observer.pump)
        self.dispatch(env, 'Left', 'post-fullscreen-left', left, fixture_log)
        fixed, fixed_log = self.start(
            [sys.executable, ROOT / 'scripts/gnome-smoke.py',
             '--test-window', 'COSMIC fixed', '--test-window-fixed'],
            'fixed-native-gtk-window', env, stdin=True)
        fixed_title = 'Window Zones test: COSMIC fixed'
        def fixed_frame():
            frame = self.observer.frame(fixed_title)
            state = self.fixture_state(fixed_log)
            return frame if frame and state and state[2] and not state[5] and state[:2] == frame[2:] else None
        fixed_before = until(fixed_frame, 'native fixture fixed committed frame', 8, self.observer.pump)[:]
        self.observe('fixed-size-precondition', fixed_before, fixed_log, title=fixed_title)
        # Probe dimensions different from the fixed-size frame through the production API.
        # The public geometry plus GTK state must remain unchanged for rejection.
        self.observer.roundtrip()
        present, window_id, *_ = self.call('GetFocusedWindow')
        self.check('constraint probe captures identified native window', present and window_id != 0)
        constrained = self.call('MoveWindow', '(tiiuu)', (window_id, 10, 10, max(100, left[2] - 80), max(100, height - 80)))
        self.check('fixed-size move rejected before changes', constrained[0] == 2, constrained)
        self.observe('fixed-size-rejection-unchanged', fixed_before, fixed_log, title=fixed_title)
        self.stop(fixed, graceful=True)
        until(lambda: self.observer.window(fixed_title) is None and self.fixture_state(fixture_log)[2],
              'fixed-size fixture closes and original regains focus', 8, self.observer.pump)
        self.observe('original-window-unchanged-by-fixed-probe', left, fixture_log)
        self.stop(run, graceful=True)
        tui, tui_log = self.runtime(env, 'tui')
        self.dispatch(env, 'Right', 'tui-right-half', right, fixture_log, 'wayland', False)
        self.dispatch(env, 'Backspace', 'tui-restore', left, fixture_log, 'wayland', False)
        def completed_tui_frame():
            frame = text(tui_log).split('\x1b[2J\x1b[H')[-1]
            return frame if 'Last action: alt+ctrl+backspace\n' in frame and 'Last error: none\n' in frame else None
        rendered = until(completed_tui_frame, 'TUI renders completed Restore without errors', 5, self.observer.pump)
        self.check('TUI rendered real dispatch', True, rendered)
        self.stop(tui, graceful=True)
        present, captured_id, *_ = self.call('GetFocusedWindow')
        self.check('identity probe captures original window', present and captured_id != 0)
        other, other_log = self.start(
            [sys.executable, ROOT / 'scripts/gnome-smoke.py', '--test-window', 'COSMIC other'],
            'other-native-gtk-window', env, stdin=True)
        other_title = 'Window Zones test: COSMIC other'
        until(lambda: self.observer.frame(other_title) and self.fixture_state(other_log)
              and self.fixture_state(other_log)[2] and not self.fixture_state(fixture_log)[2],
              'second owned native window has focus', 12, self.observer.pump)
        other_before = self.observer.frame(other_title)[:]
        present, other_id, *_ = self.call('GetFocusedWindow')
        self.check('identity probe focus changed', present and other_id != captured_id)
        identified = self.call('MoveWindow', '(tiiuu)', (captured_id, *right))
        self.check('captured inactive identity completes', identified[0] == 0, identified)
        self.observe('captured-inactive-window', right, fixture_log, active=False)
        present, focused_id, *_ = self.call('GetFocusedWindow')
        self.check('identified move preserves other window and focus',
                   present and focused_id == other_id
                   and self.observer.frame(other_title) == other_before
                   and self.fixture_state(other_log)[:2] == other_before[2:], other_before)
        self.stop(other, graceful=True)
        until(lambda: self.fixture_state(fixture_log)[2], 'focus returns to original window',
              8, self.observer.pump)
        # Fresh status proves config persistence but must not invent persisted history.
        _, fresh = self.cli(env, 'status-after-runtimes', ['status'], explicit=False)
        self.check('config is unchanged and rediscovered', self.config.read_bytes() == self.original_config
                   and 'config state: Loaded' in fresh, fresh)
        self.check('fresh status does not persist dispatch history',
                   'last action: <none>' in fresh and 'dispatch state: Idle' in fresh, fresh)
        present, closed_id, *_ = self.call('GetFocusedWindow')
        self.check('closing probe captures stable window id', present and closed_id != 0)
        self.stop(fixture, graceful=True)
        until(lambda: self.observer.window('Window Zones test: COSMIC') is None,
              'public toplevel closure', 8, self.observer.pump)
        closed = self.call('MoveWindow', '(tiiuu)', (closed_id, *left))
        self.check('closed identified window reports WINDOW_GONE', closed[0] == 1, closed)
        empty_run, _ = self.runtime(env, 'run', 'empty-run')
        _, no_focus = self.cli(env, 'no-focused-window', ['dispatch', 'Ctrl+Alt+Left'], success=False)
        self.check('no focused window dispatch is explicit', 'Dispatch state: Error' in no_focus and 'no focused window' in no_focus.lower(), no_focus)
        self.stop(empty_run, graceful=True)
        self.check('all smoke scenarios completed', True, {'usable_area': area, 'initial_frame': baseline})

    def finish(self):
        cleanup_ok = False
        try:
            if self.observer:
                self.observer.close()
            owned_process.cleanup(self.registry, self.owner, os.getpid())
            for child in self.children:
                child.wait(timeout=5)
            cleanup_ok = True
            self.check('owned process cleanup', True)
        except BaseException as error:
            self.checks.append({'name': 'owned process cleanup', 'passed': False, 'detail': str(error)})
        finally:
            for stream in self.streams:
                stream.close()
            self.actions.close()
            # Keep config/shortcuts with evidence, never remove scratch after failed cleanup.
            shutil.copytree(self.private / 'config', self.work / 'private-config')
            if cleanup_ok:
                shutil.rmtree(self.private)
            (self.work / 'results.json').write_text(json.dumps({'passed': all(row['passed'] for row in self.checks),
                'checks': self.checks, 'failures': sum(not row['passed'] for row in self.checks),
                'cleanup_complete': cleanup_ok}, indent=2) + '\n')
            print('COSMIC evidence:', self.work)
            count = len(self.checks)
            failed = sum(not row['passed'] for row in self.checks)
            print(f'Smoke assertions (cosmic): {count}; passed: {count - failed}; failed: {failed}; skipped: 0')
        return cleanup_ok


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/debug/window_zones')
    parser.add_argument('--compositor', '--cosmic-binary', dest='cosmic_binary', type=Path,
                        default=Path(os.environ.get('WINDOW_ZONES_COSMIC_COMPOSITOR',
                                                    str(ROOT / 'target/cosmic-comp-build/debug/cosmic-comp'))))
    parser.add_argument('--parent-wayland', type=Path, help='Absolute parent socket; defaults to the current WAYLAND_DISPLAY')
    parser.add_argument('--evidence-dir', '--output-dir', dest='output_dir', type=Path,
                        help='New evidence directory (default target/verification/cosmic-TIMESTAMP-ID)')
    parser.add_argument('--startup-timeout', type=float, default=45)
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    args.cosmic_binary = args.cosmic_binary.resolve()
    if args.startup_timeout <= 0 or args.startup_timeout > 120:
        parser.error('--startup-timeout must be in (0, 120] seconds')
    smoke = Smoke(args)
    def interrupted(signum, _frame):
        raise Failure('Interrupted by signal ' + str(signum))
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    success = False
    try:
        smoke.run()
        success = True
    except BaseException as error:
        smoke.checks.append({'name': 'smoke execution', 'passed': False, 'detail': str(error)})
        (smoke.work / 'failure.txt').write_text(traceback.format_exc())
        print(str(error), file=sys.stderr)
    finally:
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        cleanup = smoke.finish()
    return 0 if success and cleanup else 1


if __name__ == '__main__':
    sys.exit(main())
