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
const loop = new GLib.MainLoop(null, false);
let phase = 'BEFORE';
proxy.connect('g-signal', (_proxy, _sender, name, parameters) => {
    if (name !== 'HotkeyPressed' || parameters.deep_unpack()[0] !== valid)
        return;
    print(`EVENT ${valid} ${phase}`);
    loop.quit();
});

function waitForHotkey() {
    const timeout = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 10000, () => {
        expired = true;
        print(`HOTKEY_TIMEOUT ${phase}`);
        loop.quit();
        return GLib.SOURCE_REMOVE;
    });
    let expired = false;
    loop.run();
    if (!expired)
        GLib.source_remove(timeout);
}

try {
    proxy.call_sync('RegisterHotkeys', new GLib.Variant('(as)', [[valid]]),
        Gio.DBusCallFlags.NONE, 5000, null);
    print('VALID_OK');
} catch (error) {
    print(`VALID_FAILED ${Gio.DBusError.get_remote_error(error)} ${error.message}`);
}
waitForHotkey();
try {
    proxy.call_sync('RegisterHotkeys', new GLib.Variant('(as)', [[valid, 'f25']]),
        Gio.DBusCallFlags.NONE, 5000, null);
    print('INVALID_ACCEPTED');
} catch (error) {
    print(`INVALID_REJECTED ${Gio.DBusError.get_remote_error(error)} ${error.message}`);
}
phase = 'AFTER';
waitForHotkey();
try {
    proxy.call_sync('RegisterHotkeys', new GLib.Variant('(as)', [[]]),
        Gio.DBusCallFlags.NONE, 5000, null);
} catch (_) {
}
