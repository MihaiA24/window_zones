#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"

print_usage() {
  cat <<'EOF'
Usage: ./scripts/install.sh [OPTIONS]

Install Window Zones into a local prefix; optional integrations are per-user.

Options:
  -p, --prefix PATH     Installation prefix (default: $HOME/.local)
  -b, --binary NAME     Binary name to install (default: window_zones)
      --debug           Install a debug build instead of release
      --release         Install a release build (default)
      --gnome-extension Install and enable the GNOME Shell companion
      --kwin-script     Install/upgrade the KWin script and Rust companion
      --autostart       Start the App and any installed KWin companion at login
      --init-config     Write a starter config without overwriting an existing one
      --uninstall       Remove this prefix's recorded installations; keep config
  -h, --help            Show this help message
EOF
}

warn() {
  printf 'Warning: %s\n' "$*" >&2
}

# GNOME Shell only knows extensions it scanned at login, so `gnome-extensions enable`
# refuses one installed during the session; the enabled-extensions key is what the
# next login reads. $1 is add or remove.
gnome_enabled_list() {
  command -v gsettings >/dev/null 2>&1 || return 1
  local current next entry="'$GNOME_ID'"
  current="$(gsettings get org.gnome.shell enabled-extensions)" || return 1
  case "$1" in
    add)
      [[ "$current" != *"$entry"* ]] || return 0
      case "$current" in
        '@as []'|'[]') next="[$entry]" ;;
        *) next="${current%]}, $entry]" ;;
      esac
      ;;
    remove)
      [[ "$current" == *"$entry"* ]] || return 0
      next="${current//", $entry"/}"
      next="${next//"$entry, "/}"
      next="${next//"$entry"/}"
      ;;
  esac
  gsettings set org.gnome.shell enabled-extensions "$next"
}

INSTALL_PREFIX="${WINDOW_ZONES_INSTALL_PREFIX:-$HOME/.local}"
BINARY_NAME="window_zones"
BUILD_PROFILE="release"
GNOME_EXTENSION=0
KWIN_SCRIPT=0
AUTOSTART=0
INIT_CONFIG=0
UNINSTALL=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    -p|--prefix)
      INSTALL_PREFIX="${2:?missing argument for --prefix}"
      shift 2
      ;;
    -b|--binary)
      BINARY_NAME="${2:?missing argument for --binary}"
      shift 2
      ;;
    --debug) BUILD_PROFILE="debug"; shift ;;
    --release) BUILD_PROFILE="release"; shift ;;
    --gnome-extension) GNOME_EXTENSION=1; shift ;;
    --kwin-script) KWIN_SCRIPT=1; shift ;;
    --autostart) AUTOSTART=1; shift ;;
    --init-config) INIT_CONFIG=1; shift ;;
    --uninstall) UNINSTALL=1; shift ;;
    -h|--help) print_usage; exit 0 ;;
    *) printf 'Unknown option: %s\n\n' "$1" >&2; print_usage; exit 1 ;;
  esac
done

case "$BINARY_NAME" in
  ''|.|..|*/*|*$'\n'*|*$'\r'*)
    printf 'Invalid binary name: %s\n' "$BINARY_NAME" >&2
    exit 1
    ;;
esac
case "$INSTALL_PREFIX" in
  ''|*$'\n'*|*$'\r'*) echo 'The installation prefix must be a non-empty, single-line path.' >&2; exit 1 ;;
  /*) ;;
  *) INSTALL_PREFIX="$PWD/$INSTALL_PREFIX" ;;
esac
if [[ "$UNINSTALL" -eq 0 && ( "$AUTOSTART" -eq 1 || "$INIT_CONFIG" -eq 1 ) && "$BINARY_NAME" != window_zones ]]; then
  echo '--autostart and --init-config require --binary window_zones (the App binary).' >&2
  exit 1
fi
OS="$(uname -s)"
if [[ "$UNINSTALL" -eq 0 && "$OS" == Darwin && ( "$GNOME_EXTENSION" -eq 1 || "$KWIN_SCRIPT" -eq 1 ) ]]; then
  echo '--gnome-extension and --kwin-script install Linux compositor companions; macOS needs neither.' >&2
  exit 1
fi

if [[ "$UNINSTALL" -eq 0 ]]; then
  mkdir -p -- "$INSTALL_PREFIX"
fi
if [[ -d "$INSTALL_PREFIX" ]]; then
  INSTALL_PREFIX="$(cd -- "$INSTALL_PREFIX" && pwd -P)"
fi

CONFIG_HOME="${XDG_CONFIG_HOME:-$HOME/.config}"
[[ "$CONFIG_HOME" == /* ]] || CONFIG_HOME="$HOME/.config"
APP_CONFIG="$CONFIG_HOME/window_zones/config.toml"
[[ "$OS" != Darwin ]] || APP_CONFIG="$HOME/Library/Application Support/window_zones/config.toml"
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
[[ "$DATA_HOME" == /* ]] || DATA_HOME="$HOME/.local/share"
INSTALL_DIR="$INSTALL_PREFIX/bin"
INSTALL_RECORD="$INSTALL_PREFIX/share/window-zones/install-record"
UNIT_DIR="$CONFIG_HOME/systemd/user"
UNIT_FILE="$UNIT_DIR/window-zones.service"
AUTOSTART_FILE="$CONFIG_HOME/autostart/window_zones.desktop"
KWIN_UNIT_FILE="$UNIT_DIR/window-zones-kwin.service"
KWIN_AUTOSTART_FILE="$CONFIG_HOME/autostart/window_zones_kwin.desktop"
KDE_LAUNCHER="$INSTALL_PREFIX/share/window-zones/kde-session"
KDE_SESSION_STATE="$INSTALL_PREFIX/share/window-zones/kde-session-running"
GNOME_ID='window-zones@mihai-a24'
GNOME_DIR="$DATA_HOME/gnome-shell/extensions/$GNOME_ID"
KWIN_ID='window-zones'
KWIN_DIR="$DATA_HOME/kwin/scripts/$KWIN_ID"
LAUNCH_LABEL='org.window_zones.run'
LAUNCH_AGENT="$HOME/Library/LaunchAgents/$LAUNCH_LABEL.plist"
# ponytail: launchd never rotates this log; add a newsyslog/rotation step if it grows large.
LAUNCH_LOG="$HOME/Library/Logs/window_zones.log"
LAUNCH_TARGET="gui/$UID/$LAUNCH_LABEL"
# Bash < 4.4 (macOS /bin/bash 3.2) treats "${INSTALLED[@]}" of an empty array as
# unset under set -u; expand it as ${INSTALLED[@]+"${INSTALLED[@]}"} instead.
INSTALLED=()
if [[ -f "$INSTALL_RECORD" ]]; then
  while IFS= read -r item || [[ -n "$item" ]]; do
    INSTALLED+=("$item")
  done <"$INSTALL_RECORD"
fi

# Record ownership so uninstall never guesses which other binaries/packages to remove.
record_install() {
  recorded "$1" && return 0
  mkdir -p "$(dirname -- "$INSTALL_RECORD")"
  printf '%s\n' "$1" >>"$INSTALL_RECORD"
  INSTALLED+=("$1")
}

recorded() {
  local item
  for item in ${INSTALLED[@]+"${INSTALLED[@]}"}; do
    [[ "$item" != "$1" ]] || return 0
  done
  return 1
}

systemd_escape() {
  local value="$1"
  value="${value//\\/\\\\}"
  value="${value//\"/\\\"}"
  value="${value//%/%%}"
  value="${value//\$/\$\$}"
  printf '%s' "$value"
}

desktop_escape() {
  local value="$1"
  value="${value//\\/\\\\\\\\}"
  value="${value//\"/\\\\\"}"
  value="${value//\$/\\\\\$}"
  value="${value//\`/\\\\\`}"
  value="${value//%/%%}"
  printf '%s' "$value"
}

unit_owned() {
  [[ -f "$1" ]] && grep -Fqx -- "ExecStart=\"$(systemd_escape "$2")\"${3:+ $3}" "$1"
}

desktop_owned() {
  [[ -f "$1" ]] && grep -Fqx -- "Exec=\"$(desktop_escape "$2")\"${3:+ $3}" "$1"
}

kde_launcher_owned() {
  [[ -f "$KDE_LAUNCHER" ]] && grep -Fqx -- "# Window Zones KDE login: $INSTALL_DIR" "$KDE_LAUNCHER"
}

kde_session_running() {
  local pid started
  [[ -f "$KDE_SESSION_STATE/owner" ]] || return 1
  {
    IFS= read -r pid || return 1
    IFS= read -r started || return 1
  } <"$KDE_SESSION_STATE/owner"
  case "$pid" in ''|*[!0-9]*) return 1 ;; esac
  [[ -n "$started" && "$(ps -p "$pid" -o lstart= 2>/dev/null)" == "$started" ]] || return 1
  KDE_SESSION_PID="$pid"
}

stop_kde_session() {
  local attempt
  if kde_launcher_owned && kde_session_running; then
    kill -TERM "$KDE_SESSION_PID" 2>/dev/null || return 0
    for attempt in 1 2 3 4 5; do
      kde_session_running || return 0
      sleep 1
    done
    echo 'The owned KDE login launcher did not stop; refusing to remove its binaries.' >&2
    exit 1
  fi
}

remove_unit() {
  local file="$1" binary="$2" args="$3" name="${1##*/}"
  if unit_owned "$file" "$binary" "$args"; then
    if user_systemd_available; then
      systemctl --user disable --now "$name" || warn "Could not stop/disable $name."
    else
      warn "User systemd is unreachable; stop any running $name manually."
    fi
    if [[ "$UNIT_DIR/graphical-session.target.wants/$name" -ef "$file" ]]; then
      rm -f -- "$UNIT_DIR/graphical-session.target.wants/$name"
    fi
    rm -f -- "$file"
    if user_systemd_available; then
      systemctl --user daemon-reload || warn 'Could not reload user systemd.'
    fi
  elif [[ -e "$file" ]]; then
    warn "$name no longer references this prefix; leaving it untouched."
  fi
}

user_systemd_available() {
  command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1
}

launch_agent_loaded() {
  launchctl print "$LAUNCH_TARGET" >/dev/null 2>&1
}

launch_agent_owned() {
  [[ -f "$LAUNCH_AGENT" ]] \
    && [[ "$(/usr/libexec/PlistBuddy -c 'Print :ProgramArguments:0' "$LAUNCH_AGENT" 2>/dev/null)" == "$INSTALL_DIR/window_zones" ]]
}

if [[ "$UNINSTALL" -eq 1 ]]; then
  if recorded kwin-autostart; then
    stop_kde_session
  fi
  for item in ${INSTALLED[@]+"${INSTALLED[@]}"}; do
    case "$item" in
      autostart)
        remove_unit "$UNIT_FILE" "$INSTALL_DIR/window_zones" run
        if desktop_owned "$AUTOSTART_FILE" "$INSTALL_DIR/window_zones" run; then
          rm -f -- "$AUTOSTART_FILE"
        fi
        ;;
      kwin-autostart)
        remove_unit "$KWIN_UNIT_FILE" "$INSTALL_DIR/window_zones_kwin" ''
        if desktop_owned "$KWIN_AUTOSTART_FILE" /bin/bash "\"$(desktop_escape "$KDE_LAUNCHER")\""; then
          rm -f -- "$KWIN_AUTOSTART_FILE"
        fi
        if kde_launcher_owned; then
          rm -f -- "$KDE_LAUNCHER"
        fi
        ;;
      launch-agent)
        if launch_agent_owned; then
          if launch_agent_loaded; then
            launchctl bootout "$LAUNCH_TARGET" || warn "Could not stop $LAUNCH_LABEL; stop any running App before removing its binary."
          fi
          rm -f -- "$LAUNCH_AGENT"
        else
          warn 'LaunchAgent no longer references this prefix; leaving it untouched.'
        fi
        ;;
      gnome-extension)
        if [[ ! -f "$GNOME_DIR/.window-zones-install-prefix" ]] \
          || [[ "$(<"$GNOME_DIR/.window-zones-install-prefix")" != "$INSTALL_PREFIX" ]]; then
          warn 'GNOME companion ownership does not match this prefix; leaving it enabled and installed. Reinstall with --gnome-extension from the intended prefix before uninstalling.'
          continue
        fi
        if command -v gnome-extensions >/dev/null 2>&1; then
          gnome-extensions disable "$GNOME_ID" 2>/dev/null || true
        fi
        gnome_enabled_list remove || warn 'Could not remove the GNOME extension from org.gnome.shell enabled-extensions.'
        echo 'GNOME companion removed; log out/in to unload it from the running Shell.'
        rm -rf -- "$GNOME_DIR"
        ;;
      kwin-script)
        if [[ ! -f "$KWIN_DIR/.window-zones-install-prefix" ]] \
          || [[ "$(<"$KWIN_DIR/.window-zones-install-prefix")" != "$INSTALL_PREFIX" ]]; then
          warn 'KWin package ownership does not match this prefix; leaving it enabled and installed.'
          continue
        fi
        if command -v kwriteconfig6 >/dev/null 2>&1; then
          kwriteconfig6 --file kwinrc --group Plugins --key "${KWIN_ID}Enabled" false || warn 'Could not disable the KWin script in kwinrc.'
        fi
        if command -v qdbus6 >/dev/null 2>&1; then
          qdbus6 org.kde.KWin /Scripting org.kde.kwin.Scripting.unloadScript "$KWIN_ID" || warn 'Could not unload the KWin script; log out/in to unload it.'
        fi
        if command -v kpackagetool6 >/dev/null 2>&1 && [[ -d "$KWIN_DIR" ]]; then
          kpackagetool6 --type=KWin/Script --remove "$KWIN_ID" || warn 'KWin package removal failed; removing the installed package directory directly.'
        fi
        rm -rf -- "$KWIN_DIR"
        echo 'KWin script removed; log out/in if the current session still has it loaded.'
        ;;
    esac
  done
  # Stop the listener and unload companions before removing their binaries.
  for item in ${INSTALLED[@]+"${INSTALLED[@]}"}; do
    case "$item" in
      binary:*)
        name="${item#binary:}"
        case "$name" in
          ''|.|..|*/*|*$'\n'*|*$'\r'*) warn "Ignoring invalid binary record: $item" ;;
          *) rm -f -- "$INSTALL_DIR/$name" ;;
        esac
        ;;
    esac
  done
  rm -f -- "$INSTALL_RECORD"
  printf 'Uninstalled recorded Window Zones files from %s. Configuration was kept.\n' "$INSTALL_PREFIX"
  exit 0
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo 'cargo is required to build Window Zones; install the Rust toolchain and re-run this script.' >&2
  exit 1
fi
if ! command -v python3 >/dev/null 2>&1; then
  echo 'python3 is required to read Cargo build metadata.' >&2
  exit 1
fi
if [[ "$KWIN_SCRIPT" -eq 1 ]] && ! command -v kpackagetool6 >/dev/null 2>&1; then
  echo '--kwin-script requires kpackagetool6; install the Plasma 6 package tools and re-run this script.' >&2
  exit 1
fi
if [[ "$OS" == Linux ]]; then
  if ! command -v pkg-config >/dev/null 2>&1 || ! pkg-config --exists dbus-1; then
    echo 'Linux builds require pkg-config and D-Bus development libraries.' >&2
    echo 'Arch/CachyOS: sudo pacman -S --needed pkgconf dbus' >&2
    echo 'Debian/Ubuntu: sudo apt install pkg-config libdbus-1-dev' >&2
    echo 'Fedora: sudo dnf install pkgconf-pkg-config dbus-devel' >&2
    exit 1
  fi
fi

cd -- "$PROJECT_ROOT"
BINARIES=("$BINARY_NAME")
BUILD_ARGS=(build --locked --bin "$BINARY_NAME")
if [[ "$BUILD_PROFILE" == release ]]; then
  BUILD_ARGS+=(--release)
fi
if [[ "$KWIN_SCRIPT" -eq 1 && "$BINARY_NAME" != window_zones_kwin ]]; then
  BUILD_ARGS+=(--bin window_zones_kwin)
  BINARIES+=(window_zones_kwin)
fi
cargo "${BUILD_ARGS[@]}"
TARGET_DIR="$(cargo metadata --locked --no-deps --format-version 1 \
  | python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')"
mkdir -p "$INSTALL_DIR"
for name in "${BINARIES[@]}"; do
  SOURCE_BIN="$TARGET_DIR/$BUILD_PROFILE/$name"
  if [[ ! -x "$SOURCE_BIN" ]]; then
    printf 'Build did not produce expected binary: %s\n' "$SOURCE_BIN" >&2
    exit 1
  fi
  install -m 755 -- "$SOURCE_BIN" "$INSTALL_DIR/$name"
  record_install "binary:$name"
  printf 'Installed: %s\n' "$INSTALL_DIR/$name"
done

if [[ "$GNOME_EXTENSION" -eq 1 ]]; then
  mkdir -p "$GNOME_DIR"
  install -m 644 -- "$PROJECT_ROOT/gnome-extension/metadata.json" "$PROJECT_ROOT/gnome-extension/extension.js" "$GNOME_DIR/"
  printf '%s\n' "$INSTALL_PREFIX" >"$GNOME_DIR/.window-zones-install-prefix"
  record_install gnome-extension
  if command -v gnome-extensions >/dev/null 2>&1 && gnome-extensions enable "$GNOME_ID" 2>/dev/null; then
    echo 'GNOME companion enabled.'
  elif gnome_enabled_list add; then
    echo 'GNOME companion enabled for your next login (org.gnome.shell enabled-extensions).'
  else
    warn "Could not enable the GNOME companion; after logging back in, run: gnome-extensions enable $GNOME_ID"
  fi
  printf 'GNOME companion: %s\n' "$GNOME_DIR"
  echo 'GNOME on Wayland needs a log out/in before a newly installed extension loads.'
  printf 'Verify: gnome-extensions info %s (State: ACTIVE)\n' "$GNOME_ID"
  echo 'Verify: gsettings get org.gnome.shell disable-user-extensions (must be false)'
fi

if [[ "$KWIN_SCRIPT" -eq 1 ]]; then
  if [[ -d "$KWIN_DIR" ]]; then
    kpackagetool6 --type=KWin/Script --upgrade "$PROJECT_ROOT/kwin-script"
  else
    kpackagetool6 --type=KWin/Script --install "$PROJECT_ROOT/kwin-script"
  fi
  printf '%s\n' "$INSTALL_PREFIX" >"$KWIN_DIR/.window-zones-install-prefix"
  record_install kwin-script
  echo 'KWin script: window-zones (enable Window Zones in System Settings -> Window Management -> KWin Scripts).'
  echo 'After an upgrade, disable/re-enable the script to reload it; upgrade the script and companion together.'
  if [[ "$AUTOSTART" -eq 0 ]] && ! recorded kwin-autostart; then
    printf 'Keep the companion running on your graphical session bus: "%s/window_zones_kwin"\n' "$INSTALL_DIR"
  fi
fi

if [[ "$INIT_CONFIG" -eq 1 ]]; then
  "$INSTALL_DIR/window_zones" init || warn 'Starter config could not be initialized; run window_zones init and resolve the reported error.'
fi

KDE_AUTOSTART=0
if [[ "$OS" == Linux ]] && recorded kwin-script && recorded binary:window_zones_kwin; then
  KDE_AUTOSTART=1
fi

if [[ "$AUTOSTART" -eq 1 && "$OS" == Darwin ]]; then
  mkdir -p "$(dirname -- "$LAUNCH_AGENT")" "$(dirname -- "$LAUNCH_LOG")"
  xml_escape() { printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'; }
  RESTART_LOADED_AGENT=0
  if launch_agent_loaded && launch_agent_owned; then
    RESTART_LOADED_AGENT=1
  fi
  # No KeepAlive: launchd cannot exempt exit 75 (another instance owns the hotkeys)
  # from restarts, so the agent starts once per login/load and is never respawned.
  cat >"$LAUNCH_AGENT" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>$LAUNCH_LABEL</string>
  <key>ProgramArguments</key>
  <array>
    <string>$(xml_escape "$INSTALL_DIR/window_zones")</string>
    <string>run</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>StandardOutPath</key>
  <string>$(xml_escape "$LAUNCH_LOG")</string>
  <key>StandardErrorPath</key>
  <string>$(xml_escape "$LAUNCH_LOG")</string>
</dict>
</plist>
EOF
  # launchd refuses agents that are group/world writable (e.g. under umask 002).
  chmod 644 "$LAUNCH_AGENT"
  record_install launch-agent
  if [[ "$RESTART_LOADED_AGENT" -eq 1 ]]; then
    # Keep the loaded job: bootout followed immediately by bootstrap can race
    # launchd's asynchronous teardown on reinstall.
    launchctl kickstart -k "$LAUNCH_TARGET" || warn "Could not restart $LAUNCH_LABEL."
    printf 'Autostart: %s (loaded; restart requested)\n' "$LAUNCH_AGENT"
  else
    if launch_agent_loaded; then
      launchctl bootout "$LAUNCH_TARGET" || warn "Could not unload the previous $LAUNCH_LABEL."
    fi
    if launchctl bootstrap "gui/$UID" "$LAUNCH_AGENT"; then
      printf 'Autostart: %s (loaded; starts at every login)\n' "$LAUNCH_AGENT"
    else
      warn "Could not load the LaunchAgent into gui/$UID; it loads at your next login, or run: launchctl bootstrap gui/$UID \"$LAUNCH_AGENT\""
    fi
  fi
  printf 'Restart: launchctl kickstart -k %s\n' "$LAUNCH_TARGET"
  printf 'Stop until next login: launchctl bootout %s\n' "$LAUNCH_TARGET"
  printf 'Logs: tail -f "%s"\n' "$LAUNCH_LOG"
  printf 'Allow %s in System Settings > Privacy & Security > Accessibility (again after each reinstall).\n' "$INSTALL_DIR/window_zones"
elif [[ "$AUTOSTART" -eq 1 ]]; then
  if [[ "$KDE_AUTOSTART" -eq 1 && ! -x "$INSTALL_DIR/window_zones_kwin" ]]; then
    echo 'The recorded KWin companion is missing; reinstall with --kwin-script --autostart.' >&2
    exit 1
  fi
  if [[ "$KDE_AUTOSTART" -eq 1 ]] && ! user_systemd_available \
    && ! command -v busctl >/dev/null 2>&1 && ! command -v dbus-send >/dev/null 2>&1; then
    echo 'KDE XDG autostart needs busctl or dbus-send to wait for the companion session bus name.' >&2
    exit 1
  fi
  mkdir -p "$UNIT_DIR"
  cat >"$UNIT_FILE" <<EOF
[Unit]
Description=Window Zones hotkey listener
PartOf=graphical-session.target
After=graphical-session.target
$(if [[ "$KDE_AUTOSTART" -eq 1 ]]; then printf 'Wants=window-zones-kwin.service\nAfter=window-zones-kwin.service\n'; fi)

[Service]
Type=simple
ExecStart="$(systemd_escape "$INSTALL_DIR/window_zones")" run
Restart=on-failure
RestartSec=2
RestartPreventExitStatus=75

[Install]
WantedBy=graphical-session.target
EOF
  record_install autostart
  if [[ "$KDE_AUTOSTART" -eq 1 ]]; then
    cat >"$KWIN_UNIT_FILE" <<EOF
[Unit]
Description=Window Zones KWin companion
PartOf=graphical-session.target
After=graphical-session.target

[Service]
Type=dbus
BusName=org.window_zones.KWin
ExecStart="$(systemd_escape "$INSTALL_DIR/window_zones_kwin")"
Restart=on-failure
RestartSec=2

[Install]
WantedBy=graphical-session.target
EOF
    record_install kwin-autostart
  fi
  if user_systemd_available; then
    systemctl --user daemon-reload
    if [[ "$KDE_AUTOSTART" -eq 1 ]]; then
      stop_kde_session
      systemctl --user enable window-zones-kwin.service
      systemctl --user restart window-zones-kwin.service
      if desktop_owned "$KWIN_AUTOSTART_FILE" /bin/bash "\"$(desktop_escape "$KDE_LAUNCHER")\""; then
        rm -f -- "$KWIN_AUTOSTART_FILE"
      fi
    fi
    systemctl --user enable window-zones.service
    if desktop_owned "$AUTOSTART_FILE" "$INSTALL_DIR/window_zones" run; then
      rm -f -- "$AUTOSTART_FILE"
    fi
    if [[ ":${XDG_CURRENT_DESKTOP:-}:" == *:GNOME:* ]] \
      && ! busctl --user status org.window_zones.Gnome >/dev/null 2>&1; then
      # Starting now would only retry until GNOME Shell loads the companion at login.
      printf 'Autostart: %s (enabled; starts at your next login, once GNOME Shell loads the companion)\n' "$UNIT_FILE"
    else
      systemctl --user restart window-zones.service
      printf 'Autostart: %s (enabled and running)\n' "$UNIT_FILE"
    fi
    echo 'Manage it: systemctl --user status|stop|start|restart window-zones'
    echo 'Logs: journalctl --user -u window-zones -f'
  else
    mkdir -p "$(dirname -- "$AUTOSTART_FILE")"
    if [[ "$KDE_AUTOSTART" -eq 1 ]]; then
      {
        printf '#!/usr/bin/env bash\n# Window Zones KDE login: %s\n' "$INSTALL_DIR"
        printf 'INSTALL_DIR=%q\nSESSION_STATE=%q\n' "$INSTALL_DIR" "$KDE_SESSION_STATE"
        cat <<'EOF'
set -u
umask 077
if ! mkdir "$SESSION_STATE" 2>/dev/null; then
  old_pid='' old_start=''
  if [[ -f "$SESSION_STATE/owner" ]]; then
    { IFS= read -r old_pid; IFS= read -r old_start; } <"$SESSION_STATE/owner"
  fi
  case "$old_pid" in ''|*[!0-9]*) exit 75 ;; esac
  if [[ -n "$old_start" && "$(ps -p "$old_pid" -o lstart= 2>/dev/null)" == "$old_start" ]]; then
    exit 75
  fi
  rm -f -- "$SESSION_STATE/owner"
  rmdir "$SESSION_STATE" && mkdir "$SESSION_STATE" || exit 75
fi
children=()
stop_children() {
  if [[ -n "${children[*]-}" ]]; then
    kill -TERM "${children[@]}" 2>/dev/null || true
    wait "${children[@]}" 2>/dev/null || true
    children=()
  fi
}
cleanup() {
  stop_children
  rm -f -- "$SESSION_STATE/owner"
  rmdir "$SESSION_STATE"
}
trap cleanup EXIT
trap 'exit 0' TERM INT HUP
trap 'cleanup; trap - EXIT; exec "$0"' USR1
printf '%s\n%s\n' "$$" "$(ps -p "$$" -o lstart=)" >"$SESSION_STATE/owner"
supervise() {
  child=''
  trap 'if [[ -n "$child" ]]; then kill -TERM "$child" 2>/dev/null; wait "$child" 2>/dev/null; fi; exit 0' TERM INT
  trap '' HUP USR1
  while :; do
    "$@" &
    child=$!
    result=0
    wait "$child" || result=$?
    child=''
    if [[ "$1" == "$INSTALL_DIR/window_zones" && ( "$result" -eq 0 || "$result" -eq 75 ) ]]; then
      exit "$result"
    fi
    sleep 2 &
    child=$!
    wait "$child" || true
    child=''
  done
}
supervise "$INSTALL_DIR/window_zones_kwin" &
children+=("$!")
companion_ready() {
  if command -v busctl >/dev/null 2>&1; then
    busctl --user status org.window_zones.KWin >/dev/null 2>&1
  else
    dbus-send --session --type=method_call --print-reply --dest=org.freedesktop.DBus \
      /org/freedesktop/DBus org.freedesktop.DBus.GetNameOwner string:org.window_zones.KWin >/dev/null 2>&1
  fi
}
until companion_ready; do
  sleep 1 &
  children+=("$!")
  wait "$!" || true
  unset 'children[1]'
done
supervise "$INSTALL_DIR/window_zones" run &
children+=("$!")
wait "${children[@]}"
EOF
      } >"$KDE_LAUNCHER"
      chmod 755 "$KDE_LAUNCHER"
      cat >"$KWIN_AUTOSTART_FILE" <<EOF
[Desktop Entry]
Type=Application
Name=Window Zones KDE session
Exec="/bin/bash" "$(desktop_escape "$KDE_LAUNCHER")"
Terminal=false
EOF
      if desktop_owned "$AUTOSTART_FILE" "$INSTALL_DIR/window_zones" run; then
        rm -f -- "$AUTOSTART_FILE"
      fi
      if kde_session_running; then
        kill -USR1 "$KDE_SESSION_PID"
      fi
      printf 'User systemd is unreachable; KDE companion and App use XDG login supervision: %s\n' "$KWIN_AUTOSTART_FILE"
    else
      cat >"$AUTOSTART_FILE" <<EOF
[Desktop Entry]
Type=Application
Name=Window Zones
Exec="$(desktop_escape "$INSTALL_DIR/window_zones")" run
X-GNOME-Autostart-enabled=true
Terminal=false
EOF
      printf 'User systemd is unreachable; using XDG autostart: %s\n' "$AUTOSTART_FILE"
    fi
    echo 'To switch to systemd later, re-run the installer with --autostart.'
  fi
fi

# A running listener keeps executing the replaced binary until it restarts.
if [[ "$AUTOSTART" -eq 0 && "$BINARY_NAME" == window_zones && "$OS" == Darwin ]] \
  && launch_agent_owned && [[ "$(launchctl print "$LAUNCH_TARGET" 2>/dev/null)" == *'state = running'* ]]; then
  if launchctl kickstart -k "$LAUNCH_TARGET"; then
    echo "Restarted $LAUNCH_LABEL onto the new binary."
  else
    warn "Could not restart $LAUNCH_LABEL; run: launchctl kickstart -k $LAUNCH_TARGET"
  fi
fi

if [[ "$AUTOSTART" -eq 0 && "$OS" == Linux ]]; then
  if [[ "$KWIN_SCRIPT" -eq 1 || "$BINARY_NAME" == window_zones_kwin ]] \
    && recorded kwin-autostart && unit_owned "$KWIN_UNIT_FILE" "$INSTALL_DIR/window_zones_kwin" '' \
    && user_systemd_available && systemctl --user is-active --quiet window-zones-kwin.service; then
    systemctl --user restart window-zones-kwin.service
    echo 'Restarted window-zones-kwin.service onto the new binary.'
  fi
  if [[ "$BINARY_NAME" == window_zones ]] && recorded autostart \
    && unit_owned "$UNIT_FILE" "$INSTALL_DIR/window_zones" run \
    && user_systemd_available && systemctl --user is-active --quiet window-zones.service; then
    systemctl --user restart window-zones.service
    echo 'Restarted window-zones.service onto the new binary.'
  fi
  if recorded kwin-autostart && kde_launcher_owned && kde_session_running; then
    kill -USR1 "$KDE_SESSION_PID"
    echo 'Restarted the owned KDE login launcher onto the new binaries.'
  fi
fi

printf 'Installation complete (profile: %s, prefix: %s).\n' "$BUILD_PROFILE" "$INSTALL_PREFIX"

# Whether typing the command in a new terminal runs what was just installed.
INSTALLED_BIN="$INSTALL_DIR/$BINARY_NAME"
RESOLVED_BIN="$(command -v -- "$BINARY_NAME" 2>/dev/null || true)"
if [[ -n "$RESOLVED_BIN" && "$RESOLVED_BIN" -ef "$INSTALLED_BIN" ]]; then
  printf 'Ready: run `%s` from any terminal, e.g. `%s --help`.\n' "$BINARY_NAME" "$BINARY_NAME"
elif [[ -n "$RESOLVED_BIN" ]]; then
  warn "\`$BINARY_NAME\` runs $RESOLVED_BIN, not $INSTALLED_BIN; remove that copy or put $INSTALL_DIR earlier on PATH."
else
  printf '%s is not on PATH. Add it once, then open a new terminal:\n' "$INSTALL_DIR"
  case "${SHELL##*/}" in
    fish) printf '  fish_add_path "%s"\n' "$INSTALL_DIR" ;;
    zsh) printf "  echo 'export PATH=\"%s:\$PATH\"' >> ~/.zshrc\n" "$INSTALL_DIR" ;;
    *) printf "  echo 'export PATH=\"%s:\$PATH\"' >> ~/.bashrc\n" "$INSTALL_DIR" ;;
  esac
fi

if [[ "$BINARY_NAME" == window_zones && "$INIT_CONFIG" -eq 0 && ! -e "$APP_CONFIG" ]]; then
  echo 'Next: create a starter config with `window_zones init` (or re-run with --init-config).'
fi
if [[ "$BINARY_NAME" == window_zones && "$GNOME_EXTENSION" -eq 0 && ":${XDG_CURRENT_DESKTOP:-}:" == *:GNOME:* && ! -d "$GNOME_DIR" ]]; then
  echo 'GNOME detected: the App needs its Shell companion here; re-run with --gnome-extension.'
fi
