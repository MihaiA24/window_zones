import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
import Shell from 'gi://Shell';
import St from 'gi://St';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import {WINDOW_ANIMATION_TIME} from 'resource:///org/gnome/shell/ui/windowManager.js';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const SERVICE_NAME = 'org.window_zones.Gnome';
const OBJECT_PATH = '/org/window_zones/Gnome';
const INTERFACE_NAME = 'org.window_zones.Gnome2';
const CAPABILITIES = [
    'focused-window',
    'displays',
    'move-resize',
    'hotkeys',
    'transition-metrics',
];

export const INTERFACE_XML = `
<node>
  <interface name="${INTERFACE_NAME}">
    <method name="GetCapabilities">
      <arg name="capabilities" type="as" direction="out"/>
    </method>
    <method name="GetFocusedWindow">
      <arg name="present" type="b" direction="out"/>
      <arg name="window_id" type="t" direction="out"/>
      <arg name="x" type="i" direction="out"/>
      <arg name="y" type="i" direction="out"/>
      <arg name="width" type="u" direction="out"/>
      <arg name="height" type="u" direction="out"/>
    </method>
    <method name="GetDisplays">
      <arg name="displays" type="a(siiuu)" direction="out"/>
    </method>
    <method name="MoveWindow">
      <arg name="window_id" type="t" direction="in"/>
      <arg name="x" type="i" direction="in"/>
      <arg name="y" type="i" direction="in"/>
      <arg name="width" type="u" direction="in"/>
      <arg name="height" type="u" direction="in"/>
    </method>
    <method name="GetLastTransitionMetrics">
      <arg name="request_ms" type="d" direction="out"/>
      <arg name="completion_ms" type="d" direction="out"/>
    </method>
    <method name="RegisterHotkeys">
      <arg name="hotkeys" type="as" direction="in"/>
    </method>
    <signal name="HotkeyPressed">
      <arg name="hotkey" type="s"/>
    </signal>
  </interface>
</node>`;
const INTERFACE_INFO = Gio.DBusNodeInfo.new_for_xml(INTERFACE_XML).interfaces[0];
const SHELL_PID = new Gio.Credentials().get_unix_pid();

const NON_APPLICATION_WINDOW_TYPES = new Set([
    Meta.WindowType.DESKTOP,
    Meta.WindowType.DOCK,
    Meta.WindowType.MENU,
    Meta.WindowType.TOOLBAR,
    Meta.WindowType.DROPDOWN_MENU,
    Meta.WindowType.POPUP_MENU,
    Meta.WindowType.TOOLTIP,
    Meta.WindowType.NOTIFICATION,
    Meta.WindowType.COMBO,
    Meta.WindowType.DND,
    Meta.WindowType.OVERRIDE_OTHER,
]);


const MODIFIER_TOKENS = new Map([
    ['alt', '<Alt>'],
    ['ctrl', '<Control>'],
    ['shift', '<Shift>'],
    ['cmd', '<Super>'],
]);

const KEY_TOKENS = new Map([
    ['escape', 'Escape'],
    ['pageup', 'Page_Up'],
    ['pagedown', 'Page_Down'],
    ['return', 'Return'],
    ['left', 'Left'],
    ['right', 'Right'],
    ['up', 'Up'],
    ['down', 'Down'],
    ['space', 'space'],
    ['tab', 'Tab'],
    ['backspace', 'BackSpace'],
    ['delete', 'Delete'],
    ['home', 'Home'],
    ['end', 'End'],
    ['insert', 'Insert'],
    ['minus', 'minus'],
    ['equal', 'equal'],
    ['comma', 'comma'],
    ['dot', 'period'],
    ['slash', 'slash'],
    ['quote', 'apostrophe'],
    ['semicolon', 'semicolon'],
    ['leftbracket', 'bracketleft'],
    ['rightbracket', 'bracketright'],
    ['backslash', 'backslash'],
    ['backquote', 'grave'],
    ['printscreen', 'Print'],
    ['scrolllock', 'Scroll_Lock'],
    ['capslock', 'Caps_Lock'],
    ['numlock', 'Num_Lock'],
    ['pause', 'Pause'],
]);


function dbusError(name, message) {
    const error = new Error(message);
    error.name = name;
    return error;
}

function validFrame(frame) {
    return frame && [frame.x, frame.y].every(value =>
        Number.isInteger(value) && value >= -2147483648 && value <= 2147483647)
        && [frame.width, frame.height].every(value =>
            Number.isInteger(value) && value > 0 && value <= 4294967295);
}

function canonicalAccelerator(hotkey) {
    if (/\s/.test(hotkey) || !/^(alt\+)?(ctrl\+)?(shift\+)?(cmd\+)?[^+]+$/.test(hotkey))
        throw new Error(`non-canonical hotkey '${hotkey}'`);

    const tokens = hotkey.split('+');
    const key = tokens.pop();
    const modifiers = tokens.map(token => MODIFIER_TOKENS.get(token));
    let gdkKey = KEY_TOKENS.get(key);
    if (!gdkKey && /^f([1-9]|1[0-9]|2[0-4])$/.test(key))
        gdkKey = key.toUpperCase();
    if (!gdkKey && /^[a-z0-9]$/.test(key))
        gdkKey = key;
    if (!gdkKey)
        throw new Error(`unsupported or non-canonical GNOME accelerator key '${key}' in '${hotkey}'`);

    return `${modifiers.join('')}${gdkKey}`;
}


export class WindowZonesService {
    constructor() {
        this._accelerators = new Map();
        this._hotkeysByAction = new Map();
        this._allowedKeybindings = new Map();
        this._controllerSender = null;
        this._connection = null;
        this._registrationId = 0;
        this._ownerWatchId = 0;
        this._displayIds = new Map();
        this._nextDisplayId = 1;
        this._moves = new Map();
        this._transitionMetrics = new Map();
        this._acceleratorSignalId = global.display.connect(
            'accelerator-activated',
            (_display, action) => this._emitHotkey(action));
    }

    export(connection) {
        this._connection = connection;
        this._registrationId = connection.register_object(
            OBJECT_PATH,
            INTERFACE_INFO,
            this._methodCall.bind(this),
            null,
            null);
        this._ownerWatchId = connection.signal_subscribe(
            'org.freedesktop.DBus',
            'org.freedesktop.DBus',
            'NameOwnerChanged',
            null,
            null,
            Gio.DBusSignalFlags.NONE,
            (_connection, _sender, _path, _interface, _signal, parameters) => {
                const [name, _oldOwner, newOwner] = parameters.deep_unpack();
                if (newOwner === '') {
                    this._transitionMetrics.delete(name);
                    for (const move of this._moves.values()) {
                        if (move.sender === name)
                            move.senderAlive = false;
                    }
                }
                if (name === this._controllerSender && newOwner === '') {
                    this._clearAccelerators();
                    this._controllerSender = null;
                }
            });
    }

    destroy() {
        this._clearAccelerators();
        for (const move of [...this._moves.values()])
            move.finish(dbusError('org.window_zones.Gnome.Error.Unavailable', 'companion disabled during transition'));
        this._transitionMetrics.clear();

        if (this._acceleratorSignalId !== 0) {
            global.display.disconnect(this._acceleratorSignalId);
            this._acceleratorSignalId = 0;
        }

        if (this._connection) {
            if (this._ownerWatchId !== 0) {
                this._connection.signal_unsubscribe(this._ownerWatchId);
                this._ownerWatchId = 0;
            }
            if (this._registrationId !== 0) {
                this._connection.unregister_object(this._registrationId);
                this._registrationId = 0;
            }
        }
        this._connection = null;
    }

    _methodCall(_connection, sender, _objectPath, _interfaceName, methodName, parameters, invocation) {
        try {
            switch (methodName) {
            case 'GetCapabilities':
                invocation.return_value(new GLib.Variant('(as)', [CAPABILITIES]));
                return;
            case 'GetFocusedWindow':
                invocation.return_value(
                    new GLib.Variant('(btiiuu)', this.GetFocusedWindow()));
                return;
            case 'GetDisplays':
                invocation.return_value(new GLib.Variant('(a(siiuu))', [this.GetDisplays()]));
                return;
            case 'MoveWindow': {
                const [id, x, y, width, height] = parameters.deep_unpack();
                this.MoveWindow(sender, id, x, y, width, height).then(
                    () => invocation.return_value(new GLib.Variant('()', [])),
                    error => this._returnError(invocation, error));
                return;
            }
            case 'GetLastTransitionMetrics': {
                const metrics = this._transitionMetrics.get(sender);
                if (!metrics)
                    throw dbusError('org.window_zones.Gnome.Error.InvalidWindowState', 'this caller has no completed transition');
                invocation.return_value(new GLib.Variant('(dd)', metrics));
                return;
            }
            case 'RegisterHotkeys': {
                const [hotkeys] = parameters.deep_unpack();
                this._registerHotkeys(sender, hotkeys);
                invocation.return_value(new GLib.Variant('()', []));
                return;
            }
            default:
                invocation.return_dbus_error(
                    'org.freedesktop.DBus.Error.UnknownMethod',
                    `Unknown method: ${methodName}`);
            }
        } catch (error) {
            this._returnError(invocation, error);
        }
    }

    _returnError(invocation, error) {
        const name = typeof error?.name === 'string'
            && error.name.startsWith('org.window_zones.Gnome.Error.')
            ? error.name
            : 'org.freedesktop.DBus.Error.Failed';
        invocation.return_dbus_error(name, error?.message ?? String(error));
    }

    GetFocusedWindow() {
        const window = this._applicationWindow(global.display.get_focus_window());
        if (!window)
            return [false, 0, 0, 0, 0, 0];

        const frame = window.get_frame_rect();
        return [true, window.get_id(), frame.x, frame.y, frame.width, frame.height];
    }

    GetDisplays() {
        const monitors = Main.layoutManager.monitors ?? [];
        if (monitors.length === 0)
            throw new Error('GNOME reported no active displays');

        const activeDisplayIds = new Map();
        const displays = monitors.map((_monitor, index) => {
            const rect = Main.layoutManager.getWorkAreaForMonitor(index);
            const key = [rect.x, rect.y, rect.width, rect.height].join(':');
            let id = this._displayIds.get(key);
            if (!id)
                id = `display-${this._nextDisplayId++}`;
            activeDisplayIds.set(key, id);
            return [id, rect.x, rect.y, rect.width, rect.height];
        });
        this._displayIds = activeDisplayIds;
        return displays;
    }

    MoveWindow(sender, id, x, y, width, height) {
        const started = GLib.get_monotonic_time();
        const window = global.display.list_all_windows()
            .find(candidate => candidate.get_id() === id);
        if (!window)
            throw dbusError(
                'org.window_zones.Gnome.Error.WindowGone',
                `window ${id} no longer exists`);
        if (!this._applicationWindow(window))
            throw dbusError(
                'org.window_zones.Gnome.Error.InvalidWindowState',
                'window is not an eligible application window; leave overview or unlock the session first');
        if (width === 0 || height === 0)
            throw dbusError(
                'org.window_zones.Gnome.Error.InvalidWindowState',
                'window target must have positive dimensions');
        if (window.is_fullscreen())
            throw dbusError(
                'org.window_zones.Gnome.Error.InvalidWindowState',
                'window is fullscreen; leave fullscreen first');
        // allows_resize() excludes maximized windows; resizeable reflects client constraints.
        if (!window.resizeable)
            throw dbusError(
                'org.window_zones.Gnome.Error.InvalidWindowState',
                'window does not allow resizing');

        this._moves.get(window)?.finish(
            dbusError('org.window_zones.Gnome.Error.InvalidWindowState', 'transition superseded by another request'));
        this._transitionMetrics.delete(sender);
        const actor = window.get_compositor_private();
        const restoring = window.get_maximize_flags() !== 0;
        if (actor)
            global.window_manager.emit('kill-window-effects', actor);
        const oldFrame = window.get_frame_rect();
        const initialFrame = {
            x: oldFrame.x, y: oldFrame.y, width: oldFrame.width, height: oldFrame.height,
        };
        const settings = St.Settings.get();
        let clone = null;
        if (actor && settings.enable_animations
            && settings.reducedMotion !== St.ReducedMotion.REDUCE
            && (restoring || oldFrame.x !== x || oldFrame.y !== y || oldFrame.width !== width || oldFrame.height !== height)) {
            // One compositor snapshot; do not send a resize request on every animation frame.
            clone = new St.Widget({
                content: actor.paint_to_content(oldFrame),
                x: oldFrame.x,
                y: oldFrame.y,
                width: oldFrame.width,
                height: oldFrame.height,
            });
            global.window_group.add_child(clone);
            global.window_group.set_child_above_sibling(clone, actor);
            actor.freeze();
        }
        let frozen = clone !== null;
        let requestMs = 0;
        let requested = false;
        let nativeSizeId = 0;

        return new Promise((resolve, reject) => {
            let sourceId = 0;
            let destroyedId = 0;
            let finished = false;
            let liveActor = actor;
            const move = {
                sender,
                senderAlive: true,
                finish: error => {
                    if (finished)
                        return;
                    finished = true;
                    if (sourceId !== 0)
                        GLib.source_remove(sourceId);
                    if (nativeSizeId !== 0)
                        global.window_manager.disconnect(nativeSizeId);
                    if (liveActor) {
                        liveActor.disconnect(destroyedId);
                        if (clone) {
                            liveActor.remove_all_transitions();
                            liveActor.set_scale(1, 1);
                            liveActor.translation_x = 0;
                            liveActor.translation_y = 0;
                            if (frozen) {
                                liveActor.thaw();
                                frozen = false;
                            }
                        }
                    }
                    clone?.destroy();
                    this._moves.delete(window);
                    let finalFrame = null;
                    try {
                        if (error?.name !== 'org.window_zones.Gnome.Error.WindowGone') {
                            if (!global.display.list_all_windows().includes(window)) {
                                error = dbusError('org.window_zones.Gnome.Error.WindowGone', `window ${id} closed during transition`);
                            } else {
                                finalFrame = window.get_frame_rect();
                            }
                        }
                    } catch (readError) {
                        error ??= readError;
                    }
                    if (!error && (!validFrame(finalFrame)
                        || Math.abs(finalFrame.x - x) > 2 || Math.abs(finalFrame.y - y) > 2
                        || Math.abs(finalFrame.width - width) > 2 || Math.abs(finalFrame.height - height) > 2))
                        error = dbusError('org.window_zones.Gnome.Error.InvalidWindowState', 'window changed geometry during transition');
                    if (error && error.name !== 'org.window_zones.Gnome.Error.WindowGone'
                        && validFrame(initialFrame) && validFrame(finalFrame)
                        && (finalFrame.x !== initialFrame.x || finalFrame.y !== initialFrame.y
                            || finalFrame.width !== initialFrame.width || finalFrame.height !== initialFrame.height)) {
                        error = dbusError('org.window_zones.Gnome.Error.PartialMove', JSON.stringify({
                            window_id: String(id),
                            observed: [finalFrame.x, finalFrame.y, finalFrame.width, finalFrame.height],
                            message: error.message ?? String(error),
                        }));
                    }
                    if (error) {
                        reject(error);
                    } else {
                        if (move.senderAlive)
                            this._transitionMetrics.set(sender, [requestMs, (GLib.get_monotonic_time() - started) / 1000]);
                        resolve();
                    }
                },
            };
            this._moves.set(window, move);
            if (actor) {
                destroyedId = actor.connect('destroy', () => {
                    liveActor = null;
                    move.finish(dbusError('org.window_zones.Gnome.Error.WindowGone', `window ${id} closed during transition`));
                });
            }
            const requestFrame = () => {
                const centerX = x + Math.floor(width / 2);
                const centerY = y + Math.floor(height / 2);
                const targetMonitor = this.GetDisplays().findIndex(([, left, top, monitorWidth, monitorHeight]) =>
                    centerX >= left && centerX < left + monitorWidth
                    && centerY >= top && centerY < top + monitorHeight);
                if (targetMonitor >= 0 && window.get_monitor() !== targetMonitor) {
                    // The restore configure is acknowledged before changing monitor bounds.
                    window.move_to_monitor(targetMonitor);
                }
                window.move_resize_frame(true, x, y, width, height);
                requestMs = (GLib.get_monotonic_time() - started) / 1000;
                requested = true;
            };
            sourceId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 16, () => {
                try {
                    const frame = window.get_frame_rect();
                    if (requested && Math.abs(frame.x - x) <= 2 && Math.abs(frame.y - y) <= 2
                        && Math.abs(frame.width - width) <= 2 && Math.abs(frame.height - height) <= 2) {
                        sourceId = 0;
                        if (clone) {
                            actor.translation_x = oldFrame.x - x;
                            actor.translation_y = oldFrame.y - y;
                            actor.scale_x = oldFrame.width / width;
                            actor.scale_y = oldFrame.height / height;
                            clone.ease({
                                x, y,
                                scale_x: width / oldFrame.width,
                                scale_y: height / oldFrame.height,
                                opacity: 0,
                                duration: WINDOW_ANIMATION_TIME,
                                mode: Clutter.AnimationMode.EASE_OUT_QUAD,
                                onStopped: completed => move.finish(completed ? null
                                    : dbusError('org.window_zones.Gnome.Error.InvalidWindowState', 'animation interrupted')),
                            });
                            actor.ease({
                                scale_x: 1,
                                scale_y: 1,
                                translation_x: 0,
                                translation_y: 0,
                                duration: WINDOW_ANIMATION_TIME,
                                mode: Clutter.AnimationMode.EASE_OUT_QUAD,
                            });
                            if (frozen) {
                                actor.thaw();
                                frozen = false;
                            }
                        } else {
                            move.finish(null);
                        }
                        return GLib.SOURCE_REMOVE;
                    }
                    if (GLib.get_monotonic_time() - started > 500_000)
                        throw dbusError('org.window_zones.Gnome.Error.InvalidWindowState',
                            `window did not reach requested ${x},${y},${width},${height}; observed ${frame.x},${frame.y},${frame.width},${frame.height}`);
                } catch (error) {
                    sourceId = 0;
                    move.finish(error);
                    return GLib.SOURCE_REMOVE;
                }
                return GLib.SOURCE_CONTINUE;
            });
            try {
                if (restoring && actor) {
                    // Wait for the restore configure; a late ACK must not overwrite the target.
                    nativeSizeId = global.window_manager.connect_after('size-changed', (_manager, changedActor) => {
                        if (changedActor !== actor)
                            return;
                        global.window_manager.disconnect(nativeSizeId);
                        nativeSizeId = 0;
                        try {
                            requestFrame();
                        } catch (error) {
                            move.finish(error);
                        }
                    });
                    Main.wm.skipNextEffect(actor);
                    window.unmaximize();
                } else {
                    if (restoring)
                        window.unmaximize();
                    requestFrame();
                }
            } catch (error) {
                move.finish(error);
            }
        });
    }

    _registerHotkeys(sender, hotkeys) {
        if (!sender)
            throw dbusError(
                'org.freedesktop.DBus.Error.AccessDenied',
                'D-Bus sender is unavailable');
        if (this._controllerSender !== null && this._controllerSender !== sender)
            throw dbusError(
                'org.window_zones.Gnome.Error.Busy',
                'another App instance owns the registered hotkey set');

        const next = new Map();
        const grabbed = [];

        try {
            for (const hotkey of hotkeys) {
                if (next.has(hotkey))
                    throw new Error(`duplicate hotkey '${hotkey}'`);

                const accelerator = canonicalAccelerator(hotkey);
                const existingAction = this._accelerators.get(hotkey);
                if (existingAction !== undefined) {
                    next.set(hotkey, existingAction);
                    continue;
                }

                const action = global.display.grab_accelerator(accelerator, 0);
                if (action === 0)
                    throw new Error(`GNOME rejected hotkey '${hotkey}' (${accelerator})`);

                grabbed.push(action);
                const bindingName = Meta.external_binding_name_for_action(action);
                Main.wm.allowKeybinding(bindingName, Shell.ActionMode.NORMAL);
                this._allowedKeybindings.set(action, bindingName);
                next.set(hotkey, action);
            }
        } catch (error) {
            for (const action of grabbed)
                this._ungrabAccelerator(action);
            throw dbusError(
                'org.window_zones.Gnome.Error.Unsupported',
                error.message);
        }

        for (const [hotkey, action] of this._accelerators) {
            if (next.get(hotkey) !== action)
                this._ungrabAccelerator(action);
        }
        this._accelerators = next;
        this._hotkeysByAction = new Map([...next].map(([hotkey, action]) => [action, hotkey]));
        this._controllerSender = next.size === 0 ? null : sender;
    }

    _ungrabAccelerator(action) {
        const bindingName = this._allowedKeybindings.get(action);
        if (bindingName) {
            Main.wm.allowKeybinding(bindingName, Shell.ActionMode.NONE);
            this._allowedKeybindings.delete(action);
        }
        global.display.ungrab_accelerator(action);
    }

    _clearAccelerators() {
        for (const action of this._accelerators.values())
            this._ungrabAccelerator(action);
        this._accelerators.clear();
        this._hotkeysByAction.clear();
    }


    _emitHotkey(action) {
        const hotkey = this._hotkeysByAction.get(action);
        if (hotkey !== undefined && this._connection && this._controllerSender) {
            this._connection.emit_signal(
                this._controllerSender,
                OBJECT_PATH,
                INTERFACE_NAME,
                'HotkeyPressed',
                new GLib.Variant('(s)', [hotkey]));
        }
    }

    _applicationWindow(window) {
        if (
            Main.overview?.visible === true
            || Main.sessionMode?.isLocked === true
            || Main.screenShield?.locked === true
        )
            return null;

        if (!window)
            return null;
        if (NON_APPLICATION_WINDOW_TYPES.has(window.get_window_type())
            || window.is_override_redirect()
            || window.get_pid() === SHELL_PID)
            return null;

        const frame = window.get_frame_rect();
        if (frame.width <= 0 || frame.height <= 0)
            return null;

        return window;
    }
}

export default class WindowZonesExtension extends Extension {
    enable() {
        this._service = null;
        this._ownerId = Gio.bus_own_name(
            Gio.BusType.SESSION,
            SERVICE_NAME,
            Gio.BusNameOwnerFlags.NONE,
            connection => {
                if (this._service)
                    this._service.destroy();
                this._service = new WindowZonesService();
                this._service.export(connection);
            },
            () => {},
            () => {
                if (this._service) {
                    this._service.destroy();
                    this._service = null;
                }
            });
    }

    disable() {
        if (this._service) {
            this._service.destroy();
            this._service = null;
        }

        if (this._ownerId) {
            Gio.bus_unown_name(this._ownerId);
            this._ownerId = 0;
        }
    }
}
