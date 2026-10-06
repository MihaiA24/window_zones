# Installation Runbook

## Prerequisites

- Rust toolchain with `cargo` in `PATH`.
- On Linux, `pkg-config` and D-Bus, X11, XTest, and Xi development libraries are
  required for tray, KWin companion, and global hotkey support:
  - Arch/CachyOS: `sudo pacman -S --needed pkgconf dbus libx11 libxtst libxi`
  - Debian/Ubuntu: `sudo apt install pkg-config libdbus-1-dev libx11-dev libxtst-dev libxi-dev`
  - Fedora: `sudo dnf install pkgconf-pkg-config dbus-devel libX11-devel libXtst-devel libXi-devel`
- `$HOME/.local/bin` (or `<prefix>/bin`) on `PATH` to type `window_zones` in any
  terminal; the installer checks this and prints the exact line to add for your
  shell (fish, zsh, or bash) when it is missing.
- For GNOME Wayland, GNOME Shell 50 and `gnome-extensions`; see
  [GNOME Wayland](gnome-wayland.md) for companion lifecycle and diagnostics.
- For KDE Plasma Wayland, `kpackagetool6` and the graphical user's session bus;
  see [KDE Wayland](kde-wayland.md) for script and companion lifecycle.

Run the installer as your desktop user, not with `sudo`. Binaries use the chosen
prefix; companions and autostart are per-user. `XDG_CONFIG_HOME` and
`XDG_DATA_HOME` override the usual `~/.config` and `~/.local/share` directories.

## Install and initialize

The default installs a release build of the App to `~/.local/bin/window_zones`:

```bash
./scripts/install.sh --init-config
```

The installer ends by checking that typing `window_zones` runs the binary it just
installed: it prints `Ready: run window_zones from any terminal` when it does,
a warning naming the other copy when an earlier `PATH` entry shadows it, or the
command that adds `<prefix>/bin` to `PATH` for your login shell (for example
`fish_add_path "$HOME/.local/bin"`).

`--init-config` runs the installed `window_zones init`. On Linux it writes a
starter config to `${XDG_CONFIG_HOME:-$HOME/.config}/window_zones/config.toml`.
It never overwrites an existing config: an existing file is reported and returns
success. An initialization error is printed as a warning; resolve it before
starting the listener. You can run the same step separately:

```bash
"$HOME/.local/bin/window_zones" init
```

Install the appropriate companion and start the listener automatically:

```bash
# GNOME Wayland
./scripts/install.sh --gnome-extension --init-config --autostart

# KDE Plasma Wayland
./scripts/install.sh --kwin-script --init-config --autostart
```

On KDE, keep `window_zones_kwin` running on the same graphical user's session
bus, as described below. The App service does not supervise that companion.

### Installer options

| Option | Effect |
| --- | --- |
| `-p, --prefix PATH` | Binary installation prefix; default `$HOME/.local` or `WINDOW_ZONES_INSTALL_PREFIX`. |
| `-b, --binary NAME` | Select a Cargo binary; default `window_zones`. |
| `--release` | Build/install the release profile (default). |
| `--debug` | Build/install the debug profile. |
| `--gnome-extension` | Install the GNOME Shell companion and attempt to enable it. |
| `--kwin-script` | Install/upgrade the KWin script and install `window_zones_kwin` alongside the selected binary. |
| `--init-config` | Run the installed App's non-overwriting `init` command. Requires `--binary window_zones`. |
| `--autostart` | Install the App's systemd user unit, or an XDG desktop entry when user systemd is unreachable. Requires `--binary window_zones`. |
| `--uninstall` | Remove installations recorded for the chosen prefix; preserve the user config. No build is performed. |
| `-h, --help` | Show usage. |

For example, use a custom prefix or a debug build:

```bash
./scripts/install.sh --prefix "$HOME/apps/window-zones" --binary window_zones --init-config
./scripts/install.sh --debug
```

The script resolves the repository relative to its location, builds the selected
binaries with `cargo build --locked`, and replaces `<prefix>/bin/<binary>`.
`CARGO_TARGET_DIR` is honored. Re-running the same options replaces the installed
files without duplicating ownership records or overwriting config. Re-run with
`--autostart` after changing the prefix so the service points at the new binary.

## GNOME Shell companion

`--gnome-extension` installs `metadata.json` and `extension.js` into
`${XDG_DATA_HOME:-$HOME/.local/share}/gnome-shell/extensions/window-zones@mihai-a24/`
and enables it. GNOME Shell only knows extensions it scanned at login, so
`gnome-extensions enable` refuses one installed during the session; the
installer then adds `window-zones@mihai-a24` to
`org.gnome.shell enabled-extensions` itself, which the next login reads.

**GNOME on Wayland needs a log out/in before a newly installed extension loads.**
An upgrade also needs the companion to be reloaded; follow the
[GNOME lifecycle runbook](gnome-wayland.md). Uninstall disables the extension and
removes it from `enabled-extensions`.

Verify after logging back in:

```bash
gnome-extensions info window-zones@mihai-a24
gsettings get org.gnome.shell disable-user-extensions
```

The extension must report **State: ACTIVE** and `disable-user-extensions` must
be **false**. If user extensions are globally disabled, explicitly enable them
with `gsettings set org.gnome.shell disable-user-extensions false`.

## KWin script and companion

`--kwin-script` installs the `KWin/Script` package from `kwin-script/` with
`kpackagetool6`, or upgrades the installed package with `--upgrade`. Its package
ID is **`window-zones`**. It also builds and installs
`<prefix>/bin/window_zones_kwin`; upgrade the script and binary together.

Enable **Window Zones** in **System Settings -> Window Management -> KWin
Scripts**. After upgrading, disable/re-enable the script to reload its code and
release old shortcut registrations. Start the Rust companion before the App
and keep it running on the graphical user's session bus:

```bash
"$HOME/.local/bin/window_zones_kwin"
```

The App's autostart unit starts only `window_zones run`, not the KWin companion.
See [KDE Wayland](kde-wayland.md) for startup and runtime checks.

## Autostart, supervision, and logs

`--autostart` writes
`${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/window-zones.service` with:

```ini
[Unit]
Description=Window Zones hotkey listener
PartOf=graphical-session.target
After=graphical-session.target

[Service]
Type=simple
ExecStart="<prefix>/bin/window_zones" run
Restart=on-failure
RestartSec=2
RestartPreventExitStatus=75

[Install]
WantedBy=graphical-session.target
```

The installer substitutes and escapes the absolute installed binary path. When
user systemd is reachable, it runs `systemctl --user daemon-reload` and
`systemctl --user enable window-zones.service`, then starts (or restarts onto
the new binary) the service with `systemctl --user restart window-zones.service`.
On GNOME, when the companion is not on the session bus yet (it was just
installed and GNOME Shell loads it at login), the service is only enabled and
starts at the next login instead of retrying until then. Re-running the
installer without `--autostart` also restarts an active service so it runs the
replaced binary.

It removes a previous `window_zones.desktop` fallback to avoid two autostart
launchers. The unit participates in the graphical session lifecycle: it starts
with the desktop session and stops at logout. The `run` command continues when
stdin is closed or is not a terminal, and writes logs to stdout/stderr, captured
by the user journal:

```bash
systemctl --user status window-zones      # state, pid, last log lines
systemctl --user stop window-zones        # stop until the next login
systemctl --user start window-zones
systemctl --user disable --now window-zones   # stop and no longer start at login
journalctl --user -u window-zones -f
```

While the service runs, problems also appear as one desktop notification that is
updated in place: config errors and refused hotkeys at once, and an unavailable
companion only after it has been unreachable for 20 seconds, so the login race
and companion restarts stay silent. The GNOME companion declares the
`unlock-dialog` session mode, so locking the screen does not unload it; its
hotkeys only act in normal mode.

The App's single-instance lock rejects a second listener with exit status **75**
(`EX_TEMPFAIL`). `RestartPreventExitStatus=75` stops systemd retrying that exit,
so an already-running instance does not cause a restart/log loop. Other failures
are retried after two seconds. Stop the other listener before manually starting
or restarting the service.

If user systemd is unreachable, the installer prints a note and writes
`${XDG_CONFIG_HOME:-$HOME/.config}/autostart/window_zones.desktop` instead. That
entry runs the same installed `window_zones run` at desktop login; it has no
systemd supervision or guaranteed journal capture. The unit is still installed.
To switch to systemd later, remove the desktop fallback before enabling it:

```bash
rm -f "${XDG_CONFIG_HOME:-$HOME/.config}/autostart/window_zones.desktop"
systemctl --user daemon-reload
systemctl --user enable --now window-zones.service
```

## Verify and uninstall

```bash
"$HOME/.local/bin/window_zones" --help
"$HOME/.local/bin/window_zones" status
```

Substitute your prefix if it differs. Follow the compositor-specific runbook for
companion and hotkey verification.

Uninstall using the same prefix used for installation:

```bash
./scripts/install.sh --uninstall
./scripts/install.sh --prefix "$HOME/apps/window-zones" --uninstall
```

The installer records its installed binaries and optional features in
`<prefix>/share/window-zones/install-record`. Uninstall stops/disables the App
service when user systemd is reachable, removes its unit and desktop entry,
disables/removes its GNOME extension, unloads/disables the KWin script when the
Plasma tools are available and removes its package, and removes recorded
binaries (including the KWin companion). No other extension, KWin package, or
binary is removed. **The config is kept.** Repeated uninstall is safe. Older
installations without an ownership record must first be upgraded with this
installer and the same feature flags, or be removed manually.

If session tools are unreachable, stop any manually running App or KWin
companion and log out/in to unload a removed compositor companion. The installer
prints warnings for session operations it cannot complete.
