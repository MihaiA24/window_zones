#!/usr/bin/env python3
"""Exercise native GNOME windows/input and observe authorized desktop sessions."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import uuid

# importlib-based verifier regressions do not put scripts/ on sys.path.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from smoke import owned_process


INDICATOR_UUID = 'appindicatorsupport@rgcjonas.gmail.com'
RESOURCE_VARIABLES = ('WINDOW_ZONES_SMOKE_GNOME_DATA_DIRS',
                      'WINDOW_ZONES_SMOKE_PIPEWIRE_RUNTIME_DIR',
                      'WINDOW_ZONES_SMOKE_PIPEWIRE_REMOTE')


def resource_sources(environment):
    encoded_roots = environment.get(RESOURCE_VARIABLES[0])
    if encoded_roots is None:
        data_home = Path(environment.get('XDG_DATA_HOME') or
                         Path(environment.get('HOME') or Path.home()) / '.local/share')
        if not data_home.is_absolute():
            data_home = Path(environment.get('HOME') or Path.home()) / '.local/share'
        roots = [str(data_home), *(value for value in
                                  (environment.get('XDG_DATA_DIRS') or '/usr/local/share:/usr/share').split(os.pathsep)
                                  if value and Path(value).is_absolute())]
    else:
        roots = json.loads(encoded_roots)
    if not isinstance(roots, list) or not roots or any(not isinstance(value, str) or
                                                      not Path(value).is_absolute() for value in roots):
        raise RuntimeError('AppIndicator resource roots must be a nonempty list of absolute paths')
    runtime = environment.get(RESOURCE_VARIABLES[1],
                              environment.get('PIPEWIRE_RUNTIME_DIR') or environment.get('XDG_RUNTIME_DIR', ''))
    remote = environment.get(RESOURCE_VARIABLES[2], environment.get('PIPEWIRE_REMOTE') or 'pipewire-0')
    if not runtime or not Path(runtime).is_absolute() or not remote:
        raise RuntimeError('PipeWire capture requires the original absolute runtime directory and remote socket name')
    socket_path = Path(runtime) / remote
    if not socket_path.is_socket():
        raise RuntimeError('PipeWire socket is unavailable: ' + str(socket_path))
    return list(dict.fromkeys(map(Path, roots))), Path(runtime), remote


def find_appindicator(roots, shell_major):
    diagnostics = []
    for base in roots:
        extension = base / 'gnome-shell/extensions' / INDICATOR_UUID
        try:
            metadata = json.loads((extension / 'metadata.json').read_text())
            versions = metadata.get('shell-version', [])
            if (metadata.get('uuid') == INDICATOR_UUID and isinstance(versions, list) and
                    str(shell_major) in versions and (extension / 'extension.js').is_file()):
                return extension
            diagnostics.append(str(extension) + ': supported Shell versions ' + str(versions))
        except (OSError, ValueError, AttributeError) as error:
            diagnostics.append(str(extension) + ': ' + str(error))
    raise RuntimeError('No installed AppIndicator compatible with GNOME ' + str(shell_major) +
                       '; searched ' + '; '.join(diagnostics))


def isolated_environment(work, pipewire_runtime, pipewire_remote, source):
    environment = dict(source)
    for name in ('DISPLAY', 'WAYLAND_DISPLAY', 'WAYLAND_SOCKET', 'XAUTHORITY', 'SSH_CONNECTION',
                 'SWAYSOCK', 'HYPRLAND_INSTANCE_SIGNATURE', 'KDE_FULL_SESSION', 'KDE_SESSION_VERSION',
                 'GNOME_DESKTOP_SESSION_ID', 'XDG_SESSION_DESKTOP', 'DESKTOP_SESSION',
                 'DBUS_SESSION_BUS_ADDRESS', 'DBUS_STARTER_ADDRESS', 'DBUS_STARTER_BUS_TYPE',
                 'DCONF_PROFILE', *RESOURCE_VARIABLES):
        environment.pop(name, None)
    for name, directory in (('HOME', 'home'), ('XDG_CONFIG_HOME', 'config'), ('XDG_DATA_HOME', 'data'),
                            ('XDG_STATE_HOME', 'state'), ('XDG_CACHE_HOME', 'cache'),
                            ('XDG_RUNTIME_DIR', 'runtime')):
        environment[name] = str(work / directory)
    environment.update(XDG_DATA_DIRS='/usr/local/share:/usr/share', GSETTINGS_BACKEND='keyfile',
                       XDG_CURRENT_DESKTOP='GNOME', XDG_SESSION_TYPE='wayland',
                       WAYLAND_DISPLAY='window-zones-native-smoke', GDK_BACKEND='wayland', GTK_A11Y='none',
                       PIPEWIRE_RUNTIME_DIR=str(pipewire_runtime), PIPEWIRE_REMOTE=pipewire_remote)
    return environment


def completed_tui_frame(output, *, first=False):
    output = re.sub(r'\x1b\[[0-?]*[ -/]*[@-~]', '', output).replace('\r', '')
    if first:
        before, prompt, _after = output.partition('window-zones tui> ')
        output = before + prompt
    if 'Window Zones TUI' not in output:
        return ''
    frame = 'Window Zones TUI' + output.rsplit('Window Zones TUI', 1)[1]
    return frame if re.search(r'^Last action: [^\n]+\nLast error: [^\n]*\nLast event: [^\n]*\n'
                              r'[\s\S]*window-zones tui> $', frame, re.M) else ''


def register_child(process, registry, owner):
    owned_process.register(registry, owner, os.getpid(), process.pid)
    # An already-reaped child retains its real status even when /proc is gone.
    if process.poll() is None and process.pid not in owned_process.records(registry):
        process.terminate()
        process.wait(timeout=5)
        raise RuntimeError('Cannot capture stable identity of live smoke child ' + str(process.pid))


def reduced_motion_supported(shell_major, schema):
    # Shell 50 predates St.ReducedMotion, even with newer desktop schemas installed.
    if shell_major < 51:
        return False
    if schema is None or not schema.has_key('reduced-motion'):
        raise RuntimeError('GNOME 51 requires org.gnome.desktop.a11y.interface reduced-motion')
    return True


def capture_endpoints(graph, source_id, source_pid, consumer_name, consumer_pid):
    nodes = [item for item in graph if item['type'] == 'PipeWire:Interface:Node']
    source = [item for item in nodes if item['id'] == source_id]
    consumer = [item for item in nodes
                if item.get('info', {}).get('props', {}).get('node.name') == consumer_name]
    if not source or not consumer:
        return None
    if len(source) != 1 or len(consumer) != 1:
        raise RuntimeError('Ambiguous private PipeWire capture nodes')
    clients = {item['id']: item for item in graph if item['type'] == 'PipeWire:Interface:Client'}
    for node, pid in ((source[0], source_pid), (consumer[0], consumer_pid)):
        client = clients.get(node['info']['props'].get('client.id'))
        if client is None:
            return None
        props = client['info']['props']
        if str(props.get('pipewire.sec.pid', props.get('application.process.id'))) != str(pid):
            raise RuntimeError('PipeWire capture node does not belong to its owned process')
    ports = []
    for node, direction in ((source[0], 'output'), (consumer[0], 'input')):
        matches = [item for item in graph if item['type'] == 'PipeWire:Interface:Port'
                   and item.get('info', {}).get('props', {}).get('node.id') == node['id']
                   and item['info'].get('direction') == direction]
        if not matches:
            return None
        if len(matches) != 1:
            raise RuntimeError('Ambiguous private PipeWire capture ports')
        ports.append(matches[0]['id'])
    return source_id, consumer[0]['id'], *ports


def monitor_rectangles(state):
    layout = state[3].get('layout-mode')
    if layout not in (1, 2):
        raise RuntimeError('Unknown Mutter monitor coordinate layout')
    modes = {tuple(spec): next((mode for mode in available if mode[-1].get('is-current')), None)
             for spec, available, _properties in state[1]}
    rectangles = []
    for x, y, scale, transform, primary, specs, _properties in state[2]:
        mode = modes.get(tuple(specs[0]))
        if mode is None or scale <= 0:
            raise RuntimeError('Logical monitor has no current physical mode')
        width, height = mode[1:3]
        if transform in (1, 3, 5, 7):
            width, height = height, width
        divisor = scale if layout == 1 else 1
        rectangles.append(dict(x=x, y=y, width=round(width / divisor), height=round(height / divisor),
                               primary=primary, connector=specs[0][0]))
    if not rectangles:
        raise RuntimeError('Mutter reports no logical monitors')
    return rectangles


def xwayland_transform(rectangles, size):
    # Use independent desktop metadata, never the window/target being checked.
    origin_x = min(rectangle['x'] for rectangle in rectangles)
    origin_y = min(rectangle['y'] for rectangle in rectangles)
    width = max(rectangle['x'] + rectangle['width'] for rectangle in rectangles) - origin_x
    height = max(rectangle['y'] + rectangle['height'] for rectangle in rectangles) - origin_y
    if (width <= 0 or height <= 0 or size[0] % width or size[1] % height
            or size[0] // width != size[1] // height or size[0] // width < 1):
        raise RuntimeError('Xwayland desktop lacks a uniform integral logical scale: '
                           + str(size) + ' versus ' + str((origin_x, origin_y, width, height)))
    return size[0] // width, origin_x, origin_y


def observe_xwayland(record):
    from gi.repository import Gio
    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    state = bus.call_sync('org.gnome.Mutter.DisplayConfig', '/org/gnome/Mutter/DisplayConfig',
                          'org.gnome.Mutter.DisplayConfig', 'GetCurrentState', None, None,
                          Gio.DBusCallFlags.NONE, 3000, None).unpack()
    rectangles = monitor_rectangles(state)
    size = tuple(map(int, subprocess.check_output(
        ['xdotool', 'getdisplaygeometry'], text=True, timeout=3).split()))
    transform = xwayland_transform(rectangles, size)
    record.write_text(json.dumps(dict(monitors=rectangles, x11_size=size, transform=transform),
                                 indent=2) + '\n')
    print(*transform)


def capture_current_monitor(output, window_id=None):
    from gi.repository import Gio, GLib
    if output.exists():
        raise FileExistsError('Refusing to overwrite capture evidence: ' + str(output))
    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    def call(destination, path, interface, method, params=None):
        return bus.call_sync(destination, path, interface, method, params, None,
                             Gio.DBusCallFlags.NONE, 3000, None).unpack()
    state = call('org.gnome.Mutter.DisplayConfig', '/org/gnome/Mutter/DisplayConfig',
                 'org.gnome.Mutter.DisplayConfig', 'GetCurrentState')
    rectangles = monitor_rectangles(state)
    if window_id is None:
        selected = next((rectangle for rectangle in rectangles if rectangle['primary']), None)
    else:
        size = tuple(map(int, subprocess.check_output(
            ['xdotool', 'getdisplaygeometry'], text=True, timeout=3).split()))
        scale, origin_x, origin_y = xwayland_transform(rectangles, size)
        fields = dict(line.split('=', 1) for line in subprocess.check_output(
            ['xdotool', 'getwindowgeometry', '--shell', str(window_id)],
            text=True, timeout=3).splitlines())
        center_x = origin_x + (int(fields['X']) + int(fields['WIDTH']) / 2) / scale
        center_y = origin_y + (int(fields['Y']) + int(fields['HEIGHT']) / 2) / scale
        selected = next((rectangle for rectangle in rectangles
                         if rectangle['x'] <= center_x < rectangle['x'] + rectangle['width']
                         and rectangle['y'] <= center_y < rectangle['y'] + rectangle['height']), None)
    if selected is None:
        raise RuntimeError('No compositor monitor contains the capture target')
    registry = output.with_suffix('.capture-pids')
    registry.open('x').close()
    owner = str(output) + '-capture-' + uuid.uuid4().hex
    session, consumer = None, None
    destination = 'org.gnome.Mutter.ScreenCast'
    try:
        session = call(destination, '/org/gnome/Mutter/ScreenCast', destination,
                       'CreateSession', GLib.Variant('(a{sv})', ({},)))[0]
        stream = call(destination, session, destination + '.Session', 'RecordMonitor',
                      GLib.Variant('(sa{sv})', (selected['connector'], {})))[0]
        nodes = []
        bus.signal_subscribe(destination, destination + '.Stream', 'PipeWireStreamAdded', stream,
                             None, Gio.DBusSignalFlags.NONE,
                             lambda _c, _s, _p, _i, _n, value: nodes.append(value.unpack()[0]))
        source_pid = call('org.freedesktop.DBus', '/org/freedesktop/DBus',
                          'org.freedesktop.DBus', 'GetConnectionUnixProcessID',
                          GLib.Variant('(s)', (destination,)))[0]
        call(destination, session, destination + '.Session', 'Start')
        deadline = time.monotonic() + 15
        while not nodes and time.monotonic() < deadline:
            while GLib.MainContext.default().pending():
                GLib.MainContext.default().iteration(False)
            time.sleep(0.05)
        if not nodes:
            raise RuntimeError('No PipeWire stream from the seated compositor')
        name = 'window-zones-seated-capture-' + uuid.uuid4().hex
        log_path = output.with_suffix('.capture.log')
        pending_signal = None
        def defer_termination(signum, _frame):
            nonlocal pending_signal
            pending_signal = signum
        previous = {signum: signal.signal(signum, defer_termination)
                    for signum in (signal.SIGINT, signal.SIGTERM)}
        try:
            with log_path.open('w') as log:
                consumer = subprocess.Popen([
                    'gst-launch-1.0', '--quiet', 'pipewiresrc', 'path=' + str(nodes[0]),
                    'autoconnect=false', 'stream-properties=props,node.name=(string)' + name,
                    'num-buffers=1', '!', 'videoconvert', '!', 'pngenc', '!', 'filesink',
                    'location=' + str(output)],
                    env=dict(os.environ, WINDOW_ZONES_SMOKE_PRIVATE_OWNER=owner),
                    stdout=log, stderr=subprocess.STDOUT)
                register_child(consumer, registry, owner)
        finally:
            for signum, handler in previous.items():
                signal.signal(signum, handler)
            if pending_signal:
                os.kill(os.getpid(), pending_signal)
        endpoints = None
        while time.monotonic() < deadline:
            if consumer.poll() is not None:
                raise RuntimeError('Capture exited before linking; see ' + str(log_path))
            graph = json.loads(subprocess.check_output(
                ['pw-dump'], text=True, timeout=max(0.01, min(3, deadline - time.monotonic()))))
            endpoints = capture_endpoints(graph, nodes[0], source_pid, name, consumer.pid)
            if endpoints is not None:
                break
            time.sleep(0.05)
        if endpoints is None:
            raise RuntimeError('Owned seated PipeWire ports did not appear; see ' + str(log_path))
        output.with_suffix('.capture.json').write_text(json.dumps(
            dict(monitor=selected, window_id=window_id, source_pid=source_pid,
                 consumer_pid=consumer.pid, endpoints=endpoints), indent=2) + '\n')
        subprocess.run(['pw-link', '-w', str(endpoints[2]), str(endpoints[3])],
                       check=True, timeout=max(0.01, min(3, deadline - time.monotonic())))
        consumer.wait(timeout=max(0.01, deadline - time.monotonic()))
        if consumer.returncode or not output.is_file() or output.read_bytes()[:8] != b'\x89PNG\r\n\x1a\n':
            raise RuntimeError('No completed compositor PNG; see ' + str(log_path))
        print('Mutter ScreenCast monitor ' + selected['connector'] + ': ' + str(output))
    finally:
        try:
            if session:
                call(destination, session, destination + '.Session', 'Stop')
        finally:
            owned_process.cleanup(registry, owner, os.getpid())
            if consumer is not None:
                consumer.wait(timeout=5)


def test_window(title):
    import gi
    gi.require_version('Gtk', '4.0')
    gi.require_version('Gdk', '4.0')
    from gi.repository import Gdk, GLib, Gtk
    Gtk.init()
    window = Gtk.Window(title='Window Zones test: ' + title)
    window.set_default_size(720, 480)
    window.set_decorated(False)
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=16)
    for setter in (box.set_margin_top, box.set_margin_bottom, box.set_margin_start, box.set_margin_end):
        setter(24)
    box.append(Gtk.Label(label='Window Zones — real interactive ' + title + ' window', wrap=True))
    box.append(Gtk.Label(label='Native keyboard, mouse, zone, monitor, and window-state checks.', wrap=True))
    entry = Gtk.Entry(placeholder_text='Type here to verify real keyboard input')
    box.append(entry)
    entry.connect('changed', lambda widget: print('TEXT_CHANGED', widget.get_text(), flush=True))
    status = Gtk.Label(label='Waiting for input', wrap=True)
    button = Gtk.Button(label='Verify mouse interaction')
    button.connect('clicked', lambda _: (status.set_label('Mouse click received'), print('CLICK_RECEIVED', flush=True)))
    box.append(button)
    box.append(status)
    window.set_child(box)
    loop = GLib.MainLoop()
    window.connect('close-request', lambda _: (loop.quit(), False)[1])
    keys = Gtk.EventControllerKey()
    keys.set_propagation_phase(Gtk.PropagationPhase.CAPTURE)
    def key_pressed(_controller, keyval, keycode, modifiers):
        print('KEY_RECEIVED', Gdk.keyval_name(keyval), keycode, int(modifiers), flush=True)
        return False
    keys.connect('key-pressed', key_pressed)
    window.add_controller(keys)
    def command(_source, _condition):
        line = sys.stdin.readline().strip()
        if line == 'maximize':
            window.maximize()
        elif line == 'restore':
            window.unfullscreen()
            window.unmaximize()
            window.set_resizable(True)
            status.set_label('Resizing enabled')
        elif line == 'fullscreen':
            window.fullscreen()
        elif line == 'fixed':
            window.set_resizable(False)
            # Wayland size limits apply on surface commit; redraw this undecorated fixture.
            status.set_label('Resizing disabled')
        elif line == 'quit':
            loop.quit()
            return GLib.SOURCE_REMOVE
        return GLib.SOURCE_CONTINUE
    GLib.io_add_watch(sys.stdin.fileno(), GLib.IO_IN, command)
    last = None
    def geometry():
        nonlocal last
        surface = window.get_surface()
        tiled = bool(surface and surface.get_state() & Gdk.ToplevelState.TILED)
        current = (window.get_width(), window.get_height(), window.is_active(), window.is_maximized(), window.is_fullscreen(), window.get_resizable(), tiled)
        if current != last:
            print('WINDOW_STATE', json.dumps(current), flush=True)
            for name, widget in (('ENTRY_CENTER', entry), ('BUTTON_CENTER', button)):
                mapped, bounds = widget.compute_bounds(window)
                if mapped:
                    center = bounds.get_center()
                    print(name, json.dumps([center.x, center.y]), flush=True)
            last = current
        return GLib.SOURCE_CONTINUE
    GLib.timeout_add(50, geometry)
    window.present()
    print('TEST_WINDOW_READY', Gdk.Display.get_default().get_name(), flush=True)
    loop.run()


def run(binary, keep_logs, output_dir=None):
    root = Path(__file__).resolve().parent.parent
    if output_dir is None:
        work = Path(tempfile.mkdtemp(prefix='window-zones-native-gnome-'))
    else:
        work = output_dir.resolve()
        work.mkdir(parents=True, exist_ok=False)
    print('EVIDENCE', work, flush=True)
    processes = []
    registry = work / 'owned-pids'
    registry.touch()
    owner = str(work)
    checks = []
    remote_path = None
    connection = None
    runtime = None
    window = None
    def check(name, condition, detail=''):
        checks.append({'name': name, 'passed': bool(condition), 'detail': str(detail)})
        print(('PASS ' if condition else 'FAIL ') + name + ': ' + str(detail), flush=True)
        if not condition:
            raise AssertionError(name + ': ' + str(detail))
    def call(dest, path, interface, method, params=None):
        return connection.call_sync(dest, path, interface, method, params, None, Gio.DBusCallFlags.NONE, 3000, None)
    def companion(method, params=None):
        return call('org.window_zones.Gnome', '/org/window_zones/Gnome', 'org.window_zones.Gnome2', method, params).unpack()
    def focused():
        return companion('GetFocusedWindow')
    def set_overview(active):
        # Observe Shell state rather than assuming Overview transitions fit a key-event delay.
        destination, path = 'org.gnome.Shell', '/org/gnome/Shell'
        call(destination, path, 'org.freedesktop.DBus.Properties', 'Set',
             GLib.Variant('(ssv)', ('org.gnome.Shell', 'OverviewActive', GLib.Variant('b', active))))
        check('Overview ' + ('opens' if active else 'closes'),
              wait_for(lambda: call(destination, path, 'org.freedesktop.DBus.Properties', 'Get',
                                    GLib.Variant('(ss)', ('org.gnome.Shell', 'OverviewActive'))).unpack()[0] == active))
    def remote(method, params=None):
        return call('org.gnome.Mutter.RemoteDesktop', remote_path, 'org.gnome.Mutter.RemoteDesktop.Session', method, params)
    def wait_for(predicate, timeout=8):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            while GLib.MainContext.default().pending():
                GLib.MainContext.default().iteration(False)
            try:
                if predicate():
                    return True
            except GLib.Error:
                pass
            time.sleep(0.05)
        return False
    def start(args, name, child_env=None):
        log_path = work / (name + '.log')
        log = open(log_path, 'w')
        pending_signal = None
        def defer_termination(signum, _frame):
            nonlocal pending_signal
            pending_signal = signum
        previous = {signum: signal.signal(signum, defer_termination)
                    for signum in (signal.SIGINT, signal.SIGTERM)}
        try:
            child_env = dict(env if child_env is None else child_env, WINDOW_ZONES_SMOKE_PRIVATE_OWNER=owner)
            process = subprocess.Popen(args, env=child_env, stdin=subprocess.PIPE, stdout=log, stderr=subprocess.STDOUT, text=True, start_new_session=True)
            processes.append(process)
            register_child(process, registry, owner)
        finally:
            log.close()
            for signum, handler in previous.items():
                signal.signal(signum, handler)
            if pending_signal:
                os.kill(os.getpid(), pending_signal)
        return process, log_path
    def text(log):
        return log.read_text(errors='replace')
    def assert_tui_frame(name, log, checkpoint=0, *, action=None, binding=None, first=False):
        frame = ''
        def matches():
            if not frame or any(value not in frame for value in
                                ('Window backend: gnome-wayland\n', 'Hotkey state: Registered',
                                 'Last error: none\n', ' (Loaded)\n')):
                return False
            if action is not None and 'Last action: ' + action + '\n' not in frame:
                return False
            if binding is not None:
                hotkey, zone = binding
                return re.search(r'^  ' + re.escape(hotkey) + r' -> .*zone: "' +
                                 re.escape(zone) + r'"', frame, re.M) is not None
            return True
        def ready():
            nonlocal frame
            frame = completed_tui_frame(text(log)[checkpoint:], first=first)
            return bool(frame) and (first or matches())
        check(name, wait_for(ready) and matches(), text(log)[checkpoint:])

    def send(process, command):
        process.stdin.write(command + '\n')
        process.stdin.flush()
    def stop(process):
        if process and process.poll() is None:
            send(process, 'quit')
            process.wait(timeout=8)
    def inject(codes):
        for code in codes:
            remote('NotifyKeyboardKeycode', GLib.Variant('(ub)', (code, True)))
        for code in reversed(codes):
            remote('NotifyKeyboardKeycode', GLib.Variant('(ub)', (code, False)))
        time.sleep(0.3)
    def shortcut(code):
        inject([56, 29, 125, code])
    def focus_entry(log):
        entry_x, entry_y = json.loads(re.findall(r'^ENTRY_CENTER (.+)$', text(log), re.M)[-1])
        frame = focused()
        remote('NotifyPointerMotionAbsolute', GLib.Variant('(sdd)', (stream, frame[2] + entry_x, frame[3] + entry_y)))
        remote('NotifyPointerButton', GLib.Variant('(ib)', (272, True)))
        remote('NotifyPointerButton', GLib.Variant('(ib)', (272, False)))
    def window_state(log):
        states = re.findall(r'^WINDOW_STATE (.+)$', text(log), re.M)
        return json.loads(states[-1]) if states else None
    def assert_frame(name, expected, log):
        def matches():
            current = focused()
            state = window_state(log)
            return current[0] and tuple(current[2:]) == tuple(expected) and state and tuple(state[:2]) == tuple(expected[2:]) and state[2]
        check(name, wait_for(matches), 'expected ' + str(expected) + ', observed ' + str(focused()) + ', GTK ' + str(window_state(log)))
    def dispatch(hotkey, expected, name, log):
        result = subprocess.run([str(binary), '--backend', 'auto', '--config', str(config), 'dispatch', hotkey], env=env, capture_output=True, text=True, timeout=8)
        (work / (name + '-dispatch.log')).write_text(result.stdout + result.stderr)
        check(name + ' dispatch', result.returncode == 0 and 'Dispatch state: Succeeded' in result.stdout, result.stdout.strip() + result.stderr)
        assert_frame(name + ' native and client geometry', expected, log)
    def runtime_dispatch(hotkey, expected, name, log):
        checkpoint = len(text(runtime_log))
        send(runtime, 'dispatch ' + hotkey)
        check(name + ' dispatch',
              wait_for(lambda: 'Dispatch state: Succeeded' in text(runtime_log)[checkpoint:]),
              text(runtime_log)[checkpoint:])
        assert_frame(name + ' native and client geometry', expected, log)
    def rejected_dispatch(hotkey, name):
        result = subprocess.run([str(binary), '--backend', 'auto', '--config', str(config), 'dispatch', hotkey], env=env, capture_output=True, text=True, timeout=8)
        check(name + ' reports dispatch failure', result.returncode == 0 and 'Dispatch state: Error' in result.stdout, result.stdout + result.stderr)
        return result.stdout + result.stderr
    def maximize_dispatch(fixture, log, hotkey, expected, name):
        send(fixture, 'maximize')
        check(name + ' starts natively maximized', wait_for(lambda: window_state(log)[3]), window_state(log))
        dispatch(hotkey, expected, name, log)
        check(name + ' clears native maximization', wait_for(lambda: not window_state(log)[3]), window_state(log))
    def tile_dispatch(log, key, tiled_frame, hotkey, expected, name):
        inject([125, key])
        assert_frame(name + ' starts in native tile', tiled_frame, log)
        check(name + ' reports native tiling', window_state(log)[6], window_state(log))
        dispatch(hotkey, expected, name, log)
        check(name + ' clears native tiling', wait_for(lambda: not window_state(log)[6]), window_state(log))
    # Keep the original a-h indices and the custom/next/previous i/n/p bindings.
    zones = ['left-half', 'right-half', 'left-third', 'center-third', 'right-third',
             'left-two-thirds', 'right-two-thirds', 'maximize', 'top-half', 'bottom-half',
             'top-left', 'top-right', 'bottom-left', 'bottom-right']
    keycodes = [30, 48, 46, 32, 18, 33, 34, 35, 36, 37, 38, 50, 24, 16]
    hotkeys = ['Alt+Ctrl+Cmd+' + key for key in 'abcdefghjklmoq']
    canonical_hotkeys = [hotkey.lower() for hotkey in hotkeys]
    binding_count = len(hotkeys) + 5
    def valid_config(first_zone='left-half'):
        result = '[zones.custom-center]\nx = 20\ny = 20\nwidth = 60\nheight = 60\n\n'
        for hotkey, zone in zip(hotkeys, [first_zone] + zones[1:]):
            result += f'[[bindings]]\nhotkey = "{hotkey}"\naction = {{ type = "move-to-zone", zone = "{zone}" }}\n\n'
        result += '[[bindings]]\nhotkey = "Alt+Ctrl+Cmd+i"\naction = { type = "move-to-zone", zone = "custom-center" }\n\n'
        result += '[[bindings]]\nhotkey = "Alt+Ctrl+Cmd+n"\naction = { type = "move-to-next-display" }\n\n'
        result += '[[bindings]]\nhotkey = "Alt+Ctrl+Cmd+p"\naction = { type = "move-to-previous-display" }\n\n'
        result += '[[bindings]]\nhotkey = "Alt+Ctrl+Cmd+r"\naction = { type = "center" }\n\n'
        result += '[[bindings]]\nhotkey = "Alt+Ctrl+Cmd+s"\naction = { type = "restore" }\n'
        return result
    def targets(area):
        x, y, w, h = area
        return [(x, y, w // 2, h), (x + w - w // 2, y, w // 2, h),
                (x, y, w // 3, h), (x + w // 3, y, w - 2 * (w // 3), h),
                (x + w - w // 3, y, w // 3, h), (x, y, w - w // 3, h),
                (x + w // 3, y, w - w // 3, h), (x, y, w, h),
                (x, y, w, h // 2), (x, y + h - h // 2, w, h // 2),
                (x, y, w // 2, h // 2), (x + w - w // 2, y, w // 2, h // 2),
                (x, y + h - h // 2, w // 2, h // 2),
                (x + w - w // 2, y + h - h // 2, w // 2, h // 2)]
    def tray_items():
        items = call('org.kde.StatusNotifierWatcher', '/StatusNotifierWatcher', 'org.freedesktop.DBus.Properties', 'Get', GLib.Variant('(ss)', ('org.kde.StatusNotifierWatcher', 'RegisteredStatusNotifierItems'))).unpack()[0]
        matching = []
        for item in items:
            destination, separator, path = item.partition('/')
            path = '/' + path if separator else '/StatusNotifierItem'
            properties = call(destination, path, 'org.freedesktop.DBus.Properties', 'GetAll', GLib.Variant('(s)', ('org.kde.StatusNotifierItem',))).unpack()[0]
            if properties.get('Title') == 'Window Zones':
                matching.append((destination, path, properties))
        return matching
    def tray_menu():
        destination, _path, properties = tray_items()[0]
        menu = properties['Menu']
        layout = call(destination, menu, 'com.canonical.dbusmenu', 'GetLayout', GLib.Variant('(iias)', (0, -1, []))).unpack()[1]
        return destination, menu, layout
    def tray_labels():
        return [child[1].get('label', '') for child in tray_menu()[2][2]]
    def tray_control(label):
        destination, menu, layout = tray_menu()
        identifier = next((identifier for identifier, properties, _ in layout[2]
                           if properties.get('label') == label), None)
        if identifier is None:
            raise RuntimeError('Tray control missing: ' + label)
        call(destination, menu, 'com.canonical.dbusmenu', 'Event', GLib.Variant('(isvu)', (identifier, 'clicked', GLib.Variant('i', 0), 0)))
    def start_capture(name, tail, *, timeout=15, wait_active=False):
        deadline = time.monotonic() + timeout
        node_name = 'window-zones-capture-' + str(os.getpid()) + '-' + name + '-' + uuid.uuid4().hex
        # Capture labels also name GTK fixtures; keep their state logs intact.
        process, log = start(['gst-launch-1.0', '--quiet', 'pipewiresrc',
                              'path=' + str(nodes[0]), 'autoconnect=false',
                              'stream-properties=props,node.name=(string)' + node_name, *tail], name + '-capture')
        endpoints = None
        def graph():
            result = subprocess.run(['pw-dump'], env=env, capture_output=True, text=True,
                                    timeout=max(0.01, min(3, deadline - time.monotonic())))
            (work / (name + '-pw-dump.log')).write_text(result.stderr)
            if result.returncode != 0:
                raise RuntimeError('Cannot inspect private capture graph: ' + result.stderr)
            snapshot = json.loads(result.stdout)
            (work / (name + '-pipewire.json')).write_text(json.dumps(snapshot, indent=2) + '\n')
            return snapshot
        def ports_ready():
            nonlocal endpoints
            if process.poll() is not None:
                raise RuntimeError('Capture exited before linking; see ' + str(log) + ': ' + text(log))
            endpoints = capture_endpoints(graph(), nodes[0], shell.pid, node_name, process.pid)
            return endpoints is not None
        if not wait_for(ports_ready, min(8, max(0, deadline - time.monotonic()))):
            raise RuntimeError('Private capture ports did not appear; see ' + str(log))
        link, link_log = start(['pw-link', '-w', str(endpoints[2]), str(endpoints[3])], name + '-link')
        link.wait(timeout=max(0.01, min(5, deadline - time.monotonic())))
        if link.returncode != 0:
            raise RuntimeError('Private capture link failed; see ' + str(link_log) + ': ' + text(link_log))
        if wait_active:
            def active():
                if process.poll() is not None:
                    raise RuntimeError('Capture ended before animation input; see ' + str(log))
                snapshot = graph()
                if capture_endpoints(snapshot, nodes[0], shell.pid, node_name, process.pid) != endpoints:
                    raise RuntimeError('Private capture endpoints changed after linking')
                return any(item['type'] == 'PipeWire:Interface:Link'
                           and item.get('info', {}).get('output-node-id') == endpoints[0]
                           and item['info'].get('input-node-id') == endpoints[1]
                           and item['info'].get('state') == 'active' for item in snapshot)
            if not wait_for(active, max(0, deadline - time.monotonic())):
                raise RuntimeError('Private capture link never became active; see ' + str(log))
        return process, log, deadline
    def capture_screen(name):
        # Frame geometry settles before Shell's monitor-move animation finishes.
        time.sleep(0.3)
        screenshot = work / (name + '.png')
        capture, capture_log, deadline = start_capture(
            name, ['num-buffers=1', '!', 'videoconvert', '!', 'pngenc', '!', 'filesink',
                   'location=' + str(screenshot)])
        try:
            capture.wait(timeout=max(0.01, deadline - time.monotonic()))
        except subprocess.TimeoutExpired as error:
            raise RuntimeError('Compositor screenshot timed out; see ' + str(capture_log)) from error
        check(name + ' real compositor screenshot', capture.returncode == 0 and screenshot.is_file() and screenshot.read_bytes()[:8] == b'\x89PNG\r\n\x1a\n', text(capture_log) + str(screenshot))
    try:
        import gi
        gi.require_version('Gtk', '4.0')
        from gi.repository import Gio, GLib

        resource_roots, pipewire_runtime, pipewire_remote = resource_sources(os.environ)
        shell_version = subprocess.check_output(['gnome-shell', '--version'], text=True).strip()
        version_match = re.search(r'GNOME Shell (\d+)(?:\.\d+)*', shell_version)
        check('Installed GNOME version is supported', version_match and version_match[1] in ('50', '51'), shell_version)
        schemas = Gio.SettingsSchemaSource.get_default()
        reduced_motion = reduced_motion_supported(
            int(version_match[1]), schemas.lookup('org.gnome.desktop.a11y.interface', True) if schemas else None)
        print('CAPABILITY reduced-motion:', 'required GNOME 51 preference' if reduced_motion
              else 'not applicable: GNOME 50 predates St.ReducedMotion; animation checks remain enabled',
              flush=True)
        for tool in ('pw-dump', 'pw-link'):
            check('PipeWire ' + tool + ' tool is installed', shutil.which(tool) is not None, shutil.which(tool))
        for plugin in ('pipewiresrc', 'videoconvert', 'pngenc', 'filesink', 'multifilesink'):
            result = subprocess.run(['gst-inspect-1.0', plugin], capture_output=True, text=True, timeout=8)
            (work / ('gstreamer-' + plugin + '.log')).write_text(result.stdout + result.stderr)
            check('GStreamer ' + plugin + ' capture plugin is installed', result.returncode == 0, result.stderr)
        for directory in ('config/glib-2.0/settings', 'data/gnome-shell/extensions/window-zones@mihai-a24',
                          'runtime', 'home', 'state', 'cache'):
            (work / directory).mkdir(parents=True, exist_ok=True)
        os.chmod(work / 'runtime', 0o700)
        indicator_uuid = INDICATOR_UUID
        indicator_dir = find_appindicator(resource_roots, version_match[1])
        check('Installed AppIndicator supports real GNOME tray testing', indicator_dir.is_dir(), indicator_dir)
        shutil.copytree(indicator_dir, work / 'data/gnome-shell/extensions' / indicator_uuid)
        # A fresh HOME otherwise opens Shell's modal first-login tour over the fixture.
        (work / 'config/glib-2.0/settings/keyfile').write_text(
            "[org/gnome/shell]\nenabled-extensions=['window-zones@mihai-a24', '" + indicator_uuid + "']\n"
            "disable-user-extensions=false\ndisable-extension-version-validation=false\n"
            f"welcome-dialog-last-shown-version='{version_match[1]}.0'\n")
        for name in ('metadata.json', 'extension.js'):
            shutil.copy2(root / 'gnome-extension' / name, work / 'data/gnome-shell/extensions/window-zones@mihai-a24' / name)
        env = isolated_environment(work, pipewire_runtime, pipewire_remote, os.environ)
        env['WINDOW_ZONES_SMOKE_PRIVATE_OWNER'] = owner
        bus, bus_log = start(['dbus-daemon', '--session', '--nofork', '--print-address=1'], 'session-bus')
        if not wait_for(lambda: text(bus_log).strip() or bus.poll() is not None):
            raise RuntimeError('Private session bus did not start')
        address = text(bus_log).splitlines()[0]
        env['DBUS_SESSION_BUS_ADDRESS'] = address
        connection = Gio.DBusConnection.new_for_address_sync(address, Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION, None, None)
        shell, shell_log = start(['gnome-shell', '--headless', '--force-animations', '--wayland-display', env['WAYLAND_DISPLAY'], '--virtual-monitor', '1920x1080', '--virtual-monitor', '1600x900'], 'shell')
        check('Companion loaded by real GNOME', wait_for(lambda: 'move-resize' in companion('GetCapabilities')[0], 20), shell_version)
        check('GNOME finishes startup before input', wait_for(lambda: 'GNOME Shell started' in text(shell_log)), text(shell_log))
        check('Two real compositor displays and reserved panel area',
              wait_for(lambda: len(available := companion('GetDisplays')[0]) == 2 and available[0][2] > 0),
              companion('GetDisplays')[0])
        displays = companion('GetDisplays')[0]
        area = displays[0][1:]
        second_area = displays[1][1:]
        expected_zones = targets(area)
        remote_path = call('org.gnome.Mutter.RemoteDesktop', '/org/gnome/Mutter/RemoteDesktop', 'org.gnome.Mutter.RemoteDesktop', 'CreateSession').unpack()[0]
        remote_id = call('org.gnome.Mutter.RemoteDesktop', remote_path, 'org.freedesktop.DBus.Properties', 'Get', GLib.Variant('(ss)', ('org.gnome.Mutter.RemoteDesktop.Session', 'SessionId'))).unpack()[0]
        screen_path = call('org.gnome.Mutter.ScreenCast', '/org/gnome/Mutter/ScreenCast', 'org.gnome.Mutter.ScreenCast', 'CreateSession', GLib.Variant('(a{sv})', ({'remote-desktop-session-id': GLib.Variant('s', remote_id)},))).unpack()[0]
        stream = call('org.gnome.Mutter.ScreenCast', screen_path, 'org.gnome.Mutter.ScreenCast.Session', 'RecordMonitor', GLib.Variant('(sa{sv})', ('', {}))).unpack()[0]
        nodes = []
        connection.signal_subscribe('org.gnome.Mutter.ScreenCast', 'org.gnome.Mutter.ScreenCast.Stream', 'PipeWireStreamAdded', stream, None, Gio.DBusSignalFlags.NONE, lambda _c, _s, _p, _i, _n, value: nodes.append(value.unpack()[0]))
        remote('Start')
        check('Native compositor capture stream starts', wait_for(lambda: nodes), nodes)
        window, window_log = start([sys.executable, str(Path(__file__).resolve()), '--test-window', 'native Wayland'], 'native-window')
        check('Native Wayland GTK window mapped', wait_for(lambda: window_state(window_log) and window_state(window_log)[:2] == [720, 480]), text(window_log))
        remote('NotifyPointerMotionRelative', GLib.Variant('(dd)', (500.0, 400.0)))
        set_overview(False)
        inject([56, 15])
        focus_ready = wait_for(lambda: focused()[0])
        if not focus_ready:
            capture_screen('focus-failure')
            overview = call('org.gnome.Shell', '/org/gnome/Shell', 'org.freedesktop.DBus.Properties',
                            'Get', GLib.Variant('(ss)', ('org.gnome.Shell', 'OverviewActive'))).unpack()[0]
            (work / 'focus-failure.json').write_text(json.dumps({
                'overview_active': overview, 'companion': focused(), 'gtk': window_state(window_log),
            }, indent=2) + '\n')
        check('Native virtual keyboard establishes application focus', focus_ready, focused())
        capture_screen('focus-before-input')
        focus_entry(window_log)
        inject([30])
        check('Ordinary keyboard input reaches GTK', wait_for(lambda: 'TEXT_CHANGED a\n' in text(window_log)), text(window_log))
        config = work / 'config.toml'
        config.write_text(valid_config())
        status = subprocess.check_output([str(binary), '--backend', 'auto', '--config', str(config), 'status'], env=env, text=True)
        check('Auto selects GNOME with configured bindings', 'Using runtime window backend: gnome-wayland' in status and 'binding count: ' + str(binding_count) in status, status.strip())
        for zone, hotkey, expected in zip(zones, hotkeys, expected_zones):
            dispatch(hotkey, expected, zone, window_log)
        x, y, w, h = area
        custom = (x + w * 20 // 100, y + h * 20 // 100, w * 80 // 100 - w * 20 // 100, h * 80 // 100 - h * 20 // 100)
        dispatch('Alt+Ctrl+Cmd+i', custom, 'custom-center', window_log)
        dispatch(hotkeys[0], expected_zones[0], 'monitor-baseline', window_log)
        # Opposite-half landing on the physically adjacent display, not enumeration wrapping.
        dispatch(hotkeys[1], expected_zones[1], 'directional-right-first-snap', window_log)
        dispatch(hotkeys[1], targets(second_area)[0], 'directional-right-crosses-display', window_log)
        dispatch(hotkeys[1], targets(second_area)[1], 'directional-right-next-snap', window_log)
        dispatch(hotkeys[1], targets(second_area)[1], 'directional-right-outer-edge', window_log)
        dispatch(hotkeys[0], targets(second_area)[0], 'directional-left-first-snap', window_log)
        dispatch(hotkeys[0], expected_zones[1], 'directional-left-crosses-display', window_log)
        dispatch(hotkeys[0], expected_zones[0], 'directional-left-next-snap', window_log)
        dispatch(hotkeys[0], expected_zones[0], 'directional-left-outer-edge', window_log)
        dispatch('Alt+Ctrl+Cmd+n', targets(second_area)[0], 'next-monitor-preserves-half', window_log)
        dispatch('Alt+Ctrl+Cmd+p', expected_zones[0], 'previous-monitor-preserves-half', window_log)
        dispatch('Alt+Ctrl+Cmd+p', targets(second_area)[0], 'previous-monitor-wraps', window_log)
        dispatch('Alt+Ctrl+Cmd+n', expected_zones[0], 'next-monitor-wraps', window_log)
        dispatch('Alt+Ctrl+Cmd+i', custom, 'proportional-monitor-baseline', window_log)
        sx, sy, sw, sh = second_area
        proportional = (sx + round((custom[0] - x) * sw / w), sy + round((custom[1] - y) * sh / h), round(custom[2] * sw / w), round(custom[3] * sh / h))
        dispatch('Alt+Ctrl+Cmd+n', proportional, 'proportional-monitor-move', window_log)
        returned = (x + round((proportional[0] - sx) * w / sw), y + round((proportional[1] - sy) * h / sh), round(proportional[2] * w / sw), round(proportional[3] * h / sh))
        dispatch('Alt+Ctrl+Cmd+p', returned, 'proportional-monitor-return', window_log)
        runtime, runtime_log = start([str(binary), '--backend', 'auto', '--config', str(config), 'run'], 'auto-run')
        check('Auto run registers its actual accelerator set', wait_for(lambda: 'Hotkeys registered: ' + str(binding_count) in text(runtime_log)), text(runtime_log))
        try:
            companion('RegisterHotkeys', GLib.Variant('(as)', ([canonical_hotkeys[0]],)))
            raise AssertionError('Another controller replaced the runtime accelerator set')
        except GLib.Error as error:
            check('Second controller cannot steal active runtime bindings', Gio.DBusError.get_remote_error(error) == 'org.window_zones.Gnome.Error.Busy', error)
        for zone, code, expected in zip(zones, keycodes, expected_zones):
            shortcut(code)
            assert_frame('Auto run global hotkey ' + zone, expected, window_log)
        send(runtime, 'status')
        check('Auto run reports the hotkey action', wait_for(lambda: 'last action: ' + canonical_hotkeys[-1] in text(runtime_log)), text(runtime_log))
        quarter_width, quarter_height = w // 2, h // 2
        centered_quarter = (x + (w - quarter_width) // 2, y + (h - quarter_height) // 2,
                            quarter_width, quarter_height)
        runtime_dispatch('Alt+Ctrl+Cmd+r', centered_quarter, 'Interactive center shares accelerator history', window_log)
        shortcut(31)
        assert_frame('Global Restore returns to before the first runtime move', returned, window_log)
        checkpoint = len(text(runtime_log))
        send(runtime, 'dispatch Alt+Ctrl+Cmd+s')
        check('Restore consumes the same-runtime restore point',
              wait_for(lambda: 'nothing to restore' in text(runtime_log)[checkpoint:]),
              text(runtime_log)[checkpoint:])
        assert_frame('Consumed Restore leaves geometry unchanged', returned, window_log)
        shortcut(keycodes[0])
        assert_frame('Global center starts from an off-center half', expected_zones[0], window_log)
        centered_half = (x + (w - w // 2) // 2, y, w // 2, h)
        shortcut(19)
        assert_frame('Global center keeps the native window size', centered_half, window_log)
        runtime_dispatch('Alt+Ctrl+Cmd+s', returned, 'Interactive Restore shares accelerator history', window_log)
        shortcut(23)
        assert_frame('Global custom-zone hotkey', custom, window_log)
        shortcut(49)
        assert_frame('Global next-display hotkey', proportional, window_log)
        shortcut(25)
        assert_frame('Global previous-display hotkey', returned, window_log)
        config.write_text(valid_config('right-half'))
        time.sleep(0.7)
        shortcut(keycodes[0])
        assert_frame('File watcher replaces action without restart', expected_zones[1], window_log)
        config.write_text('[[bindings]]\nthis is not valid TOML\n')
        time.sleep(0.7)
        send(runtime, 'reload')
        send(runtime, 'status')
        check('Malformed reload is exposed', wait_for(lambda: 'config state: Error' in text(runtime_log)), text(runtime_log))
        shortcut(keycodes[2])
        assert_frame('Malformed config retains previous registered action', expected_zones[2], window_log)
        for name, invalid, code, expected in (
            ('Out-of-bounds custom zone', valid_config('right-half').replace('width = 60', 'width = 90'), 23, custom),
            ('Duplicate canonical binding', valid_config('right-half') + '\n[[bindings]]\nhotkey = "Cmd+Ctrl+Alt+A"\naction = { type = "move-to-zone", zone = "maximize" }\n', keycodes[0], expected_zones[1]),
        ):
            checkpoint = len(text(runtime_log))
            config.write_text(invalid)
            time.sleep(0.7)
            send(runtime, 'reload')
            send(runtime, 'status')
            check(name + ' rejected', wait_for(lambda: 'config state: Error' in text(runtime_log)[checkpoint:]), text(runtime_log)[checkpoint:])
            shortcut(code)
            assert_frame(name + ' retains last valid config', expected, window_log)
        checkpoint = len(text(runtime_log))
        config.unlink()
        def config_deleted_status():
            send(runtime, 'status')
            return 'binding count: 0' in text(runtime_log)[checkpoint:] and 'config state: Missing' in text(runtime_log)[checkpoint:]
        check('Deleted config resets bindings', wait_for(config_deleted_status), text(runtime_log)[checkpoint:])
        checkpoint = len(text(window_log))
        shortcut(keycodes[0])
        check('Removed hotkey reaches GTK instead of moving the window', wait_for(lambda: 'KEY_RECEIVED a ' in text(window_log)[checkpoint:]) and tuple(focused()[2:]) == tuple(expected_zones[1]), text(window_log)[checkpoint:])
        config.write_text(valid_config())
        time.sleep(0.7)
        shortcut(keycodes[0])
        assert_frame('Valid config recovers', expected_zones[0], window_log)
        call('org.gnome.Shell', '/org/gnome/Shell', 'org.gnome.Shell.Extensions', 'DisableExtension', GLib.Variant('(s)', ('window-zones@mihai-a24',)))
        check('Companion disconnect remains explicit', wait_for(lambda: 'GNOME companion is unavailable' in text(runtime_log)), text(runtime_log))
        check('Auto run survives loss without X11 fallback', runtime.poll() is None and 'Window backend: x11' not in text(runtime_log), runtime.pid)
        send(runtime, 'status')
        call('org.gnome.Shell', '/org/gnome/Shell', 'org.gnome.Shell.Extensions', 'EnableExtension', GLib.Variant('(s)', ('window-zones@mihai-a24',)))
        check('Same runtime reconnects and rebinds', wait_for(lambda: 'Hotkey registration now recovered' in text(runtime_log)), text(runtime_log))
        shortcut(keycodes[1])
        assert_frame('Hotkey works after companion recovery', expected_zones[1], window_log)
        send(runtime, 'restart')
        check('CLI restart remains usable', wait_for(lambda: 'Runtime restarted' in text(runtime_log)), text(runtime_log))
        checkpoint = len(text(runtime_log))
        send(runtime, 'dispatch Alt+Ctrl+Cmd+s')
        check('CLI restart clears native restore history',
              wait_for(lambda: 'nothing to restore' in text(runtime_log)[checkpoint:]),
              text(runtime_log)[checkpoint:])
        assert_frame('Restore after restart leaves native geometry unchanged', expected_zones[1], window_log)
        shortcut(keycodes[0])
        assert_frame('Hotkey works after CLI restart', expected_zones[0], window_log)
        stop(runtime)
        check('Auto run quits cleanly', runtime.returncode == 0, runtime.returncode)
        runtime = None
        tui, tui_log = start([str(binary), '--backend', 'auto', '--config', str(config), 'tui'], 'auto-tui')
        runtime = tui
        assert_tui_frame('Auto TUI shows native capabilities and bindings', tui_log, action='<none>')
        config.write_text(valid_config('right-half'))
        checkpoint = len(text(tui_log))
        send(tui, 'reload')
        assert_tui_frame('TUI reload applies the replacement binding', tui_log, checkpoint,
                         binding=(canonical_hotkeys[0], 'right-half'), first=True)
        checkpoint = len(text(tui_log))
        send(tui, 'dispatch ' + hotkeys[0])
        assert_frame('TUI manual dispatch moves real window', expected_zones[1], window_log)
        assert_tui_frame('TUI manual dispatch completes successfully', tui_log, checkpoint,
                         action=canonical_hotkeys[0], first=True)
        checkpoint = len(text(tui_log))
        send(tui, 'restart')
        assert_tui_frame('TUI restart clears action history and reloads persisted bindings', tui_log, checkpoint,
                         action='<none>', binding=(canonical_hotkeys[0], 'right-half'), first=True)
        checkpoint = len(text(tui_log))
        send(tui, 'dispatch ' + hotkeys[2])
        assert_frame('TUI remains usable after restart', expected_zones[2], window_log)
        assert_tui_frame('TUI dispatch after restart completes successfully', tui_log, checkpoint,
                         action=canonical_hotkeys[2], first=True)
        config.write_text(valid_config())
        checkpoint = len(text(tui_log))
        send(tui, 'reload')
        assert_tui_frame('TUI reload restores the original persisted binding', tui_log, checkpoint,
                         binding=(canonical_hotkeys[0], 'left-half'), first=True)
        checkpoint = len(text(tui_log))
        shortcut(keycodes[0])
        assert_frame('Real GNOME accelerator drives auto TUI', expected_zones[0], window_log)
        assert_tui_frame('TUI reports the successful actual hotkey action', tui_log, checkpoint,
                         action=canonical_hotkeys[0])
        stop(tui)
        check('Auto TUI quits cleanly', tui.returncode == 0, tui.returncode)
        runtime = None
        tray, tray_log = start([str(binary), '--tray', '--backend', 'auto', '--config', str(config), 'run'], 'auto-tray')
        check('Tray registers with the real GNOME AppIndicator', wait_for(lambda: 'Tray menu started' in text(tray_log) and tray_items()), text(tray_log))
        check('Tray exposes the installed grid icon', tray_items()[0][2]['IconName'] == 'view-grid-symbolic', tray_items()[0][2])
        checkpoint = len(text(tray_log))
        tray_control('Show status')
        check('Tray status reports live registrations', wait_for(lambda: 'hotkey state: Registered' in text(tray_log)[checkpoint:]), text(tray_log)[checkpoint:])
        shortcut(keycodes[1])
        assert_frame('Tray runtime handles native global hotkey', expected_zones[1], window_log)
        config.write_text(valid_config('right-half'))
        tray_control('Reload')
        check('Tray reload processes the config', wait_for(lambda: 'Reload requested' in text(tray_log)), text(tray_log))
        time.sleep(0.7)
        shortcut(keycodes[2])
        assert_frame('Tray reload preserves other bindings', expected_zones[2], window_log)
        shortcut(keycodes[0])
        assert_frame('Tray reload replaces configured action', expected_zones[1], window_log)
        tray_control('Restart')
        check('Tray restart completes', wait_for(lambda: 'Runtime restarted' in text(tray_log)), text(tray_log))
        shortcut(keycodes[2])
        assert_frame('Hotkey works after tray restart', expected_zones[2], window_log)
        check('Status updates and restart keep exactly one tray icon', len(tray_items()) == 1, [(destination, path) for destination, path, _properties in tray_items()])
        check('Tray labels follow the last native hotkey action', wait_for(lambda: any(canonical_hotkeys[2] in label for label in tray_labels())), tray_labels())
        last_action_label = next(label for label in tray_labels() if canonical_hotkeys[2] in label)
        checkpoint = len(text(tray_log))
        tray_control(last_action_label)
        check('Tray status row opens current runtime status', wait_for(lambda: canonical_hotkeys[2] in text(tray_log)[checkpoint:]), text(tray_log)[checkpoint:])
        config.write_text('[[bindings]\n')
        check('Tray labels expose the invalid configuration path', wait_for(lambda: any(str(config) in label for label in tray_labels())), tray_labels())
        shortcut(keycodes[1])
        assert_frame('Tray keeps valid bindings after config error', expected_zones[1], window_log)
        config.write_text(valid_config())
        check('Tray labels clear a recovered config error', wait_for(lambda: all(str(config) not in label for label in tray_labels())), tray_labels())
        check('Config error and recovery preserve one tray icon', len(tray_items()) == 1, tray_items())
        shortcut(keycodes[2])
        assert_frame('Recovered tray configuration moves the native window', expected_zones[2], window_log)
        button_x, button_y = json.loads(re.findall(r'^BUTTON_CENTER (.+)$', text(window_log), re.M)[-1])
        frame = focused()
        remote('NotifyPointerMotionAbsolute', GLib.Variant('(sdd)', (stream, frame[2] + button_x, frame[3] + button_y)))
        remote('NotifyPointerButton', GLib.Variant('(ib)', (272, True)))
        remote('NotifyPointerButton', GLib.Variant('(ib)', (272, False)))
        check('Real mouse click activates the GTK button', wait_for(lambda: 'CLICK_RECEIVED' in text(window_log)), text(window_log))
        capture_screen('native-tray-and-mouse')
        tray_control('Quit')
        tray.wait(timeout=8)
        check('Tray quit terminates cleanly', tray.returncode == 0, tray.returncode)
        config.write_text(valid_config())
        maximize_dispatch(window, window_log, hotkeys[0], expected_zones[0], 'zone-from-maximized')
        maximize_dispatch(window, window_log, 'Alt+Ctrl+Cmd+n', second_area, 'next-display-from-maximized')
        maximize_dispatch(window, window_log, 'Alt+Ctrl+Cmd+p', area, 'previous-display-from-maximized')
        send(window, 'fullscreen')
        check('Real GTK fullscreen state enters compositor', wait_for(lambda: window_state(window_log)[4]), window_state(window_log))
        rejected = rejected_dispatch(hotkeys[0], 'Fullscreen dispatch')
        check('Fullscreen action is rejected without clearing state', 'Dispatch state: Error' in rejected and window_state(window_log)[4], rejected.strip())
        send(window, 'restore')
        check('Window restores after fullscreen check', wait_for(lambda: not window_state(window_log)[4]), window_state(window_log))
        send(window, 'fixed')
        check('GTK window disables resizing', wait_for(lambda: not window_state(window_log)[5]), window_state(window_log))
        time.sleep(0.3)
        rejected = rejected_dispatch(hotkeys[0], 'Fixed-size dispatch')
        check('Fixed-size window is rejected', 'Dispatch state: Error' in rejected and 'does not allow resizing' in rejected, rejected.strip())
        send(window, 'restore')
        check('Window becomes resizable again', wait_for(lambda: window_state(window_log)[5]), window_state(window_log))
        dispatch('Alt+Ctrl+Cmd+i', custom, 'tiling-baseline', window_log)
        tile_dispatch(window_log, 105, expected_zones[0], 'Alt+Ctrl+Cmd+i', custom, 'custom-zone-from-tiled')
        tile_dispatch(window_log, 105, expected_zones[0], 'Alt+Ctrl+Cmd+n', targets(second_area)[0], 'next-display-from-tiled')
        tile_dispatch(window_log, 106, targets(second_area)[1], 'Alt+Ctrl+Cmd+p', expected_zones[1], 'previous-display-from-tiled')
        dispatch('Alt+Ctrl+Cmd+i', custom, 'overview-baseline', window_log)
        overview_window_id = focused()[1]
        set_overview(True)
        check('Overview excludes the application from focused-window actions', wait_for(lambda: not focused()[0]), focused())
        rejected = rejected_dispatch(hotkeys[0], 'Overview dispatch')
        check('Overview dispatch is rejected', 'Dispatch state: Error' in rejected and 'NoFocusedWindow' in rejected, rejected.strip())
        set_overview(False)
        inject([56, 15])
        check('Overview exit returns actions to the captured application',
              wait_for(lambda: focused()[:2] == (True, overview_window_id) and window_state(window_log)[2]),
              focused())
        assert_frame('Overview leaves the window geometry unchanged', custom, window_log)
        dispatch(hotkeys[1], expected_zones[1], 'dry-run-baseline', window_log)
        dry = subprocess.check_output([str(binary), '--backend', 'dry-run', '--config', str(config), 'dispatch', hotkeys[0]], env=env, text=True)
        check('Dry-run calculates placement without touching real window', 'Dispatch state: Succeeded' in dry and tuple(focused()[2:]) == tuple(expected_zones[1]), dry.strip())
        xdisplay = re.search(r'Using public X11 display (:\d+)', text(shell_log)).group(1)
        xauth = next((work / 'runtime').glob('.mutter-Xwaylandauth.*'))
        forced_env = dict(env, DISPLAY=xdisplay, XAUTHORITY=str(xauth))
        forced = subprocess.run([str(binary), '--backend', 'x11', '--config', str(config), 'status'],
                                env=forced_env, capture_output=True, text=True, timeout=8)
        (work / 'explicit-x11-rejection.log').write_text(forced.stdout + forced.stderr)
        check('Explicit X11 is rejected within GNOME Wayland',
              forced.returncode != 0 and
              'X11 backend is unavailable while WAYLAND_DISPLAY or XDG_SESSION_TYPE=wayland is set' in forced.stderr and
              'native Wayland compositor integration' in forced.stderr, forced.stdout + forced.stderr)
        assert_frame('Explicit X11 rejection leaves native geometry unchanged', expected_zones[1], window_log)
        capture_screen('native-window')
        events = []
        subscription = connection.signal_subscribe('org.window_zones.Gnome', 'org.window_zones.Gnome2', 'HotkeyPressed', '/org/window_zones/Gnome', None, Gio.DBusSignalFlags.NONE, lambda _c, _s, _p, _i, _n, value: events.append(value.unpack()[0]))
        companion('RegisterHotkeys', GLib.Variant('(as)', ([canonical_hotkeys[0]],)))
        try:
            companion('RegisterHotkeys', GLib.Variant('(as)', ([canonical_hotkeys[0], canonical_hotkeys[1], 'alt+ctrl+cmd+f25'],)))
            raise AssertionError('Invalid accelerator set was accepted')
        except GLib.Error as error:
            check('Companion rejects invalid accelerator transaction', Gio.DBusError.get_remote_error(error) == 'org.window_zones.Gnome.Error.Unsupported', error)
        shortcut(keycodes[0])
        check('Failed transaction retains the previous native accelerator', wait_for(lambda: events == [canonical_hotkeys[0]]), events)
        checkpoint = len(text(window_log))
        shortcut(keycodes[1])
        check('Failed transaction releases newly grabbed accelerator', wait_for(lambda: 'KEY_RECEIVED b ' in text(window_log)[checkpoint:]) and events == [canonical_hotkeys[0]], text(window_log)[checkpoint:] + str(events))
        companion('RegisterHotkeys', GLib.Variant('(as)', ([],)))
        connection.signal_unsubscribe(subscription)
        default_config = work / 'config/window_zones/config.toml'
        default_config.parent.mkdir(parents=True)
        default_config.write_text('''[[bindings]]
hotkey = "Cmd+Ctrl+Left"
action = { type = "move-to-zone", zone = "left-half" }
[[bindings]]
hotkey = "Cmd+Ctrl+Right"
action = { type = "move-to-zone", zone = "right-half" }
[[bindings]]
hotkey = "Cmd+Ctrl+Down"
action = { type = "move-to-next-display" }
[[bindings]]
hotkey = "Cmd+Ctrl+Up"
action = { type = "move-to-previous-display" }
''')
        launcher_env = env.copy()
        launcher_env['WINDOW_ZONES_BINARY'] = str(binary)
        launcher = str(root / 'scripts/run.sh')
        launched, launched_log = start([launcher], 'default-launcher-status', launcher_env)
        launched.wait(timeout=8)
        check('Default launcher safely previews the discovered config', launched.returncode == 0 and 'dry-run' in text(launched_log) and str(default_config) in text(launched_log) and 'binding count: 4' in text(launched_log) and tuple(focused()[2:]) == tuple(expected_zones[1]), text(launched_log))
        runtime, runtime_log = start([launcher, '--backend', 'auto', 'run'], 'default-launcher-auto-run', launcher_env)
        check('Auto run discovers and registers default arrow bindings', wait_for(lambda: 'Hotkeys registered: 4' in text(runtime_log)), text(runtime_log))
        for code, expected, name in ((105, expected_zones[0], 'left'), (106, expected_zones[1], 'right'), (108, second_area, 'next monitor'), (103, area, 'previous monitor')):
            if code in (108, 103):
                send(window, 'maximize')
                check('Default ' + name + ' shortcut starts maximized', wait_for(lambda: window_state(window_log)[3]), window_state(window_log))
            inject([29, 125, code])
            assert_frame('Default Ctrl+Super arrow: ' + name, expected, window_log)
            if code in (108, 103):
                check('Default ' + name + ' shortcut restores maximized window', wait_for(lambda: not window_state(window_log)[3]), window_state(window_log))
        stop(runtime)
        check('Default auto run quits cleanly', runtime.returncode == 0, runtime.returncode)
        runtime, runtime_log = start([launcher, '--backend', 'auto', 'tui'], 'default-launcher-auto-tui', launcher_env)
        check('Auto TUI discovers default config and native capability', wait_for(lambda: 'Window Zones TUI' in text(runtime_log) and 'Hotkey state: Registered' in text(runtime_log) and str(default_config) in text(runtime_log)), text(runtime_log))
        send(window, 'maximize')
        check('Default auto TUI shortcut starts from native maximization', wait_for(lambda: window_state(window_log)[3]), window_state(window_log))
        inject([29, 125, 105])
        assert_frame('Default auto TUI handles real Ctrl+Super+Left', expected_zones[0], window_log)
        check('Default auto TUI shortcut restores maximized window', wait_for(lambda: not window_state(window_log)[3]), window_state(window_log))
        # A rapid double press must not drop the second move or lose logging.
        stop(runtime)
        perf_path = work / 'transitions.jsonl'
        runtime, runtime_log = start([str(binary), '--backend', 'auto', '--perf-log', str(perf_path), 'tui'], 'performance-tui')
        check('Performance TUI registers arrow bindings', wait_for(lambda: 'Hotkey state: Registered' in text(runtime_log)), text(runtime_log))
        def perf_rows():
            return [json.loads(line) for line in perf_path.read_text().splitlines()] if perf_path.exists() else []
        def transition_rows():
            return [row for row in perf_rows() if row['event'] == 'dispatch']
        session = perf_rows()[0]
        check('Logger records actual process memory before moving', session['pid'] == runtime.pid and session['rss_kib'] > 0 and session['peak_rss_kib'] >= session['rss_kib'], session)
        transition_count = len(transition_rows())
        for _ in range(2):
            for code in (29, 125, 106):
                remote('NotifyKeyboardKeycode', GLib.Variant('(ub)', (code, True)))
            for code in (106, 125, 29):
                remote('NotifyKeyboardKeycode', GLib.Variant('(ub)', (code, False)))
            time.sleep(0.03)
        assert_frame('Rapid Right twice lands on next display nearest half', targets(second_area)[0], window_log)
        check('Logger retains both rapid transitions', wait_for(lambda: len(transition_rows()) == transition_count + 2), perf_rows())
        rows = transition_rows()[transition_count:]
        check('Transition completion includes real animation rather than request acknowledgment', all(row['result'] == 'ok' and row['metrics_error'] is None and 0 <= row['request_ms'] < 100 and 200 <= row['completion_ms'] <= row['dispatch_ms'] and row['rss_kib'] > 0 for row in rows), rows)
        metrics_isolated = False
        try:
            companion('GetLastTransitionMetrics')
        except GLib.Error as error:
            metrics_isolated = Gio.DBusError.get_remote_error(error) == 'org.window_zones.Gnome.Error.InvalidWindowState'
        check('Independent caller cannot read another controller transition metrics', metrics_isolated)
        checkpoint = len(text(runtime_log))
        send(runtime, 'restart')
        assert_tui_frame('Performance TUI restarts without losing logger', runtime_log, checkpoint,
                         action='<none>', first=True)
        transition_count = len(transition_rows())
        inject([29, 125, 105])
        assert_frame('Left returns to previous display nearest half after restart', expected_zones[1], window_log)
        check('Transition after restart appends to the same log', wait_for(lambda: len(transition_rows()) == transition_count + 1), perf_rows())
        send(window, 'maximize')
        check('Performance fixture enters maximized state', wait_for(lambda: window_state(window_log)[3]), window_state(window_log))
        transition_count = len(transition_rows())
        inject([29, 125, 106])
        assert_frame('Maximized transition reaches right half', expected_zones[1], window_log)
        check('Maximized completion includes the finished visual transition', wait_for(lambda: len(transition_rows()) == transition_count + 1) and transition_rows()[-1]['result'] == 'ok' and 200 <= transition_rows()[-1]['completion_ms'] <= transition_rows()[-1]['dispatch_ms'], transition_rows()[-1])
        subprocess.run(['gsettings', 'set', 'org.gnome.desktop.interface', 'enable-animations', 'false'], env=env, check=True)
        time.sleep(0.1)
        transition_count = len(transition_rows())
        inject([29, 125, 105])
        assert_frame('Disabled animation still places window correctly', expected_zones[0], window_log)
        check('Disabled animation logs completion without animation delay', wait_for(lambda: len(transition_rows()) == transition_count + 1) and transition_rows()[-1]['result'] == 'ok' and transition_rows()[-1]['completion_ms'] < 200, transition_rows()[-1])
        subprocess.run(['gsettings', 'set', 'org.gnome.desktop.interface', 'enable-animations', 'true'], env=env, check=True)
        if reduced_motion:
            subprocess.run(['gsettings', 'set', 'org.gnome.desktop.a11y.interface', 'reduced-motion', 'reduce'], env=env, check=True)
            time.sleep(0.1)
            transition_count = len(transition_rows())
            inject([29, 125, 106])
            assert_frame('Reduced motion still places window correctly', expected_zones[1], window_log)
            check('Reduced motion avoids custom movement animation', wait_for(lambda: len(transition_rows()) == transition_count + 1) and transition_rows()[-1]['result'] == 'ok' and transition_rows()[-1]['completion_ms'] < 200, transition_rows()[-1])
            subprocess.run(['gsettings', 'set', 'org.gnome.desktop.a11y.interface', 'reduced-motion', 'no-preference'], env=env, check=True)
        send(window, 'fullscreen')
        check('Performance fixture enters fullscreen', wait_for(lambda: window_state(window_log)[4]), window_state(window_log))
        transition_count = len(transition_rows())
        inject([29, 125, 106])
        check('Rejected transition is logged without stale success timings', wait_for(lambda: len(transition_rows()) == transition_count + 1 and transition_rows()[-1]['result'] == 'error') and transition_rows()[-1]['request_ms'] is None and transition_rows()[-1]['completion_ms'] is None and 'fullscreen' in transition_rows()[-1]['error'], transition_rows()[-1])
        send(window, 'restore')
        check('Performance fixture restores after rejection', wait_for(lambda: not window_state(window_log)[4]), window_state(window_log))
        # Capture real compositor frames while moving, for visual proof of intermediate placement.
        dispatch('Alt+Ctrl+Cmd+i', custom, 'animation-capture-baseline', window_log)
        recording, recording_log, recording_deadline = start_capture(
            'animation-frames', ['num-buffers=16', '!', 'videoconvert', '!', 'pngenc',
                                 '!', 'multifilesink', 'location=' + str(work / 'animation-%02d.png')],
            timeout=8, wait_active=True)
        inject([29, 125, 105])
        try:
            recording.wait(timeout=max(0.01, recording_deadline - time.monotonic()))
        except subprocess.TimeoutExpired as error:
            raise RuntimeError('Compositor animation capture timed out; see ' + str(recording_log)) from error
        assert_frame('Animated move ends at requested geometry', expected_zones[0], window_log)
        frames = sorted(work.glob('animation-*.png'))
        check('Compositor transition capture finishes with sixteen real PNG frames',
              recording.returncode == 0 and len(frames) == 16
              and all(frame.read_bytes()[:8] == b'\x89PNG\r\n\x1a\n' for frame in frames),
              text(recording_log) + str(frames))
        inject([29, 125, 106])
        assert_frame('Default auto TUI handles real Ctrl+Super+Right', expected_zones[1], window_log)
        stop(runtime)
        check('Default auto TUI quits cleanly', runtime.returncode == 0, runtime.returncode)
        runtime = None
        xenv = env.copy()
        xenv.update(GDK_BACKEND='x11', DISPLAY=xdisplay, XAUTHORITY=str(xauth))
        xwindow, xwindow_log = start([sys.executable, str(Path(__file__).resolve()), '--test-window', 'XWayland'], 'xwayland-window', xenv)
        check('Real XWayland GTK window maps and receives focus', wait_for(lambda: window_state(xwindow_log) and window_state(xwindow_log)[:3] == [720, 480, True]), text(xwindow_log))
        focus_entry(xwindow_log)
        inject([48])
        check('Native keyboard reaches XWayland GTK entry', wait_for(lambda: 'TEXT_CHANGED b\n' in text(xwindow_log)), text(xwindow_log))
        dispatch(hotkeys[0], expected_zones[0], 'xwayland-left-half', xwindow_log)
        dispatch('Alt+Ctrl+Cmd+n', targets(second_area)[0], 'xwayland-next-monitor', xwindow_log)
        dispatch('Alt+Ctrl+Cmd+p', expected_zones[0], 'xwayland-larger-monitor-return', xwindow_log)
        dispatch(hotkeys[1], expected_zones[1], 'xwayland-directional-right-first', xwindow_log)
        dispatch(hotkeys[1], targets(second_area)[0], 'xwayland-directional-right-crosses', xwindow_log)
        dispatch(hotkeys[0], expected_zones[1], 'xwayland-directional-left-crosses', xwindow_log)
        maximize_dispatch(xwindow, xwindow_log, hotkeys[0], expected_zones[0], 'xwayland-zone-from-maximized')
        maximize_dispatch(xwindow, xwindow_log, 'Alt+Ctrl+Cmd+n', second_area, 'xwayland-next-from-maximized')
        maximize_dispatch(xwindow, xwindow_log, 'Alt+Ctrl+Cmd+p', area, 'xwayland-previous-from-maximized')
        tile_dispatch(xwindow_log, 105, expected_zones[0], 'Alt+Ctrl+Cmd+i', custom, 'xwayland-custom-from-tiled')
        tile_dispatch(xwindow_log, 105, expected_zones[0], 'Alt+Ctrl+Cmd+n', targets(second_area)[0], 'xwayland-next-from-tiled')
        tile_dispatch(xwindow_log, 106, targets(second_area)[1], 'Alt+Ctrl+Cmd+p', expected_zones[1], 'xwayland-previous-from-tiled')
        dispatch(hotkeys[0], expected_zones[0], 'xwayland-final-left-half', xwindow_log)
        capture_screen('native-and-xwayland-windows')
        stop(xwindow)
        assert_frame('Closing XWayland restores native window focus', expected_zones[1], window_log)
        stop(window)
        check('No application window is reported after closing fixtures', wait_for(lambda: not focused()[0]), focused())
        rejected = rejected_dispatch(hotkeys[0], 'No-focused-window dispatch')
        check('No-focused-window dispatch is explicit', 'Dispatch state: Error' in rejected and 'NoFocusedWindow' in rejected, rejected.strip())
        print('NATIVE_GNOME_SMOKE_COMPLETE', flush=True)
    except BaseException as error:
        if not checks or checks[-1]['passed']:
            checks.append({'name': 'Smoke execution error', 'passed': False, 'detail': str(error)})
        raise
    finally:
        if remote_path and connection:
            try:
                remote('Stop')
            except GLib.Error:
                pass
        try:
            owned_process.cleanup(registry, owner, os.getpid())
            for process in reversed(processes):
                process.wait(timeout=5)
        except Exception as error:
            checks.append({'name': 'Owned process cleanup', 'passed': False, 'detail': str(error)})
            raise
        finally:
            (work / 'results.json').write_text(json.dumps(checks, indent=2) + '\n')
            print('Smoke checks:', len(checks), 'failures:', sum(not row['passed'] for row in checks), flush=True)
            if keep_logs or any(not row['passed'] for row in checks):
                print('Logs retained:', work, flush=True)
            else:
                shutil.rmtree(work)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=Path(__file__).resolve().parent.parent / 'target/debug/window_zones')
    parser.add_argument('--keep-logs', action='store_true')
    parser.add_argument('--output-dir', type=Path, help='Create this evidence directory instead of a temporary one')
    parser.add_argument('--test-window', metavar='TITLE')
    parser.add_argument('--xwayland-transform', type=Path,
                        help='Record current Mutter/Xwayland layout and print scale and logical origin')
    parser.add_argument('--capture', type=Path, help='Capture the current GNOME compositor without input')
    parser.add_argument('--capture-window', type=int, help='Choose the captured monitor by this Xwayland window')
    args = parser.parse_args()
    if args.test_window:
        test_window(args.test_window)
    else:
        interrupted_signal = None
        def terminate(signum, _frame):
            global interrupted_signal
            interrupted_signal = signum
            for handled in (signal.SIGINT, signal.SIGTERM):
                signal.signal(handled, signal.SIG_IGN)
            raise SystemExit(128 + signum)
        signal.signal(signal.SIGTERM, terminate)
        signal.signal(signal.SIGINT, terminate)
        try:
            if args.xwayland_transform:
                observe_xwayland(args.xwayland_transform)
            elif args.capture:
                capture_current_monitor(args.capture, args.capture_window)
            else:
                run(args.binary.resolve(), args.keep_logs, args.output_dir)
        finally:
            if interrupted_signal:
                signal.signal(interrupted_signal, signal.SIG_DFL)
                os.kill(os.getpid(), interrupted_signal)
