# Installation Runbook

## Prerequisites
- Rust toolchain with `cargo` in `PATH`.
- On Linux, `pkg-config` and D-Bus development headers are required for tray
  and KWin companion support:
  - Debian/Ubuntu: `sudo apt install pkg-config libdbus-1-dev`
  - Fedora: `sudo dnf install pkgconf-pkg-config dbus-devel`
- (Optional) `$HOME/.local/bin` on `PATH` for default installation location.
- For GNOME Wayland, GNOME Shell 50 and `gnome-extensions` are required when
  using the optional companion; install steps are in
  `docs/runbooks/gnome-wayland.md`.
- For KDE Plasma Wayland, `kpackagetool6` is required to install the KWin
  script; setup steps are in `docs/runbooks/kde-wayland.md`.

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

On Linux, install the KDE companion binary as a second step:

```bash
./scripts/install.sh --prefix "$HOME/.local" --binary window_zones_kwin
```

## What the script does
- Resolves project root relative to the script location.
- Builds the selected binary with Cargo (`--locked`, matching lockfile).
- Copies that binary to `<prefix>/bin/<binary>`.

## Verification

```bash
$HOME/.local/bin/window_zones --help
```

If installation is to a custom location, replace the path accordingly.

For GNOME Wayland installation and extension lifecycle, continue with
`docs/runbooks/gnome-wayland.md`.
For KDE Plasma Wayland installation and KWin script lifecycle, continue with
`docs/runbooks/kde-wayland.md`.
