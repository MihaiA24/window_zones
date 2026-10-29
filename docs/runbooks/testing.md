# Testing Runbook

## Local gate

```bash
./scripts/test.sh
```

What this verifies:

- `cargo fmt --all -- --check` (formatting)
- `cargo test --locked` (unit + integration + doc tests)
- `cargo clippy --locked --all-targets --all-features` (only if clippy is installed)
- `bash -n` for every `scripts/**/*.sh` file, including sourced smoke modules and shell fixtures
- `node --check gnome-extension/extension.js`
- `gjs -m gnome-extension/tests/run.js` (GNOME Companion contract harness, only if `gjs` is installed)
- Linux only: `bash scripts/test-install.sh` (isolated installer/service ownership and lifecycle, plus literal-prefix PATH suggestions for installed bash/zsh/fish). Other hosts explicitly skip this `/proc`-dependent fixture and continue the remaining checks; native macOS installation remains a separate verification path.
- `node kwin-script/tests/run.js` and `qmllint kwin-script/contents/code/main.js` when available (native Qt syntax)
- module-aware Node syntax checks for JavaScript fixtures under `scripts/` and `src/`
- Python parsing of every `scripts/**/*.py` file, including extracted smoke fixtures
- Linux only: `python3 .agents/skills/verify-window-zones/test_verify.py` (verifier ownership, completed-child observation, native-runtime cleanup, dry-run, freshness after nested Rust/shell/fixture additions, edits and removals, evidence retention, TUI dispatch errors, JSON mismatches, median/spread, and application-only clock scope). Other platforms explicitly skip this Linux harness and continue the Cargo checks.
- Linux only: `python3 scripts/smoke/test_helpers.py` (owned-window selection, PID-reuse/orphan cleanup, signal exit, non-vacuous TUI responses and persistent helper build storage).

The GNOME and KDE adapter contract tests use fake compositor services on private
`dbus-daemon` instances; KWin controller ownership is also checked through the real
Rust dispatcher. Neither requires a running compositor session.

Verifier `drive tests` uses persistent `target/verification-tests` Cargo output.
It must not overwrite the App binaries pinned by `launch`; test-only dependency
features can produce different binaries even when the source is unchanged.

## Agent-driven verification

For checkout-owned launches, drives, fixed-oracle checks, performance measurement and evidence,
follow the [verification skill](../../.agents/skills/verify-window-zones/SKILL.md) and [feature map](../../.agents/skills/verify-window-zones/features/README.md).

`scripts/smoke.sh` is the public dispatcher. Its sourced modules under
`scripts/smoke/` share the entry point's paths, process ownership and assertion
state; do not execute them separately. Literal generated helpers live in
`scripts/smoke/fixtures/`. The native gate builds the Rust seam helper against the
public crate API using the repository lockfile.

For direct smoke-runner integrations, `WINDOW_ZONES_BINARY` pins the app binary,
`WINDOW_ZONES_SMOKE_ROOT` selects a new absolute scratch directory, and
`WINDOW_ZONES_SMOKE_DISPLAY` selects a free X11 display number. An occupied
display is rejected rather than attached to; retain proof with `--keep-logs`.

X11 and KDE discover visible editor windows through the shared PID-owned lookup;
a matching title alone is not enough. Fixture copies retain their original paths
and permissions without separate copy-only wrappers.

X11 one-shot movement waits for the server acknowledgement and the WM's observed
frame before its connection closes. The smoke runner dispatches each placement once,
without retrying a reported success, then independently checks geometry. Its TUI
check first establishes a different geometry and requires the dispatched move and
completed response to succeed; a requested action label alone is not proof.

## V1 verification matrix

This runbook defines V1 Release gates and how to reproduce them. Report results, exact versions and display layouts, dates, limitations, and retained evidence paths in the PR description. Verifier cleanup preserves logs and captures under ignored `target/verification/`; do not check them into the repository. Synthetic sessions do not verify physical hardware or the user's installed companion.

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

Native smoke helpers require Python 3.9+ and Linux pidfd support for safe process
signaling. The GNOME gates require `gnome-shell`, `dbus-daemon`,
`gsettings`, Xwayland, `/usr/bin/python3` with PyGObject/GTK 4, and
`gst-launch-1.0`/`gst-inspect-1.0` with PipeWire, video-conversion and PNG plugins,
plus `pw-dump` and `pw-link` from the PipeWire tools package.
A running user PipeWire daemon/session manager is required. The isolated gate
also needs an installed AppIndicator compatible with the tested Shell version.

Before isolation the verifier records the original user/system XDG data roots
and PipeWire endpoint. The driver copies AppIndicator privately; it does not
restore host HOME/config/data/runtime, desktop identity or the session bus.
Compatible user-installed and system-installed extensions are both supported.
On CachyOS/Arch the GNOME tool dependencies are:

```bash
sudo pacman -S --needed python-gobject gtk4 gstreamer gst-plugins-base gst-plugins-good gst-plugin-pipewire pipewire wireplumber xorg-xwayland dbus util-linux
```

The X11 gate additionally needs Xvfb, Openbox, xdotool, xrandr, xdpyinfo,
xprop, xkbcomp, GNOME Text Editor, ffmpeg and a private D-Bus session:

```bash
sudo pacman -S --needed xorg-server-xvfb openbox xdotool xorg-xrandr xorg-xdpyinfo xorg-xprop xorg-xkbcomp gnome-text-editor ffmpeg dbus
```

Mutter's RemoteDesktop API supplies compositor-native keyboard and mouse
input; its linked ScreenCast stream supplies actual rendered screenshots.
The driver explicitly links the exact owned Mutter output to its uniquely
named GStreamer consumer after checking both owning PIDs and ports. The
PipeWire graph and consumer logs are retained; no host session-manager policy
is changed and no generic or foreign stream is adopted.
This gate does not require `/dev/uinput` or alter seated extension settings.
The owned Shell uses `--force-animations` so software rendering does not silently
bypass the enabled-animation path. Disabled animations are checked on both
supported Shell versions; reduced motion is additionally required on GNOME 51.
GNOME 50 predates that API and skips only that preference-specific case.
Overview uses Shell's public `OverviewActive` property with
an observed-state wait, followed by captured-window identity and frame checks.
The virtual pointer is created before leaving startup Overview because creating
that device can reopen it.

The X11 adapter owns exclusive `XGrabKey` accelerators (including CapsLock/NumLock variants); `xdotool` XTEST injection reaches these grabs without delivering the accelerator to the focused application. It does not require XRecord. The owned Openbox fixture uses an empty keyboard map so distro defaults do not preempt the test chords; a separate competing client proves real grab refusal.

Pure X11 checks run with `cargo test --lib x11`. The ignored `x11_real_server_grabs_are_exclusive_atomic_and_released` test requires `WZ_X11_TEST_DISPLAY` and, when authentication is enabled, `XAUTHORITY` for an isolated X server, never the user's desktop. Use Xvfb or a separate rootful `Xwayland -geometry 800x600 -shm` on an isolated headless compositor, without `-enable-ei-portal`. Run it with `cargo test --lib x11_real_server_grabs_are_exclusive_atomic_and_released -- --ignored`.
The gate runs both `x11_real_server*` tests serially on its own display,
including remapping and a nonstandard NumLock slot. It also temporarily
constrains its owned client's minimum size, requires a partial-frame error,
and observes exact Restore before restoring the original hints and baseline.

`gnome-live` has two prerequisites on the seated session:

- The Companion must match the checkout and be active: `window-zones@mihai-a24` in `org.gnome.shell enabled-extensions` **and** `org.gnome.shell disable-user-extensions` set to `false`. After replacing source/metadata, a fresh login is required to load it; matching disk files plus an ACTIVE cached extension are not upgrade proof. The master switch suppresses the extension even when it is listed, which reads exactly like an extension that refuses to load.
- `/dev/uinput` must be writable by the user running the automated gate. `gnome-live` fails immediately without authorized virtual-keyboard access and has no XTEST or physical-key fallback. Never grant world-writable access. An administrator can authorize a temporary per-user ACL using the saved/restored procedure in [the GNOME runbook](gnome-wayland.md#manual-smoke-checklist). Physical presses can provide separate manual evidence, not a pass of the automated gate.

If a companion-dependent seated phase is blocked, it emits one phase-level
`SKIP` with the prerequisite reason rather than duplicating every unreachable
check label. Failed prerequisite assertions remain failures; the skip is not
evidence that any dependent check ran.

The seated observer records independent Mutter monitor layout/modes and Xwayland
desktop dimensions before comparing frames in logical units. It never derives
scale from the target window and fails unsupported nonuniform mappings instead
of rounding away discrepancies. Captures use an owned Mutter ScreenCast stream
on the fixture's monitor; Xwayland-root `x11grab` can be black on Wayland and is
not accepted as compositor proof. Each accelerator also needs fresh successful
App status, with post-input focus and Overview state retained for diagnosis.

`kde-live` needs the `kwin` package (`kwin_wayland`, `kpackagetool6`, `kwriteconfig6`), `dbus-update-activation-environment`, and working Xwayland XTEST-to-EIS delivery. It builds its own private bus, XDG tree, KWin configuration, and compositor. KWin runs on its `--virtual` framebuffer backend with two 1200x900 outputs; a nested windowed backend was rejected because the outer compositor resizes those output windows mid-run. This is Synthetic session evidence: KWin uses `Session::Type::Noop` and never owns the login seat, so even a passing run cannot close the seated KDE Release gate.

The `x11`, `gnome-live`, and `kde-live` gates assert the ADR 0009 window contract on disposable windows they start themselves, observed with `xdotool`/`xprop` (and, on KDE, a throwaway KWin script reading `tile`, `maximizeMode`, `fullScreen`, and `frameGeometry`). Verification must also cover opposite-half crossing, restoration ordering, animation completion, and `--perf-log`.

- A captured window id still moves the original window after focus moved to a second test window; the focused window's frame and focus do not change.
- Moving the captured id of a closed window reports WindowGone, and the other window does not move.
- A maximized window and a tiled window are restored before the target frame is applied (X11: Openbox vertical maximization stands in for tiling; GNOME: real Super+Left edge tiling over `/dev/uinput`; KDE: `slotWindowQuickTileLeft`).
- A fullscreen window is rejected with `leave fullscreen first`, and its fullscreen state and frame do not change.
- A hotkey replacement the platform refuses (X11: another client already grabs `Alt+Ctrl+F8`; GNOME: `Alt+Ctrl+Left` is reserved; KDE: the conflict incumbent holds `Alt+Ctrl+F24`) is reported as `were refused`, keeps the previous binding count and `hotkey state: Registered`, and the previous accelerator still moves the window.

The X11 and seated `gnome-live` checks build a small helper crate against the public `window_zones` API (`x11rb` from the repository lockfile), so its crates must be available to Cargo. A pinned app binary is never rebuilt; the separate helper uses persistent Cargo target storage, not copied evidence. X11 runs its ignored real-server tests only on the owned display. `kde-live` needs KWin 6 scripting with `workspace.slotWindowQuickTileLeft()` and the window `tile` property.

The harness preserves the invoking user's `CARGO_HOME` and `RUSTUP_HOME` before
isolating the App's `HOME`; helper builds and the ignored X11 test therefore
use the existing toolchain rather than a nonexistent default in the test home.

### macOS native smoke (ungated)

To repeat the lower-screen expansion regression, start a dedicated disposable
application process with one normal resizable window on the primary display,
below the target top edge and shorter than the usable height. Keep it frontmost
and use its GUI PID, not the `open` launcher PID:

```bash
WZ_MACOS_TEST_PID="$FIXTURE_PID" WZ_MACOS_TEST_AREA="$FIXTURE_USABLE_AREA_JSON" cargo test --locked --lib \
  expansion_finishes_at_target_origin -- --ignored --test-threads=1
```

`FIXTURE_USABLE_AREA_JSON` is an independently observed primary usable area,
for example the JSON shape `{"x":0,"y":25,"width":1440,"height":875}` in global
top-left coordinates. Record the real values for the test desktop, not this
example. Libtest uses worker threads; AppKit display enumeration is main-thread
only, so this fixture input must come from an independent native observer.

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
In the actual macOS `run`/`tui` process, change monitor and Dock/menu-bar layout,
then dispatch again and independently compare the refreshed usable areas.
Require unchanged foreground focus and no utility Dock icon. Cross-compilation
and the background-thread rejection regression do not establish that proof.

### Minimum verification

If you only need fast feedback:

```bash
cargo test --locked
```
