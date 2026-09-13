#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"

print_usage() {
  cat <<'EOF'
Usage: ./scripts/install.sh [OPTIONS]

Install the window_zones binary into a local prefix.

Options:
  -p, --prefix PATH     Installation prefix (default: $HOME/.local)
  -b, --binary NAME     Binary name to install (default: window_zones)
      --debug           Install a debug build instead of release
      --release         Install a release build (default)
      --autostart       Start window_zones run at desktop login
  -h, --help            Show this help message
EOF
}

INSTALL_PREFIX="${WINDOW_ZONES_INSTALL_PREFIX:-$HOME/.local}"
BINARY_NAME="window_zones"
BUILD_PROFILE="release"
AUTOSTART=0

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
    --debug)
      BUILD_PROFILE="debug"
      shift
      ;;
    --release)
      BUILD_PROFILE="release"
      shift
      ;;
    --autostart)
      AUTOSTART=1
      shift
      ;;
    -h|--help)
      print_usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      echo
      print_usage
      exit 1
      ;;
  esac
done

if [[ "$AUTOSTART" -eq 1 && "$BINARY_NAME" != window_zones ]]; then
  echo "--autostart requires --binary window_zones (the listener binary)." >&2
  exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo is required to build window_zones" >&2
  exit 1
fi

if [[ "$(uname -s)" == "Linux" ]]; then
  if ! command -v pkg-config >/dev/null 2>&1 \
    || ! pkg-config --exists dbus-1 x11 xtst xi; then
    echo "Linux builds require pkg-config and D-Bus, X11, XTest, and Xi development libraries." >&2
    echo "Install them (Debian/Ubuntu: sudo apt install pkg-config libdbus-1-dev libx11-dev libxtst-dev libxi-dev)." >&2
    echo "Fedora: sudo dnf install pkgconf-pkg-config dbus-devel libX11-devel libXtst-devel libXi-devel." >&2
    echo "Then re-run ./scripts/install.sh." >&2
    exit 1
  fi
fi

cd "$PROJECT_ROOT"

if [[ "$BUILD_PROFILE" == "release" ]]; then
  cargo build --locked --release
  SOURCE_BIN="$PROJECT_ROOT/target/release/$BINARY_NAME"
else
  cargo build --locked
  SOURCE_BIN="$PROJECT_ROOT/target/debug/$BINARY_NAME"
fi

if [[ ! -x "$SOURCE_BIN" ]]; then
  echo "Build did not produce expected binary: $SOURCE_BIN" >&2
  exit 1
fi

INSTALL_DIR="$INSTALL_PREFIX/bin"
mkdir -p "$INSTALL_DIR"
INSTALL_DIR="$(cd -- "$INSTALL_DIR" && pwd)"
cp "$SOURCE_BIN" "$INSTALL_DIR/$BINARY_NAME"
chmod +x "$INSTALL_DIR/$BINARY_NAME"

if [[ "$AUTOSTART" -eq 1 ]]; then
  AUTOSTART_DIR="$HOME/.config/autostart"
  mkdir -p "$AUTOSTART_DIR"
  # Desktop Exec quoting is not shell quoting; escape literal field codes too.
  EXEC_PATH="$INSTALL_DIR/$BINARY_NAME"
  EXEC_PATH="${EXEC_PATH//\\/\\\\\\\\}"
  EXEC_PATH="${EXEC_PATH//\"/\\\\\"}"
  EXEC_PATH="${EXEC_PATH//\$/\\\\\$}"
  EXEC_PATH="${EXEC_PATH//\`/\\\\\`}"
  EXEC_PATH="${EXEC_PATH//%/%%}"
  cat >"$AUTOSTART_DIR/window_zones.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Window Zones
Exec="$EXEC_PATH" run
X-GNOME-Autostart-enabled=true
Terminal=false
EOF
  echo "Autostart: $AUTOSTART_DIR/window_zones.desktop"
fi

echo "Installed: $INSTALL_DIR/$BINARY_NAME"
echo "Profile: $BUILD_PROFILE"
echo "Tip: add $INSTALL_DIR to PATH if it is not already present"
