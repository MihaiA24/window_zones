
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
function signal() {
    const handlers = new Set();
    return {
        connect: callback => handlers.add(callback),
        disconnect: callback => handlers.delete(callback),
        emit: value => Array.from(handlers).forEach(callback => callback(value)),
        count: () => handlers.size,
    };
}
function window(id) {
    const calls = [];
    let geometry = {x: 0, y: 0, width: 640, height: 480};
    return {
        internalId: id, managed: true, normalWindow: true, moveable: true, resizeable: true,
        frameGeometryChanged: signal(), calls,
        setMaximize: (vertical, horizontal) => calls.push([vertical, horizontal]),
        get frameGeometry() { return geometry; },
        set frameGeometry(value) { calls.push('frame'); geometry = value; },
    };
}
const first = window('window-1');
const second = window('window-2');
const updates = [];
const workspace = {
    activeWindow: first, screens: [], stackingOrder: [first, second],
    windowActivated: signal(), windowRemoved: signal(),
};
const context = vm.createContext({
    workspace, console,
    QTimer: function() { this.timeout = signal(); this.start = () => {}; this.stop = () => {}; },
    callDBus: (...args) => { if (args[3] === 'UpdateFocusedWindow') updates.push(JSON.parse(args[4])); },
});
vm.runInContext(fs.readFileSync(process.argv[1], 'utf8'), context);
for (let i = 0; i < 100; i++) {
    workspace.activeWindow = i % 2 ? first : second;
    workspace.windowActivated.emit(workspace.activeWindow);
}
assert.equal(first.frameGeometryChanged.count(), 1);
assert.equal(second.frameGeometryChanged.count(), 1);
updates.length = 0;
first.frameGeometryChanged.emit();
assert.equal(updates.length, 1);
assert.equal(updates[0][1], 'window-1');
workspace.activeWindow = second;
first.tile = {unmanage: value => {
    assert.equal(value, first);
    first.calls.push('untile');
    first.tile = null;
}};
vm.runInContext("moveWindow('window-1', -10, 20, 800, 600, error => { if (error) throw error; })", context);
assert.equal(JSON.stringify(first.frameGeometry), '{"x":-10,"y":20,"width":800,"height":600}');
assert.deepEqual(first.calls, ['untile', [false, false], 'frame']);
assert.equal(second.calls.length, 0);
first.calls.length = 0;
first.tile = {};
vm.runInContext("moveWindow('window-1', 10, 20, 800, 600, error => { if (error) throw error; })", context);
assert.equal(first.tile, null);
assert.deepEqual(first.calls, [[true, true], [false, false], 'frame']);
first.fullScreen = true;
first.calls.length = 0;
assert.throws(() => vm.runInContext("moveWindow('window-1', 0, 0, 800, 600)", context), /leave fullscreen first/);
assert.equal(first.calls.length, 0);
assert.throws(() => vm.runInContext("moveWindow('missing', 0, 0, 800, 600)", context),
    error => error.dbusError === 'org.window_zones.KWin.Error.WindowGone' && error.message === 'missing');
workspace.windowRemoved.emit(first);
assert.equal(first.frameGeometryChanged.count(), 0);
for (const hotkey of ['ctrl+alt+a', 'ALT+a', 'alt+ctrl+f25', 'alt++a', 'alt+enter']) {
    assert.throws(() => vm.runInContext(`shortcutForHotkey(${JSON.stringify(hotkey)})`, context),
        error => error.dbusError === 'org.window_zones.KWin.Error.Unsupported');
}
