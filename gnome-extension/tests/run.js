#!/usr/bin/env -S gjs -m
// Exercise the real service dispatcher without a Shell session or session bus.
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import {display, makeWindow, Main, Meta, Shell} from './stubs.js';

const here = Gio.File.new_for_uri(import.meta.url).get_parent();
const source = here.get_parent().get_child('extension.js');
const stubs = here.get_child('stubs.js').get_uri();
const [, bytes] = source.load_contents(null);
const redirected = new TextDecoder().decode(bytes)
    .replace(/^import Meta from 'gi:\/\/Meta';$/m, `import {Meta} from '${stubs}';`)
    .replace(/^import Shell from 'gi:\/\/Shell';$/m, `import {Shell} from '${stubs}';`)
    .replace(/^import \* as Main from 'resource:.*$/m, `import {Main} from '${stubs}';`)
    .replace(/^import \{Extension\} from 'resource:.*$/m, `import {Extension} from '${stubs}';`);
const [fd, copy] = GLib.file_open_tmp('window-zones-contract-XXXXXX.js');
GLib.close(fd);
let companion;
try {
    GLib.file_set_contents(copy, redirected);
    companion = await import(Gio.File.new_for_path(copy).get_uri());
} finally {
    GLib.unlink(copy);
}
const {WindowZonesService, INTERFACE_XML} = companion;
const info = Gio.DBusNodeInfo.new_for_xml(INTERFACE_XML).interfaces[0];
const methods = new Map(info.methods.map(method => [method.name, method]));

let checks = 0;
function equal(actual, expected, description) {
    checks++;
    if (JSON.stringify(actual) !== JSON.stringify(expected))
        throw new Error(`${description}: got ${JSON.stringify(actual)}, want ${JSON.stringify(expected)}`);
}

equal(info.name, 'org.window_zones.Gnome2', 'strict interface major');
equal(Object.fromEntries(info.methods.map(method => [method.name, [
    `(${(method.in_args ?? []).map(arg => arg.signature).join('')})`,
    `(${(method.out_args ?? []).map(arg => arg.signature).join('')})`,
]])), {
    GetCapabilities: ['()', '(as)'],
    GetFocusedWindow: ['()', '(btiiuu)'],
    GetDisplays: ['()', '(a(siiuu))'],
    MoveWindow: ['(tiiuu)', '()'],
    RegisterHotkeys: ['(as)', '()'],
}, 'interface XML matches the wire contract');

const connection = {
    emitted: [],
    register_object(path, interfaceInfo, callback) {
        equal(path, '/org/window_zones/Gnome', 'stable object path');
        equal(interfaceInfo.name, info.name, 'exported interface');
        this.methodCall = callback;
        return 11;
    },
    unregister_object(id) { equal(id, 11, 'object registration released'); },
    signal_subscribe(_sender, _interface, _signal, _path, _arg0, _flags, callback) {
        this.ownerChanged = callback;
        return 12;
    },
    signal_unsubscribe(id) { equal(id, 12, 'owner listener released'); },
    emit_signal(destination, path, interfaceName, name, variant) {
        equal([destination, path, interfaceName, name, variant.get_type_string()],
            [null, '/org/window_zones/Gnome', info.name, 'HotkeyPressed', '(s)'],
            'hotkey signal contract');
        this.emitted.push(variant.deep_unpack());
    },
    vanish(sender) {
        this.ownerChanged(this, 'org.freedesktop.DBus', '/org/freedesktop/DBus',
            'org.freedesktop.DBus', 'NameOwnerChanged',
            new GLib.Variant('(sss)', [sender, sender, '']));
    },
};
const service = new WindowZonesService();
service.export(connection);

function call(name, args = [], sender = ':1.42', expectedError = null) {
    const method = methods.get(name);
    const inSignature = `(${(method.in_args ?? []).map(arg => arg.signature).join('')})`;
    const outSignature = `(${(method.out_args ?? []).map(arg => arg.signature).join('')})`;
    let replies = 0;
    let result;
    connection.methodCall(connection, sender, '/org/window_zones/Gnome', info.name, name,
        new GLib.Variant(inSignature, args), {
            return_value(variant) {
                replies++;
                equal(expectedError, null, `${name} must not succeed`);
                equal(variant.get_type_string(), outSignature, `${name} reply matches XML out args`);
                result = variant.deep_unpack();
            },
            return_dbus_error(errorName, message) {
                replies++;
                equal(errorName, expectedError, `${name} error: ${message}`);
                result = message;
            },
        });
    equal(replies, 1, `${name} returns exactly one reply`);
    return result;
}
const invalid = 'org.window_zones.Gnome.Error.InvalidWindowState';
const unsupported = 'org.window_zones.Gnome.Error.Unsupported';
const window = makeWindow(4294967301);
const other = makeWindow(4294967302);
display.windows = [window, other];
display.focus = window;

equal(call('GetCapabilities'), [['focused-window', 'displays', 'move-resize', 'hotkeys']],
    'capabilities are one array out argument');
equal(call('GetDisplays'), [[
    ['display-1', -1920, 27, 1920, 1053],
    ['display-2', 0, 0, 2560, 1440],
]], 'display work areas exclude the panel and retain negative coordinates');
const focused = call('GetFocusedWindow');
equal(focused, [true, 4294967301, -300, -20, 801, 602], 'focused frame carries uint64 identity');
display.focus = other;
equal(call('MoveWindow', [focused[1], 10, 20, 960, 1053]), [], 'move returns the empty tuple');
equal(window.operations, [['move', true, 10, 20, 960, 1053]], 'move identifies original window after focus changed');
equal(other.operations, [], 'new focused window is untouched');

for (const [flags, tiled] of [[1, false], [2, false], [3, false], [2, true]]) {
    window.maximizeFlags = flags;
    window.tiled = tiled;
    window.operations = [];
    call('MoveWindow', [window.id, -100, 27, 600, 700]);
    equal(window.operations, [['restore'], ['move', true, -100, 27, 600, 700]],
        `maximize flags ${flags}, tiled ${tiled}: restore precedes exact frame move`);
    equal([window.maximizeFlags, window.tiled], [0, false], 'restore clears constrained state');
}
window.operations = [];
window.fullscreen = true;
window.maximizeFlags = 3;
call('MoveWindow', [window.id, 0, 0, 100, 100], ':1.42', invalid);
equal(window.operations, [], 'fullscreen rejection does not restore or move');
window.fullscreen = false;
window.resizable = false;
call('MoveWindow', [window.id, 0, 0, 100, 100], ':1.42', invalid);
equal(window.operations, [], 'non-resizable rejection leaves state untouched');
window.resizable = true;
window.maximizeFlags = 0;
call('MoveWindow', [window.id, 0, 0, 0, 100], ':1.42', invalid);
call('MoveWindow', [77, 0, 0, 100, 100], ':1.42', 'org.window_zones.Gnome.Error.WindowGone');

display.focus = null;
equal(call('GetFocusedWindow'), [false, 0, 0, 0, 0, 0], 'absent focus retains uint64 reply type');
for (const overrides of [
    {windowType: Meta.WindowType.DOCK}, {pid: 9999}, {overrideRedirect: true},
    {frame: {x: 0, y: 0, width: 0, height: 10}},
]) {
    display.focus = makeWindow(99, overrides);
    equal(call('GetFocusedWindow')[0], false, 'Shell and unusable surfaces are excluded');
}
display.focus = window;
for (const [object, property] of [
    [Main.overview, 'visible'], [Main.sessionMode, 'isLocked'], [Main.screenShield, 'locked'],
]) {
    object[property] = true;
    equal(call('GetFocusedWindow')[0], false, `${property} excludes focused applications`);
    call('MoveWindow', [window.id, 0, 0, 100, 100], ':1.42', invalid);
    object[property] = false;
}

call('RegisterHotkeys', [['alt+ctrl+left', 'cmd+1', 'shift+f24']]);
const originalGrabs = [...display.grabbed];
equal(originalGrabs.map(([, accelerator]) => accelerator),
    ['<Alt><Control>Left', '<Super>1', '<Shift>F24'], 'canonical vocabulary maps to accelerators');
for (const action of display.grabbed.keys())
    equal(Main.wm.allowed.get(Meta.external_binding_name_for_action(action)), Shell.ActionMode.NORMAL,
        'external accelerator is enabled in normal action mode');
display.activate(originalGrabs[0][0]);
equal(connection.emitted, [['alt+ctrl+left']], 'event echoes canonical hotkey');
for (const hotkey of [
    'Ctrl+Left', ' ctrl+left', 'ctrl+left ', 'a\n', 'f24\n', 'ctrl+alt+left', 'super+left',
    'ctrl++left', 'ctrl+ctrl+left', 'shift+cmd+ctrl+left', 'ctrl+F1', 'ctrl+f25', 'ctrl',
]) {
    call('RegisterHotkeys', [[hotkey]], ':1.42', unsupported);
    equal([...display.grabbed], originalGrabs, `reject '${hotkey}' without changing previous set`);
}
call('RegisterHotkeys', [['alt+ctrl+left', 'alt+ctrl+left']], ':1.42', unsupported);
display.rejected.add('<Alt>Right');
call('RegisterHotkeys', [['alt+ctrl+left', 'ctrl+right', 'alt+right']], ':1.42', unsupported);
equal([...display.grabbed], originalGrabs, 'failed replacement rolls back new grabs and reuses retained ones');
equal(Main.wm.allowed.get('external-grab-4'), Shell.ActionMode.NONE, 'rollback disables newly allowed binding');
display.rejected.clear();
call('RegisterHotkeys', [['ctrl+cmd+left']], ':1.99', 'org.window_zones.Gnome.Error.Busy');
equal([...display.grabbed], originalGrabs, 'another controller cannot replace the set');
connection.vanish(':1.99');
equal([...display.grabbed], originalGrabs, 'unrelated owner loss does not release grabs');
call('RegisterHotkeys', [['alt+ctrl+left', 'ctrl+cmd+right']]);
equal([...display.grabbed][0], originalGrabs[0], 'replacement retains unchanged action identity');
display.activate(originalGrabs[1][0]);
equal(connection.emitted.length, 1, 'removed accelerator no longer emits');
connection.vanish(':1.42');
equal(display.grabbed.size, 0, 'owner vanish releases every grab');
equal([...Main.wm.allowed.values()].every(mode => mode === Shell.ActionMode.NONE), true,
    'owner vanish disables every allowed binding');
display.activate(originalGrabs[0][0]);
equal(connection.emitted.length, 1, 'owner vanish clears action lookup');
call('RegisterHotkeys', [['ctrl+cmd+left']], ':1.99');
call('RegisterHotkeys', [[]], ':1.99');
equal(display.grabbed.size, 0, 'empty set releases controller ownership and grabs');
call('RegisterHotkeys', [['ctrl+cmd+left']]);
service.destroy();
equal(display.grabbed.size, 0, 'destroy releases every grab');
equal(display.handler, null, 'destroy disconnects accelerator listener');
print(`GNOME companion contract: ${checks} checks passed`);
