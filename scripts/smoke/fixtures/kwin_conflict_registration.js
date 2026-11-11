function register(keys, label) {
    const pending = request(keys);
    let result = [false, '', ''];
    while (GLib.get_monotonic_time() < pending.deadline) {
        result = requestResult(pending);
        if (result[0])
            break;
        GLib.usleep(100000);
    }
    print(`${label}_RESULT ${JSON.stringify(result)}`);
    print(`${label}_READY`);
}

register([valid], 'VALID');
poll(6);
register(['alt+ctrl+f24'], 'CONFLICT');
poll(15);
