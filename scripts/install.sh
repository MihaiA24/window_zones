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
  -h, --help            Show this help message
EOF
}

INSTALL_PREFIX="${WINDOW_ZONES_INSTALL_PREFIX:-$HOME/.local}"
BINARY_NAME="window_zones"
BUILD_PROFILE="release"

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

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo is required to build window_zones" >&2
  exit 1
fi

if [[ "$(uname -s)" == "Linux" ]] && ! command -v pkg-config >/dev/null 2>&1; then
  echo "pkg-config is required on Linux to build window_zones with tray support." >&2
  echo "Install it and dbus dev headers (example: sudo apt install pkg-config libdbus-1-dev)." >&2
  echo "Then re-run ./scripts/install.sh." >&2
  exit 1
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
cp "$SOURCE_BIN" "$INSTALL_DIR/$BINARY_NAME"
chmod +x "$INSTALL_DIR/$BINARY_NAME"

echo "Installed: $INSTALL_DIR/$BINARY_NAME"
echo "Profile: $BUILD_PROFILE"
echo "Tip: add $INSTALL_DIR to PATH if it is not already present"
