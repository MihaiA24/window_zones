# Testing Runbook

## Local gate

```bash
./scripts/test.sh
```

What this verifies:

- `cargo fmt --all -- --check` (formatting)
- `cargo test --locked` (unit + integration + doc tests)
- `cargo clippy --locked --all-targets --all-features` (only if clippy is installed)
- `bash -n scripts/*.sh` (shell syntax, including the Smoke harness)
- `node --check gnome-extension/extension.js`
- `gjs -m gnome-extension/tests/run.js` (GNOME Companion contract harness, only if `gjs` is installed)
- `node --check kwin-script/contents/code/main.js`
- Python syntax for `scripts/gnome-smoke.py` (system Python, without generating cache files)
- `qmllint kwin-script/contents/code/main.js` (QJSEngine syntax, which `node` does not enforce; only if `qmllint` is installed)

The repository test script covers formatting, unit/integration/doc tests in one
Cargo test invocation, and clippy when installed. The GNOME and KDE adapter
contract tests use fake compositor services on private `dbus-daemon` instances;
they do not require a running compositor session.

The compositor scripts can be syntax-checked with:

```bash
node --check gnome-extension/extension.js
node --check kwin-script/contents/code/main.js
```

## V1 verification matrix

This runbook defines V1 Release gates and how to reproduce them. Record results, exact versions and display layouts, dates, and limitations only in the PR description. Verification logs and captures are temporary artifacts, not repository records. Synthetic sessions do not verify physical hardware or the user's installed companion.

| Gate | Release policy | Required verification |
|---|---|---|
| GNOME Wayland and TUI lifecycle | blocking | Synthetic session and expanded seated run with the upgraded `Gnome2` companion |
| Linux X11 and TUI lifecycle | blocking | Xvfb/Openbox run, including identified moves, exclusive hotkey rollback and TUI lifecycle |
| KDE Plasma Wayland | deferred, non-blocking | Seated KWin 6.x run with the protocol-3 script and companion; a nested pass cannot close this gate |
| Windows | deferred, non-blocking | Manual Windows run |

Release gate status is exactly one of `pending`, `blocking - pass`, `blocking - fail`, or `deferred - unverified`. macOS, Sway, and Hyprland are ungated integrations. Linux Smoke runs report `Smoke assertions (<gate>): N; passed: P; failed: F; skipped: S`.

Run the GNOME Wayland or Linux X11 Smoke run with:
```bash
./scripts/smoke.sh gnome       # isolated real GNOME: interactive GTK Wayland/XWayland windows, native input, all zones, monitors, CLI/TUI/tray, recovery, screenshots
./scripts/smoke.sh gnome-live  # seated live GNOME session: focused window, placement, real accelerator capture, Companion disconnect/recovery, TUI lifecycle
./scripts/smoke.sh kde-live    # nested KWin 6.x Synthetic session: the same checks against a real KWin, never seated evidence
./scripts/smoke.sh x11
```
The TUI lifecycle Smoke runs cover launch, reload, dispatch, restart, and quit in each corresponding Synthetic or seated session.

The isolated GNOME 51 gate uses `/usr/bin/python3` with PyGObject and GTK 4,
`gst-launch-1.0` with the PipeWire, video-conversion, and PNG plugins, and a
running user PipeWire daemon. It copies the installed GNOME-compatible
AppIndicator extension into its private XDG tree for real tray checks.
On CachyOS/Arch the smoke dependencies are:

```bash
sudo pacman -S --needed python-gobject gtk4 gstreamer gst-plugins-base gst-plugins-good gst-plugin-pipewire
```

Mutter's RemoteDesktop API supplies compositor-native keyboard and mouse
input; its linked ScreenCast stream supplies actual rendered screenshots.
This gate does not require `/dev/uinput` or alter seated extension settings.

The X11 adapter owns exclusive `XGrabKey` accelerators (including CapsLock/NumLock variants); `xdotool` XTEST injection reaches these grabs without delivering the accelerator to the focused application. It does not require XRecord. The owned Openbox fixture uses an empty keyboard map so distro defaults do not preempt the test chords; a separate competing client proves real grab refusal.

Pure X11 checks run with `cargo test --lib x11`. The ignored `x11_real_server_grabs_are_exclusive_atomic_and_released` test requires `WZ_X11_TEST_DISPLAY` and, when authentication is enabled, `XAUTHORITY` for an isolated X server, never the user's desktop. Use Xvfb or a separate rootful `Xwayland -geometry 800x600 -shm` on an isolated headless compositor, without `-enable-ei-portal`. Run it with `cargo test --lib x11_real_server_grabs_are_exclusive_atomic_and_released -- --ignored`.

`gnome-live` has two prerequisites on the seated session:

- The Companion must be active: `window-zones@mihai-a24` in `org.gnome.shell enabled-extensions` **and** `org.gnome.shell disable-user-extensions` set to `false`. The master switch suppresses the extension even when it is listed, which reads exactly like an extension that refuses to load.
- `/dev/uinput` must be writable by the user running the automated gate. `gnome-live` fails immediately without authorized virtual-keyboard access and has no XTEST or physical-key fallback. Never grant world-writable access. An administrator can authorize a temporary per-user ACL using the saved/restored procedure in [the GNOME runbook](gnome-wayland.md#manual-smoke-checklist). Physical presses can provide separate manual evidence, not a pass of the automated gate.

`kde-live` needs the `kwin` package (`kwin_wayland`, `kpackagetool6`, `kwriteconfig6`), `dbus-update-activation-environment`, and working Xwayland XTEST-to-EIS delivery. It builds its own private bus, XDG tree, KWin configuration, and compositor. KWin runs on its `--virtual` framebuffer backend with two 1200x900 outputs; a nested windowed backend was rejected because the outer compositor resizes those output windows mid-run. This is Synthetic session evidence: KWin uses `Session::Type::Noop` and never owns the login seat, so even a passing run cannot close the seated KDE Release gate.

The `x11`, `gnome-live`, and `kde-live` gates assert the ADR 0009 window contract on disposable windows they start themselves, observed with `xdotool`/`xprop` (and, on KDE, a throwaway KWin script reading `tile`, `maximizeMode`, `fullScreen`, and `frameGeometry`). Verification must also cover opposite-half crossing, restoration ordering, animation completion, and `--perf-log`.

- A captured window id still moves the original window after focus moved to a second test window; the focused window's frame and focus do not change.
- Moving the captured id of a closed window reports WindowGone, and the other window does not move.
- A maximized window and a tiled window are restored before the target frame is applied (X11: Openbox vertical maximization stands in for tiling; GNOME: real Super+Left edge tiling over `/dev/uinput`; KDE: `slotWindowQuickTileLeft`).
- A fullscreen window is rejected with `leave fullscreen first`, and its fullscreen state and frame do not change.
- A hotkey replacement the platform refuses (X11: another client already grabs `Alt+Ctrl+F8`; GNOME: `Alt+Ctrl+Left` is reserved; KDE: the conflict incumbent holds `Alt+Ctrl+F24`) is reported as `were refused`, keeps the previous binding count and `hotkey state: Registered`, and the previous accelerator still moves the window.

The X11 and GNOME checks build a small helper crate in `target/smoke-seam-helper` against the public `window_zones` API (`x11rb` from the repository lockfile), so the first run needs the crates available to `cargo`, from the network or a vendored source. It captures and moves windows through `X11WindowSystem`, sets EWMH states, and holds the competing X11 grab. The X11 gate also runs the ignored `x11_real_server_grabs_are_exclusive_atomic_and_released` test against its own Xvfb display. `kde-live` needs KWin 6 scripting with `workspace.slotWindowQuickTileLeft()` and the window `tile` property.

The harness preserves the invoking user's `CARGO_HOME` and `RUSTUP_HOME` before
isolating the App's `HOME`; helper builds and the ignored X11 test therefore
use the existing toolchain rather than a nonexistent default in the test home.

### macOS native smoke (ungated)

To repeat the lower-screen expansion regression, start a dedicated disposable
application process with one normal resizable window on the primary display,
below the target top edge and shorter than the usable height. Keep it frontmost
and use its GUI PID, not the `open` launcher PID:

```bash
WZ_MACOS_TEST_PID="$FIXTURE_PID" cargo test --locked --lib \
  expansion_finishes_at_target_origin -- --ignored --test-threads=1
```

The test checks the native window's owning PID before mutation and refuses a
different process. It is ignored by default because it requires the real GUI
and Accessibility permission. Capture an independent application frame after
the test. For zoom/tiling restoration, let the external native animation settle
before applying the move; observe the final frame after the move too, rather
than relying on an immediate readback during an ongoing animation. Close only
the fixture and remove its scratch files afterward.

To check retained Accessibility references, use a frontmost disposable fixture
and its owning GUI PID:

```bash
WZ_MACOS_TEST_PID="$FIXTURE_PID" cargo test --locked --lib \
  closed_fixture_releases_retained_window -- --ignored --test-threads=1
```

This regression deliberately terminates the fixture. Never supply a user's
application PID. It requires a real macOS GUI and Accessibility permission;
cross-compilation cannot verify this lifecycle.

The macOS hotkey adapter owns one keyboard event tap per process and reuses it
across `restart`; both Control keys activate `ctrl` bindings. Configured key
presses, their repeats, and their matching releases are consumed. Unconfigured
and unknown keys pass through. When macOS disables an unresponsive tap, the
adapter reconciles held keys against the session key state and re-enables the
same tap rather than leaving a disabled listener or starting a duplicate.
See Apple's [event-tap recovery API](https://developer.apple.com/documentation/coregraphics/cgevent/tapenable(tap:enable:)).

Grant the launching terminal or App Accessibility permission in System
Settings before running native hotkey checks. If creation or re-enabling fails,
the App reports hotkeys unavailable; restore permission and use `restart`.
With all keys released, the native recovery regression can be run with:

```bash
cargo test --locked --lib \
  native_disabled_tap_reenables_same_port_and_survives_registration_restart \
  -- --ignored --test-threads=1
```

This check creates and disables a real owned tap, delivers both native
tap-disabled notification types through its callback, checks that the same port
is enabled again, and exercises native key translation after registration
restart. It does not cause a real OS timeout or post keys to an application.
Also check actual left/right Control capture, repeat/release suppression in a
disposable focused application, and OS-induced timeout recovery before and
after a runtime `restart`; unit event delivery alone cannot verify those paths.

### Minimum verification

If you only need fast feedback:

```bash
cargo test --locked
```
