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
      --autostart       Start window_zones run with the graphical session
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

CONFIG_HOME="${XDG_CONFIG_HOME:-$HOME/.config}"
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
INSTALL_DIR="$INSTALL_PREFIX/bin"
INSTALL_RECORD="$INSTALL_PREFIX/share/window-zones/install-record"
UNIT_DIR="$CONFIG_HOME/systemd/user"
UNIT_FILE="$UNIT_DIR/window-zones.service"
AUTOSTART_FILE="$CONFIG_HOME/autostart/window_zones.desktop"
GNOME_ID='window-zones@mihai-a24'
GNOME_DIR="$DATA_HOME/gnome-shell/extensions/$GNOME_ID"
KWIN_ID='window-zones'
KWIN_DIR="$DATA_HOME/kwin/scripts/$KWIN_ID"
INSTALLED=()
if [[ -f "$INSTALL_RECORD" ]]; then
  while IFS= read -r item || [[ -n "$item" ]]; do
    INSTALLED+=("$item")
  done <"$INSTALL_RECORD"
fi

# Record ownership so uninstall never guesses which other binaries/packages to remove.
record_install() {
  local item
  for item in "${INSTALLED[@]}"; do
    [[ "$item" != "$1" ]] || return 0
  done
  mkdir -p "$(dirname -- "$INSTALL_RECORD")"
  printf '%s\n' "$1" >>"$INSTALL_RECORD"
  INSTALLED+=("$1")
}

user_systemd_available() {
  command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1
}

if [[ "$UNINSTALL" -eq 1 ]]; then
  for item in "${INSTALLED[@]}"; do
    case "$item" in
      autostart)
        if user_systemd_available; then
          systemctl --user disable --now window-zones.service || warn 'Could not stop/disable window-zones.service; stop any running App before removing its binary.'
        else
          warn 'User systemd is unreachable; stop any running App manually.'
        fi
        rm -f -- "$UNIT_FILE" "$UNIT_DIR/graphical-session.target.wants/window-zones.service" "$AUTOSTART_FILE"
        if user_systemd_available; then
          systemctl --user daemon-reload || warn 'Could not reload user systemd; run systemctl --user daemon-reload when available.'
        fi
        ;;
      gnome-extension)
        if command -v gnome-extensions >/dev/null 2>&1; then
          gnome-extensions disable "$GNOME_ID" 2>/dev/null || true
        fi
        gnome_enabled_list remove || warn 'Could not remove the GNOME extension from org.gnome.shell enabled-extensions.'
        echo 'GNOME companion removed; log out/in to unload it from the running Shell.'
        rm -rf -- "$GNOME_DIR"
        ;;
      kwin-script)
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
  for item in "${INSTALLED[@]}"; do
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
if [[ "$KWIN_SCRIPT" -eq 1 ]] && ! command -v kpackagetool6 >/dev/null 2>&1; then
  echo '--kwin-script requires kpackagetool6; install the Plasma 6 package tools and re-run this script.' >&2
  exit 1
fi
if [[ "$(uname -s)" == Linux ]]; then
  if ! command -v pkg-config >/dev/null 2>&1 || ! pkg-config --exists dbus-1 x11 xtst xi; then
    echo 'Linux builds require pkg-config and D-Bus, X11, XTest, and Xi development libraries.' >&2
    echo 'Arch/CachyOS: sudo pacman -S --needed pkgconf dbus libx11 libxtst libxi' >&2
    echo 'Debian/Ubuntu: sudo apt install pkg-config libdbus-1-dev libx11-dev libxtst-dev libxi-dev' >&2
    echo 'Fedora: sudo dnf install pkgconf-pkg-config dbus-devel libX11-devel libXtst-devel libXi-devel' >&2
    exit 1
  fi
fi

cd -- "$PROJECT_ROOT"
BUILD_ARGS=(build --locked --bin "$BINARY_NAME")
if [[ "$BUILD_PROFILE" == release ]]; then
  BUILD_ARGS+=(--release)
fi
if [[ "$KWIN_SCRIPT" -eq 1 && "$BINARY_NAME" != window_zones_kwin ]]; then
  BUILD_ARGS+=(--bin window_zones_kwin)
fi
cargo "${BUILD_ARGS[@]}"
TARGET_DIR="${CARGO_TARGET_DIR:-$PROJECT_ROOT/target}"
mkdir -p "$INSTALL_DIR"
INSTALL_DIR="$(cd -- "$INSTALL_DIR" && pwd)"
BINARIES=("$BINARY_NAME")
if [[ "$KWIN_SCRIPT" -eq 1 && "$BINARY_NAME" != window_zones_kwin ]]; then
  BINARIES+=(window_zones_kwin)
fi
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
  record_install kwin-script
  echo 'KWin script: window-zones (enable Window Zones in System Settings -> Window Management -> KWin Scripts).'
  echo 'After an upgrade, disable/re-enable the script to reload it; upgrade the script and companion together.'
  printf 'Keep the companion running on your graphical session bus: "%s/window_zones_kwin"\n' "$INSTALL_DIR"
fi

if [[ "$INIT_CONFIG" -eq 1 ]]; then
  "$INSTALL_DIR/window_zones" init || warn 'Starter config could not be initialized; run window_zones init and resolve the reported error.'
fi

if [[ "$AUTOSTART" -eq 1 ]]; then
  mkdir -p "$UNIT_DIR"
  # systemd ExecStart quoting: escape quotes/backslashes, specifiers, and environment expansion.
  EXEC_PATH="$INSTALL_DIR/window_zones"
  EXEC_PATH="${EXEC_PATH//\\/\\\\}"
  EXEC_PATH="${EXEC_PATH//\"/\\\"}"
  EXEC_PATH="${EXEC_PATH//%/%%}"
  EXEC_PATH="${EXEC_PATH//\$/\$\$}"
  cat >"$UNIT_FILE" <<EOF
[Unit]
Description=Window Zones hotkey listener
PartOf=graphical-session.target
After=graphical-session.target

[Service]
Type=simple
ExecStart="$EXEC_PATH" run
Restart=on-failure
RestartSec=2
RestartPreventExitStatus=75

[Install]
WantedBy=graphical-session.target
EOF
  record_install autostart
  if user_systemd_available; then
    systemctl --user daemon-reload
    systemctl --user enable window-zones.service
    rm -f -- "$AUTOSTART_FILE"
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
    # Desktop Exec quoting is not shell quoting; escape literal field codes too.
    EXEC_PATH="$INSTALL_DIR/window_zones"
    EXEC_PATH="${EXEC_PATH//\\/\\\\\\\\}"
    EXEC_PATH="${EXEC_PATH//\"/\\\\\"}"
    EXEC_PATH="${EXEC_PATH//\$/\\\\\$}"
    EXEC_PATH="${EXEC_PATH//\`/\\\\\`}"
    EXEC_PATH="${EXEC_PATH//%/%%}"
    cat >"$AUTOSTART_FILE" <<EOF
[Desktop Entry]
Type=Application
Name=Window Zones
Exec="$EXEC_PATH" run
X-GNOME-Autostart-enabled=true
Terminal=false
EOF
    printf 'User systemd is unreachable; using XDG autostart: %s\n' "$AUTOSTART_FILE"
    printf 'Unit also installed at %s; to switch later, remove the desktop file and run:\n' "$UNIT_FILE"
    echo 'systemctl --user daemon-reload && systemctl --user enable --now window-zones.service'
  fi
fi

# A running listener keeps executing the replaced binary until it restarts.
if [[ "$AUTOSTART" -eq 0 && "$BINARY_NAME" == window_zones ]] && user_systemd_available \
  && systemctl --user is-active --quiet window-zones.service; then
  systemctl --user restart window-zones.service
  echo 'Restarted window-zones.service onto the new binary.'
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

if [[ "$BINARY_NAME" == window_zones && "$INIT_CONFIG" -eq 0 && ! -e "$CONFIG_HOME/window_zones/config.toml" ]]; then
  echo 'Next: create a starter config with `window_zones init` (or re-run with --init-config).'
fi
if [[ "$BINARY_NAME" == window_zones && "$GNOME_EXTENSION" -eq 0 && ":${XDG_CURRENT_DESKTOP:-}:" == *:GNOME:* && ! -d "$GNOME_DIR" ]]; then
  echo 'GNOME detected: the App needs its Shell companion here; re-run with --gnome-extension.'
fi
