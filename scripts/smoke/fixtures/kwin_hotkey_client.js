import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

const address = GLib.getenv('DBUS_SESSION_BUS_ADDRESS');
const valid = GLib.getenv('KDE_VALID_HOTKEY');
const connection = Gio.DBusConnection.new_for_address_sync(
    address,
    Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT |
    Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION,
    null,
    null);
const proxy = Gio.DBusProxy.new_sync(
    connection,
    Gio.DBusProxyFlags.NONE,
    null,
    'org.window_zones.KWin',
    '/org/window_zones/KWin',
    'org.window_zones.KWin3',
    null);

function request(keys) {
    const reply = proxy.call_sync(
        'RegisterHotkeys',
        new GLib.Variant('(s)', [JSON.stringify(keys)]),
        Gio.DBusCallFlags.NONE,
        5000,
        null);
    return reply.deep_unpack()[0];
}

function requestResult(id) {
    const reply = proxy.call_sync(
        'GetRequestResult',
        new GLib.Variant('(s)', [String(id)]),
        Gio.DBusCallFlags.NONE,
        5000,
        null);
    return reply.deep_unpack();
}

function poll(seconds) {
    const deadline = Date.now() + seconds * 1000;
    while (Date.now() < deadline) {
        try {
            const result = proxy.call_sync(
                'GetNextHotkey',
                new GLib.Variant('()', []),
                Gio.DBusCallFlags.NONE,
                5000,
                null).deep_unpack();
            if (result[0])
                print(`EVENT ${result[1]}`);
        } catch (_) {
        }
        GLib.usleep(100000);
    }
}

