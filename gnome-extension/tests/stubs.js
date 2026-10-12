// Shell-hosted APIs are stubbed; Gio, GLib and D-Bus variants remain real.
import GLib from 'gi://GLib';

export const settings = {enable_animations: true, reducedMotion: 0};
export const Clutter = {AnimationMode: {EASE_OUT_QUAD: 0}};
export const WINDOW_ANIMATION_TIME = 250;

export class TestActor {
    constructor(properties = {}) {
        Object.assign(this, properties);
        this.handlers = new Map();
        this.nextHandler = 1;
        this.transition = 0;
        this.frozen = false;
        this.destroyed = false;
    }
    connect(name, callback) {
        const id = this.nextHandler++;
        this.handlers.set(id, {name, callback});
        return id;
    }
    disconnect(id) { this.handlers.delete(id); }
    paint_to_content() { return {}; }
    freeze() { this.frozen = true; }
    thaw() { this.frozen = false; }
    set_scale(x, y) { this.scale_x = x; this.scale_y = y; }
    ease(properties) {
        this.transition = GLib.timeout_add(GLib.PRIORITY_DEFAULT, properties.duration, () => {
            this.transition = 0;
            Object.assign(this, properties);
            properties.onStopped?.(true);
            return GLib.SOURCE_REMOVE;
        });
    }
    remove_all_transitions() {
        if (this.transition !== 0) {
            GLib.source_remove(this.transition);
            this.transition = 0;
        }
    }
    destroy() {
        this.destroyed = true;
        this.remove_all_transitions();
        for (const {name, callback} of [...this.handlers.values()]) {
            if (name === 'destroy')
                callback();
        }
    }
}

export const St = {
    Settings: {get: () => settings},
    ReducedMotion: {REDUCE: 1},
    Widget: TestActor,
};

const windowManager = {
    handlers: new Map(),
    nextHandler: 1,
    connect_after(name, callback) {
        const id = this.nextHandler++;
        this.handlers.set(id, {name, callback});
        return id;
    },
    disconnect(id) { this.handlers.delete(id); },
    emit(name, actor) {
        for (const handler of [...this.handlers.values()]) {
            if (handler.name === name)
                handler.callback(this, actor);
        }
    },
};
export const Meta = {
    WindowType: Object.fromEntries([
        'NORMAL', 'DESKTOP', 'DOCK', 'MENU', 'TOOLBAR', 'DROPDOWN_MENU',
        'POPUP_MENU', 'TOOLTIP', 'NOTIFICATION', 'COMBO', 'DND', 'OVERRIDE_OTHER',
    ].map((name, index) => [name, index])),
    external_binding_name_for_action: action => `external-grab-${action}`,
};

export const Shell = {ActionMode: {NONE: 0, NORMAL: 1}};
export class Extension {}

export const Main = {
    overview: {visible: false},
    sessionMode: {isLocked: false},
    screenShield: {locked: false},
    wm: {
        allowed: new Map(),
        allowKeybinding(name, mode) {
            this.allowed.set(name, mode);
        },
        skipNextEffect() {},
    },
    layoutManager: {
        monitors: [{index: 0}, {index: 1}],
        getWorkAreaForMonitor: index => index === 0
            ? {x: -1920, y: 27, width: 1920, height: 1053}
            : {x: 0, y: 0, width: 2560, height: 1440},
    },
};

export function makeWindow(id, overrides = {}) {
    return {
        id,
        frame: {x: -300, y: -20, width: 801, height: 602},
        windowType: Meta.WindowType.NORMAL,
        pid: 1234,
        overrideRedirect: false,
        maximizeFlags: 0,
        tiled: false,
        fullscreen: false,
        resizable: true,
        monitor: 1,
        actor: null,
        operations: [],
        get_id() { return this.id; },
        get_frame_rect() { return this.frame; },
        get_window_type() { return this.windowType; },
        get_pid() { return this.pid; },
        is_override_redirect() { return this.overrideRedirect; },
        get_maximize_flags() { return this.maximizeFlags; },
        is_fullscreen() { return this.fullscreen; },
        get resizeable() { return this.resizable; },
        allows_resize() { return this.resizable && this.maximizeFlags === 0 && !this.tiled; },
        get_compositor_private() { return this.actor; },
        get_monitor() { return this.monitor; },
        move_to_monitor(index) {
            this.operations.push(['monitor', index]);
            this.monitor = index;
        },
        // Meta-18.gir: unmaximize takes only its instance, not flags. Mutter's
        // edge tiling uses VERTICAL maximization, cleared by this same method.
        unmaximize(...args) {
            if (args.length !== 0)
                throw new Error('Shell 50 unmaximize takes no arguments');
            this.operations.push(['restore']);
            this.maximizeFlags = 0;
            this.tiled = false;
            if (this.actor) {
                GLib.idle_add(GLib.PRIORITY_DEFAULT, () => {
                    windowManager.emit('size-changed', this.actor);
                    return GLib.SOURCE_REMOVE;
                });
            }
        },
        move_resize_frame(userOp, x, y, width, height) {
            if (this.maximizeFlags !== 0 || this.tiled)
                throw new Error('window must be restored before moving');
            this.operations.push(['move', userOp, x, y, width, height]);
            this.frame = {x, y, width, height};
        },
        ...overrides,
    };
}

export const display = {
    focus: null,
    windows: [],
    grabbed: new Map(),
    rejected: new Set(),
    handler: null,
    nextAction: 1,
    connect(name, callback) {
        this.handler = {name, callback};
        return 7;
    },
    disconnect() { this.handler = null; },
    get_focus_window() { return this.focus; },
    list_all_windows() { return this.windows; },
    grab_accelerator(accelerator) {
        if (this.rejected.has(accelerator) || [...this.grabbed.values()].includes(accelerator))
            return 0;
        const action = this.nextAction++;
        this.grabbed.set(action, accelerator);
        return action;
    },
    ungrab_accelerator(action) { this.grabbed.delete(action); },
    activate(action) { this.handler.callback(this, action); },
};

globalThis.global = {
    display,
    window_manager: windowManager,
    window_group: {
        children: [],
        add_child(actor) { this.children.push(actor); },
        set_child_above_sibling() {},
    },
};
