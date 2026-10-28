"""Press one accelerator on a kernel virtual keyboard.

Xwayland XTEST events never reach a compositor-level accelerator grab, so the only way to
evidence real hotkey capture on a seated GNOME session is a device libinput can see.
"""
import fcntl, os, struct, sys, time

UI_SET_EVBIT, UI_SET_KEYBIT = 0x40045564, 0x40045565
UI_DEV_CREATE, UI_DEV_DESTROY = 0x5501, 0x5502
EV_SYN, EV_KEY, SYN_REPORT = 0, 1, 0
KEYS = {
    'ctrl': 29, 'alt': 56, 'shift': 42, 'super': 125, 'cmd': 125,
    'left': 105, 'right': 106, 'up': 103, 'down': 108, 'pause': 119,
}

combo = [KEYS[name] for name in sys.argv[1].lower().split('+')]
fd = os.open('/dev/uinput', os.O_WRONLY | os.O_NONBLOCK)
try:
    fcntl.ioctl(fd, UI_SET_EVBIT, EV_KEY)
    for code in dict.fromkeys(combo):
        fcntl.ioctl(fd, UI_SET_KEYBIT, code)
    os.write(fd, struct.pack('=80sHHHHI' + 'i' * 256,
                             b'window-zones-smoke', 3, 0x6a6a, 0x6b6b, 1, 0, *([0] * 256)))
    fcntl.ioctl(fd, UI_DEV_CREATE)
    time.sleep(1.0)  # libinput has to notice the device before it forwards anything

    def emit(etype, code, value):
        # native 'l' matches the kernel's time fields: 24 bytes on LP64, 16 on 32-bit
        os.write(fd, struct.pack('llHHi', 0, 0, etype, code, value))

    for code in combo:
        emit(EV_KEY, code, 1)
        emit(EV_SYN, SYN_REPORT, 0)
    time.sleep(0.05)
    for code in reversed(combo):
        emit(EV_KEY, code, 0)
        emit(EV_SYN, SYN_REPORT, 0)
    time.sleep(0.3)
    fcntl.ioctl(fd, UI_DEV_DESTROY)
finally:
    os.close(fd)
