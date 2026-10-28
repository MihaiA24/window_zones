function register(keys, label) {
    const id = request(keys);
    let result = [false, '', ''];
    for (let attempt = 0; attempt < 100; attempt++) {
        result = requestResult(id);
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
