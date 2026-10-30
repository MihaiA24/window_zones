'use strict';

const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

function signal() {
    const handlers = new Set();
    return {
        connect: callback => handlers.add(callback),
        disconnect: callback => handlers.delete(callback),
        emit: (...args) => Array.from(handlers).forEach(callback => callback(...args)),
    };
}

function scriptSession(windows = []) {
    let now = 0;
    const timers = [];
    const calls = [];
    const pending = [];
    const shortcuts = new Map();
    const workspace = {
        screens: [{name: 'output-1'}], currentDesktop: {}, activeWindow: windows[0] || null,
        stackingOrder: windows, windowRemoved: signal(),
        clientArea: () => ({x: 0, y: 40, width: 1200, height: 860}),
    };
    class QTimer {
        constructor() {
            this.timeout = signal();
            this.active = false;
            timers.push(this);
        }
        start() {
            this.active = true;
            this.deadline = now + this.interval;
        }
        stop() {
            this.active = false;
        }
    }
    const context = vm.createContext({
        console, QTimer, Date: {now: () => now},
        KWin: {MaximizeArea: 2},
        workspace,
        registerShortcut: (title, _text, _sequence, callback) => {
            shortcuts.set(title, callback);
            return true;
        },
        callDBus: (service, objectPath, iface, method, ...args) => {
            const callback = args.pop();
            const call = {service, objectPath, iface, method, args, callback};
            calls.push(call);
            pending.push(call);
        },
    });
    vm.runInContext(fs.readFileSync(path.join(__dirname, '../contents/code/main.js'), 'utf8'), context);
    return {
        workspace, shortcuts,
        calls: method => calls.filter(call => call.method === method),
        take(method) {
            const index = pending.findIndex(call => call.method === method);
            assert.notEqual(index, -1, `KWin must call ${method}`);
            return pending.splice(index, 1)[0];
        },
        advance(milliseconds) {
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
        },
    };
}

function refresh(poll, id) {
    poll.callback(id, 'refresh-displays', '', 0, 0, 0, 0, '[]');
}

const session = scriptSession();
session.take('RegisterCompanion').callback();
session.take('UpdateDisplays').callback();
session.take('UpdateFocusedWindow').callback();
session.advance(2000);
assert.equal(session.calls('NextRequest').length, 1, 'a healthy long poll must not be duplicated');
assert.equal(session.calls('RegisterCompanion').length, 1);

refresh(session.take('NextRequest'), 1);
const staleDisplay = session.take('UpdateDisplays');
assert.equal(session.calls('CompleteRequest').length, 0, 'refresh must wait for the display reply');
// The service disappears while the refresh is active: Qt never delivers its error callback.
session.advance(1000);
assert.equal(session.calls('CompleteRequest').length, 1, 'a missing display callback must finish before the App deadline');
assert.deepEqual(session.calls('CompleteRequest')[0].args.slice(0, 3),
    ['1', false, 'org.window_zones.KWin.Error.Unavailable']);
const stalePoll = session.take('NextRequest');
session.advance(4000);
const reconnect = session.take('RegisterCompanion');
assert.deepEqual(reconnect.args, ['3', false]);
reconnect.callback();
session.take('UpdateDisplays').callback();
session.take('UpdateFocusedWindow').callback();

refresh(session.take('NextRequest'), 1);
const recoveredDisplay = session.take('UpdateDisplays');
const displayCalls = session.calls('UpdateDisplays').length;
// An old service may finally answer after the recovered service has reused a request id.
stalePoll.callback(1, 'refresh-displays', '', 0, 0, 0, 0, '[]');
staleDisplay.callback();
assert.equal(session.calls('UpdateDisplays').length, displayCalls, 'obsolete polls must not dispatch into the recovered session');
assert.equal(session.calls('CompleteRequest').length, 1, 'a stale display reply must not complete a later request');
recoveredDisplay.callback();
assert.deepEqual(session.calls('CompleteRequest')[1].args, ['1', true, '', '']);
const polls = session.calls('NextRequest').length;
staleDisplay.callback();
recoveredDisplay.callback();
session.advance(2000);
assert.equal(session.calls('CompleteRequest').length, 2, 'each request completes exactly once, including late replies and timers');
assert.equal(session.calls('NextRequest').length, polls, 'late replies and recovery must leave only the healthy poll');
assert.equal(session.calls('RegisterCompanion').length, 2);

console.log('KWin refresh callback recovery: passed');

function readySession(windows = []) {
    const result = scriptSession(windows);
    result.take('RegisterCompanion').callback();
    result.take('UpdateDisplays').callback();
    result.take('UpdateFocusedWindow').callback();
    return result;
}

function movableWindow(id, initial) {
    let frame = initial;
    return {
        internalId: id, managed: true, normalWindow: true, moveable: true, resizeable: true,
        maximizeMode: 0, frameGeometryChanged: signal(), maximizedChanged: signal(),
        tileChanged: signal(), damaged: signal(), setMaximize: () => {},
        get frameGeometry() { return frame; },
        set frameGeometry(value) { this.requested = value; },
        commit(value) {
            frame = value;
            this.frameGeometryChanged.emit();
        },
    };
}

const original = {x: 90, y: 100, width: 700, height: 500};
const target = {x: 0, y: 40, width: 600, height: 860};
const constrained = {x: 0, y: 40, width: 800, height: 860};
function requestMove(moves, id, frame = target) {
    moves.take('NextRequest').callback(id, 'move', 'window-1',
        frame.x, frame.y, frame.width, frame.height, '[]');
}
function partialResult(moves, expected, message) {
    const result = moves.take('CompleteRequest');
    assert.deepEqual(result.args.slice(1, 3), [false, 'org.window_zones.KWin.Error.PartialMove']);
    const payload = JSON.parse(result.args[3]);
    assert.equal(payload.window_id, 'window-1');
    assert.deepEqual(payload.observed, [expected.x, expected.y, expected.width, expected.height]);
    if (message)
        assert.match(payload.message, message);
}

// A Wayland client may commit a frame later, and its minimum size can clamp the request.
const first = movableWindow('window-1', original);
const second = movableWindow('window-2', {x: 900, y: 100, width: 200, height: 300});
const moves = readySession([first, second]);
requestMove(moves, 1);
assert.equal(moves.calls('CompleteRequest').length, 0, 'assignment is not a committed frame');
moves.workspace.activeWindow = second;
first.commit(constrained);
moves.advance(750);
partialResult(moves, constrained);
assert.equal(second.requested, undefined, 'observation and placement retain the captured identity');
moves.advance(1000);
assert.equal(moves.calls('CompleteRequest').length, 1, 'the observation completes once');

// An exact asynchronous commit remains observed through the bounded completion boundary.
moves.workspace.activeWindow = first;
requestMove(moves, 2, original);
assert.equal(moves.calls('CompleteRequest').length, 1);
first.commit(original);
assert.equal(moves.calls('CompleteRequest').length, 1, 'a matching frame is not a stable completion');
moves.advance(750);
assert.deepEqual(moves.take('CompleteRequest').args, ['2', true, '', '']);

// Xwayland reports restored maximize state before the client restores its CSD/configure.
// Even a frame that already equals the target must wait for that restoration before placement.
const restoring = movableWindow('window-1', target);
restoring.maximizeMode = 3;
restoring.setMaximize = () => {
    restoring.maximizeMode = 0;
    restoring.maximizedChanged.emit();
};
let placements = 0;
Object.defineProperty(restoring, 'frameGeometry', {
    set(value) {
        placements++;
        this.requested = value;
        this.commit(value);
    },
});
const restored = readySession([restoring]);
requestMove(restored, 10);
assert.equal(restored.calls('CompleteRequest').length, 0, 'initial equality precedes restoration');
assert.equal(placements, 0, 'do not configure the target with maximized CSD extents');
restored.advance(25);
restoring.commit({x: 0, y: 40, width: 560, height: 810});
restoring.damaged.emit(restoring);
assert.equal(placements, 0, 'placement must not run inside the restoration commit');
restored.advance(0);
assert.equal(placements, 1, 'place once after the restored client frame boundary');
restored.advance(725);
assert.deepEqual(restored.take('CompleteRequest').args, ['10', true, '', '']);
assert.deepEqual({...restoring.frameGeometry}, target);

// A late configure after an initially matching assignment must not be reported as success.
requestMove(restored, 11);
assert.equal(restored.calls('CompleteRequest').length, 1);
restoring.commit(constrained);
restored.advance(750);
partialResult(restored, constrained);
assert.equal(placements, 2, 'a constrained/late frame is observed, never repeatedly overwritten');

// Native Wayland acknowledges maximize restoration after committing its restored frame.
const native = movableWindow('window-1', target);
native.maximizeMode = 3;
const nativeRestore = readySession([native]);
requestMove(nativeRestore, 12, original);
native.commit(original);
assert.equal(native.requested, undefined, 'a restored frame alone is not a maximize acknowledgement');
native.maximizeMode = 0;
native.maximizedChanged.emit();
nativeRestore.advance(0);
assert.deepEqual({...native.requested}, original, 'Restore retains its exact captured target');
nativeRestore.advance(750);
assert.deepEqual(nativeRestore.take('CompleteRequest').args, ['12', true, '', '']);

const synchronous = movableWindow('window-1', original);
Object.defineProperty(synchronous, 'frameGeometry', {
    set(value) {
        this.requested = value;
        this.commit(constrained);
    },
});
const clamped = readySession([synchronous]);
requestMove(clamped, 9);
clamped.advance(750);
partialResult(clamped, constrained);

// Unmaximizing/untile can change the frame even if the final assignment throws.
const interrupted = movableWindow('window-1', original);
interrupted.setMaximize = () => {
    interrupted.commit(constrained);
    throw new Error('restore interrupted');
};
const failures = readySession([interrupted]);
requestMove(failures, 3);
partialResult(failures, constrained, /restore interrupted/);

const closing = movableWindow('window-1', original);
const closed = readySession([closing]);
requestMove(closed, 4);
closed.workspace.stackingOrder = [];
closed.workspace.activeWindow = null;
closed.workspace.windowRemoved.emit(closing);
Object.defineProperty(closing, 'deleted', {
    get() { throw new Error('the native QObject has already been destroyed'); },
});
closed.advance(750);
assert.deepEqual(closed.take('CompleteRequest').args,
    ['4', false, 'org.window_zones.KWin.Error.WindowGone', 'window-1']);

const unchanged = readySession([movableWindow('window-1', original)]);
requestMove(unchanged, 5);
unchanged.advance(750);
assert.equal(unchanged.take('CompleteRequest').args[2], 'org.window_zones.KWin.Error.Operation',
    'an unchanged frame must not create partial-move history');
console.log('KWin captured-frame move observation: passed');

const bindings = readySession();
const left = 'alt+ctrl+left', right = 'alt+ctrl+right';
bindings.take('NextRequest').callback(6, 'register-hotkeys', '', 0, 0, 0, 0, JSON.stringify([left]));
bindings.take('action').callback([]);
assert.equal(bindings.calls('CompleteRequest').length, 0, 'assignment readback precedes commit');
bindings.take('SetShortcut').callback();
assert.deepEqual(bindings.take('CompleteRequest').args, ['6', true, '', '']);
bindings.take('NextRequest').callback(7, 'register-hotkeys', '', 0, 0, 0, 0, JSON.stringify([right]));
bindings.take('action').callback(['kwin', 'unrelated action', '', '']);
assert.equal(bindings.take('CompleteRequest').args[2], 'org.window_zones.KWin.Error.Unsupported');
assert.equal(bindings.calls('SetShortcut').length, 1, 'a refused replacement never mutates native keys');
bindings.shortcuts.get(`Window Zones Hotkey ${left}`)();
assert.deepEqual(bindings.take('HotkeyPressed').args, [left]);
bindings.take('NextRequest').callback(8, 'register-hotkeys', '', 0, 0, 0, 0, JSON.stringify([right]));
bindings.take('action').callback([]);
const assignRight = bindings.take('SetShortcut');
assert.equal(assignRight.args[0], right);
assignRight.callback();
const clearLeft = bindings.take('SetShortcut');
assert.deepEqual(clearLeft.args, [left, 0]);
assert.equal(bindings.calls('CompleteRequest').length, 2);
clearLeft.callback();
assert.deepEqual(bindings.take('CompleteRequest').args, ['8', true, '', '']);
bindings.shortcuts.get(`Window Zones Hotkey ${left}`)();
assert.equal(bindings.calls('HotkeyPressed').length, 1, 'a committed replacement disables old delivery');
bindings.shortcuts.get(`Window Zones Hotkey ${right}`)();
assert.deepEqual(bindings.take('HotkeyPressed').args, [right]);
bindings.advance(1000);
assert.equal(bindings.calls('CompleteRequest').length, 3, 'no post-commit timer may refuse the known new set');
console.log('KWin precommit hotkey replacement: passed');
