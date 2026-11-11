'use strict';

const fs = require('node:fs');
const vm = require('node:vm');
const readline = require('node:readline');
const callbacks = new Map();
const timers = new Set();
const frames = [];
const completions = [];
let sequence = 0;

function signal() {
    const handlers = new Set();
    return {
        connect: callback => handlers.add(callback),
        disconnect: callback => handlers.delete(callback),
        emit: (...args) => [...handlers].forEach(callback => callback(...args)),
    };
}

class QTimer {
    constructor() { this.timeout = signal(); timers.add(this); }
    start() {
        this.stop();
        const tick = () => {
            this.handle = null;
            this.timeout.emit();
            if (!this.singleShot)
                this.handle = setTimeout(tick, this.interval);
        };
        this.handle = setTimeout(tick, this.interval);
    }
    stop() { clearTimeout(this.handle); this.handle = null; }
}

const windows = [1, 2, 3].map(index => ({
    internalId: `window-${index}`, managed: true, normalWindow: true,
    moveable: true, resizeable: true, maximizeMode: 0, tile: null, fullScreen: false,
    frameGeometryChanged: signal(), maximizedChanged: signal(), tileChanged: signal(),
    damaged: signal(), setMaximize() {},
    _frame: {x: 90, y: 100, width: 700, height: 500},
    get frameGeometry() { return this._frame; },
    set frameGeometry(frame) {
        this._frame = frame;
        frames.push([this.internalId, frame.x, frame.y, frame.width, frame.height]);
        // Focus changes must not redirect the remaining queued placements.
        workspace.activeWindow = windows[index % windows.length];
        this.frameGeometryChanged.emit();
    },
}));
const workspace = {
    screens: [{name: 'output-1'}], currentDesktop: 1,
    activeWindow: windows[0], stackingOrder: windows,
    windowActivated: signal(), windowRemoved: signal(),
    clientArea: () => ({x: 0, y: 0, width: 1800, height: 900}),
};
const send = value => process.stdout.write(JSON.stringify(value) + '\n');
const watchdog = setTimeout(() => {
    console.error('queued KWin script fixture did not finish');
    process.exit(1);
}, 10000);
readline.createInterface({input: process.stdin}).on('line', line => {
    const reply = JSON.parse(line);
    const call = callbacks.get(reply.id);
    callbacks.delete(reply.id);
    if (call.method === 'CompleteRequest' && completions.length === 3) {
        clearTimeout(watchdog);
        for (const timer of timers) timer.stop();
        send({frames, completions});
        process.exit(0);
    } else {
        call.callback(...reply.args);
    }
});
const context = vm.createContext({
    console: {warn: (...args) => console.error(...args)},
    QTimer, workspace, KWin: {MaximizeArea: 2}, registerShortcut: () => true,
    callDBus(service, objectPath, iface, method, ...args) {
        const callback = args.pop();
        const id = ++sequence;
        callbacks.set(id, {method, callback});
        if (method === 'CompleteRequest') completions.push(args);
        send({id, service, objectPath, iface, method, args});
    },
});
vm.runInContext(fs.readFileSync(process.argv[1], 'utf8'), context);
