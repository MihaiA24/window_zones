#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "${DISPLAY:-}" >"$KDE_XDISPLAY_FILE"
printf '%s\n' "${XAUTHORITY:-}" >"$KDE_XAUTHORITY_FILE"
printf '%s\n' "${DBUS_SESSION_BUS_ADDRESS:-}" >"$KDE_CLIENT_BUS_FILE"
