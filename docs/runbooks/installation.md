# Installation Runbook

## Prerequisites
- Rust toolchain with `cargo` in `PATH`.
- On Linux, `pkg-config` and D-Bus development headers are required for tray support:
  - Debian/Ubuntu: `sudo apt install pkg-config libdbus-1-dev`
  - Fedora: `sudo dnf install pkgconf-pkg-config dbus-devel`
- (Optional) `$HOME/.local/bin` on `PATH` for default installation location.

## Steps

1. Install in release mode (default):

```bash
./scripts/install.sh
```

2. Install to a custom prefix:

```bash
./scripts/install.sh --prefix "$HOME/.local" --binary window_zones
```

3. Install a debug build (faster iteration):

```bash
./scripts/install.sh --debug
```

## What the script does
- Resolves project root relative to the script location.
- Builds the crate with Cargo (`--locked`, matching lockfile).
- Copies the built binary to `<prefix>/bin/window_zones`.

## Verification

```bash
$HOME/.local/bin/window_zones --help
```

If installation is to a custom location, replace the path accordingly.
