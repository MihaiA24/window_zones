#!/usr/bin/env bash
set -euo pipefail
# Record the shell's stable identity before kwin_wayland exec can make environ
# unreadable. The parent accepts only this saved PID/start handoff.
/usr/bin/python3 "$KDE_OWNED_PROCESS_HELPER" publish "$KDE_PID_REGISTRY" \
    "$WINDOW_ZONES_SMOKE_OWNER" "$KDE_RUNNER_PID" "$$" "$KDE_KWIN_PID_FILE" || exit 1
printf '%s\n' "$DBUS_SESSION_BUS_ADDRESS" >"$KDE_BUS_FILE"
# Xwayland can activate the EIS portal before the first client starts.
dbus-update-activation-environment "WAYLAND_DISPLAY=$KDE_SOCKET"
"$KDE_COMPANION_BIN" >"$KDE_SERVICE_LOG" 2>&1 &
printf '%s\n' "$!" >"$KDE_SERVICE_PID_FILE"
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
