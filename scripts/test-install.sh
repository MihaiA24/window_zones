#!/usr/bin/env bash
set -euo pipefail
PROJECT_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
exec python3 - "$PROJECT_ROOT" <<'PY'
import configparser
import json
import os
from pathlib import Path
import re
import shlex
import signal
import subprocess
import sys
import tempfile
import time

root = Path(sys.argv[1])
fixture_binary = r'''
import json, os, signal, subprocess, sys, time
from pathlib import Path
role = Path(__file__).name
pid = os.getpid()
started = subprocess.run(['/bin/ps', '-p', str(pid), '-o', 'lstart='], text=True, capture_output=True).stdout.strip()
ready = Path(os.environ['WZI_READY'])
def event(kind):
    data = dict(kind=kind, role=role, pid=pid, stamp=started, version=VERSION, path=__file__, args=sys.argv[1:])
    with open(os.environ['WZI_EVENTS'], 'a') as log:
        log.write(json.dumps(data) + '\n')
def stop(*args):
    if role == 'window_zones_kwin' and ready.exists() and ready.read_text() == str(pid):
        ready.unlink()
    event('exit')
    sys.exit(0)
signal.signal(signal.SIGTERM, stop)
signal.signal(signal.SIGINT, stop)
if sys.argv[1:] == ['init']:
    config = Path(os.environ['XDG_CONFIG_HOME']) / 'window_zones/config.toml'
    config.parent.mkdir(parents=True, exist_ok=True)
    if not config.exists():
        config.write_text('starter config\n')
    sys.exit(0)
event('start')
if role == 'window_zones_kwin':
    time.sleep(0.25)
    ready.write_text(str(pid))
elif not ready.exists():
    event('early-app')
    sys.exit(91)
while True:
    if role == 'window_zones' and Path(os.environ['WZI_EXIT75']).exists():
        event('exit75')
        sys.exit(75)
    time.sleep(0.05)
'''

# All desktop/service commands are fixture executables, including on a real KDE host.
# The small manager executes generated units rather than testing installer source text.
fixture_tool = r'''
import ast, configparser, json, os, shlex, shutil, signal, subprocess, sys, time
from pathlib import Path
name = Path(sys.argv[0]).name
args = sys.argv[1:]
base = Path(os.environ['WZI_FIXTURE'])
config = Path(os.environ['XDG_CONFIG_HOME'])
ready = Path(os.environ['WZI_READY'])
def log(kind, **data):
    with open(os.environ['WZI_EVENTS'], 'a') as stream:
        stream.write(json.dumps(dict(kind=kind, tool=name, args=args, **data)) + '\n')
def stamp(pid):
    return subprocess.run(['/bin/ps', '-p', str(pid), '-o', 'lstart='], text=True, capture_output=True).stdout.strip()
def live(record):
    return bool(record and stamp(record['pid']) == record['stamp'])
def state():
    file = base / 'services.json'
    return json.loads(file.read_text()) if file.exists() else {}
def save(value):
    (base / 'services.json').write_text(json.dumps(value))
def stop(unit):
    value = state()
    record = value.pop(unit, None)
    if live(record):
        os.kill(record['pid'], signal.SIGTERM)
        for _ in range(100):
            if '"kind": "exit"' in Path(os.environ['WZI_EVENTS']).read_text():
                events = [json.loads(line) for line in Path(os.environ['WZI_EVENTS']).read_text().splitlines()]
                if any(item.get('pid') == record['pid'] and item['kind'] == 'exit' for item in events):
                    break
            time.sleep(0.02)
    save(value)
def start(unit, restart=False):
    value = state()
    if not restart and live(value.get(unit)):
        return
    parser = configparser.ConfigParser(interpolation=None, strict=False)
    file = config / 'systemd/user' / unit
    if not parser.read(file):
        raise RuntimeError('missing unit ' + str(file))
    for dependency in parser.get('Unit', 'Wants', fallback='').split():
        if dependency.endswith('.service'):
            start(dependency)
    stop(unit)
    command = [part.replace('%%', '%').replace('$$', '$') for part in shlex.split(parser['Service']['ExecStart'])]
    child = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                             stderr=subprocess.DEVNULL, start_new_session=True)
    value = state()
    value[unit] = dict(pid=child.pid, stamp=stamp(child.pid))
    save(value)
    if parser['Service'].get('Type') == 'dbus':
        for _ in range(200):
            if ready.exists() and ready.read_text() == str(child.pid):
                break
            time.sleep(0.02)
        else:
            raise RuntimeError('companion did not acquire its bus name')
if name == 'uname':
    print('Linux')
elif name == 'pkg-config':
    pass
elif name == 'cargo':
    if args[0] == 'metadata':
        print(json.dumps(dict(target_directory=os.environ['CARGO_TARGET_DIR'])))
        sys.exit(0)
    profile = 'release' if '--release' in args else 'debug'
    target = Path(os.environ['CARGO_TARGET_DIR']) / profile
    target.mkdir(parents=True, exist_ok=True)
    for index, argument in enumerate(args):
        if argument == '--bin':
            binary = target / args[index + 1]
            binary.write_text('#!' + sys.executable + '\nVERSION = ' + repr(os.environ['WZI_VERSION']) + '\n' +
                              (base / 'binary.py').read_text())
            binary.chmod(0o755)
elif name == 'kpackagetool6':
    log('package')
    package = Path(os.environ['XDG_DATA_HOME']) / 'kwin/scripts/window-zones'
    if '--remove' in args:
        shutil.rmtree(package)
    else:
        shutil.copytree(Path(args[-1]), package, dirs_exist_ok=True)
elif name == 'systemctl':
    args = [arg for arg in args if arg != '--user']
    if args[0] == 'show-environment':
        sys.exit(0 if os.environ['WZI_SYSTEMD'] == '1' else 1)
    if os.environ['WZI_SYSTEMD'] != '1':
        sys.exit(1)
    log('systemctl')
    action, unit = args[0], args[-1]
    if action == 'enable':
        link = config / 'systemd/user/graphical-session.target.wants' / unit
        link.parent.mkdir(parents=True, exist_ok=True)
        if not link.exists():
            link.symlink_to(config / 'systemd/user' / unit)
    elif action in ('disable', 'stop'):
        stop(unit)
        if action == 'disable':
            (config / 'systemd/user/graphical-session.target.wants' / unit).unlink(missing_ok=True)
    elif action in ('start', 'restart'):
        start(unit, action == 'restart')
    elif action == 'is-active':
        sys.exit(0 if live(state().get(unit)) else 1)
    elif action != 'daemon-reload':
        raise RuntimeError('unexpected systemctl action: ' + repr(args))
elif name in ('busctl', 'dbus-send'):
    sys.exit(0 if ready.exists() else 1)
elif name in ('kwriteconfig6', 'qdbus6'):
    log('kwin-setting')
elif name == 'gnome-extensions':
    log('gnome-extension')
    if args not in (['enable', 'window-zones@mihai-a24'], ['disable', 'window-zones@mihai-a24']):
        raise RuntimeError('unexpected GNOME extension command: ' + repr(args))
    # A newly installed extension is unknown to the running Shell until next login.
    sys.exit(1)
elif name == 'gsettings':
    log('gnome-setting')
    if args[1:3] != ['org.gnome.shell', 'enabled-extensions']:
        raise RuntimeError('unexpected GNOME setting: ' + repr(args))
    file = base / 'gnome-enabled.json'
    enabled = json.loads(file.read_text()) if file.exists() else ['unrelated@example']
    if args == ['get', 'org.gnome.shell', 'enabled-extensions']:
        print(repr(enabled))
    elif len(args) == 4 and args[0] == 'set':
        enabled = ast.literal_eval(args[3])
        assert isinstance(enabled, list) and all(isinstance(item, str) for item in enabled)
        file.write_text(json.dumps(enabled))
    else:
        raise RuntimeError('unexpected gsettings command: ' + repr(args))
else:
    raise RuntimeError('unexpected desktop command: ' + name)
'''

with tempfile.TemporaryDirectory(prefix='window-zones-installer-') as temporary:
    base = Path(temporary)
    tools = base / 'tools'
    tools.mkdir()
    events_file = base / 'events.jsonl'
    events_file.touch()
    (base / 'binary.py').write_text(fixture_binary)
    for name in ('uname', 'pkg-config', 'cargo', 'kpackagetool6', 'systemctl', 'busctl',
                 'dbus-send', 'kwriteconfig6', 'qdbus6', 'launchctl', 'gnome-extensions', 'gsettings'):
        tool = tools / name
        tool.write_text('#!' + sys.executable + '\n' + fixture_tool)
        tool.chmod(0o755)
    env = dict(os.environ, HOME=str(base / 'home'), XDG_CONFIG_HOME=str(base / 'config'),
               XDG_DATA_HOME=str(base / 'data'), XDG_CURRENT_DESKTOP='KDE',
               CARGO_TARGET_DIR=str(base / 'target'), PATH=str(tools) + ':/usr/bin:/bin',
               SHELL='/bin/bash', WZI_FIXTURE=str(base), WZI_EVENTS=str(events_file),
               WZI_READY=str(base / 'ready'), WZI_EXIT75=str(base / 'exit75'),
               WZI_SYSTEMD='1', WZI_VERSION='one')
    Path(env['HOME']).mkdir()
    config = Path(env['XDG_CONFIG_HOME'])
    witness = config / 'window_zones/config.toml'
    witness.parent.mkdir(parents=True)
    witness.write_text('user config witness\n')
    prefix = base / r'''prefix $cash 25% "quoted" 'single' \back`tick'''
    replacement = base / 'replacement'
    fallback = base / r'''fallback = $cash 25% "quoted" 'single' \back`tick'''
    launchers = []

    def events():
        return [json.loads(line) for line in events_file.read_text().splitlines()]

    def starts(role=None, version=None, path=None):
        return [item for item in events() if item['kind'] == 'start'
                and (role is None or item['role'] == role)
                and (version is None or item['version'] == version)
                and (path is None or Path(item['path']) == path)]

    def wait_for(predicate, description):
        until = time.monotonic() + 12
        while time.monotonic() < until:
            if predicate():
                return
            time.sleep(0.05)
        raise AssertionError(description)

    def command(arguments):
        result = subprocess.run(arguments, env=env, text=True, capture_output=True)
        if result.returncode:
            raise AssertionError(f'{arguments!r}: exit {result.returncode}\n{result.stdout}\n{result.stderr}')
        return result

    def install(location, *options):
        return command(['/bin/bash', str(root / 'scripts/install.sh'), '--prefix', str(location), *options])

    def ctl(*args):
        return command([str(tools / 'systemctl'), '--user', *args])

    def desktop_command(file):
        parser = configparser.ConfigParser(interpolation=None)
        parser.read(file)
        value = parser['Desktop Entry']['Exec']
        value = re.sub(r'\\([\\sntr])', lambda match: {'\\': '\\', 's': ' ', 'n': '\n', 't': '\t', 'r': '\r'}[match[1]], value)
        return [part.replace('\\$', '$').replace('\\`', '`').replace('%%', '%') for part in shlex.split(value)]

    try:
        env['XDG_CURRENT_DESKTOP'] = 'GNOME'
        gnome_id = 'window-zones@mihai-a24'
        gnome_dir = Path(env['XDG_DATA_HOME']) / 'gnome-shell/extensions' / gnome_id
        gnome_marker = gnome_dir / '.window-zones-install-prefix'
        gnome_settings = base / 'gnome-enabled.json'
        unrelated = gnome_dir.parent / 'unrelated@example/metadata.json'
        unrelated.parent.mkdir(parents=True)
        unrelated.write_text('unrelated extension witness\n')
        gnome_replacement = base / r'''GNOME replacement $cash 25% "quoted" 'single' \back`tick'''

        def gnome_files():
            return {str(file.relative_to(gnome_dir)): file.read_bytes()
                    for file in gnome_dir.rglob('*') if file.is_file()}

        install(prefix, '--gnome-extension', '--init-config')
        install(gnome_replacement, '--gnome-extension')
        replacement_files = gnome_files()
        replacement_settings = gnome_settings.read_bytes()
        offset = len(events())
        result = install(prefix, '--uninstall')
        assert gnome_files() == replacement_files, 'stale prefix removed replacement GNOME files'
        assert gnome_settings.read_bytes() == replacement_settings, 'stale prefix changed replacement GNOME activation'
        assert not any(item['kind'] in ('gnome-extension', 'gnome-setting') for item in events()[offset:]), 'stale prefix called GNOME tools'
        assert 'leaving it enabled and installed' in result.stderr
        assert '--gnome-extension' in result.stderr, 'ownership warning must explain how to reclaim the companion'
        assert gnome_marker.read_text() == str(gnome_replacement) + '\n'
        assert json.loads(gnome_settings.read_text()) == ['unrelated@example', gnome_id]
        assert not (prefix / 'bin/window_zones').exists()

        (gnome_dir / 'extension.js').write_text('old companion revision\n')
        install(gnome_replacement, '--gnome-extension', '--init-config')
        assert gnome_files() == replacement_files, 'same-prefix reinstall did not replace the companion'
        record = (gnome_replacement / 'share/window-zones/install-record').read_text().splitlines()
        assert record.count('gnome-extension') == 1
        install(gnome_replacement, '--uninstall')
        assert not gnome_dir.exists(), 'owning prefix did not remove its GNOME companion'
        assert not (gnome_replacement / 'bin/window_zones').exists()
        assert json.loads(gnome_settings.read_text()) == ['unrelated@example']
        assert unrelated.read_text() == 'unrelated extension witness\n'
        assert witness.read_text() == 'user config witness\n'

        for owner in (None, str(gnome_replacement)):
            install(prefix, '--gnome-extension')
            if owner is None:
                gnome_marker.unlink()
            else:
                gnome_marker.write_text(owner + '\n')
            unowned_files = gnome_files()
            unowned_settings = gnome_settings.read_bytes()
            offset = len(events())
            result = install(prefix, '--uninstall')
            assert gnome_files() == unowned_files, 'unowned GNOME files were removed'
            assert gnome_settings.read_bytes() == unowned_settings, 'unowned GNOME activation was changed'
            assert not any(item['kind'] in ('gnome-extension', 'gnome-setting') for item in events()[offset:]), 'unowned uninstall called GNOME tools'
            assert 'leaving it enabled and installed' in result.stderr and '--gnome-extension' in result.stderr
            assert unrelated.read_text() == 'unrelated extension witness\n'
            assert witness.read_text() == 'user config witness\n'
        print('PASS GNOME prefix replacement, owned reinstall/uninstall, unowned preservation, config preservation')
        env['XDG_CURRENT_DESKTOP'] = 'KDE'

        install(str(prefix) + '/', '--kwin-script', '--autostart', '--init-config')
        wait_for(lambda: starts('window_zones'), 'systemd App did not start')
        assert not any(item['kind'] == 'early-app' for item in events()), 'App started before companion readiness'
        actions = [item['args'] for item in events() if item['kind'] == 'systemctl']
        assert actions.index(['restart', 'window-zones-kwin.service']) < actions.index(['restart', 'window-zones.service'])
        ctl('stop', 'window-zones.service')
        ctl('stop', 'window-zones-kwin.service')
        count = len(starts())
        ctl('start', 'window-zones.service')
        wait_for(lambda: len(starts()) >= count + 2, 'App login dependency did not start companion')
        env['WZI_VERSION'] = 'two'
        install(prefix, '--kwin-script')
        wait_for(lambda: starts('window_zones_kwin', 'two') and starts('window_zones', 'two'), 'reinstall did not replace both active binaries')
        record = (prefix / 'share/window-zones/install-record').read_text().splitlines()
        assert len(record) == len(set(record)), 'duplicate ownership records'
        assert 'kwin-autostart' in record
        install(prefix, '--autostart')
        assert witness.read_text() == 'user config witness\n'
        install(replacement, '--kwin-script', '--autostart')
        replacement_pid = starts('window_zones_kwin', path=replacement / 'bin/window_zones_kwin')[-1]['pid']
        offset = len(events())
        install(prefix)
        assert not any(item['kind'] == 'systemctl' and item['args'][0] == 'restart' for item in events()[offset:]), 'stale prefix restarted replacement service'
        offset = len(events())
        install(prefix, '--uninstall')
        assert not any(item['kind'] == 'systemctl' and item['args'][0] == 'disable' for item in events()[offset:]), 'stale prefix stopped replacement service'
        assert not any(item['kind'] == 'kwin-setting' for item in events()[offset:]), 'stale prefix changed replacement KWin activation'
        assert Path(env['WZI_READY']).read_text() == str(replacement_pid)
        assert (Path(env['XDG_DATA_HOME']) / 'kwin/scripts/window-zones').is_dir()
        install(str(replacement) + '/./', '--uninstall')
        assert not (config / 'systemd/user/window-zones-kwin.service').exists()
        assert not (config / 'systemd/user/window-zones.service').exists()
        assert witness.read_text() == 'user config witness\n'
        print('PASS systemd ordering, recorded companion reuse, reinstall, ownership, uninstall')

        env['WZI_SYSTEMD'] = '0'
        env['WZI_VERSION'] = 'three'
        install(fallback, '--kwin-script')
        install(fallback, '--autostart')
        desktop = config / 'autostart/window_zones_kwin.desktop'
        assert desktop.exists() and not (config / 'autostart/window_zones.desktop').exists()
        child = subprocess.Popen(desktop_command(desktop), env=env, stdin=subprocess.DEVNULL,
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        launchers.append(child)
        wait_for(lambda: starts('window_zones', 'three'), 'XDG launcher did not start App')
        assert not any(item['kind'] == 'early-app' for item in events())
        duplicate = subprocess.run(desktop_command(desktop), env=env, capture_output=True)
        assert duplicate.returncode == 75, 'second XDG launcher was not rejected'
        previous = starts('window_zones_kwin', 'three')[-1]['pid']
        os.kill(previous, signal.SIGKILL)
        wait_for(lambda: starts('window_zones_kwin', 'three')[-1]['pid'] != previous, 'XDG companion was not recovered')
        previous = starts('window_zones', 'three')[-1]['pid']
        os.kill(previous, signal.SIGKILL)
        wait_for(lambda: starts('window_zones', 'three')[-1]['pid'] != previous, 'XDG App was not recovered')
        Path(env['WZI_EXIT75']).touch()
        wait_for(lambda: any(item['kind'] == 'exit75' for item in events()), 'App did not exit 75')
        count = len(starts('window_zones', 'three'))
        time.sleep(2.5)
        assert len(starts('window_zones', 'three')) == count, 'XDG launcher retried duplicate App owner'
        Path(env['WZI_EXIT75']).unlink()
        env['WZI_VERSION'] = 'four'
        install(fallback, '--kwin-script')
        wait_for(lambda: starts('window_zones_kwin', 'four') and starts('window_zones', 'four'), 'XDG reinstall did not restart onto new binaries')
        running = {item['pid'] for item in starts(version='four')}
        install(str(fallback) + '/', '--uninstall')
        child.wait(timeout=8)
        wait_for(lambda: running <= {item.get('pid') for item in events() if item['kind'] == 'exit'},
                 'XDG logout/uninstall left supervised binaries running')
        assert not desktop.exists()
        assert not (fallback / 'share/window-zones/kde-session').exists()
        assert not (fallback / 'share/window-zones/kde-session-running').exists()
        assert not (fallback / 'bin/window_zones_kwin').exists()
        assert witness.read_text() == 'user config witness\n'
        print('PASS XDG ordering, companion/App recovery, duplicate-owner exit, reinstall, uninstall')
    finally:
        for child in launchers:
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()
        # Kill only fixture binaries whose process start identity still matches this run.
        for record in starts() if events_file.exists() else []:
            current = subprocess.run(['/bin/ps', '-p', str(record['pid']), '-o', 'lstart='], text=True, capture_output=True).stdout.strip()
            if current and current == record['stamp']:
                try:
                    os.kill(record['pid'], signal.SIGTERM)
                except ProcessLookupError:
                    pass
PY
