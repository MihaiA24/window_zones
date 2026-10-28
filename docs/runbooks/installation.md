# Installation Runbook

## Prerequisites

- Rust toolchain with `cargo`, and Python 3 for Cargo metadata, in `PATH`.
- On Linux, `pkg-config` and D-Bus development headers are required for tray
  and KWin companion support:
  - Debian/Ubuntu: `sudo apt install pkg-config libdbus-1-dev`
  - Fedora: `sudo dnf install pkgconf-pkg-config dbus-devel`
- (Optional) `$HOME/.local/bin` on `PATH` for default installation location.
- For GNOME Wayland, GNOME Shell 50 or 51 and `gnome-extensions` are required when
  using the optional companion; install steps are in
  `docs/runbooks/gnome-wayland.md`.
- For KDE Plasma Wayland, `kpackagetool6` is required to install the KWin
  script; setup steps are in `docs/runbooks/kde-wayland.md`.

Run the installer as your desktop user, not with `sudo`. Binaries use the chosen
prefix; companions and autostart are per-user. On Linux, absolute `XDG_CONFIG_HOME`
and `XDG_DATA_HOME` override `~/.config` and `~/.local/share`; empty or relative
values fall back to those home directories, matching config discovery.

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

`--init-config` runs the installed `window_zones init`. It writes a starter config
to `$XDG_CONFIG_HOME/window_zones/config.toml` when that variable is absolute,
otherwise `$HOME/.config/window_zones/config.toml` on Linux, and to
`~/Library/Application Support/window_zones/config.toml` on macOS.
It never overwrites an existing config: an existing file is reported and returns
success. An initialization error is printed as a warning; resolve it before
starting the listener. You can run the same step separately:

```bash
"$HOME/.local/bin/window_zones" init
```

Install the appropriate companion (Linux) and start the listener automatically:

```bash
# GNOME Wayland
./scripts/install.sh --gnome-extension --init-config --autostart

# KDE Plasma Wayland
./scripts/install.sh --kwin-script --init-config --autostart

# macOS
./scripts/install.sh --init-config --autostart
```

On KDE, the installer also starts and supervises `window_zones_kwin` when
`--autostart` is selected. Enable the KWin script once in System Settings.

### Installer options

| Option | Effect |
| --- | --- |
| `-p, --prefix PATH` | Binary installation prefix; default `$HOME/.local` or `WINDOW_ZONES_INSTALL_PREFIX`. |
| `-b, --binary NAME` | Select a Cargo binary; default `window_zones`. |
| `--release` | Build/install the release profile (default). |
| `--debug` | Build/install the debug profile. |
| `--gnome-extension` | Linux only: install the GNOME Shell companion and attempt to enable it. Refused on macOS. |
| `--kwin-script` | Linux only: install/upgrade the KWin script and install `window_zones_kwin` alongside the selected binary. Refused on macOS. |
| `--init-config` | Run the installed App's non-overwriting `init` command. Requires `--binary window_zones`. |
| `--autostart` | Linux: install the App's systemd user unit, or XDG login entry when user systemd is unreachable. An installed KWin companion is started and supervised too. macOS: install and load a per-user LaunchAgent. Requires `--binary window_zones`. |
| `--uninstall` | Remove installations recorded for the chosen prefix; preserve the user config. No build is performed. |
| `-h, --help` | Show usage. |

For example, use a custom prefix or a debug build:

```bash
./scripts/install.sh --prefix "$HOME/apps/window-zones" --binary window_zones --init-config
./scripts/install.sh --debug
```

The script resolves the repository relative to its location, builds the selected
binaries with `cargo build --locked`, and replaces `<prefix>/bin/<binary>`.
`CARGO_TARGET_DIR` and Cargo's configured target directory are honored. Prefixes are canonicalized before ownership records and autostart paths are derived. Re-running the same options replaces the installed
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
[GNOME lifecycle runbook](gnome-wayland.md). The installer records the owning
prefix in the companion's `.window-zones-install-prefix` marker. Uninstall
disables the extension, removes its enabled-list entry and deletes its files
only while that marker matches the selected prefix. Missing or different
ownership leaves the companion and activation unchanged; reinstall with
`--gnome-extension` from the intended prefix before removing it.

Upgrade the binary and companion together: the combined contract uses
`org.window_zones.Gnome2`, with identified-window moves and optional
`transition-metrics`; a previously loaded `Gnome1` companion is incompatible.

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

The KWin script and Rust companion use protocol major 3 for identified-window
moves and live display/shortcut updates; older files are incompatible.

Enable **Window Zones** in **System Settings -> Window Management -> KWin
Scripts**. After upgrading, disable/re-enable the script to reload its code and
release old shortcut registrations.

With `--kwin-script --autostart`, the installer manages both the Rust companion
and App on the graphical user's session bus. Running `--autostart` later also
recognizes the companion recorded for the same prefix. With user systemd it
installs `window-zones-kwin.service` and orders the App after the companion
acquires `org.window_zones.KWin`. The App stays running while the companion
recovers, reconnecting when it becomes available again.

Without reachable user systemd, `window_zones_kwin.desktop` starts an installed
KDE session launcher at login. It starts the companion, waits for its bus name,
then starts the App and supervises both. This fallback needs `busctl` or
`dbus-send`. Reinstall restarts an owned active service or launcher onto the
new binaries; the KWin script itself still needs the reload above.

For an install **without `--autostart`**, start the Rust companion manually
before the App and keep it running:

```bash
"$HOME/.local/bin/window_zones_kwin"
```

See [KDE Wayland](kde-wayland.md) for startup and runtime checks.

## Autostart, supervision, and logs

### Linux: systemd user unit

On Linux, `--autostart` writes
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
installer without `--autostart` also restarts active services only when their
installed executable paths still belong to the selected prefix.

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

For KDE, use the same commands with `window-zones-kwin` to manage the companion
and `journalctl --user -u window-zones-kwin -f` for its logs. Both units stop at
graphical logout. Stop the App first when deliberately shutting down both.

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

If user systemd is unreachable, non-KDE installs use
`${XDG_CONFIG_HOME:-$HOME/.config}/autostart/window_zones.desktop`. That entry
runs the installed `window_zones run` at login without supervision or guaranteed
journal capture. KDE installs instead use `window_zones_kwin.desktop` and the
owned `<prefix>/share/window-zones/kde-session` launcher described above.
The launcher retries failures after two seconds, but does not retry an App
that exits normally or with **75**. It records its process start identity so
reinstall/uninstall signal only that prefix's owned launcher. Stopping the
launcher also stops its supervised App and companion before removing its owner
record.

The systemd units are still installed. To switch from XDG to systemd later,
re-run the installer with `--autostart` once user systemd is reachable; it stops
the owned KDE launcher, removes the owned desktop fallback and starts the units.

### macOS LaunchAgent

On macOS, `--autostart` writes
`~/Library/LaunchAgents/org.window_zones.run.plist`, which runs
`"<prefix>/bin/window_zones" run` (absolute path, XML-escaped) with `RunAtLoad`
and sends stdout/stderr to `~/Library/Logs/window_zones.log`. It then runs
`launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/org.window_zones.run.plist`
for a new registration. A loaded agent already pointing to the same executable
is restarted with `launchctl kickstart -k`, avoiding an immediate unload/reload.
Changing the installation prefix unloads the earlier registration and bootstraps
the replacement. launchd loads the agent at every GUI login and stops it at
logout. If `bootstrap` fails (for example over SSH without a GUI login), the
installer warns; the agent still loads at the next login. Re-running the
installer without `--autostart` restarts a running agent only when its plist
still names this prefix's executable.

The agent has no `KeepAlive`: launchd cannot exempt exit status **75** (another
instance owns the hotkeys) from restarts, so the listener starts once per login
or load and is never respawned. Restart it yourself after a crash:

```bash
launchctl print gui/$(id -u)/org.window_zones.run      # state, pid, last exit status
launchctl kickstart -k gui/$(id -u)/org.window_zones.run   # start or restart
launchctl bootout gui/$(id -u)/org.window_zones.run    # stop until the next login
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/org.window_zones.run.plist   # load again
tail -f ~/Library/Logs/window_zones.log
```

The log is appended to and never rotated; truncate it with
`: > ~/Library/Logs/window_zones.log` if it grows. To stop autostart permanently,
uninstall (below) or remove the plist after `bootout`.

Window moves use native Accessibility, not `osascript` or System Events. Under
launchd the installed `<prefix>/bin/window_zones` binary needs the grant:

- **Accessibility**: System Settings > Privacy & Security > Accessibility, click
  **+**, press Cmd+Shift+G, enter the binary path (for example
  `~/.local/bin/window_zones`) and enable it. Global input capture must also be
  permitted; follow any Input Monitoring prompt for the responsible process.

If macOS no longer recognizes the grant after a rebuild or reinstall, re-add the
installed binary, then
`launchctl kickstart -k gui/$(id -u)/org.window_zones.run`. A terminal run may use
the terminal app's permission instead. A listener that failed to register needs
runtime `restart` or a process restart after permission is restored;
registration retries alone do not reconstruct it.

Background problem/recovery notices use `osascript` Standard Additions, with
text passed as arguments rather than interpolated code. Check the sender shown
in Notification Center and its preferences when notices are not visible.
Follow the [native macOS smoke procedure](testing.md#macos-native-smoke-ungated).

## Verify and uninstall

```bash
"$HOME/.local/bin/window_zones" --help
"$HOME/.local/bin/window_zones" status
```

Substitute your prefix if it differs. Follow the compositor-specific runbook for
companion and hotkey verification. Report observed results, limitations, and
retained evidence paths in the PR description. Verifier cleanup preserves its
evidence; do not check verification artifacts into the repository.

Uninstall using the same prefix used for installation:

```bash
./scripts/install.sh --uninstall
./scripts/install.sh --prefix "$HOME/apps/window-zones" --uninstall
```

The installer records its installed binaries and optional features in
`<prefix>/share/window-zones/install-record`. On Linux, uninstall stops/disables
the App and KWin companion services only when their units still reference this
prefix, then removes their owned units and desktop entries. It stops the owned
KDE fallback launcher before removing its binaries. GNOME extension cleanup and
KWin script cleanup each require their package's ownership marker to match this
prefix; an old prefix cannot disable or delete a replacement companion. KWin
activation cleanup also requires the Plasma tools. On macOS, it boots out
the loaded LaunchAgent and removes its plist only if the plist's executable
still belongs to the selected prefix; an old prefix cannot uninstall another
prefix's replacement agent. The log file is kept. It then removes recorded
binaries (including the KWin companion). No other extension, KWin package,
launch agent, or binary is removed. **The config is kept.**
Repeated uninstall is safe. Older installations without an ownership record must
first be upgraded with this installer and the same feature flags, or be removed
manually.

If session tools are unreachable, stop any manually running App or KWin
companion and log out/in to unload a removed compositor companion. The installer
prints warnings for session operations it cannot complete.
