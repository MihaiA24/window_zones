'use strict';

const SERVICE_NAME = 'org.window_zones.KWin';
const OBJECT_PATH = '/org/window_zones/KWin';
const INTERFACE_NAME = 'org.window_zones.KWin2';
const PROTOCOL_MAJOR = 2;
const REQUEST_MOVE = 'move';
const REQUEST_REGISTER_HOTKEYS = 'register-hotkeys';

const CAPABILITIES = [
    'focused-window',
    'displays',
    'move-resize',
    'hotkeys',
];

// Qt::KeyboardModifier and Qt::Key values for KGlobalAccel's integer-key query.
const MODIFIER_NAMES = new Map([
    ['alt', ['Alt', 0x08000000]],
    ['ctrl', ['Ctrl', 0x04000000]],
    ['shift', ['Shift', 0x02000000]],
    ['cmd', ['Meta', 0x10000000]],
]);

const KEY_NAMES = new Map([
    ['escape', ['Esc', 0x01000000]],
    ['pageup', ['PgUp', 0x01000016]],
    ['pagedown', ['PgDown', 0x01000017]],
    ['return', ['Return', 0x01000004]],
    ['space', ['Space', 0x20]],
    ['tab', ['Tab', 0x01000001]],
    ['backspace', ['Backspace', 0x01000003]],
    ['delete', ['Del', 0x01000007]],
    ['insert', ['Ins', 0x01000006]],
    ['home', ['Home', 0x01000010]],
    ['end', ['End', 0x01000011]],
    ['left', ['Left', 0x01000012]],
    ['right', ['Right', 0x01000014]],
    ['up', ['Up', 0x01000013]],
    ['down', ['Down', 0x01000015]],
    ['minus', ['-', 0x2d]],
    ['equal', ['=', 0x3d]],
    ['comma', [',', 0x2c]],
    ['dot', ['.', 0x2e]],
    ['slash', ['/', 0x2f]],
    ['quote', ["'", 0x27]],
    ['semicolon', [';', 0x3b]],
    ['leftbracket', ['[', 0x5b]],
    ['rightbracket', [']', 0x5d]],
    ['backslash', ['\\', 0x5c]],
    ['backquote', ['`', 0x60]],
    ['printscreen', ['Print', 0x01000009]],
    ['scrolllock', ['ScrollLock', 0x01000026]],
    ['capslock', ['CapsLock', 0x01000024]],
    ['numlock', ['NumLock', 0x01000025]],
    ['pause', ['Pause', 0x01000008]],
]);

let activeHotkeys = new Set();
let registeredShortcuts = new Set();
let polling = false;
let pollStartedAt = 0;
let processingRequest = false;
let companionRetryTimer = null;
const trackedWindows = new Map();

function errorMessage(error) {
    if (error && typeof error.message === 'string')
        return error.message;
    return String(error);
}

function protocolError(kind, message) {
    const error = new Error(message);
    error.dbusError = `org.window_zones.KWin.Error.${kind}`;
    return error;
}

function invoke(method, args, callback) {
    try {
        callDBus(
            SERVICE_NAME,
            OBJECT_PATH,
            INTERFACE_NAME,
            method,
            ...args,
            callback);
        return true;
    } catch (error) {
        console.warn(`Window Zones KWin D-Bus ${method} failed: ${errorMessage(error)}`);
        return false;
    }
}

function displayId(output, index) {
    const name = output && typeof output.name === 'string' ? output.name : '';
    return name || `kwin-output-${index}`;
}

function rectPayload(rect) {
    return [
        Math.round(rect.x),
        Math.round(rect.y),
        Math.round(rect.width),
        Math.round(rect.height),
    ];
}

function displayData() {
    if (typeof workspace === 'undefined' || !workspace)
        throw new Error('KWin workspace API is unavailable');
    const screens = workspace.screens;
    if (!screens || typeof screens.forEach !== 'function')
        throw new Error('KWin reported no active outputs');
    if (typeof workspace.clientArea !== 'function'
        || typeof KWin === 'undefined'
        || typeof KWin.MaximizeArea === 'undefined')
        throw new Error('KWin work-area API is unavailable');

    const displays = [];
    screens.forEach((output, index) => {
        // KWin.WorkArea ignores the output and returns the desktop-wide union, which reports the
        // same rectangle for every display. MaximizeArea is the per-output area minus struts.
        const workArea = workspace.clientArea(
            KWin.MaximizeArea,
            output,
            workspace.currentDesktop);
        const [x, y, width, height] = rectPayload(workArea);
        if (width <= 0 || height <= 0)
            throw new Error(`KWin reported an invalid work area for output ${displayId(output, index)}`);
        displays.push({
            id: displayId(output, index),
            x,
            y,
            width,
            height,
        });
    });
    if (displays.length === 0)
        throw new Error('KWin reported no active outputs');
    return displays;
}

function focusedWindow() {
    if (typeof workspace === 'undefined' || !workspace)
        return null;

    const window = workspace.activeWindow;
    if (!window
        || window.deleted
        || window.specialWindow
        || window.managed === false
        || window.normalWindow !== true
        || !window.frameGeometry)
        return null;

    const [x, y, width, height] = rectPayload(window.frameGeometry);
    if (width <= 0 || height <= 0)
        return null;

    return {window, x, y, width, height};
}
function trackFocusedWindow(window) {
    if (!window
        || trackedWindows.has(String(window.internalId))
        || !window.frameGeometryChanged
        || typeof window.frameGeometryChanged.connect !== 'function')
        return;

    const handler = () => {
        if (typeof workspace !== 'undefined'
            && workspace
            && workspace.activeWindow === window)
            publishFocusedWindow();
    };
    trackedWindows.set(String(window.internalId), {window, handler});
    window.frameGeometryChanged.connect(handler);
}

function focusedPayload() {
    const focused = focusedWindow();
    if (!focused)
        return [false, '', 0, 0, 0, 0];

    return [
        true,
        String(focused.window.internalId),
        focused.x,
        focused.y,
        focused.width,
        focused.height,
    ];
}
function publishFocusedWindow() {
    invoke('UpdateFocusedWindow', [JSON.stringify(focusedPayload())], () => {});
}

function publishDisplays() {
    const displays = displayData().map(item => [
        item.id,
        item.x,
        item.y,
        item.width,
        item.height,
    ]);
    invoke('UpdateDisplays', [JSON.stringify(displays)], () => {});
}

function publishState() {
    try {
        publishDisplays();
        publishFocusedWindow();
    } catch (error) {
        console.warn(`Window Zones KWin state update failed: ${errorMessage(error)}`);
    }
}

function moveWindow(windowId, x, y, width, height) {
    if (width <= 0 || height <= 0)
        throw new Error('window target must have positive dimensions');

    const windows = typeof workspace.windowList === 'function'
        ? workspace.windowList() : workspace.stackingOrder;
    const window = Array.prototype.find.call(windows,
        candidate => !candidate.deleted && String(candidate.internalId) === windowId);
    if (!window)
        throw protocolError('WindowGone', windowId);
    if (window.fullScreen)
        throw new Error('window is fullscreen; leave fullscreen first');
    if (window.managed === false || window.normalWindow !== true
        || window.moveable !== true || window.resizeable !== true)
        throw new Error('window does not allow moving and resizing');

    const tile = window.tile;
    if (tile) {
        if (typeof tile.unmanage === 'function') {
            // KWin 6.5: unmanage clears the requested quick/custom tile, including restore geometry.
            tile.unmanage(window);
        } else {
            // Earlier KWin 6: tile=null only detaches the tile. A maximize transition also clears
            // the non-scriptable quickTileMode; finish in the restored state before placement.
            window.tile = null;
            window.setMaximize(true, true);
        }
    }
    window.setMaximize(false, false);

    window.frameGeometry = {x, y, width, height};
    publishFocusedWindow();
}

function shortcutForHotkey(hotkey) {
    const tokens = hotkey.split('+');
    const key = tokens.pop();
    const modifiers = [];
    let modifierMask = 0;
    for (const [token, [name, mask]] of MODIFIER_NAMES) {
        if (tokens[modifiers.length] === token) {
            modifiers.push(name);
            modifierMask |= mask;
        }
    }
    if (modifiers.length !== tokens.length)
        throw protocolError('Unsupported', `unsupported KWin shortcut modifiers in '${hotkey}'`);

    let keyData = KEY_NAMES.get(key);
    if (!keyData && /^f([1-9]|1[0-9]|2[0-4])$/.test(key))
        keyData = [key.toUpperCase(), 0x0100002f + Number(key.slice(1))];
    if (!keyData && /^[a-z0-9]$/.test(key))
        keyData = [key.toUpperCase(), key.toUpperCase().charCodeAt(0)];
    if (!keyData)
        throw protocolError('Unsupported', `unsupported KWin shortcut key '${key}' in '${hotkey}'`);

    return {
        sequence: [...modifiers, keyData[0]].join('+'),
        key: modifierMask | keyData[1],
    };
}

function emitHotkey(hotkey) {
    if (!activeHotkeys.has(hotkey))
        return;
    invoke('HotkeyPressed', [hotkey], () => {});
}
function registerHotkeys(hotkeys, callback) {
    // QJSEngine has no object spread.
    const next = hotkeys.map(hotkey => Object.assign({hotkey}, shortcutForHotkey(hotkey)));
    // JS-owned QTimer: no deleteLater (not exposed to scripts; the TypeError killed the callback).
    const timer = new QTimer();
    let finished = false;
    const finish = error => {
        if (finished)
            return;
        finished = true;
        timer.stop();
        callback(error);
    };
    // KWin does not invoke callDBus callbacks on errors. Fail before the App's one-second
    // request deadline and ignore late replies so a timed-out replacement cannot commit.
    timer.interval = 750;
    timer.singleShot = true;
    timer.timeout.connect(() => finish(protocolError('Unavailable', 'KGlobalAccel shortcut preflight timed out')));
    timer.start();

    function preflight(index) {
        if (finished)
            return;
        try {
            if (index < next.length) {
                const entry = next[index];
                // action(int) returns [component, action, ...] for the first registrant,
                // the actual dispatch winner; getGlobalShortcutsByKey is not ordered.
                callDBus('org.kde.kglobalaccel', '/kglobalaccel', 'org.kde.KGlobalAccel',
                    'action', entry.key, winner => {
                        if (finished)
                            return;
                        // QJSEngine hands D-Bus string lists over as sequences, not Arrays.
                        if (!winner || typeof winner.length !== 'number'
                            || (winner.length !== 0 && winner.length !== 4)) {
                            finish(protocolError('Unavailable', 'KGlobalAccel returned an invalid shortcut owner'));
                        } else if (winner.length !== 0
                            && (winner[0] !== 'kwin'
                                || winner[1] !== `Window Zones Hotkey ${entry.hotkey}`)) {
                            finish(protocolError('Unsupported', `KWin rejected shortcut '${entry.hotkey}': accelerator is already held by '${winner[1]}'; choose a different binding`));
                        } else {
                            preflight(index + 1);
                        }
                    });
                return;
            }

            // No QAction or active-set mutation occurs until every accelerator passes.
            next.forEach(entry => {
                if (registeredShortcuts.has(entry.hotkey))
                    return;
                const title = `Window Zones Hotkey ${entry.hotkey}`;
                const registered = registerShortcut(
                    title,
                    `Window Zones binding ${entry.hotkey}`,
                    entry.sequence,
                    () => emitHotkey(entry.hotkey));
                if (!registered)
                    throw protocolError('Unavailable', `KWin rejected shortcut '${entry.hotkey}'`);
                registeredShortcuts.add(entry.hotkey);
            });
            activeHotkeys = new Set(hotkeys);
            finish();
        } catch (error) {
            finish(error);
        }
    }
    preflight(0);
}

function complete(requestId, ok, errorName, message) {
    invoke('CompleteRequest', [String(requestId), ok, errorName, message], () => {});
}

function processRequest(requestId, kind, windowId, x, y, width, height, hotkeysJson, callback) {
    const finish = error => {
        complete(requestId, !error, error ? (error.dbusError || 'org.window_zones.KWin.Error.Operation') : '',
            error ? errorMessage(error) : '');
        callback();
    };
    try {
        const hotkeys = JSON.parse(hotkeysJson);
        if (!Array.isArray(hotkeys)
            || hotkeys.some(hotkey => typeof hotkey !== 'string'))
            throw new Error('KWin returned an invalid hotkey request payload');

        if (kind === REQUEST_MOVE) {
            moveWindow(windowId, x, y, width, height);
        } else if (kind === REQUEST_REGISTER_HOTKEYS) {
            registerHotkeys(hotkeys, finish);
            return;
        } else {
            throw new Error(`unknown KWin request kind '${kind}'`);
        }
        finish();
    } catch (error) {
        finish(error);
    }
}

function pollRequests() {
    if (polling)
        return;
    polling = true;
    pollStartedAt = Date.now();

    const accepted = invoke('NextRequest', [],
        (requestId, kind, windowId, x, y, width, height, hotkeysJson) => {
            if (requestId !== 0) {
                processingRequest = true;
                processRequest(requestId, kind, windowId, x, y, width, height, hotkeysJson, () => {
                    processingRequest = false;
                    polling = false;
                    pollRequests();
                });
            } else {
                // The companion defers an empty reply for up to one second; this is not a
                // synchronous empty-poll loop and queued moves wake it within one service tick.
                polling = false;
                pollRequests();
            }
        });
    if (!accepted)
        polling = false;
}

function apiAvailable() {
    return typeof workspace !== 'undefined'
        && workspace
        && workspace.screens
        && typeof workspace.screens.forEach === 'function'
        && typeof workspace.clientArea === 'function'
        && typeof KWin !== 'undefined'
        && typeof KWin.MaximizeArea !== 'undefined'
        && typeof registerShortcut === 'function'
        && typeof callDBus === 'function';
}
function connectCompanion(resetState) {
    if (!apiAvailable()) {
        invoke('RegisterCompanion', [String(0), resetState], () => {});
        return;
    }

    invoke('RegisterCompanion', [String(PROTOCOL_MAJOR), resetState], () => {
        publishState();
        pollRequests();
    });
}

function retryCompanion() {
    // KWin omits callDBus error callbacks. Recover a lost poll, never duplicate a healthy one.
    if (processingRequest || (polling && Date.now() - pollStartedAt < 3000))
        return;
    polling = false;
    connectCompanion(false);
}
function startCompanionRetry() {
    if (typeof QTimer === 'undefined')
        return;
    companionRetryTimer = new QTimer();
    companionRetryTimer.interval = 2000;
    companionRetryTimer.timeout.connect(retryCompanion);
    companionRetryTimer.start();
}

if (typeof workspace !== 'undefined' && workspace) {
    if (workspace.windowActivated)
        workspace.windowActivated.connect(window => {
            trackFocusedWindow(window);
            publishFocusedWindow();
        });
    if (workspace.windowRemoved)
        workspace.windowRemoved.connect(window => {
            const tracked = trackedWindows.get(String(window.internalId));
            if (tracked) {
                tracked.window.frameGeometryChanged.disconnect(tracked.handler);
                trackedWindows.delete(String(window.internalId));
            }
            publishFocusedWindow();
        });
    trackFocusedWindow(workspace.activeWindow);
    if (workspace.screensChanged)
        workspace.screensChanged.connect(() => publishState());
}

connectCompanion(true);
startCompanionRetry();
