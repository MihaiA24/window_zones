import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
import Shell from 'gi://Shell';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const SERVICE_NAME = 'org.window_zones.Gnome';
const OBJECT_PATH = '/org/window_zones/Gnome';
const INTERFACE_NAME = 'org.window_zones.Gnome1';
const CAPABILITIES = [
    'focused-window',
    'displays',
    'move-resize',
    'hotkeys',
];

const INTERFACE_XML = `
<node>
  <interface name="${INTERFACE_NAME}">
    <method name="GetCapabilities">
      <arg name="capabilities" type="as" direction="out"/>
    </method>
    <method name="GetFocusedWindow">
      <arg name="present" type="b" direction="out"/>
      <arg name="display_id" type="s" direction="out"/>
      <arg name="x" type="i" direction="out"/>
      <arg name="y" type="i" direction="out"/>
      <arg name="width" type="u" direction="out"/>
      <arg name="height" type="u" direction="out"/>
    </method>
    <method name="GetDisplays">
      <arg name="displays" type="a(siiuu)" direction="out"/>
    </method>
    <method name="MoveFocusedWindow">
      <arg name="x" type="i" direction="in"/>
      <arg name="y" type="i" direction="in"/>
      <arg name="width" type="u" direction="in"/>
      <arg name="height" type="u" direction="in"/>
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

function canonicalAccelerator(hotkey) {
    const tokens = hotkey
        .split('+')
        .map(token => token.trim().toLowerCase())
        .filter(Boolean);
    const modifiers = [];
    let key = null;

    for (const token of tokens) {
        const modifier = MODIFIER_TOKENS.get(token);
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

    let gdkKey = KEY_TOKENS.get(key);
    if (!gdkKey && /^f([1-9]|1[0-9]|2[0-4])$/.test(key))
        gdkKey = key.toUpperCase();
    if (!gdkKey && /^[a-z0-9]$/.test(key))
        gdkKey = key;
    if (!gdkKey)
        throw new Error(`unsupported GNOME accelerator key '${key}' in '${hotkey}'`);

    return `${modifiers.join('')}${gdkKey}`;
}

function containsPoint(rect, x, y) {
    return x >= rect.x && x < rect.x + rect.width
        && y >= rect.y && y < rect.y + rect.height;
}

class WindowZonesService {
    constructor() {
        this._accelerators = new Map();
        this._allowedKeybindings = new Map();
        this._controllerSender = null;
        this._connection = null;
        this._registrationId = 0;
        this._ownerWatchId = 0;
        this._displayIds = new Map();
        this._nextDisplayId = 1;
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
                if (name === this._controllerSender && newOwner === '') {
                    this._clearAccelerators();
                    this._controllerSender = null;
                }
            });
    }

    destroy() {
        this._clearAccelerators();

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
            case 'GetCapabilities': {
                const [capabilities] = this.GetCapabilities();
                invocation.return_value(new GLib.Variant('(as)', [capabilities]));
                return;
            }
            case 'GetFocusedWindow':
                invocation.return_value(
                    new GLib.Variant('(bsiiuu)', this.GetFocusedWindow()));
                return;
            case 'GetDisplays': {
                const [displays] = this.GetDisplays();
                invocation.return_value(new GLib.Variant('(a(siiuu))', [displays]));
                return;
            }
            case 'MoveFocusedWindow': {
                const [x, y, width, height] = parameters.deep_unpack();
                this.MoveFocusedWindow(x, y, width, height);
                invocation.return_value(new GLib.Variant('()', []));
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
            const name = typeof error?.name === 'string'
                && error.name.startsWith('org.window_zones.Gnome.Error.')
                ? error.name
                : 'org.freedesktop.DBus.Error.Failed';
            invocation.return_dbus_error(name, error?.message ?? String(error));
        }
    }

    GetCapabilities() {
        return [CAPABILITIES];
    }

    GetFocusedWindow() {
        const window = this._focusedWindow();
        if (!window)
            return [false, '', 0, 0, 0, 0];

        const frame = window.get_frame_rect();
        const centerX = frame.x + Math.floor(frame.width / 2);
        const centerY = frame.y + Math.floor(frame.height / 2);
        const displays = this._displayData();
        const display = displays.find(item => containsPoint(item.rect, centerX, centerY));
        const displayId = display ? display.id : `unmatched:${centerX}:${centerY}`;

        return [true, displayId, frame.x, frame.y, frame.width, frame.height];
    }

    GetDisplays() {
        return [this._displayData().map(item => [
            item.id,
            item.rect.x,
            item.rect.y,
            item.rect.width,
            item.rect.height,
        ])];
    }

    MoveFocusedWindow(x, y, width, height) {
        const window = this._focusedWindow();
        if (!window)
            throw dbusError(
                'org.window_zones.Gnome.Error.InvalidWindowState',
                'no focused application window');
        if (width === 0 || height === 0)
            throw dbusError(
                'org.window_zones.Gnome.Error.InvalidWindowState',
                'window target must have positive dimensions');
        const maximizeFlags =
            typeof window.get_maximize_flags === 'function'
                ? window.get_maximize_flags()
                : 0;
        const isTiled = typeof window.tile_mode === 'number' && window.tile_mode !== 0;
        if (
            maximizeFlags !== 0
            || isTiled
            || window.maximized_horizontally
            || window.maximized_vertically
        )
            throw dbusError(
                'org.window_zones.Gnome.Error.InvalidWindowState',
                'focused window is maximized or tiled; restore it before moving');
        if (typeof window.is_fullscreen === 'function' && window.is_fullscreen())
            throw dbusError(
                'org.window_zones.Gnome.Error.InvalidWindowState',
                'focused window is fullscreen; leave fullscreen before moving');
        if (typeof window.allows_resize === 'function' && !window.allows_resize())
            throw dbusError(
                'org.window_zones.Gnome.Error.InvalidWindowState',
                'focused window does not allow resizing');

        window.move_resize_frame(true, x, y, width, height);
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

                const existingAction = this._accelerators.get(hotkey);
                if (existingAction !== undefined) {
                    next.set(hotkey, existingAction);
                    continue;
                }

                const accelerator = canonicalAccelerator(hotkey);
                const action = global.display.grab_accelerator(
                    accelerator,
                    Meta.KeyBindingFlags.NONE);
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
    }


    _emitHotkey(action) {
        for (const [hotkey, registeredAction] of this._accelerators) {
            if (registeredAction !== action)
                continue;

            if (this._connection) {
                this._connection.emit_signal(
                    null,
                    OBJECT_PATH,
                    INTERFACE_NAME,
                    'HotkeyPressed',
                    new GLib.Variant('(s)', [hotkey]));
            }
            return;
        }
    }

    _focusedWindow() {
        if (
            Main.overview?.visible === true
            || Main.sessionMode?.isLocked === true
            || Main.screenShield?.locked === true
        )
            return null;

        const window = global.display.get_focus_window();
        if (!window || typeof window.get_frame_rect !== 'function')
            return null;

        if (typeof window.get_window_type === 'function'
            && NON_APPLICATION_WINDOW_TYPES.has(window.get_window_type()))
            return null;
        if (
            typeof global.get_pid === 'function'
            && typeof window.get_pid === 'function'
            && window.get_pid() === global.get_pid()
        )
            return null;

        const frame = window.get_frame_rect();
        if (frame.width <= 0 || frame.height <= 0)
            return null;

        return window;
    }

    _displayData() {
        const monitors = Main.layoutManager.monitors ?? [];
        if (monitors.length === 0)
            throw new Error('GNOME reported no active displays');
        if (typeof Main.layoutManager.getWorkAreaForMonitor !== 'function')
            throw new Error('GNOME work-area API is unavailable');

        const activeDisplayIds = new Map();
        const displays = monitors.map((_monitor, index) => {
            const rect = Main.layoutManager.getWorkAreaForMonitor(index);
            const key = [rect.x, rect.y, rect.width, rect.height].join(':');
            let id = this._displayIds.get(key);
            if (!id)
                id = `display-${this._nextDisplayId++}`;
            activeDisplayIds.set(key, id);
            return {id, rect};
        });
        this._displayIds = activeDisplayIds;
        return displays;
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
