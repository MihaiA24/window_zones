import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

const valid = GLib.getenv('LIVE_VALID_HOTKEY');
const address = GLib.getenv('DBUS_SESSION_BUS_ADDRESS');
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
    'org.window_zones.Gnome',
    '/org/window_zones/Gnome',
    'org.window_zones.Gnome2',
    null);
try {
    proxy.call_sync('RegisterHotkeys', new GLib.Variant('(as)', [[valid]]),
        Gio.DBusCallFlags.NONE, 5000, null);
    print('VALID_OK');
} catch (error) {
    print(`VALID_FAILED ${Gio.DBusError.get_remote_error(error)} ${error.message}`);
}
GLib.usleep(3000000);
try {
    proxy.call_sync('RegisterHotkeys', new GLib.Variant('(as)', [[valid, 'f25']]),
        Gio.DBusCallFlags.NONE, 5000, null);
    print('INVALID_ACCEPTED');
} catch (error) {
    print(`INVALID_REJECTED ${Gio.DBusError.get_remote_error(error)} ${error.message}`);
}
GLib.usleep(10000000);
try {
    proxy.call_sync('RegisterHotkeys', new GLib.Variant('(as)', [[]]),
        Gio.DBusCallFlags.NONE, 5000, null);
} catch (_) {
}
