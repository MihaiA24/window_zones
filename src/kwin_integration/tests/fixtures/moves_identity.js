
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
        maximizeMode: 0, frameGeometryChanged: signal(), maximizedChanged: signal(),
        tileChanged: signal(), damaged: signal(), calls,
        setMaximize(vertical, horizontal) {
            calls.push([vertical, horizontal]);
            this.maximizeMode = (vertical ? 2 : 0) | (horizontal ? 1 : 0);
            this.maximizedChanged.emit();
        },
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
let now = 0;
let completions = 0;
const timers = [];
function advance(milliseconds) {
    const target = now + milliseconds;
    for (;;) {
        const timer = timers.filter(timer => timer.active && timer.deadline <= target)
            .sort((left, right) => left.deadline - right.deadline)[0];
        if (!timer)
            break;
        now = timer.deadline;
        if (timer.singleShot)
            timer.active = false;
        else
            timer.deadline += timer.interval;
        timer.timeout.emit();
    }
    now = target;
}
const context = vm.createContext({
    workspace, console,
    completeMove: error => { assert.equal(error, undefined); completions++; },
    QTimer: function() {
        this.timeout = signal();
        this.start = () => { this.active = true; this.deadline = now + this.interval; };
        this.stop = () => { this.active = false; };
        timers.push(this);
    },
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
vm.runInContext("moveWindow('window-1', -10, 20, 800, 600, completeMove)", context);
assert.equal(completions, 0, 'untile must settle before placement completes');
first.damaged.emit(first);
advance(750);
assert.equal(completions, 1);
assert.equal(JSON.stringify(first.frameGeometry), '{"x":-10,"y":20,"width":800,"height":600}');
assert.equal(first.tile, null);
assert.equal(first.maximizeMode, 0);
assert.equal(second.calls.length, 0);
first.calls.length = 0;
first.tile = {};
vm.runInContext("moveWindow('window-1', 10, 20, 800, 600, completeMove)", context);
first.damaged.emit(first);
advance(750);
assert.equal(completions, 2);
assert.equal(JSON.stringify(first.frameGeometry), '{"x":10,"y":20,"width":800,"height":600}');
assert.equal(first.tile, null);
assert.equal(first.maximizeMode, 0);
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
