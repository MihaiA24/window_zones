import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
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
    ['printscreen', 'Print'],
    ['pause', 'Pause'],
    ['menu', 'Menu'],
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
        this._acceleratorSignalId = global.display.connect(
            'accelerator-activated',
            (_display, action) => this._emitHotkey(action));
        this._impl = Gio.DBusExportedObject.wrapJSObject(INTERFACE_XML, this);
    }

    export(connection) {
        this._impl.export(connection, OBJECT_PATH);
    }

    destroy() {
        for (const action of this._accelerators.values())
            global.display.ungrab_accelerator(action);
        this._accelerators.clear();

        if (this._acceleratorSignalId !== 0) {
            global.display.disconnect(this._acceleratorSignalId);
            this._acceleratorSignalId = 0;
        }

        this._impl.flush();
        this._impl.unexport();
    }

    GetCapabilities() {
        return CAPABILITIES;
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
        return this._displayData().map(item => [
            item.id,
            item.rect.x,
            item.rect.y,
            item.rect.width,
            item.rect.height,
        ]);
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

    RegisterHotkeys(hotkeys) {
        const next = new Map();
        const grabbed = [];

        try {
            for (const hotkey of hotkeys) {
                if (next.has(hotkey))
                    throw new Error(`duplicate hotkey '${hotkey}'`);

                const accelerator = canonicalAccelerator(hotkey);
                const action = global.display.grab_accelerator(accelerator, 0);
                if (action === 0)
                    throw new Error(`GNOME rejected hotkey '${hotkey}' (${accelerator})`);

                next.set(hotkey, action);
                grabbed.push(action);
            }
        } catch (error) {
            for (const action of grabbed)
                global.display.ungrab_accelerator(action);
            throw dbusError(
                'org.window_zones.Gnome.Error.Unsupported',
                error.message);
        }

        for (const action of this._accelerators.values())
            global.display.ungrab_accelerator(action);
        this._accelerators = next;
    }

    _emitHotkey(action) {
        for (const [hotkey, registeredAction] of this._accelerators) {
            if (registeredAction !== action)
                continue;

            this._impl.emit_signal(
                'HotkeyPressed',
                new GLib.Variant('(s)', [hotkey]));
            return;
        }
    }

    _focusedWindow() {
        const window = global.display.get_focus_window();
        if (!window || typeof window.get_frame_rect !== 'function')
            return null;

        if (typeof window.get_window_type === 'function') {
            const windowType = window.get_window_type();
            if (windowType === Meta.WindowType.DESKTOP || windowType === Meta.WindowType.DOCK)
                return null;
        }

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

        return monitors.map((_monitor, index) => ({
            id: `monitor-${index}`,
            rect: Main.layoutManager.getWorkAreaForMonitor(index),
        }));
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
