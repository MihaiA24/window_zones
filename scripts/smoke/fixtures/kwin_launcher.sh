#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$DBUS_SESSION_BUS_ADDRESS" >"$KDE_BUS_FILE"
# Xwayland can activate the EIS portal before the first client starts.
dbus-update-activation-environment "WAYLAND_DISPLAY=$KDE_SOCKET"
"$KDE_COMPANION_BIN" >"$KDE_SERVICE_LOG" 2>&1 &
printf '%s\n' "$!" >"$KDE_SERVICE_PID_FILE"
printf '%s\n' "$$" >"$KDE_KWIN_PID_FILE"
for ((attempt = 0; attempt < 100; attempt++)); do
    service_list=$(busctl --address="$DBUS_SESSION_BUS_ADDRESS" list 2>/dev/null || true)
    [[ "$service_list" == *'org.window_zones.KWin'* ]] && break
    sleep 0.1
done
# The virtual framebuffer backend keeps the two outputs at a fixed size. A nested windowed backend
# would make them windows of the outer compositor, which is free to resize them mid-run and did.
exec kwin_wayland \
    --virtual \
    --socket "$KDE_SOCKET" \
    --width 1200 --height 900 --scale 1 \
    --output-count 2 --xwayland \
    --no-lockscreen --no-kactivities \
    "$KDE_PUBLISH_CLIENT_ENV"
