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

This matrix is the record of record for V1 Release gate status. After the expanded ADR 0009 checks, Linux X11 and its TUI lifecycle are `blocking - pass`; the current GNOME 50 seated gate and its TUI lifecycle are `pending`. KDE Plasma Wayland and Windows remain `deferred - unverified`. Release gate status is exactly one of `pending`, `blocking - pass`, `blocking - fail`, or `deferred - unverified`. macOS, Sway, and Hyprland are ungated integrations; macOS smoke evidence is recorded separately below. Every Linux Smoke run ends with `Smoke assertions (<gate>): N; passed: P; failed: F; skipped: S`.

Run the GNOME Wayland or Linux X11 Smoke run with:
```bash
./scripts/smoke.sh gnome       # headless Synthetic GNOME session: Companion load, Capability negotiation, display enumeration, usable-area math
./scripts/smoke.sh gnome-live  # seated live GNOME session: focused window, placement, real accelerator capture, Companion disconnect/recovery, TUI lifecycle
./scripts/smoke.sh kde-live    # nested KWin 6.x Synthetic session: the same checks against a real KWin, never seated evidence
./scripts/smoke.sh x11
```
The TUI lifecycle Smoke runs cover launch, reload, dispatch, restart, and quit in each corresponding Synthetic or seated session.

The X11 adapter owns exclusive `XGrabKey` accelerators (including CapsLock/NumLock variants); `xdotool` XTEST injection reaches these grabs without delivering the accelerator to the focused application. It no longer requires XRecord. The expanded X11 gate was re-recorded successfully: 87/87 assertions. Its owned Openbox fixture uses an empty keyboard map so distro defaults do not preempt the test chords; a separate competing client still proves real grab refusal.

Pure X11 checks run with `cargo test --lib x11`. The ignored `x11_real_server_grabs_are_exclusive_atomic_and_released` test requires `WZ_X11_TEST_DISPLAY` and, when authentication is enabled, `XAUTHORITY` for an isolated X server, never the user's desktop. Use Xvfb or a separate rootful `Xwayland -geometry 800x600 -shm` on an isolated headless compositor, without `-enable-ei-portal`; GNOME's rootless XWayland did not deliver XTEST keys to X11 clients in the headless configuration tested here. Run it with `cargo test --lib x11_real_server_grabs_are_exclusive_atomic_and_released -- --ignored`.

`gnome-live` has two prerequisites on the seated session:

- The Companion must be active: `window-zones@mihai-a24` in `org.gnome.shell enabled-extensions` **and** `org.gnome.shell disable-user-extensions` set to `false`. The master switch suppresses the extension even when it is listed, which reads exactly like an extension that refuses to load.
- `/dev/uinput` must be writable by the user running the gate, because real accelerator capture is evidenced with a kernel virtual keyboard. Xwayland XTEST injection is not delivered to compositor accelerator grabs on GNOME 50, so `xdotool` cannot carry this check; with `/dev/uinput` unavailable the gate fails immediately and prints the remediation. Grant it for the run with `sudo modprobe uinput && sudo chmod 0666 /dev/uinput`, or persistently with a udev rule such as `KERNEL=="uinput", MODE="0660", GROUP="input"` plus membership in `input`.

`kde-live` needs the `kwin` package (`kwin_wayland`, `kpackagetool6`, `kwriteconfig6`), `dbus-update-activation-environment`, and working Xwayland XTEST-to-EIS delivery. It builds its own private bus, XDG tree, KWin configuration, and compositor. KWin runs on its `--virtual` framebuffer backend with two 1200x900 outputs; a nested windowed backend was rejected because the outer compositor resizes those output windows mid-run. This is Synthetic session evidence: KWin uses `Session::Type::Noop` and never owns the login seat, so even a passing run cannot close the seated KDE Release gate.

ADR 0009 changed every integration's move path (identified-window moves, restore-before-move, `org.window_zones.Gnome2`, KWin protocol 2, X11 `XGrabKey` hotkeys). The historical matrix below predates the expanded checks and does not close current gates. On 2026-10-06 the Synthetic `gnome` gate ran against the ADR 0009 tree on CachyOS, kernel 7.2.9-1-cachyos, GNOME Shell 50.5, Text Editor 50.1: 23 assertions, 17 passed, 0 failed, 6 skipped. That run omitted seat-dependent focus, placement, accelerator, and recovery checks.

The `x11`, `gnome-live`, and `kde-live` gates now assert the ADR 0009 window contract on disposable windows they start themselves, observed with `xdotool`/`xprop` (and, on KDE, a throwaway KWin script reading `tile`, `maximizeMode`, `fullScreen`, and `frameGeometry`). X11 and synthetic KDE have exercised these additions; the expanded seated GNOME 50 checks remain pending.

- A captured window id still moves the original window after focus moved to a second test window; the focused window's frame and focus do not change.
- Moving the captured id of a closed window reports WindowGone, and the other window does not move.
- A maximized window and a tiled window are restored before the target frame is applied (X11: Openbox vertical maximization stands in for tiling; GNOME: real Super+Left edge tiling over `/dev/uinput`; KDE: `slotWindowQuickTileLeft`).
- A fullscreen window is rejected with `leave fullscreen first`, and its fullscreen state and frame do not change.
- A hotkey replacement the platform refuses (X11: another client already grabs `Alt+Ctrl+F8`; GNOME: `Alt+Ctrl+Left` is reserved; KDE: the conflict incumbent holds `Alt+Ctrl+F24`) is reported as `were refused`, keeps the previous binding count and `hotkey state: Registered`, and the previous accelerator still moves the window.

The X11 and GNOME checks build a small helper crate in `target/smoke-seam-helper` against the public `window_zones` API (`x11rb` from the repository lockfile), so the first run needs the crates available to `cargo`, from the network or a vendored source. It captures and moves windows through `X11WindowSystem`, sets EWMH states, and holds the competing X11 grab. The X11 gate also runs the ignored `x11_real_server_grabs_are_exclusive_atomic_and_released` test against its own Xvfb display. `kde-live` needs KWin 6 scripting with `workspace.slotWindowQuickTileLeft()` and the window `tile` property.

### Current readiness evidence (2026-10-10)

| Configuration | Gate status | Observed outcome | Evidence |
|---|---|---|---|
| Linux X11, isolated Xvfb + Openbox in a Debian 13 aarch64 `rust:1.95-trixie` container | blocking - pass | 87 assertions: 87 passed, 0 failed, 0 skipped; includes ADR 0009, real exclusive grabs/input, refused reload retaining the old grab, and TUI lifecycle | `target/verify-window-zones/readiness-linux-x11/` |
| GNOME 50 seated session | pending | No reachable seat. The configured CachyOS SSH target timed out on port 22. Container GNOME Shell 48.7 could not activate `org.freedesktop.login1`: synthetic run 6 assertions, 2 passed, 4 failed. Live preflight: 2 assertions, 1 passed, 1 failed because `/dev/uinput` is not writable | `readiness-linux-gnome-system-bus/`, `readiness-linux-gnome-live/` under the same evidence root |
| KDE Plasma Wayland seated session | deferred - unverified | No seated run. Latest synthetic KWin: 180 assertions, 162 passed, 15 input-dependent failures, 3 skips; private PipeWire did not fix delivery. See [the KDE runbook](kde-wayland.md#nested-kwin-gate) | `readiness-linux-kde-activation/`, `readiness-linux-kde-pipewire/` under the same evidence root |
| Windows | deferred - unverified | No new Windows verification in this readiness work | Historical record below |

Local regressions: Linux `./scripts/test.sh` passed 146 library tests (1 ignored),
15 main tests, and 170 GNOME companion checks. macOS passed 91 library tests
(1 GUI regression ignored by default) and 10 main tests; the owned live
`expansion_finishes_at_target_origin` regression passed separately.
Formatting, clippy, and shell/compositor syntax checks passed. The
maintained dry-run scenario `readiness-20261010-final` passed all 21 steps, with
zero live-backend program calls and instance locks. Program sentinels do not
observe native Framework calls; dry-run evidence never closes a live gate.

### macOS native smoke (ungated)

The native adapter and runtime were exercised on disposable AppKit windows and
isolated installation homes. Evidence: `target/verify-window-zones/readiness-macos/`.

- Captured identity survived rename/refocus; moves still targeted the captured
  window. Closed IDs, same-title replacements, and owning-app exit returned
  `WindowGone` without moving another window.
- Lower-screen expansion reached exactly `[0,30,1720,1410]`; shrinking/moving
  across displays, returning to primary, native zoom restoration, and settled
  native `Window > Move & Resize > Left` restoration were observed. The latter
  retained the exact target over 12 samples spanning 1.2 seconds.
- Fullscreen and non-resizable windows were rejected before mutation.
- The second display constrained a requested `[3440,355,864,1085]` to
  `[3440,356,864,1084]`. Independent AppKit and delayed Accessibility reads agree;
  the adapter returns `window constrained placement`, not success or a tolerance.
- A first `CGEventTapCreate` failure was injected; runtime `restart` created one
  replacement hook. A CGEvent-injected configured F8 reached the real hook and
  dispatched the exact full-height left-half move. Exactly two tap creations
  were recorded in `hotkey-restart-bound-key-final.log`.
- `/bin/bash` 3.2 installed, reinstalled, upgraded prefixes, and uninstalled an
  actual isolated LaunchAgent. Same-program reinstall changed its PID through
  `kickstart -k`. A stale-prefix uninstall preserved the replacement program
  and PID; final uninstall removed the owned agent/binaries/plist and retained
  config/logs.
- Actual config-error and matching recovery notices appeared in Notification
  Center and as captured on-screen banners, attributed to Script Editor. Quoted
  config text containing `&` and `$name` arrived unchanged. No notification
  preferences were changed. See `notification-surface-final.json`,
  `notification-onscreen.json`, `notification-problem-banner.png`, and
  `notification-recovery.png`.

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

### Historical matrix (pre-expanded ADR 0009 checks)

These rows retain their original outcomes and dates; they are not current gate
closure evidence.

| Gate | Status | Environment | Verified configuration | Date | How to reproduce |
|---|---|---|---|---|---|
| GNOME Wayland | blocking - pass | Synthetic session: `gnome-shell --headless` with virtual monitors 1920x1080 and 1600x900, isolated dconf/data/runtime dirs on a private session bus. Seated session: the live GNOME session with two physical monitors (HDMI-1 2560x1440 at 0,0 and DP-1 3440x1440 at 2560,0), `gnome-text-editor` on XWayland as the test window, `xdotool` as the independent observer, and a `/dev/uinput` virtual keyboard as the input source | CachyOS, kernel 7.2.5-1-cachyos; GNOME Shell 50.4; mutter 50.4. Synthetic (2026-09-13, commit `3eff23c`, run over SSH): 23 assertions, 0 failures, run twice — Companion bus ownership, `GetCapabilities`, both virtual displays with the 32px panel excluded (`display-1|0|32|1920|1048`, `display-2|1920|0|1600|900`), config reload/retention/recovery; six seat-dependent checks recorded as skipped. Seated (2026-09-11, pre-ADR 0008): 72 assertions, 0 failures, run twice — focused-window discovery agreeing with `xdotool` to the pixel, left-half `0,0,1280,1440`, center-third `853,0,854,1440`, left-two-thirds `0,0,1707,1440`, cross-display move to `2560,32,2294,1408` on `display-2` and back, real accelerator capture through the kernel virtual keyboard, atomic registration with a rejected set preserving the previous one, Companion disconnect and recovery without an App restart, and invalid-config reload retaining the last valid bindings | 2026-09-13 (synthetic), 2026-09-11 (seated) | `./scripts/smoke.sh gnome` and `./scripts/smoke.sh gnome-live` |
| Linux X11 | blocking - pass | Xvfb 3520x1080 split by `xrandr --setmonitor` into logical monitors 1920x1080 and 1600x1080, Openbox as EWMH window manager, `gnome-text-editor` as the test window, `xdotool` + `xprop _NET_FRAME_EXTENTS` as the frame observer | CachyOS, kernel 7.2.5-1-cachyos; Xvfb from xorg-server 21.1.24; Openbox 3.6.1; xdotool 4.20260303.1; commit `3eff23c`. 56 assertions, 0 failures, run twice — frame-geometry zones on monitor 1, next/previous display with wrap, dispatch from a decorated frame at `1930,10` on monitor 2 landing on `1920,0,800,1080`, real `rdev` accelerator capture, invalid-config retention and recovery, TUI lifecycle. Also 56/56 on 2026-09-12 in a Debian 12 container (`rust:1-bookworm`, xorg-server 21.1.7, xdotool 3.20160805.1) | 2026-09-13 | `./scripts/smoke.sh x11` |
| TUI lifecycle (GNOME) | blocking - pass | Same Synthetic session and same seated live GNOME session | TUI launch, `reload`, `restart`, `status`, `dispatch HOTKEY`, and `quit` asserted in both halves; the seated run also asserts the dashboard reports the dispatched binding | 2026-09-13 (synthetic), 2026-09-11 (seated) | `./scripts/smoke.sh gnome` and `./scripts/smoke.sh gnome-live` |
| TUI lifecycle (X11) | blocking - pass | Same Xvfb + Openbox Synthetic session | Same as Linux X11; `reload`, `restart`, `status`, `dispatch HOTKEY`, `quit` all asserted | 2026-09-13 | `./scripts/smoke.sh x11` |
| KDE Plasma Wayland | deferred - unverified | Synthetic session: KWin 6.7.5 on its `--virtual` backend with two 1200x900 outputs and its own Xwayland, the repository KWin script installed with `kpackagetool6` into a private XDG tree, the Rust companion started before KWin on a private bus, `gnome-text-editor` on Xwayland as the test window, an independent KWin observer script reading per-output `MaximizeArea`, and XTEST-to-EIS key injection. Seated KDE session: not run | Nested configuration only: CachyOS, kernel 7.2.5-1-cachyos; KWin 6.7.5; Xwayland 24.1.13; Qt 6.11.2; commit `3eff23c`. 142 assertions, 0 failures, run twice — protocol handshake, capability set, `--backend auto` resolving `kde-wayland`, per-output display enumeration cross-checked against the observer, focused window, three zones, next/previous cross-display moves with wrap (`1200,0,800,900` on output 2), real accelerator capture, KGlobalAccel `action(int)` preflight rejecting `alt+ctrl+f25` while `alt+ctrl+left` registers, conflict refusal, script and service disconnect/recovery with an unchanged App PID, config retention, and the TUI lifecycle. Three limitations are recorded as named skips: seated accelerator capture, Plasma panel exclusion, negative-coordinate output movement. The 2026-09-12 record (138 assertions) predates the preflight and ran against a script QJSEngine could not parse; `scripts/test.sh` now runs `qmllint` on the script wherever Qt is installed. Seated KDE remains unverified | 2026-09-13 | `./scripts/smoke.sh --keep-logs kde-live`; the Release gate still closes only on a seated KWin 6.x Smoke run |
| Windows | deferred - unverified | Windows desktop session | `windows` CI job compiles and unit-tests `src/windows_window_system.rs` against `windows` 0.58; the Release gate closes with the manual run in `docs/runbooks/windows-smoke.md`. | 2026-09-11 | `windows` job in `.github/workflows/ci.yml`; run `docs/runbooks/windows-smoke.md` to close the Release gate. |

### Minimum verification

If you only need fast feedback:

```bash
cargo test --locked
```
