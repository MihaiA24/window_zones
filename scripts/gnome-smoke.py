#!/usr/bin/env python3
"""Exercise real GNOME windows and accelerators on a private session bus."""
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

import gi
from gi.repository import Gio, GLib


def test_window(title):
    gi.require_version('Gtk', '4.0')
    gi.require_version('Gdk', '4.0')
    from gi.repository import Gdk, Gtk
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
        elif line == 'present':
            window.present()
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
    checks = []
    bus_pid = None
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
        return call('org.window_zones.Gnome', '/org/window_zones/Gnome', 'org.window_zones.Gnome1', method, params).unpack()
    def focused():
        return companion('GetFocusedWindow')
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
        process = subprocess.Popen(args, env=child_env or env, stdin=subprocess.PIPE, stdout=log, stderr=subprocess.STDOUT, text=True, start_new_session=True)
        log.close()
        processes.append(process)
        return process, log_path
    def text(log):
        return log.read_text(errors='replace')
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
        check(name + ' dispatch', 'Dispatch state: Succeeded' in result.stdout, result.stdout.strip())
        assert_frame(name + ' native and client geometry', expected, log)
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
    zones = ['left-half', 'right-half', 'left-third', 'center-third', 'right-third', 'left-two-thirds', 'right-two-thirds', 'maximize']
    keycodes = [30, 48, 46, 32, 18, 33, 34, 35]
    hotkeys = ['Alt+Ctrl+Cmd+' + key for key in 'abcdefgh']
    def valid_config(first_zone='left-half'):
        result = '[zones.custom-center]\nx = 20\ny = 20\nwidth = 60\nheight = 60\n\n'
        for hotkey, zone in zip(hotkeys, [first_zone] + zones[1:]):
            result += f'[[bindings]]\nhotkey = "{hotkey}"\naction = {{ type = "move-to-zone", zone = "{zone}" }}\n\n'
        result += '[[bindings]]\nhotkey = "Alt+Ctrl+Cmd+i"\naction = { type = "move-to-zone", zone = "custom-center" }\n\n'
        result += '[[bindings]]\nhotkey = "Alt+Ctrl+Cmd+n"\naction = { type = "move-to-next-display" }\n\n'
        result += '[[bindings]]\nhotkey = "Alt+Ctrl+Cmd+p"\naction = { type = "move-to-previous-display" }\n'
        return result
    def targets(area):
        x, y, w, h = area
        return [(x, y, w // 2, h), (x + w - w // 2, y, w // 2, h),
                (x, y, w // 3, h), (x + w // 3, y, w - 2 * (w // 3), h),
                (x + w - w // 3, y, w // 3, h), (x, y, w - w // 3, h),
                (x + w // 3, y, w - w // 3, h), (x, y, w, h)]
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
        def find(node):
            identifier, properties, children = node
            if properties.get('label') == label:
                return identifier
            for child in children:
                found = find(child)
                if found is not None:
                    return found
            return None
        identifier = find(layout)
        if identifier is None:
            raise RuntimeError('Tray control missing: ' + label)
        call(destination, menu, 'com.canonical.dbusmenu', 'Event', GLib.Variant('(isvu)', (identifier, 'clicked', GLib.Variant('i', 0), 0)))
    def capture_screen(name):
        # Frame geometry settles before Shell's monitor-move animation finishes.
        time.sleep(0.3)
        screenshot = work / (name + '.png')
        capture = subprocess.run(['gst-launch-1.0', '--quiet', 'pipewiresrc', 'path=' + str(nodes[0]), 'num-buffers=1', '!', 'videoconvert', '!', 'pngenc', '!', 'filesink', 'location=' + str(screenshot)], env=env, capture_output=True, text=True, timeout=15)
        check(name + ' real compositor screenshot', capture.returncode == 0 and screenshot.is_file() and screenshot.read_bytes()[:8] == b'\x89PNG\r\n\x1a\n', capture.stdout + capture.stderr + str(screenshot))
    try:
        for directory in ('config/glib-2.0/settings', 'data/gnome-shell/extensions/window-zones@mihai-a24', 'runtime', 'home'):
            (work / directory).mkdir(parents=True, exist_ok=True)
        os.chmod(work / 'runtime', 0o700)
        indicator_uuid = 'appindicatorsupport@rgcjonas.gmail.com'
        indicator_dir = next((base / 'gnome-shell/extensions' / indicator_uuid for base in (Path(os.environ.get('XDG_DATA_HOME', str(Path.home() / '.local/share'))), Path('/usr/share')) if (base / 'gnome-shell/extensions' / indicator_uuid).is_dir()), None)
        check('Installed AppIndicator supports real GNOME tray testing', indicator_dir is not None, indicator_dir)
        shutil.copytree(indicator_dir, work / 'data/gnome-shell/extensions' / indicator_uuid, symlinks=True)
        (work / 'config/glib-2.0/settings/keyfile').write_text("[org/gnome/shell]\nenabled-extensions=['window-zones@mihai-a24', '" + indicator_uuid + "']\ndisable-user-extensions=false\n")
        for name in ('metadata.json', 'extension.js'):
            shutil.copy2(root / 'gnome-extension' / name, work / 'data/gnome-shell/extensions/window-zones@mihai-a24' / name)
        address, bus_pid = subprocess.check_output(['dbus-daemon', '--session', '--fork', '--print-address=1', '--print-pid=1'], text=True).splitlines()
        env = os.environ.copy()
        env.update(DBUS_SESSION_BUS_ADDRESS=address, XDG_RUNTIME_DIR=str(work / 'runtime'), XDG_CONFIG_HOME=str(work / 'config'), XDG_DATA_HOME=str(work / 'data'), HOME=str(work / 'home'), GSETTINGS_BACKEND='keyfile', XDG_CURRENT_DESKTOP='GNOME', XDG_SESSION_TYPE='wayland', WAYLAND_DISPLAY='window-zones-native-smoke', GDK_BACKEND='wayland', GTK_A11Y='none')
        env.update(PIPEWIRE_RUNTIME_DIR=os.environ.get('PIPEWIRE_RUNTIME_DIR', os.environ['XDG_RUNTIME_DIR']), PIPEWIRE_REMOTE=os.environ.get('PIPEWIRE_REMOTE', 'pipewire-0'))
        env.pop('DISPLAY', None)
        env.pop('XAUTHORITY', None)
        connection = Gio.DBusConnection.new_for_address_sync(address, Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION, None, None)
        shell, shell_log = start(['gnome-shell', '--headless', '--wayland-display', env['WAYLAND_DISPLAY'], '--virtual-monitor', '1920x1080', '--virtual-monitor', '1600x900'], 'shell')
        check('Companion loaded by real GNOME', wait_for(lambda: 'move-resize' in companion('GetCapabilities')[0], 20), 'GNOME ' + subprocess.check_output(['gnome-shell', '--version'], text=True).strip())
        displays = companion('GetDisplays')[0]
        check('Two real compositor displays and reserved panel area', len(displays) == 2 and displays[0][2] > 0, displays)
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
        video, video_log = start(['gst-launch-1.0', '--quiet', 'pipewiresrc', 'path=' + str(nodes[0]), '!', 'videoconvert', '!', 'fakesink', 'sync=false'], 'live-compositor-video')
        window, window_log = start([sys.executable, str(Path(__file__).resolve()), '--test-window', 'native Wayland'], 'native-window')
        check('Native Wayland GTK window mapped', wait_for(lambda: window_state(window_log) and window_state(window_log)[:2] == [720, 480]), text(window_log))
        check('GNOME finishes startup before input', wait_for(lambda: 'GNOME Shell started' in text(shell_log)), text(shell_log))
        inject([1])
        inject([56, 15])
        remote('NotifyPointerMotionRelative', GLib.Variant('(dd)', (500.0, 400.0)))
        check('Native virtual keyboard establishes application focus', wait_for(lambda: focused()[0]), focused())
        capture_screen('focus-before-input')
        focus_entry(window_log)
        inject([30])
        check('Ordinary keyboard input reaches GTK', wait_for(lambda: 'TEXT_CHANGED a\n' in text(window_log)), text(window_log))
        config = work / 'config.toml'
        config.write_text(valid_config())
        status = subprocess.check_output([str(binary), '--backend', 'auto', '--config', str(config), 'status'], env=env, text=True)
        check('Auto selects GNOME with configured bindings', 'Using runtime window backend: gnome-wayland' in status and 'binding count: 11' in status, status.strip())
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
        check('Auto run accepts global bindings', wait_for(lambda: 'Interactive session started' in text(runtime_log)), text(runtime_log))
        send(runtime, 'status')
        check('Auto run registers its actual accelerator set', wait_for(lambda: 'hotkey state: Registered' in text(runtime_log)), text(runtime_log))
        try:
            companion('RegisterHotkeys', GLib.Variant('(as)', ([hotkeys[0]],)))
            raise AssertionError('Another controller replaced the runtime accelerator set')
        except GLib.Error as error:
            check('Second controller cannot steal active runtime bindings', Gio.DBusError.get_remote_error(error) == 'org.window_zones.Gnome.Error.Busy', error)
        for zone, code, expected in zip(zones, keycodes, expected_zones):
            shortcut(code)
            assert_frame('Auto run global hotkey ' + zone, expected, window_log)
        send(runtime, 'status')
        check('Auto run reports the hotkey action', wait_for(lambda: re.search(r'last action: .*\bh\b', text(runtime_log)) is not None), text(runtime_log))
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
        shortcut(keycodes[0])
        assert_frame('Hotkey works after CLI restart', expected_zones[0], window_log)
        stop(runtime)
        check('Auto run quits cleanly', runtime.returncode == 0, runtime.returncode)
        runtime = None
        tui, tui_log = start([str(binary), '--backend', 'auto', '--config', str(config), 'tui'], 'auto-tui')
        runtime = tui
        check('Auto TUI shows native capabilities and bindings', wait_for(lambda: 'Window Zones TUI' in text(tui_log) and 'Hotkey state: Registered' in text(tui_log)), text(tui_log))
        send(tui, 'reload')
        send(tui, 'restart')
        send(tui, 'status')
        send(tui, 'dispatch ' + hotkeys[1])
        assert_frame('TUI manual dispatch moves real window', expected_zones[1], window_log)
        shortcut(keycodes[0])
        assert_frame('Real GNOME accelerator drives auto TUI', expected_zones[0], window_log)
        check('TUI reports the actual hotkey action', wait_for(lambda: re.search(r'Last action: .*\ba\b', text(tui_log)) is not None), text(tui_log))
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
        last_action_label = '  last action: ' + hotkeys[2].lower()
        check('Tray labels follow the last native hotkey action', wait_for(lambda: last_action_label in tray_labels() and '  dispatch state: Succeeded' in tray_labels()), tray_labels())
        checkpoint = len(text(tray_log))
        tray_control(last_action_label)
        check('Tray status row opens current runtime status', wait_for(lambda: last_action_label in text(tray_log)[checkpoint:]), text(tray_log)[checkpoint:])
        config.write_text('[[bindings]\n')
        check('Tray labels expose invalid configuration', wait_for(lambda: any(label.startswith('  config error: ') and label != '  config error: <none>' for label in tray_labels())), tray_labels())
        shortcut(keycodes[1])
        assert_frame('Tray keeps valid bindings after config error', expected_zones[1], window_log)
        config.write_text(valid_config())
        check('Tray labels clear a recovered config error', wait_for(lambda: '  config state: Loaded' in tray_labels() and '  config error: <none>' in tray_labels()), tray_labels())
        check('Config error and recovery preserve one tray icon', len(tray_items()) == 1, tray_items())
        shortcut(keycodes[2])
        assert_frame('Recovered tray configuration moves the native window', expected_zones[2], window_log)
        check('Tray transition finishes before pointer interaction', wait_for(lambda: '  last action: alt+ctrl+cmd+c' in tray_labels() and '  dispatch state: Succeeded' in tray_labels()), tray_labels())
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
        rejected = subprocess.check_output([str(binary), '--backend', 'auto', '--config', str(config), 'dispatch', hotkeys[0]], env=env, text=True)
        check('Fullscreen action is rejected without clearing state', 'Dispatch state: Error' in rejected and window_state(window_log)[4], rejected.strip())
        send(window, 'restore')
        check('Window restores after fullscreen check', wait_for(lambda: not window_state(window_log)[4]), window_state(window_log))
        send(window, 'fixed')
        check('GTK window disables resizing', wait_for(lambda: not window_state(window_log)[5]), window_state(window_log))
        time.sleep(0.3)
        rejected = subprocess.check_output([str(binary), '--backend', 'auto', '--config', str(config), 'dispatch', hotkeys[0]], env=env, text=True)
        check('Fixed-size window is rejected', 'Dispatch state: Error' in rejected and 'does not allow resizing' in rejected, rejected.strip())
        send(window, 'restore')
        check('Window becomes resizable again', wait_for(lambda: window_state(window_log)[5]), window_state(window_log))
        dispatch('Alt+Ctrl+Cmd+i', custom, 'tiling-baseline', window_log)
        tile_dispatch(window_log, 105, expected_zones[0], 'Alt+Ctrl+Cmd+i', custom, 'custom-zone-from-tiled')
        tile_dispatch(window_log, 105, expected_zones[0], 'Alt+Ctrl+Cmd+n', targets(second_area)[0], 'next-display-from-tiled')
        tile_dispatch(window_log, 106, targets(second_area)[1], 'Alt+Ctrl+Cmd+p', expected_zones[1], 'previous-display-from-tiled')
        dispatch('Alt+Ctrl+Cmd+i', custom, 'overview-baseline', window_log)
        inject([125])
        check('Overview excludes the application from focused-window actions', wait_for(lambda: not focused()[0]), focused())
        rejected = subprocess.check_output([str(binary), '--backend', 'auto', '--config', str(config), 'dispatch', hotkeys[0]], env=env, text=True)
        check('Overview dispatch is rejected', 'Dispatch state: Error' in rejected and 'NoFocusedWindow' in rejected, rejected.strip())
        inject([1])
        assert_frame('Overview leaves the window geometry unchanged', custom, window_log)
        dispatch(hotkeys[1], expected_zones[1], 'dry-run-baseline', window_log)
        dry = subprocess.check_output([str(binary), '--backend', 'dry-run', '--config', str(config), 'dispatch', hotkeys[0]], env=env, text=True)
        check('Dry-run calculates placement without touching real window', 'Dispatch state: Succeeded' in dry and tuple(focused()[2:]) == tuple(expected_zones[1]), dry.strip())
        forced = subprocess.run([str(binary), '--backend', 'x11', 'status'], env=env, capture_output=True, text=True)
        check('Explicit X11 is rejected within GNOME Wayland', forced.returncode != 0 and 'X11 backend is unavailable inside a Wayland session' in forced.stderr, forced.stderr.strip())
        capture_screen('native-window')
        events = []
        subscription = connection.signal_subscribe('org.window_zones.Gnome', 'org.window_zones.Gnome1', 'HotkeyPressed', '/org/window_zones/Gnome', None, Gio.DBusSignalFlags.NONE, lambda _c, _s, _p, _i, _n, value: events.append(value.unpack()[0]))
        companion('RegisterHotkeys', GLib.Variant('(as)', ([hotkeys[0]],)))
        try:
            companion('RegisterHotkeys', GLib.Variant('(as)', ([hotkeys[0], hotkeys[1], 'Alt+Ctrl+Cmd+F25'],)))
            raise AssertionError('Invalid accelerator set was accepted')
        except GLib.Error as error:
            check('Companion rejects invalid accelerator transaction', Gio.DBusError.get_remote_error(error) == 'org.window_zones.Gnome.Error.Unsupported', error)
        shortcut(keycodes[0])
        check('Failed transaction retains the previous native accelerator', wait_for(lambda: events == [hotkeys[0]]), events)
        checkpoint = len(text(window_log))
        shortcut(keycodes[1])
        check('Failed transaction releases newly grabbed accelerator', wait_for(lambda: 'KEY_RECEIVED b ' in text(window_log)[checkpoint:]) and events == [hotkeys[0]], text(window_log)[checkpoint:] + str(events))
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
        send(runtime, 'status')
        check('Auto run discovers and registers default arrow bindings', wait_for(lambda: 'hotkey state: Registered' in text(runtime_log) and 'binding count: 4' in text(runtime_log)), text(runtime_log))
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
        session = perf_rows()[0]
        check('Logger records actual process memory before moving', session['pid'] == runtime.pid and session['rss_kib'] > 0 and session['peak_rss_kib'] >= session['rss_kib'], session)
        for _ in range(2):
            for code in (29, 125, 106):
                remote('NotifyKeyboardKeycode', GLib.Variant('(ub)', (code, True)))
            for code in (106, 125, 29):
                remote('NotifyKeyboardKeycode', GLib.Variant('(ub)', (code, False)))
            time.sleep(0.03)
        assert_frame('Rapid Right twice lands on next display nearest half', targets(second_area)[0], window_log)
        check('Logger retains both rapid transitions', wait_for(lambda: len([row for row in perf_rows() if row['event'] == 'dispatch']) == 2), perf_rows())
        rows = [row for row in perf_rows() if row['event'] == 'dispatch']
        check('Transition completion includes real animation rather than request acknowledgment', all(row['result'] == 'ok' and row['metrics_error'] is None and 0 <= row['request_ms'] < 100 and 200 <= row['completion_ms'] <= row['dispatch_ms'] and row['rss_kib'] > 0 for row in rows), rows)
        metrics_isolated = False
        try:
            companion('GetLastTransitionMetrics')
        except GLib.Error as error:
            metrics_isolated = Gio.DBusError.get_remote_error(error) == 'org.window_zones.Gnome.Error.InvalidWindowState'
        check('Independent caller cannot read another controller transition metrics', metrics_isolated)
        send(runtime, 'restart')
        check('Performance TUI restarts without losing logger', wait_for(lambda: text(runtime_log).count('Last action: <none>') >= 2), text(runtime_log))
        inject([29, 125, 105])
        assert_frame('Left returns to previous display nearest half after restart', expected_zones[1], window_log)
        check('Transition after restart appends to the same log', wait_for(lambda: len([row for row in perf_rows() if row['event'] == 'dispatch']) == 3), perf_rows())
        send(window, 'maximize')
        check('Performance fixture enters maximized state', wait_for(lambda: window_state(window_log)[3]), window_state(window_log))
        inject([29, 125, 106])
        assert_frame('Maximized transition reaches right half', expected_zones[1], window_log)
        check('Maximized completion includes the finished visual transition', wait_for(lambda: len(perf_rows()) == 5) and perf_rows()[-1]['result'] == 'ok' and 200 <= perf_rows()[-1]['completion_ms'] <= perf_rows()[-1]['dispatch_ms'], perf_rows()[-1])
        subprocess.run(['gsettings', 'set', 'org.gnome.desktop.interface', 'enable-animations', 'false'], env=env, check=True)
        time.sleep(0.1)
        inject([29, 125, 105])
        assert_frame('Disabled animation still places window correctly', expected_zones[0], window_log)
        check('Disabled animation logs completion without animation delay', wait_for(lambda: len(perf_rows()) == 6) and perf_rows()[-1]['result'] == 'ok' and perf_rows()[-1]['completion_ms'] < 200, perf_rows()[-1])
        subprocess.run(['gsettings', 'set', 'org.gnome.desktop.interface', 'enable-animations', 'true'], env=env, check=True)
        subprocess.run(['gsettings', 'set', 'org.gnome.desktop.a11y.interface', 'reduced-motion', 'reduce'], env=env, check=True)
        time.sleep(0.1)
        inject([29, 125, 106])
        assert_frame('Reduced motion still places window correctly', expected_zones[1], window_log)
        check('Reduced motion avoids custom movement animation', wait_for(lambda: len(perf_rows()) == 7) and perf_rows()[-1]['result'] == 'ok' and perf_rows()[-1]['completion_ms'] < 200, perf_rows()[-1])
        subprocess.run(['gsettings', 'set', 'org.gnome.desktop.a11y.interface', 'reduced-motion', 'no-preference'], env=env, check=True)
        send(window, 'fullscreen')
        check('Performance fixture enters fullscreen', wait_for(lambda: window_state(window_log)[4]), window_state(window_log))
        inject([29, 125, 106])
        check('Rejected transition is logged without stale success timings', wait_for(lambda: perf_rows()[-1]['result'] == 'error') and perf_rows()[-1]['request_ms'] is None and perf_rows()[-1]['completion_ms'] is None and 'fullscreen' in perf_rows()[-1]['error'], perf_rows()[-1])
        send(window, 'restore')
        check('Performance fixture restores after rejection', wait_for(lambda: not window_state(window_log)[4]), window_state(window_log))
        # Capture real compositor frames while moving, for visual proof of intermediate placement.
        dispatch('Alt+Ctrl+Cmd+i', custom, 'animation-capture-baseline', window_log)
        recording, recording_log = start(['gst-launch-1.0', '--quiet', 'pipewiresrc', 'path=' + str(nodes[0]), 'num-buffers=16', '!', 'videoconvert', '!', 'pngenc', '!', 'multifilesink', 'location=' + str(work / 'animation-%02d.png')], 'animation-frames')
        inject([29, 125, 105])
        recording.wait(timeout=8)
        assert_frame('Animated move ends at requested geometry', expected_zones[0], window_log)
        check('Compositor transition capture finishes successfully', recording.returncode == 0, text(recording_log))
        inject([29, 125, 106])
        assert_frame('Default auto TUI handles real Ctrl+Super+Right', expected_zones[1], window_log)
        stop(runtime)
        check('Default auto TUI quits cleanly', runtime.returncode == 0, runtime.returncode)
        runtime = None
        xdisplay = re.search(r'Using public X11 display (:\d+)', text(shell_log)).group(1)
        xauth = next((work / 'runtime').glob('.mutter-Xwaylandauth.*'))
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
        rejected = subprocess.check_output([str(binary), '--backend', 'auto', '--config', str(config), 'dispatch', hotkeys[0]], env=env, text=True)
        check('No-focused-window dispatch is explicit', 'Dispatch state: Error' in rejected and 'NoFocusedWindow' in rejected, rejected.strip())
        print('NATIVE_GNOME_SMOKE_COMPLETE', flush=True)
    except Exception as error:
        if not checks or checks[-1]['passed']:
            checks.append({'name': 'Smoke execution error', 'passed': False, 'detail': str(error)})
        raise
    finally:
        if remote_path and connection:
            try:
                remote('Stop')
            except GLib.Error:
                pass
        for process in reversed(processes):
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
        for process in reversed(processes):
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
        if bus_pid:
            os.kill(int(bus_pid), signal.SIGTERM)
        (work / 'results.json').write_text(json.dumps(checks, indent=2) + '\n')
        print('Smoke checks:', len(checks), 'failures:', sum(not row['passed'] for row in checks), flush=True)
        if keep_logs:
            print('Logs retained:', work, flush=True)
        else:
            shutil.rmtree(work)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=Path(__file__).resolve().parent.parent / 'target/debug/window_zones')
    parser.add_argument('--keep-logs', action='store_true')
    parser.add_argument('--output-dir', type=Path, help='Create this evidence directory instead of a temporary one')
    parser.add_argument('--test-window', metavar='TITLE')
    args = parser.parse_args()
    if args.test_window:
        test_window(args.test_window)
    else:
        run(args.binary.resolve(), args.keep_logs, args.output_dir)
