#!/usr/bin/env -S gjs -m
// Exercise the real service dispatcher without a Shell session or session bus.
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import {display, makeWindow, Main, Meta, settings, Shell, St, TestActor} from './stubs.js';

const here = Gio.File.new_for_uri(import.meta.url).get_parent();
const source = here.get_parent().get_child('extension.js');
const stubs = here.get_child('stubs.js').get_uri();
const [, bytes] = source.load_contents(null);
const redirected = new TextDecoder().decode(bytes)
    .replace(/^import Clutter from 'gi:\/\/Clutter';$/m, `import {Clutter} from '${stubs}';`)
    .replace(/^import St from 'gi:\/\/St';$/m, `import {St} from '${stubs}';`)
    .replace(/^import Meta from 'gi:\/\/Meta';$/m, `import {Meta} from '${stubs}';`)
    .replace(/^import Shell from 'gi:\/\/Shell';$/m, `import {Shell} from '${stubs}';`)
    .replace(/^import \* as Main from 'resource:.*$/m, `import {Main} from '${stubs}';`)
    .replace(/^import \{WINDOW_ANIMATION_TIME\} from 'resource:.*$/m, `import {WINDOW_ANIMATION_TIME} from '${stubs}';`)
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
    GetLastTransitionMetrics: ['()', '(dd)'],
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
        this.emitted.push({destination, hotkey: variant.deep_unpack()[0]});
    },
    vanish(sender) {
        this.ownerChanged(this, 'org.freedesktop.DBus', '/org/freedesktop/DBus',
            'org.freedesktop.DBus', 'NameOwnerChanged',
            new GLib.Variant('(sss)', [sender, sender, '']));
    },
};
const service = new WindowZonesService();
service.export(connection);

async function call(name, args = [], sender = ':1.42', expectedError = null) {
    const method = methods.get(name);
    const inSignature = `(${(method.in_args ?? []).map(arg => arg.signature).join('')})`;
    const outSignature = `(${(method.out_args ?? []).map(arg => arg.signature).join('')})`;
    let replies = 0;
    const reply = await new Promise(resolve => {
        const timeout = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 1000, () => {
            resolve(null);
            return GLib.SOURCE_REMOVE;
        });
        const respond = result => {
            replies++;
            if (replies === 1) {
                GLib.source_remove(timeout);
                resolve(result);
            }
        };
        connection.methodCall(connection, sender, '/org/window_zones/Gnome', info.name, name,
            new GLib.Variant(inSignature, args), {
                return_value(variant) { respond({variant}); },
                return_dbus_error(errorName, message) { respond({errorName, message}); },
            });
    });
    equal(replies, 1, `${name} returns exactly one reply within one second`);
    if (reply.variant) {
        equal(expectedError, null, `${name} must not succeed`);
        equal(reply.variant.get_type_string(), outSignature, `${name} reply matches XML out args`);
        return reply.variant.deep_unpack();
    }
    equal(reply.errorName, expectedError, `${name} error: ${reply.message}`);
    return reply.message;
}

function delay(milliseconds) {
    return new Promise(resolve => GLib.timeout_add(GLib.PRIORITY_DEFAULT, milliseconds, () => {
        resolve();
        return GLib.SOURCE_REMOVE;
    }));
}
const invalid = 'org.window_zones.Gnome.Error.InvalidWindowState';
const unsupported = 'org.window_zones.Gnome.Error.Unsupported';
const partial = 'org.window_zones.Gnome.Error.PartialMove';
const gone = 'org.window_zones.Gnome.Error.WindowGone';
const window = makeWindow(4294967301);
const other = makeWindow(4294967302);
display.windows = [window, other];
display.focus = window;

equal(await call('GetCapabilities'), [['focused-window', 'displays', 'move-resize', 'hotkeys', 'transition-metrics']],
    'capabilities are one array out argument');
equal(await call('GetDisplays'), [[
    ['display-1', -1920, 27, 1920, 1053],
    ['display-2', 0, 0, 2560, 1440],
]], 'display work areas exclude the panel and retain negative coordinates');
const focused = await call('GetFocusedWindow');
equal(focused, [true, 4294967301, -300, -20, 801, 602], 'focused frame carries uint64 identity');
display.focus = other;
equal(await call('MoveWindow', [focused[1], 10, 20, 960, 1053]), [], 'move returns the empty tuple');
equal(window.operations, [['move', true, 10, 20, 960, 1053]], 'move identifies original window after focus changed');
equal(other.operations, [], 'new focused window is untouched');

for (const [flags, tiled] of [[1, false], [2, false], [3, false], [2, true]]) {
    window.maximizeFlags = flags;
    window.tiled = tiled;
    window.operations = [];
    await call('MoveWindow', [window.id, -100, 27, 600, 700]);
    equal(window.operations, [['restore'], ['move', true, -100, 27, 600, 700]],
        `maximize flags ${flags}, tiled ${tiled}: restore precedes exact frame move`);
    equal([window.maximizeFlags, window.tiled], [0, false], 'restore clears constrained state');
}
window.operations = [];
window.fullscreen = true;
window.maximizeFlags = 3;
await call('MoveWindow', [window.id, 0, 0, 100, 100], ':1.42', invalid);
equal(window.operations, [], 'fullscreen rejection does not restore or move');
window.fullscreen = false;
window.resizable = false;
await call('MoveWindow', [window.id, 0, 0, 100, 100], ':1.42', invalid);
equal(window.operations, [], 'non-resizable rejection leaves state untouched');
window.resizable = true;
window.maximizeFlags = 0;
await call('MoveWindow', [window.id, 0, 0, 0, 100], ':1.42', invalid);
await call('MoveWindow', [77, 0, 0, 100, 100], ':1.42', 'org.window_zones.Gnome.Error.WindowGone');

display.focus = null;
equal(await call('GetFocusedWindow'), [false, 0, 0, 0, 0, 0], 'absent focus retains uint64 reply type');
for (const overrides of [
    {windowType: Meta.WindowType.DOCK}, {pid: new Gio.Credentials().get_unix_pid()}, {overrideRedirect: true},
    {frame: {x: 0, y: 0, width: 0, height: 10}},
]) {
    display.focus = makeWindow(99, overrides);
    equal((await call('GetFocusedWindow'))[0], false, 'Shell and unusable surfaces are excluded');
}
display.focus = window;
for (const [object, property] of [
    [Main.overview, 'visible'], [Main.sessionMode, 'isLocked'], [Main.screenShield, 'locked'],
]) {
    object[property] = true;
    equal((await call('GetFocusedWindow'))[0], false, `${property} excludes focused applications`);
    await call('MoveWindow', [window.id, 0, 0, 100, 100], ':1.42', invalid);
    object[property] = false;
}

window.operations = [];
await call('MoveWindow', [window.id, -1800, 27, 600, 700]);
equal(window.operations, [['monitor', 0], ['move', true, -1800, 27, 600, 700]],
    'display move changes monitor bounds before applying the exact target');
const actor = new TestActor();
window.actor = actor;
window.maximizeFlags = 3;
window.operations = [];
await call('MoveWindow', [window.id, 10, 20, 600, 700]);
equal(window.operations, [['restore'], ['monitor', 1], ['move', true, 10, 20, 600, 700]],
    'animated maximized display move waits for restore before changing monitor');
await call('GetFocusedWindow', [], ':1.99');
await call('GetLastTransitionMetrics', [], ':1.99', invalid);
let metrics = await call('GetLastTransitionMetrics');
equal(metrics[0] >= 0 && metrics[0] < metrics[1] && metrics[1] >= 200, true,
    'Shell 50 without the reduced-motion API preserves the finished compositor animation');
equal(actor.frozen, false, 'animation releases the frozen live actor');
equal(global.window_group.children.every(clone => clone.destroyed), true, 'completed animation removes snapshots');
await call('GetLastTransitionMetrics', [], ':1.42', invalid);
St.ReducedMotion = {REDUCE: 1};
for (const [animations, reducedMotion, x] of [[false, 0, 30], [true, 1, 50], [true, 0, 70]]) {
    settings.enable_animations = animations;
    settings.reducedMotion = reducedMotion;
    await call('MoveWindow', [window.id, x, 40, 600, 700]);
    metrics = await call('GetLastTransitionMetrics');
    equal(metrics[1] >= 200, animations && reducedMotion === 0,
        `Shell 51 animations ${animations}, reduced motion ${reducedMotion}: preserves animation preference`);
}
settings.enable_animations = true;
settings.reducedMotion = 0;
window.actor = null;
for (const discovery of ['GetFocusedWindow', 'GetDisplays']) {
    await call('MoveWindow', [window.id, 80, 40, 600, 700]);
    await call(discovery);
    await call('GetLastTransitionMetrics', [], ':1.42', invalid);
}
await call('MoveWindow', [window.id, 90, 40, 600, 700]);
await call('MoveWindow', [window.id, 90, 40, 0, 700], ':1.42', invalid);
await call('GetLastTransitionMetrics', [], ':1.42', invalid);
await call('MoveWindow', [window.id, 100, 40, 600, 700]);
connection.vanish(':1.42');
await call('GetLastTransitionMetrics', [], ':1.42', invalid);

const failureWindow = makeWindow(4294967303);
display.windows.push(failureWindow);
display.focus = other;
const resize = failureWindow.move_resize_frame;
failureWindow.move_resize_frame = function (_userOp, x, y, width, height) {
    this.frame = {x, y, width: Math.max(500, width), height};
};
let failure = JSON.parse(await call('MoveWindow', [failureWindow.id, 100, 200, 300, 400], ':1.42', partial));
equal(failure.window_id, String(failureWindow.id), 'partial move retains the requested identity despite other focus');
equal(failure.observed, [100, 200, 500, 400], 'constrained placement reports the actual frame');
equal(failure.message.includes('did not reach requested'), true, 'partial move retains the original failure');
await call('GetLastTransitionMetrics', [], ':1.42', invalid);
await call('MoveWindow', [failureWindow.id, 100, 200, 300, 400], ':1.42', invalid);

const superseded = call('MoveWindow', [failureWindow.id, 200, 300, 300, 400], ':1.42', partial);
failureWindow.move_resize_frame = resize;
await call('MoveWindow', [failureWindow.id, 300, 400, 600, 700], ':1.99');
failure = JSON.parse(await superseded);
equal(failure.observed, [200, 300, 500, 400], 'superseded request reads its frame before the replacement mutates it');
equal(failure.message, 'transition superseded by another request', 'supersession preserves its failure reason');
const unmaximize = failureWindow.unmaximize;
failureWindow.maximizeFlags = 3;
failureWindow.unmaximize = function () {
    this.frame.x = 350;
    throw new Error('restore interrupted');
};
failure = JSON.parse(await call('MoveWindow', [failureWindow.id, 400, 500, 600, 700], ':1.42', partial));
equal(failure.observed, [350, 400, 600, 700], 'failed state restoration reports changed geometry even before placement');
equal(failure.message, 'restore interrupted', 'state restoration preserves its failure reason');
failureWindow.unmaximize = unmaximize;
failureWindow.maximizeFlags = 0;

failureWindow.actor = new TestActor();
const interrupted = call('MoveWindow', [failureWindow.id, 400, 500, 600, 700], ':1.42', partial);
await delay(40);
global.window_group.children.at(-1).remove_all_transitions();
failure = JSON.parse(await interrupted);
equal(failure.observed, [400, 500, 600, 700], 'interrupted animation preserves the moved frame');
equal(failure.message, 'animation interrupted', 'interrupted animation remains a failure');
equal(failureWindow.actor.frozen, false, 'interrupted animation thaws its actor');
failureWindow.actor = null;

const readFrame = failureWindow.get_frame_rect;
failureWindow.move_resize_frame = function (_userOp, x, y, width, height) {
    this.frame = {x, y, width, height};
    this.get_frame_rect = () => { throw new Error('frame readback failed'); };
    throw new Error('placement interrupted');
};
await call('MoveWindow', [failureWindow.id, 10, 20, 600, 700], ':1.42', 'org.freedesktop.DBus.Error.Failed');
failureWindow.get_frame_rect = readFrame;
for (const badFrame of [
    {x: 10, y: 20, width: 0, height: 700},
    {x: 10, y: 20, width: -1, height: 700},
    {x: 10.5, y: 20, width: 600, height: 700},
    {x: 2 ** 31, y: 20, width: 600, height: 700},
    {x: 10, y: 20, width: 2 ** 32, height: 700},
    {x: NaN, y: 20, width: 600, height: 700},
]) {
    failureWindow.frame = {x: 10, y: 20, width: 600, height: 700};
    failureWindow.move_resize_frame = function () {
        this.frame = badFrame;
        throw new Error('placement interrupted');
    };
    await call('MoveWindow', [failureWindow.id, 30, 40, 600, 700], ':1.42', 'org.freedesktop.DBus.Error.Failed');
}
failureWindow.frame = {x: 10, y: 20, width: 600, height: 700};
failureWindow.actor = new TestActor();
failureWindow.move_resize_frame = function (_userOp, x, y, width, height) {
    this.frame = {x, y, width: width + 100, height};
};
const closed = call('MoveWindow', [failureWindow.id, 30, 40, 600, 700], ':1.42', gone);
display.windows = display.windows.filter(candidate => candidate !== failureWindow);
failureWindow.actor.destroy();
await closed;
failureWindow.actor = null;
failureWindow.move_resize_frame = resize;
display.windows.push(failureWindow);

await call('RegisterHotkeys', [['alt+ctrl+left', 'cmd+1', 'shift+f24']]);
const originalGrabs = [...display.grabbed];
equal(originalGrabs.map(([, accelerator]) => accelerator),
    ['<Alt><Control>Left', '<Super>1', '<Shift>F24'], 'canonical vocabulary maps to accelerators');
for (const action of display.grabbed.keys())
    equal(Main.wm.allowed.get(Meta.external_binding_name_for_action(action)), Shell.ActionMode.NORMAL,
        'external accelerator is enabled in normal action mode');
display.activate(originalGrabs[0][0]);
equal(connection.emitted, [{destination: ':1.42', hotkey: 'alt+ctrl+left'}],
    'only the registered controller receives its hotkey event');
for (const hotkey of [
    'Ctrl+Left', ' ctrl+left', 'ctrl+left ', 'a\n', 'f24\n', 'ctrl+alt+left', 'super+left',
    'ctrl++left', 'ctrl+ctrl+left', 'shift+cmd+ctrl+left', 'ctrl+F1', 'ctrl+f25', 'ctrl',
]) {
    await call('RegisterHotkeys', [[hotkey]], ':1.42', unsupported);
    equal([...display.grabbed], originalGrabs, `reject '${hotkey}' without changing previous set`);
}
await call('RegisterHotkeys', [['alt+ctrl+left', 'alt+ctrl+left']], ':1.42', unsupported);
display.rejected.add('<Alt>Right');
await call('RegisterHotkeys', [['alt+ctrl+left', 'ctrl+right', 'alt+right']], ':1.42', unsupported);
equal([...display.grabbed], originalGrabs, 'failed replacement rolls back new grabs and reuses retained ones');
equal(Main.wm.allowed.get('external-grab-4'), Shell.ActionMode.NONE, 'rollback disables newly allowed binding');
display.rejected.clear();
await call('RegisterHotkeys', [['ctrl+cmd+left']], ':1.99', 'org.window_zones.Gnome.Error.Busy');
equal([...display.grabbed], originalGrabs, 'another controller cannot replace the set');
connection.vanish(':1.99');
equal([...display.grabbed], originalGrabs, 'unrelated owner loss does not release grabs');
await call('RegisterHotkeys', [['alt+ctrl+left', 'ctrl+cmd+right']]);
equal([...display.grabbed][0], originalGrabs[0], 'replacement retains unchanged action identity');
display.activate(originalGrabs[1][0]);
equal(connection.emitted.length, 1, 'removed accelerator no longer emits');
connection.vanish(':1.42');
equal(display.grabbed.size, 0, 'owner vanish releases every grab');
equal([...Main.wm.allowed.values()].every(mode => mode === Shell.ActionMode.NONE), true,
    'owner vanish disables every allowed binding');
display.activate(originalGrabs[0][0]);
equal(connection.emitted.length, 1, 'owner vanish clears action lookup');
await call('RegisterHotkeys', [['ctrl+cmd+left']], ':1.99');
await call('RegisterHotkeys', [[]], ':1.99');
equal(display.grabbed.size, 0, 'empty set releases controller ownership and grabs');
await call('RegisterHotkeys', [['ctrl+cmd+left']]);
const disabled = call('MoveWindow', [failureWindow.id, 50, 60, 600, 700], ':1.42', partial);
service.destroy();
failure = JSON.parse(await disabled);
equal(failure.observed, [50, 60, 600, 700], 'companion teardown preserves the changed frame');
equal(failure.message, 'companion disabled during transition', 'teardown retains its failure reason');
equal(display.grabbed.size, 0, 'destroy releases every grab');
equal(display.handler, null, 'destroy disconnects accelerator listener');
print(`GNOME companion contract: ${checks} checks passed`);
