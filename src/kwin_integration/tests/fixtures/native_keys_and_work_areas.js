
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
function signal() {
    const handlers = [];
    return {connect: callback => handlers.push(callback), emit: () => handlers.forEach(callback => callback())};
}
const left = 'alt+ctrl+left', right = 'alt+ctrl+right', extra = 'alt+ctrl+f1';
const actions = new Map(), nativeKeys = new Map(), writes = [], messages = [];
const timers = [];
let lostClearReply = null;
let workArea = {x: 0, y: 0, width: 1200, height: 860};
const workspace = {
    screens: [{name: 'output-1'}], currentDesktop: {}, activeWindow: null,
    clientArea: () => workArea, currentDesktopChanged: signal(), screensChanged: signal(),
};
const context = vm.createContext({
    workspace, KWin: {MaximizeArea: 2}, console,
    QTimer: function() { this.timeout = signal(); this.start = () => {}; this.stop = () => {}; timers.push(this); },
    registerShortcut: (title, text, sequence, callback) => {
        assert.equal(actions.has(title), false, 'reuse the existing QAction on re-add');
        actions.set(title, callback);
        return true;
    },
    callDBus: (service, path, iface, method, ...args) => {
        const callback = args.pop();
        if (method === 'RegisterCompanion' || method === 'NextRequest')
            return;
        if (method === 'action') {
            const owner = Array.from(nativeKeys).find(([hotkey, key]) => key === args[0]);
            callback(owner ? ['kwin', `Window Zones Hotkey ${owner[0]}`, '', ''] : []);
        } else if (method === 'SetShortcut') {
            assert.ok(actions.has(`Window Zones Hotkey ${args[0]}`));
            writes.push(args);
            if (args[1] === 0)
                nativeKeys.delete(args[0]);
            else
                nativeKeys.set(args[0], args[1]);
            if (args[0] === lostClearReply && args[1] === 0)
                return;
            callback();
        } else {
            messages.push([method, ...args]);
            callback();
        }
    },
});
vm.runInContext(fs.readFileSync(process.argv[1], 'utf8'), context);
function register(keys) {
    context.keys = keys;
    vm.runInContext('registerHotkeys(keys, error => { globalThis.failure = error; globalThis.finished = true; })', context);
    assert.equal(context.finished, true);
    context.finished = false;
    return context.failure;
}
assert.equal(register([left, right]), undefined);
assert.deepEqual(Array.from(nativeKeys.keys()), [left, right]);
lostClearReply = left;
context.keys = [right, extra];
vm.runInContext('registerHotkeys(keys, error => { globalThis.failure = error; globalThis.finished = true; })', context);
assert.equal(context.finished, false);
timers.at(-1).timeout.emit();
assert.equal(context.failure.dbusError, 'org.window_zones.KWin.Error.Unavailable');
assert.equal(nativeKeys.has(left), false, 'the native clear succeeded before its reply was lost');
lostClearReply = null;
assert.equal(register([left, right]), undefined);
assert.deepEqual(Array.from(nativeKeys.keys()).sort(), [left, right].sort(),
    'retrying the old set must restore a key cleared by a partially failed update');
assert.equal(register([right, extra]), undefined);
assert.deepEqual(Array.from(nativeKeys.keys()), [right, extra]);
assert.ok(writes.some(([hotkey, key]) => hotkey === left && key === 0));
const before = writes.length;
assert.throws(() => register([right, 'alt+ctrl+f25']), /unsupported KWin shortcut key/);
assert.equal(writes.length, before, 'preflight rejection must not mutate grabs');
assert.deepEqual(Array.from(nativeKeys.keys()), [right, extra]);
const incumbentKey = vm.runInContext("shortcutForHotkey('alt+ctrl+f2').key", context);
nativeKeys.set('incumbent', incumbentKey);
assert.equal(register([right, 'alt+ctrl+f2']).dbusError, 'org.window_zones.KWin.Error.Unsupported');
assert.equal(writes.length, before, 'native conflict must reject before mutation');
nativeKeys.delete('incumbent');
assert.equal(register([]), undefined);
assert.equal(nativeKeys.size, 0, 'empty set must release native keys, not just callbacks');
assert.equal(register([left]), undefined);
assert.equal(nativeKeys.size, 1);
assert.equal(actions.size, 3);
actions.get(`Window Zones Hotkey ${left}`)();
assert.ok(messages.some(message => message[0] === 'HotkeyPressed' && message[1] === left));
messages.length = 0;
workArea = {x: 0, y: 50, width: 1200, height: 810};
vm.runInContext("processRequest(10, 'refresh-displays', '', 0, 0, 0, 0, '[]', () => {})", context);
assert.deepEqual(JSON.parse(messages[0][1]), [['output-1', 0, 50, 1200, 810]]);
assert.equal(messages[1][0], 'CompleteRequest');
assert.equal(messages[1][2], true, 'complete only after the fresh state was published');
messages.length = 0;
workArea = {x: 0, y: 0, width: 1200, height: 900};
workspace.currentDesktopChanged.emit();
assert.deepEqual(JSON.parse(messages[0][1]), [['output-1', 0, 0, 1200, 900]]);
