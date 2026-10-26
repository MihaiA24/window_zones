'use strict';

const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

function signal() {
    const handlers = [];
    return {
        connect: callback => handlers.push(callback),
        emit: () => handlers.forEach(callback => callback()),
    };
}

function scriptSession() {
    let now = 0;
    const timers = [];
    const calls = [];
    const pending = [];
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
        workspace: {
            screens: [{name: 'output-1'}], currentDesktop: {}, activeWindow: null,
            clientArea: () => ({x: 0, y: 40, width: 1200, height: 860}),
        },
        registerShortcut: () => true,
        callDBus: (service, objectPath, iface, method, ...args) => {
            const callback = args.pop();
            const call = {service, objectPath, iface, method, args, callback};
            calls.push(call);
            pending.push(call);
        },
    });
    vm.runInContext(fs.readFileSync(path.join(__dirname, '../contents/code/main.js'), 'utf8'), context);
    return {
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
