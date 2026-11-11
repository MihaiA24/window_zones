#!/usr/bin/env bash
# Build the opt-in bridge locally; never install it or touch a running session.
set -euo pipefail
ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
SOURCE="$ROOT/target/cosmic-comp-src"
TARGET="$ROOT/target/cosmic-comp-build"
OVERLAY="$ROOT/cosmic-comp"
PROFILE=debug
usage() {
    cat <<'EOF'
Usage: scripts/build-cosmic.sh [--source DIR] [--target-dir DIR] [--release]
                             [--overlay DIR] [--help]
Fetch the revision pinned in cosmic-comp/revision if SOURCE does not exist,
validate a pristine or exactly bridge-patched checkout, then build cosmic-comp.
Requires Git, Cargo/Rust, Python 3 and cosmic-comp's native development dependencies.
Uses Cargo's locked dependencies. Network access is needed for a fresh build.
No sudo, installation, desktop replacement or compositor restart is performed.
Do not run against a checkout another process/person is currently editing.
EOF
}
while (($#)); do
    case "$1" in
        --source|--target-dir|--overlay)
            (($# >= 2)) || { echo "Missing value for $1" >&2; exit 2; }
            case "$1" in
                --source) SOURCE=$2 ;;
                --target-dir) TARGET=$2 ;;
                --overlay) OVERLAY=$2 ;;
            esac
            shift 2 ;;
        --release) PROFILE=release; shift ;;
        --help|-h) usage; exit 0 ;;
        *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    esac
done
SOURCE=$(realpath -m -- "$SOURCE")
TARGET=$(realpath -m -- "$TARGET")
OVERLAY=$(realpath -- "$OVERLAY")
REV=$(tr -d '\n\r' < "$OVERLAY/revision")
[[ "$REV" =~ ^[0-9a-f]{40}$ ]] || { echo 'Invalid pinned revision' >&2; exit 1; }
[[ -s "$OVERLAY/bridge.patch" && -s "$OVERLAY/window_zones.rs" ]] || {
    echo 'Missing durable bridge.patch or window_zones.rs overlay' >&2; exit 1;
}
# Capture provenance before any source is staged or compiled.
INPUTS="$(/usr/bin/python3 -c '
import hashlib, json, sys
from pathlib import Path
overlay = Path(sys.argv[1])
print(json.dumps({name: hashlib.sha256((overlay / name).read_bytes()).hexdigest()
                  for name in ("revision", "bridge.patch", "window_zones.rs")}))
' "$OVERLAY")"
mkdir -p -- "$(dirname -- "$SOURCE")" "$TARGET"
# The lock is outside SOURCE, so even a fresh clone is exclusive.
exec 9>"$SOURCE.window-zones.lock"
flock -n 9 || { echo 'Another bridge build owns this source checkout' >&2; exit 1; }
if [[ ! -e "$SOURCE" ]]; then
    git init -- "$SOURCE"
    git -C "$SOURCE" remote add origin https://github.com/pop-os/cosmic-comp.git
    git -C "$SOURCE" fetch --depth 1 origin "$REV"
    git -C "$SOURCE" checkout --detach "$REV"
fi
[[ "$(git -C "$SOURCE" rev-parse --show-toplevel)" == "$SOURCE" ]] || {
    echo 'Refusing source directory that is not a Git checkout root' >&2; exit 1;
}
[[ "$(git -C "$SOURCE" rev-parse HEAD)" == "$REV" ]] || {
    echo "Refusing checkout not at pinned revision $REV: $SOURCE" >&2; exit 1;
}
git -C "$SOURCE" diff --cached --quiet || {
    echo 'Refusing checkout with staged changes' >&2; exit 1;
}
# Construct the complete expected tree without changing the real Git index.
# Exact comparison rejects unrelated edits even inside a patched source file.
INDEX=$(mktemp "$TARGET/bridge-index.XXXXXX")
rm -- "$INDEX"
trap 'rm -f -- "$INDEX" "$INDEX.lock"' EXIT
export GIT_INDEX_FILE="$INDEX"
git -C "$SOURCE" read-tree "$REV"
git -C "$SOURCE" apply --cached --check "$OVERLAY/bridge.patch"
git -C "$SOURCE" apply --cached "$OVERLAY/bridge.patch"
BLOB=$(git -C "$SOURCE" hash-object -w -- "$OVERLAY/window_zones.rs")
git -C "$SOURCE" update-index --add --cacheinfo "100644,$BLOB,src/dbus/window_zones.rs"
EXPECTED=0
if git -C "$SOURCE" update-index -q --refresh &&
    git -C "$SOURCE" diff-files --quiet &&
    [[ -z "$(git -C "$SOURCE" ls-files --others --exclude-standard)" ]]; then
    EXPECTED=1
fi
unset GIT_INDEX_FILE
if (( ! EXPECTED )); then
    git -C "$SOURCE" diff --quiet &&
        [[ -z "$(git -C "$SOURCE" ls-files --others --exclude-standard)" ]] || {
        echo 'Refusing dirty source: it is neither pristine nor the exact durable bridge overlay' >&2
        exit 1
    }
    git -C "$SOURCE" apply --check "$OVERLAY/bridge.patch"
    [[ ! -e "$SOURCE/src/dbus/window_zones.rs" ]] || {
        echo 'Overlay destination unexpectedly exists in pristine source' >&2; exit 1;
    }
    git -C "$SOURCE" apply "$OVERLAY/bridge.patch"
    cp -- "$OVERLAY/window_zones.rs" "$SOURCE/src/dbus/window_zones.rs"
fi
ARGS=(build --locked --manifest-path "$SOURCE/Cargo.toml" --target-dir "$TARGET" --bin cosmic-comp)
[[ "$PROFILE" == release ]] && ARGS+=(--release)
BUILD_ARTIFACTS="$(CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" cargo "${ARGS[@]}" --message-format=json-render-diagnostics)"
BINARY="$(printf '%s\n' "$BUILD_ARTIFACTS" | /usr/bin/python3 -c '
import hashlib, json, sys
from pathlib import Path
paths = {
    message["executable"] for line in sys.stdin
    if (message := json.loads(line)).get("reason") == "compiler-artifact"
    and message["target"]["name"] == "cosmic-comp"
    and "bin" in message["target"]["kind"] and message.get("executable")
}
if len(paths) != 1:
    sys.exit(f"Expected one Cargo executable for cosmic-comp, found {len(paths)}")
binary = Path(paths.pop()).resolve()
overlay = Path(sys.argv[1])
inputs = json.loads(sys.argv[3])
current_inputs = {name: hashlib.sha256((overlay / name).read_bytes()).hexdigest()
                  for name in ("revision", "bridge.patch", "window_zones.rs")}
if current_inputs != inputs:
    sys.exit("Bridge sources changed during build; rebuild before publishing provenance")
proof = {"revision": sys.argv[2], "inputs": inputs,
         "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest()}
stamp = binary.with_name(binary.name + ".window-zones.json")
temporary = stamp.with_name(stamp.name + ".tmp")
temporary.write_text(json.dumps(proof, indent=2) + "\n")
temporary.replace(stamp)
print(binary)
' "$OVERLAY" "$REV" "$INPUTS")"
printf '\nLocal custom compositor: %s\n' "$BINARY"
printf '%s\n' 'Opt in only for an intentionally launched test session: WINDOW_ZONES_COSMIC_BRIDGE=1'
