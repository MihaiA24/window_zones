'use strict';
function rect(value) {
    return {
        x: Math.round(value.x),
        y: Math.round(value.y),
        width: Math.round(value.width),
        height: Math.round(value.height),
    };
}
function report() {
    const screens = [];
    workspace.screens.forEach((output, index) => {
        screens.push({
            name: output.name || `kwin-output-${index}`,
            geometry: rect(output.geometry),
            maximizeArea: rect(workspace.clientArea(
                KWin.MaximizeArea,
                output,
                workspace.currentDesktop)),
        });
    });
    // KWin 6 installs QJSEngine::ConsoleExtension and injects no print(); warnings are the one
    // level Qt logs without extra rules, so console.warn is what reaches the KWin log.
    console.warn(`KDE_OBSERVER ${JSON.stringify({screens})}`);
}
report();
const timer = new QTimer();
timer.interval = 200;
timer.timeout.connect(report);
timer.start();
