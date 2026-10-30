const accel = Gio.DBusProxy.new_sync(
    connection, Gio.DBusProxyFlags.NONE, null,
    'org.kde.kglobalaccel', '/kglobalaccel', 'org.kde.KGlobalAccel', null);
function available() {
    return accel.call_sync(
        'isGlobalShortcutAvailable',
        new GLib.Variant('(is)', [0x0d000012, '']),
        Gio.DBusCallFlags.NONE, 5000, null).deep_unpack()[0];
}

function waitResult(id, label) {
    let result = [false, '', ''];
    for (let attempt = 0; attempt < 100; attempt++) {
        try {
            result = requestResult(id);
            if (result[0])
                break;
        } catch (_) {
        }
        GLib.usleep(100000);
    }
    print(`${label}_RESULT ${result[0]} ${result[1]} ${result[2]}`);
    return result;
}

const validId = request([valid]);
waitResult(validId, 'VALID');
print('VALID_READY');
poll(6);
try {
    const invalidId = request([valid, 'alt+ctrl+f25']);
    waitResult(invalidId, 'INVALID');
} catch (error) {
    // Canonical-vocabulary rejection can happen before the script request is queued.
    print(`INVALID_RESULT true ${error.message}`);
}
print('INVALID_READY');
poll(6);
waitResult(request([]), 'EMPTY');
print(`EMPTY_AVAILABLE ${available()}`);
waitResult(request([valid]), 'READDED');
print(`READDED_AVAILABLE ${available()}`);
waitResult(request(['alt+ctrl+f23']), 'REPLACED');
print(`REPLACED_AVAILABLE ${available()}`);
waitResult(request([valid]), 'RESTORED');
print(`RESTORED_AVAILABLE ${available()}`);
print('LIFECYCLE_READY');
poll(30);
