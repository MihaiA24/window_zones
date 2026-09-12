'use strict';

const SERVICE_NAME = 'org.window_zones.KWin';
const OBJECT_PATH = '/org/window_zones/KWin';
const INTERFACE_NAME = 'org.window_zones.KWin1';
const PROTOCOL_MAJOR = 1;
const REQUEST_MOVE = 'move';
const REQUEST_REGISTER_HOTKEYS = 'register-hotkeys';

const CAPABILITIES = [
    'focused-window',
    'displays',
    'move-resize',
    'hotkeys',
];

const MODIFIER_NAMES = new Map([
    ['alt', 'Alt'],
    ['ctrl', 'Ctrl'],
    ['shift', 'Shift'],
    ['cmd', 'Meta'],
]);

const KEY_NAMES = new Map([
    ['esc', 'Esc'],
    ['escape', 'Esc'],
    ['pageup', 'PgUp'],
    ['page_up', 'PgUp'],
    ['pgup', 'PgUp'],
    ['pagedown', 'PgDown'],
    ['page_down', 'PgDown'],
    ['page_dn', 'PgDown'],
    ['pgdn', 'PgDown'],
    ['return', 'Return'],
    ['enter', 'Enter'],
    ['space', 'Space'],
    ['spacebar', 'Space'],
    ['tab', 'Tab'],
    ['backspace', 'Backspace'],
    ['delete', 'Del'],
    ['insert', 'Ins'],
    ['home', 'Home'],
    ['end', 'End'],
    ['left', 'Left'],
    ['leftarrow', 'Left'],
    ['left_arrow', 'Left'],
    ['right', 'Right'],
    ['rightarrow', 'Right'],
    ['right_arrow', 'Right'],
    ['up', 'Up'],
    ['uparrow', 'Up'],
    ['up_arrow', 'Up'],
    ['down', 'Down'],
    ['downarrow', 'Down'],
    ['down_arrow', 'Down'],
    ['minus', '-'],
    ['equal', '='],
    ['comma', ','],
    ['dot', '.'],
    ['slash', '/'],
    ['quote', "'"],
    ['semicolon', ';'],
    ['leftbracket', '['],
    ['left_bracket', '['],
    ['rightbracket', ']'],
    ['right_bracket', ']'],
    ['backslash', '\\'],
    ['backquote', '`'],
    ['print', 'Print'],
    ['printscreen', 'Print'],
    ['scrolllock', 'ScrollLock'],
    ['capslock', 'CapsLock'],
    ['numlock', 'NumLock'],
    ['pause', 'Pause'],
]);

let activeHotkeys = new Set();
let slotHotkeys = [];
let registeredShortcuts = new Set();
let polling = false;
let companionRetryTimer = null;
let trackedWindow = null;

function errorMessage(error) {
    if (error && typeof error.message === 'string')
        return error.message;
    return String(error);
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
        print(`Window Zones KWin D-Bus ${method} failed: ${errorMessage(error)}`);
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
            output,
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
        || window === trackedWindow
        || !window.frameGeometryChanged
        || typeof window.frameGeometryChanged.connect !== 'function')
        return;

    trackedWindow = window;
    window.frameGeometryChanged.connect(() => {
        if (typeof workspace !== 'undefined'
            && workspace
            && workspace.activeWindow === window)
            publishFocusedWindow();
    });
}

function focusedPayload() {
    const focused = focusedWindow();
    if (!focused)
        return [false, '', 0, 0, 0, 0];

    const displays = displayData();
    const centerX = focused.x + Math.floor(focused.width / 2);
    const centerY = focused.y + Math.floor(focused.height / 2);
    const display = displays.find(item =>
        focused.window.output
        && item.output === focused.window.output)
        || displays.find(item =>
            centerX >= item.x
            && centerX < item.x + item.width
            && centerY >= item.y
            && centerY < item.y + item.height);
    const displayIdValue = display
        ? display.id
        : `unmatched:${centerX}:${centerY}`;
    return [
        true,
        displayIdValue,
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
        print(`Window Zones KWin state update failed: ${errorMessage(error)}`);
    }
}

function moveFocusedWindow(x, y, width, height) {
    if (width <= 0 || height <= 0)
        throw new Error('window target must have positive dimensions');

    const focused = focusedWindow();
    if (!focused)
        throw new Error('no focused normal application window');

    const window = focused.window;
    if (window.fullScreen || window.tile)
        throw new Error('focused window is fullscreen or tiled; restore it before moving');
    if (window.moveable !== true || window.resizeable !== true)
        throw new Error('focused window does not allow moving and resizing');

    window.frameGeometry = {x, y, width, height};
    publishFocusedWindow();
}

function canonicalKeySequence(hotkey) {
    const tokens = hotkey
        .split('+')
        .map(token => token.trim().toLowerCase())
        .filter(Boolean);
    const modifiers = [];
    let key = null;

    for (const token of tokens) {
        const modifier = MODIFIER_NAMES.get(token);
        if (modifier) {
            if (!modifiers.includes(modifier))
                modifiers.push(modifier);
            continue;
        }
        if (key !== null)
            throw new Error(`hotkey '${hotkey}' contains multiple non-modifier keys`);
        key = token;
    }

    if (key === null)
        throw new Error(`hotkey '${hotkey}' is missing its key`);

    let keyName = KEY_NAMES.get(key);
    if (!keyName && /^f([1-9]|1[0-9]|2[0-4])$/.test(key))
        keyName = key.toUpperCase();
    if (!keyName && /^[a-z0-9]$/.test(key))
        keyName = key.toUpperCase();
    if (!keyName)
        throw new Error(`unsupported KWin shortcut key '${key}' in '${hotkey}'`);

    return [...modifiers, keyName].join('+');
}

function emitHotkey(hotkey) {
    if (!activeHotkeys.has(hotkey))
        return;
    invoke('HotkeyPressed', [hotkey], () => {});
}
function registerHotkeys(hotkeys) {
    if (hotkeys.length === slotHotkeys.length
        && hotkeys.every((hotkey, index) => hotkey === slotHotkeys[index]))
        return;

    const next = hotkeys.map(hotkey => ({
        hotkey,
        sequence: canonicalKeySequence(hotkey),
    }));

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
            throw new Error(`KWin rejected shortcut '${entry.hotkey}'`);
        registeredShortcuts.add(entry.hotkey);
    });

    slotHotkeys = next.map(entry => entry.hotkey);
    activeHotkeys = new Set(slotHotkeys);
}

function complete(requestId, ok, message) {
    invoke('CompleteRequest', [String(requestId), ok, message], () => {});
}

function processRequest(requestId, kind, x, y, width, height, hotkeysJson) {
    try {
        const hotkeys = JSON.parse(hotkeysJson);
        if (!Array.isArray(hotkeys)
            || hotkeys.some(hotkey => typeof hotkey !== 'string'))
            throw new Error('KWin returned an invalid hotkey request payload');

        if (kind === REQUEST_MOVE) {
            moveFocusedWindow(x, y, width, height);
        } else if (kind === REQUEST_REGISTER_HOTKEYS) {
            registerHotkeys(hotkeys);
        } else {
            throw new Error(`unknown KWin request kind '${kind}'`);
        }
        complete(requestId, true, '');
    } catch (error) {
        complete(requestId, false, errorMessage(error));
    }
}

function pollRequests() {
    if (polling)
        return;
    polling = true;

    const accepted = invoke('NextRequest', [],
        (requestId, kind, x, y, width, height, hotkeysJson) => {
            polling = false;
            if (requestId !== 0)
                processRequest(requestId, kind, x, y, width, height, hotkeysJson);
            pollRequests();
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
            if (trackedWindow === window)
                trackedWindow = null;
            publishFocusedWindow();
        });
    trackFocusedWindow(workspace.activeWindow);
    if (workspace.screensChanged)
        workspace.screensChanged.connect(() => publishState());
}

connectCompanion(true);
startCompanionRetry();
