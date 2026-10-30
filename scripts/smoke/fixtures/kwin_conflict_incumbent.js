'use strict';
try {
    registerShortcut(
        'Window Zones conflict incumbent',
        'Window Zones conflict incumbent',
        'Alt+Ctrl+F24',
        () => console.warn('KDE_CONFLICT_FIRED'));
    console.warn('KDE_CONFLICT_READY');
} catch (error) {
    console.warn(`KDE_CONFLICT_ERROR ${error}`);
}
