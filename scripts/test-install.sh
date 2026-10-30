#!/usr/bin/env bash
set -euo pipefail
if [[ "$(uname -s)" != Linux ]]; then
  printf 'SKIP installer fixture: Linux /proc and GNU stat are required.\n'
  exit 0
fi
PROJECT_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
exec python3 - "$PROJECT_ROOT" <<'PY'
import configparser
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import time

root = Path(sys.argv[1])
fixture_binary = r'''
import json, os, signal, sys, time
from pathlib import Path
role = Path(__file__).name
pid = os.getpid()
started = Path('/proc/self/stat').read_text().rsplit(') ', 1)[1].split()[19]
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
    config = (Path(os.environ['HOME']) / 'Library/Application Support' if os.environ['WZI_OS'] == 'Darwin'
              else Path(os.environ['XDG_CONFIG_HOME'])) / 'window_zones/config.toml'
    config.parent.mkdir(parents=True, exist_ok=True)
    if not config.exists():
        config.write_text('starter config\n')
    sys.exit(0)
event('start')
if role == 'window_zones_kwin' and Path(os.environ['WZI_KWIN_EXIT75']).exists():
    event('exit75')
    sys.exit(75)
if role == 'window_zones_kwin':
    time.sleep(0.25)
    ready.write_text(str(pid))
elif os.environ['WZI_OS'] != 'Darwin' and not ready.exists():
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
import ast, configparser, json, os, plistlib, shlex, shutil, signal, subprocess, sys, time
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
    try:
        fields = Path(f'/proc/{pid}/stat').read_text().rsplit(') ', 1)[1].split()
        return fields[19] if fields[0] != 'Z' else ''
    except (FileNotFoundError, ProcessLookupError):
        return ''
def live(record):
    return bool(record and stamp(record['pid']) == record['stamp'])
def exit_status(record):
    if record:
        events = Path(os.environ['WZI_EVENTS']).read_text().splitlines()
        if any(item.get('pid') == record['pid'] and item['kind'] == 'exit75'
               for item in map(json.loads, events)):
            return 75
    return 0
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
                return 0
            if not live(value[unit]):
                return exit_status(value[unit]) or 1
            time.sleep(0.02)
        raise RuntimeError('companion did not acquire its bus name')
    return 0
if name == 'uname':
    print(os.environ['WZI_OS'])
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
        sys.exit(start(unit, action == 'restart'))
    elif action == 'is-active':
        active = os.environ['WZI_GRAPHICAL'] == '1' if unit == 'graphical-session.target' else live(state().get(unit))
        sys.exit(0 if active else 1)
    elif action == 'show':
        record = state().get(args[1])
        print('ActiveState=' + ('active' if live(record) else 'failed'))
        print('ExecMainStatus=' + str(exit_status(record)))
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
elif name == 'plutil':
    if args[:5] != ['-extract', 'ProgramArguments.0', 'raw', '-o', '-']:
        raise RuntimeError('unexpected plutil command: ' + repr(args))
    print(plistlib.loads(Path(args[5]).read_bytes())['ProgramArguments'][0])
elif name == 'launchctl':
    log('launchctl')
    file = base / 'launch-agent.json'
    record = json.loads(file.read_text()) if file.exists() else None
    def launch(path):
        plist = plistlib.loads(Path(path).read_bytes())
        child = subprocess.Popen(plist['ProgramArguments'], stdin=subprocess.DEVNULL,
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                 start_new_session=True)
        file.write_text(json.dumps(dict(pid=child.pid, stamp=stamp(child.pid), path=path)))
    def unload():
        if live(record):
            os.kill(record['pid'], signal.SIGTERM)
            for _ in range(100):
                if not live(record):
                    break
                time.sleep(0.02)
    if args[0] == 'print':
        if not record:
            sys.exit(1)
        print('state = ' + ('running' if live(record) else 'exited'))
        print('last exit code = ' + str(exit_status(record)))
    elif args[0] == 'bootstrap':
        assert not record, 'bootstrap raced an already loaded job'
        launch(args[2])
    elif args[0] == 'kickstart':
        assert args[1] == '-k' and record, 'kickstart needs a loaded job'
        unload()
        launch(record['path'])
    elif args[0] == 'bootout':
        unload()
        file.unlink(missing_ok=True)
    else:
        raise RuntimeError('unexpected launchctl command: ' + repr(args))
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
                 'dbus-send', 'kwriteconfig6', 'qdbus6', 'launchctl', 'plutil', 'gnome-extensions', 'gsettings'):
        tool = tools / name
        tool.write_text('#!' + sys.executable + '\n' + fixture_tool)
        tool.chmod(0o755)
    env = dict(os.environ, HOME=str(base / 'home'), XDG_CONFIG_HOME=str(base / 'config'),
               XDG_DATA_HOME=str(base / 'data'), XDG_RUNTIME_DIR=str(base / 'runtime'), XDG_CURRENT_DESKTOP='KDE',
               CARGO_TARGET_DIR=str(base / 'target'), PATH=str(tools) + ':/usr/bin:/bin',
               SHELL='/bin/bash', WZI_FIXTURE=str(base), WZI_EVENTS=str(events_file),
               WZI_READY=str(base / 'ready'), WZI_EXIT75=str(base / 'exit75'),
               WZI_KWIN_EXIT75=str(base / 'kwin-exit75'), WZI_OS='Linux',
               WZI_SYSTEMD='1', WZI_GRAPHICAL='1', WZI_VERSION='one', TZ='UTC')
    Path(env['HOME']).mkdir()
    Path(env['XDG_RUNTIME_DIR']).mkdir(mode=0o700)
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

    def command(arguments, *, environment=None):
        result = subprocess.run(arguments, env=env if environment is None else environment, text=True, capture_output=True)
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

    def runtime_state(location):
        key = hashlib.sha256(str(location / 'bin').encode()).hexdigest()
        return Path(env['XDG_RUNTIME_DIR']) / ('window-zones-kde-' + key)

    def launch_desktop(file):
        child = subprocess.Popen(desktop_command(file), env=env, stdin=subprocess.DEVNULL,
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                 start_new_session=True)
        launchers.append(child)
        return child

    def identity(pid):
        try:
            fields = Path(f'/proc/{pid}/stat').read_text().rsplit(') ', 1)[1].split()
            return fields[19] if fields[0] != 'Z' else ''
        except (FileNotFoundError, ProcessLookupError):
            return ''

    try:
        path_prefix = base / r'''PATH $cash "quoted" 'single' \back`printf injected`'''
        for shell, options in (('bash', ('--noprofile', '--norc')), ('zsh', ('-f',)), ('fish', ())):
            shell_path = shutil.which(shell)
            if shell_path is None:
                print(f'SKIP {shell} PATH suggestion: shell is not installed')
                continue
            shell_home = base / ('path-home-' + shell)
            shell_home.mkdir()
            path_env = dict(env, HOME=str(shell_home), XDG_CONFIG_HOME=str(shell_home / '.config'),
                            SHELL=shell_path, XDG_CURRENT_DESKTOP='', cash='expanded-value')
            result = command(['/bin/bash', str(root / 'scripts/install.sh'), '--prefix', str(path_prefix)],
                             environment=path_env)
            marker = f'{path_prefix / "bin"} is not on PATH. Add it once, then open a new terminal:\n'
            assert marker in result.stdout, f'{shell} installation did not print a PATH suggestion'
            instruction = result.stdout.split(marker, 1)[1].splitlines()[0].strip()
            command([shell_path, *options, '-c', instruction], environment=path_env)
            check_path = ('printf "%s\\n" $PATH[1]' if shell == 'fish'
                          else f'source "$HOME/.{shell}rc"; printf "%s\\n" "${{PATH%%:*}}"')
            literal_path = command([shell_path, *options, '-c', check_path], environment=path_env).stdout.rstrip('\n')
            assert literal_path == str(path_prefix / 'bin'), f'{shell} PATH suggestion changed the prefix: {literal_path!r}'
            print(f'PASS {shell} printed PATH suggestion preserves literal prefix across shell evaluations')

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
        Path(env['WZI_EXIT75']).touch()
        result = install(prefix, '--autostart')
        assert 'not running' in result.stderr and 'exit 75' in result.stderr, 'immediate App duplicate exit was reported as running'
        assert f'Autostart: {config / "systemd/user/window-zones.service"} (enabled and running)' not in result.stdout
        Path(env['WZI_EXIT75']).unlink()
        install(prefix, '--autostart')
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

        env['WZI_SYSTEMD'] = '1'
        env['WZI_GRAPHICAL'] = '0'
        env['WZI_VERSION'] = 'three'
        install(fallback, '--kwin-script')
        offset = len(events())
        result = install(fallback, '--autostart')
        assert 'XDG login supervision' in result.stdout, 'inactive graphical target did not select XDG'
        assert not any(item['kind'] == 'systemctl' and item['args'][0] in ('enable', 'start', 'restart')
                       for item in events()[offset:]), 'inactive graphical target started systemd units'
        desktop = config / 'autostart/window_zones_kwin.desktop'
        assert desktop.exists() and not (config / 'autostart/window_zones.desktop').exists()
        state_dir = runtime_state(fallback)
        state_dir.mkdir(mode=0o700)
        (state_dir / 'lock').touch()
        lock_inode = (state_dir / 'lock').stat().st_ino
        unrelated_process = subprocess.Popen(['/bin/sleep', '60'])
        launchers.append(unrelated_process)
        (state_dir / 'owner').write_text(f'{unrelated_process.pid}\nnot-its-start-identity\n')
        (state_dir / 'companion').write_text(f'{unrelated_process.pid}\nnot-its-start-identity\n')
        child = launch_desktop(desktop)
        wait_for(lambda: starts('window_zones', 'three'), 'XDG launcher did not start App')
        assert not any(item['kind'] == 'early-app' for item in events())
        duplicate = subprocess.run(desktop_command(desktop), env=env, capture_output=True)
        assert duplicate.returncode == 75, 'second XDG launcher was not rejected'
        assert unrelated_process.poll() is None, 'stale process record signalled an unrelated process'
        old_pids = {starts(role, 'three')[-1]['pid'] for role in ('window_zones', 'window_zones_kwin')}
        child.kill()
        child.wait(timeout=8)
        child = launch_desktop(desktop)
        wait_for(lambda: all(starts(role, 'three')[-1]['pid'] not in old_pids
                             for role in ('window_zones', 'window_zones_kwin')),
                 'launcher crash left a permanent lock or stale supervised children')
        wait_for(lambda: old_pids <= {item.get('pid') for item in events() if item['kind'] == 'exit'},
                 'crashed launcher left its native children running')
        assert (state_dir / 'lock').stat().st_ino == lock_inode, 'runtime lock inode was replaced'
        env['TZ'] = 'Pacific/Honolulu'
        count = len(starts('window_zones', 'three'))
        install(fallback, '--autostart')
        wait_for(lambda: len(starts('window_zones', 'three')) > count, 'timezone change broke owned launcher restart')
        env['WZI_SYSTEMD'] = '0'
        count = len(starts('window_zones', 'three'))
        install(fallback, '--autostart')
        wait_for(lambda: len(starts('window_zones', 'three')) > count, 'unreachable user manager removed working XDG fallback')
        previous = starts('window_zones_kwin', 'three')[-1]['pid']
        os.kill(previous, signal.SIGKILL)
        wait_for(lambda: starts('window_zones_kwin', 'three')[-1]['pid'] != previous, 'XDG companion was not recovered')
        previous = starts('window_zones', 'three')[-1]['pid']
        os.kill(previous, signal.SIGKILL)
        wait_for(lambda: starts('window_zones', 'three')[-1]['pid'] != previous, 'XDG App was not recovered')
        Path(env['WZI_EXIT75']).touch()
        wait_for(lambda: any(item['kind'] == 'exit75' and item.get('version') == 'three' for item in events()), 'App did not exit 75')
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
        assert not (fallback / 'bin/window_zones_kwin').exists()
        assert witness.read_text() == 'user config witness\n'
        print('PASS XDG ordering, companion/App recovery, duplicate-owner exit, reinstall, uninstall')

        # Exit 75 from the companion is permanent too, including before bus readiness.
        install(fallback, '--kwin-script', '--autostart')
        saved_state = state_dir.with_name(state_dir.name + '-saved')
        foreign_runtime = base / 'foreign-runtime'
        foreign_runtime.mkdir(mode=0o700)
        foreign_runtime_witness = foreign_runtime / 'witness'
        foreign_runtime_witness.write_text('foreign runtime witness\n')
        state_dir.rename(saved_state)
        state_dir.symlink_to(foreign_runtime, target_is_directory=True)
        refused = subprocess.run(desktop_command(desktop), env=env, capture_output=True)
        assert refused.returncode != 0 and foreign_runtime_witness.read_text() == 'foreign runtime witness\n'
        assert set(foreign_runtime.iterdir()) == {foreign_runtime_witness}, 'launcher wrote through a foreign runtime symlink'
        state_dir.unlink()
        saved_state.rename(state_dir)
        Path(env['WZI_KWIN_EXIT75']).touch()
        count = len(starts('window_zones_kwin', 'four'))
        duplicate = launch_desktop(desktop)
        assert duplicate.wait(timeout=8) == 75, 'companion duplicate owner did not stop XDG startup'
        time.sleep(2.5)
        assert len(starts('window_zones_kwin', 'four')) == count + 1, 'XDG supervisor retried permanent companion duplicate'
        Path(env['WZI_KWIN_EXIT75']).unlink()
        count = len(starts('window_zones', 'four'))
        child = launch_desktop(desktop)
        wait_for(lambda: len(starts('window_zones', 'four')) > count, 'duplicate-owner stop prevented a later fresh launch')
        orphan_pids = {starts(role, 'four')[-1]['pid'] for role in ('window_zones', 'window_zones_kwin')}
        child.kill()
        child.wait(timeout=8)
        install(fallback, '--uninstall')
        wait_for(lambda: orphan_pids <= {item.get('pid') for item in events() if item['kind'] == 'exit'},
                 'uninstall after launcher crash left supervised binaries alive')
        print('PASS inactive-target/unreachable fallback, stale identity, launcher crash, timezone, companion duplicate')

        # A reachable manager with a failed dbus service must not claim it is running.
        env['WZI_SYSTEMD'] = env['WZI_GRAPHICAL'] = '1'
        Path(env['WZI_KWIN_EXIT75']).touch()
        result = install(fallback, '--kwin-script', '--autostart')
        assert 'not running' in result.stderr and 'exit 75' in result.stderr
        parser = configparser.ConfigParser(interpolation=None)
        parser.read(config / 'systemd/user/window-zones-kwin.service')
        assert parser['Service']['RestartPreventExitStatus'] == '75'
        Path(env['WZI_KWIN_EXIT75']).unlink()
        install(fallback, '--uninstall')

        env['WZI_OS'] = 'Darwin'
        env['XDG_CURRENT_DESKTOP'] = ''
        env['WZI_VERSION'] = 'mac-one'
        mac_prefix = base / 'macOS & <tag> "quoted"'
        mac_replacement = base / 'macOS replacement'
        mac_witness = Path(env['HOME']) / 'Library/Application Support/window_zones/config.toml'
        mac_witness.parent.mkdir(parents=True)
        mac_witness.write_text('mac user config witness\n')
        agent = Path(env['HOME']) / 'Library/LaunchAgents/org.window_zones.run.plist'
        install(mac_prefix, '--autostart', '--init-config')
        wait_for(lambda: starts('window_zones', 'mac-one'), 'LaunchAgent did not start the App')
        plist = plistlib.loads(agent.read_bytes())
        assert plist['ProgramArguments'] == [str(mac_prefix / 'bin/window_zones'), 'run']
        assert plist['RunAtLoad'] is True and 'KeepAlive' not in plist
        assert agent.stat().st_mode & 0o777 == 0o644
        assert plist['StandardOutPath'] == plist['StandardErrorPath'] == str(Path(env['HOME']) / 'Library/Logs/window_zones.log')
        for options, version in (((), 'mac-two'), (('--autostart',), 'mac-three')):
            env['WZI_VERSION'] = version
            offset = len(events())
            install(mac_prefix, *options)
            wait_for(lambda: starts('window_zones', version), 'owned LaunchAgent reinstall did not restart')
            actions = [item['args'][0] for item in events()[offset:] if item['kind'] == 'launchctl']
            assert actions.count('kickstart') == 1 and 'bootstrap' not in actions and 'bootout' not in actions
        Path(env['WZI_EXIT75']).touch()
        result = install(mac_prefix, '--autostart')
        assert 'not running' in result.stderr and 'exit 75' in result.stderr, 'LaunchAgent duplicate exit was not diagnosed'
        count = len(starts('window_zones', 'mac-three'))
        time.sleep(0.5)
        assert len(starts('window_zones', 'mac-three')) == count, 'LaunchAgent retried a duplicate owner'
        Path(env['WZI_EXIT75']).unlink()
        env['WZI_VERSION'] = 'mac-replacement'
        install(mac_replacement, '--autostart')
        wait_for(lambda: starts('window_zones', 'mac-replacement'), 'replacement LaunchAgent did not load')
        replacement_pid = starts('window_zones', 'mac-replacement')[-1]['pid']
        offset = len(events())
        install(mac_prefix)
        result = install(mac_prefix, '--uninstall')
        assert not any(item['kind'] == 'launchctl' and item['args'][0] in ('kickstart', 'bootout', 'bootstrap')
                       for item in events()[offset:]), 'stale prefix changed the replacement LaunchAgent'
        assert 'leaving it untouched' in result.stderr and identity(replacement_pid)
        assert plistlib.loads(agent.read_bytes())['ProgramArguments'][0] == str(mac_replacement / 'bin/window_zones')
        install(mac_replacement, '--uninstall')
        assert not agent.exists() and not identity(replacement_pid)
        assert mac_witness.read_text() == 'mac user config witness\n'
        assert witness.read_text() == 'user config witness\n'
        print('PASS macOS LaunchAgent XML/permissions, bootstrap, kickstart reinstall, duplicate exit, prefix ownership, uninstall')
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
            current = identity(record['pid'])
            if current and current == record['stamp']:
                try:
                    os.kill(record['pid'], signal.SIGTERM)
                except ProcessLookupError:
                    pass
PY
