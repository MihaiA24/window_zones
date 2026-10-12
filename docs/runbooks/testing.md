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

This is the record of record for V1 Release gate status. Historical readiness work through `b436262` and local Linux/GNOME work through `8ce0fcc` were verified separately. The combined tree has now passed the isolated X11 and GNOME 51 runs recorded below. GNOME's seated gate remains pending; Synthetic session evidence does not verify physical hardware or the user's installed companion.

| Combined revision gate | Status | Required evidence |
|---|---|---|
| GNOME Wayland and TUI lifecycle | pending | Combined GNOME 51 Synthetic session passed; expanded seated run with the upgraded `Gnome2` companion is still required |
| Linux X11 and TUI lifecycle | blocking - pass | Combined Xvfb/Openbox run passed, including identified moves, exclusive hotkey rollback and TUI lifecycle |
| KDE Plasma Wayland | deferred - unverified | Seated KWin 6.x run with the protocol-2 script and companion; a nested pass cannot close this gate |
| Windows | deferred - unverified | Manual Windows run |

Historical GNOME 50.4 and X11 passes remain recorded below. Incoming readiness evidence passed the expanded X11 checks but left the expanded seated GNOME 50 gate pending. Local GNOME 51 interactive checks passed in isolation; the last recorded seated companion was `ACTIVE` with changed JavaScript awaiting a fresh login. Release gate status is exactly one of `pending`, `blocking - pass`, `blocking - fail`, or `deferred - unverified`. macOS, Sway, and Hyprland are ungated integrations; macOS Smoke evidence is recorded separately below. Linux Smoke runs report `Smoke assertions (<gate>): N; passed: P; failed: F; skipped: S`.

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

The X11 adapter owns exclusive `XGrabKey` accelerators (including CapsLock/NumLock variants); `xdotool` XTEST injection reaches these grabs without delivering the accelerator to the focused application. It no longer requires XRecord. Incoming readiness evidence re-recorded the expanded X11 gate successfully: 87/87 assertions on that branch. Its owned Openbox fixture uses an empty keyboard map so distro defaults do not preempt the test chords; a separate competing client still proves real grab refusal.

Pure X11 checks run with `cargo test --lib x11`. The ignored `x11_real_server_grabs_are_exclusive_atomic_and_released` test requires `WZ_X11_TEST_DISPLAY` and, when authentication is enabled, `XAUTHORITY` for an isolated X server, never the user's desktop. Use Xvfb or a separate rootful `Xwayland -geometry 800x600 -shm` on an isolated headless compositor, without `-enable-ei-portal`; GNOME's rootless XWayland did not deliver XTEST keys to X11 clients in the headless configuration tested here. Run it with `cargo test --lib x11_real_server_grabs_are_exclusive_atomic_and_released -- --ignored`.

`gnome-live` has two prerequisites on the seated session:

- The Companion must be active: `window-zones@mihai-a24` in `org.gnome.shell enabled-extensions` **and** `org.gnome.shell disable-user-extensions` set to `false`. The master switch suppresses the extension even when it is listed, which reads exactly like an extension that refuses to load.
- `/dev/uinput` must be writable by the user running the automated gate. Xwayland XTEST injection did not reach compositor accelerator grabs on the recorded GNOME 50 configuration; `gnome-live` fails immediately without authorized virtual-keyboard access and has no XTEST or physical-key fallback. Never grant world-writable access. An administrator can authorize a temporary per-user ACL using the saved/restored procedure in [the GNOME runbook](gnome-wayland.md#manual-smoke-checklist). Physical presses can provide separate manual evidence, not a pass of the automated gate.

`kde-live` needs the `kwin` package (`kwin_wayland`, `kpackagetool6`, `kwriteconfig6`), `dbus-update-activation-environment`, and working Xwayland XTEST-to-EIS delivery. It builds its own private bus, XDG tree, KWin configuration, and compositor. KWin runs on its `--virtual` framebuffer backend with two 1200x900 outputs; a nested windowed backend was rejected because the outer compositor resizes those output windows mid-run. This is Synthetic session evidence: KWin uses `Session::Type::Noop` and never owns the login seat, so even a passing run cannot close the seated KDE Release gate.

ADR 0009 changed every integration's move path (identified-window moves, restore-before-move, `org.window_zones.Gnome2`, KWin protocol 2, X11 `XGrabKey` hotkeys). Earlier matrix rows predate those expanded checks and do not close combined gates. On 2026-10-06 the Synthetic `gnome` gate ran against the incoming ADR 0009 tree on CachyOS, kernel 7.2.9-1-cachyos, GNOME Shell 50.5, Text Editor 50.1: 23 assertions, 17 passed, 0 failed, 6 skipped. That older harness omitted focus, placement, accelerator, and recovery checks because it created no virtual input devices. The local GNOME 51 interactive driver supplies native input and supersedes that Synthetic limitation; it still does not evidence physical hardware.

The `x11`, `gnome-live`, and `kde-live` gates assert the ADR 0009 window contract on disposable windows they start themselves, observed with `xdotool`/`xprop` (and, on KDE, a throwaway KWin script reading `tile`, `maximizeMode`, `fullScreen`, and `frameGeometry`). Incoming X11 and synthetic KDE have exercised these additions; incoming expanded seated GNOME 50 checks remained pending. Combined verification must also retain the local opposite-half crossing, restoration ordering, animation completion, and `--perf-log` checks.

- A captured window id still moves the original window after focus moved to a second test window; the focused window's frame and focus do not change.
- Moving the captured id of a closed window reports WindowGone, and the other window does not move.
- A maximized window and a tiled window are restored before the target frame is applied (X11: Openbox vertical maximization stands in for tiling; GNOME: real Super+Left edge tiling over `/dev/uinput`; KDE: `slotWindowQuickTileLeft`).
- A fullscreen window is rejected with `leave fullscreen first`, and its fullscreen state and frame do not change.
- A hotkey replacement the platform refuses (X11: another client already grabs `Alt+Ctrl+F8`; GNOME: `Alt+Ctrl+Left` is reserved; KDE: the conflict incumbent holds `Alt+Ctrl+F24`) is reported as `were refused`, keeps the previous binding count and `hotkey state: Registered`, and the previous accelerator still moves the window.

The X11 and GNOME checks build a small helper crate in `target/smoke-seam-helper` against the public `window_zones` API (`x11rb` from the repository lockfile), so the first run needs the crates available to `cargo`, from the network or a vendored source. It captures and moves windows through `X11WindowSystem`, sets EWMH states, and holds the competing X11 grab. The X11 gate also runs the ignored `x11_real_server_grabs_are_exclusive_atomic_and_released` test against its own Xvfb display. `kde-live` needs KWin 6 scripting with `workspace.slotWindowQuickTileLeft()` and the window `tile` property.

The harness preserves the invoking user's `CARGO_HOME` and `RUSTUP_HOME` before
isolating the App's `HOME`; helper builds and the ignored X11 test therefore
use the existing toolchain rather than a nonexistent default in the test home.

### Combined Linux evidence (2026-10-12)

The merged working tree combines local `8ce0fcc` with incoming `b436262` and
the conflict/integration repairs. All desktop fixtures and runtime instances
were owned and isolated; no seated extension or installed binary was replaced.

| Check | Observed outcome | Evidence |
|---|---|---|
| `./scripts/test.sh` | 160 library and 22 main tests passed; 1 real-X11 test ignored here and exercised by the X11 gate. GJS Companion contract: 202 checks passed. Formatting, clippy and script syntax checks passed. | `target/verify-window-zones/merged-final-quality-20261012.log` |
| `./scripts/smoke.sh --keep-logs x11` | 87 assertions passed, zero failures/skips. CachyOS Linux 7.2.9-2-cachyos x86_64; X.Org 1.21.1.25, Openbox 3.6.1, xdotool 4.20260303.1; two logical displays in Xvfb. | `target/verify-window-zones/merged-final-x11-20261012/` |
| `./scripts/smoke.sh --keep-logs gnome` | 256 assertions passed, zero failures/skips. GNOME Shell/Mutter 51.0 headless, unequal virtual displays, native GTK Wayland/XWayland clients, RemoteDesktop input and ScreenCast pixels; CLI/TUI/tray, restoration, crossing/no-wrap, animations/reduced motion and `--perf-log` exercised. | `target/verify-window-zones/merged-final-gnome-20261012/` |
| Updated dry-run scenario and feature recipes | Final CLI/TUI scenario, config/performance-log isolation regression and cleanup-ownership drills passed with a relative `WZV_ROOT`; zero live-backend program calls or instance locks. Full movement and one-shot recipes passed earlier. Scratch and owned processes removed; evidence kept. | `target/verify-window-zones/merged-final-20261012/`, `merged-movement-20261012/`, `merged-one-shot-20261012/` |
| Fixed-size X11 refusal | Real GTK fixture advertised identical minimum/maximum dimensions. Before repair the CLI reported success despite clamping; after repair it exited 1 with `non-resizable`, without changing geometry. | `target/verify-window-zones/merged-final-x11-fixed-20261012.json` |
| macOS and Windows cross-compilation | `cargo check --locked --target x86_64-apple-darwin --all-targets` and `--target x86_64-pc-windows-gnu --all-targets` passed. These are compile checks, not native runtime evidence. | `target/verify-window-zones/merged-final-quality-20261012.log` |

The live GNOME run first exposed the nonexistent `global.get_pid()` API that a
false stub had hidden. Real GIO credentials now drive the exclusion check and
its regression. The harness also now waits for registration events rather than
an early status snapshot and checks tray data rather than obsolete label wording.
Separate CLI smoke proved that restart retains one performance-log session,
failed dispatches are logged, an invalid log destination fails before dispatch,
`tui` exits on stdin EOF, and service-style `run` stays alive with stdin closed.

Review also repaired transactional reload during a companion outage, stale
restore points after manual moves, queued Rdev presses during replacement,
absolute-XDG fallback in the installer and retained dead macOS AX references.
The first three have failing-before/passing-after consumer regressions. Actual
isolated installer uninstall proved relative-XDG fallback before and after.
Owned harness probes proved that file/parent/implicit-config symlinks and
performance-log paths cannot escape the run, reversed connector enumeration
uses spatial order, and SIGTERM/finally cleanup removes TERM-resistant owned
descendants without stopping an unrelated sentinel session.

The macOS retained-reference check is compiled but unrun here. On macOS,
`WZ_MACOS_TEST_PID="$FIXTURE_PID" cargo test --locked --lib closed_fixture_releases_retained_window -- --ignored --test-threads=1`
requires a frontmost, disposable owned fixture and deliberately terminates it;
never supply a user's application PID. macOS compilation reports an existing
future-incompatibility warning from `block 0.1.6`.

No new seated GNOME, KDE, Windows or macOS runtime gate is closed by these results.


### Incoming readiness evidence (2026-10-10)

These results were recorded by the incoming readiness branch, not this merge.
Its environmental failures do not describe the local GNOME 51 workstation.
The original record does not state every Xvfb/Openbox point release or the
macOS version; do not infer broader version coverage from those rows.

| Configuration | Gate status | Observed outcome | Evidence |
|---|---|---|---|
| Linux X11, isolated Xvfb + Openbox in a Debian 13 aarch64 `rust:1.95-trixie` container | blocking - pass | 87 assertions: 87 passed, 0 failed, 0 skipped; includes ADR 0009, real exclusive grabs/input, refused reload retaining the old grab, and TUI lifecycle | `target/verify-window-zones/readiness-linux-x11/` |
| GNOME 50 seated session | pending | No reachable seat. The configured CachyOS SSH target timed out on port 22. Container GNOME Shell 48.7 could not activate `org.freedesktop.login1`: synthetic run 6 assertions, 2 passed, 4 failed. Live preflight: 2 assertions, 1 passed, 1 failed because `/dev/uinput` is not writable | `readiness-linux-gnome-system-bus/`, `readiness-linux-gnome-live/` under the same evidence root |
| KDE Plasma Wayland seated session | deferred - unverified | No seated run. Latest synthetic KWin: 180 assertions, 162 passed, 15 input-dependent failures, 3 skips; private PipeWire did not fix delivery. See [the KDE runbook](kde-wayland.md#nested-kwin-gate) | `readiness-linux-kde-activation/`, `readiness-linux-kde-pipewire/` under the same evidence root |
| Windows | deferred - unverified | No new Windows verification in this readiness work | Historical record below |

Incoming branch regressions: Linux `./scripts/test.sh` passed 146 library tests (1 ignored),
15 main tests, and 170 GNOME companion checks. macOS passed 91 library tests
(1 GUI regression ignored by default) and 10 main tests; the owned live
`expansion_finishes_at_target_origin` regression passed separately.
Formatting, clippy, and shell/compositor syntax checks passed. The
maintained dry-run scenario `readiness-20261010-final` passed all 21 steps, with
zero live-backend program calls and instance locks. Program sentinels do not
observe native Framework calls; dry-run evidence never closes a live gate.

### macOS native smoke (ungated)

This is incoming pre-merge evidence. The original record does not name the
macOS version or every display/Framework version; these observations are not
a blanket macOS compatibility claim or verification of the combined revision.

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

### Historical branch matrix (not combined verification)

These rows retain the branch source, exact recorded configurations, outcomes,
and dates. They are not current combined gate closure evidence. Incoming
re-recordings and local GNOME 51 snapshots are separate observations, not
interchangeable passes.

| Gate | Status | Environment | Verified configuration | Date | How to reproduce |
|---|---|---|---|---|---|
| GNOME Wayland (local earlier record) | blocking - pass | Synthetic session: `gnome-shell --headless` with virtual monitors 1920x1080 and 1600x900, isolated dconf/data/runtime dirs on a private session bus. Seated session: live GNOME with two physical monitors (HDMI-1 2560x1440 at 0,0 and DP-1 3440x1440 at 2560,0), Text Editor on XWayland, `xdotool` observer, and `/dev/uinput` virtual keyboard | CachyOS, kernel 7.2.4-1-cachyos; GNOME Shell/Mutter 50.4. Synthetic: 23 assertions, 0 failures, run twice, six input-dependent checks skipped. Seated: 72 assertions, 0 failures, run twice — exact zones and usable areas, next/previous displays, real virtual-keyboard accelerators, atomic rejected-set retention, Companion recovery, invalid-config retention. This predates ADR 0009 | 2026-09-11 | Historical `./scripts/smoke.sh gnome` and `gnome-live` |
| GNOME Wayland (incoming re-recording) | blocking - pass | Same two virtual-monitor Synthetic fixture; seated evidence reused from 2026-09-11, not a new seated run | Synthetic: CachyOS, kernel 7.2.5-1-cachyos; GNOME Shell/Mutter 50.4; commit `3eff23c`, over SSH. 23 assertions, 0 failures, run twice, six input-dependent skips; usable areas `display-1: 0,32,1920,1048` and `display-2: 1920,0,1600,900`. Seated pre-ADR 0008 record: 72 assertions, 0 failures, run twice — left-half `0,0,1280,1440`, center-third `853,0,854,1440`, left-two-thirds `0,0,1707,1440`, next-display `2560,32,2294,1408`, accelerators/atomicity/recovery/config retention | 2026-09-13 (synthetic), 2026-09-11 (seated) | Historical `./scripts/smoke.sh gnome` and `gnome-live` |
| Linux X11 (incoming earlier record) | blocking - pass | Xvfb 3520x1080 split into logical monitors 1920x1080 and 1600x1080, Openbox, Text Editor, `xdotool` plus `_NET_FRAME_EXTENTS` observer | CachyOS, kernel 7.2.5-1-cachyos; xorg-server 21.1.24; Openbox 3.6.1; xdotool 4.20260303.1; commit `3eff23c`. 56 assertions, 0 failures, run twice — exact frame zones, wrapping display moves, monitor-2 dispatch from `1930,10` to `1920,0,800,1080`, then-`rdev` capture, invalid-config retention/recovery, TUI. Also 56/56 on 2026-09-12 in Debian 12 `rust:1-bookworm`, xorg-server 21.1.7, xdotool 3.20160805.1; predates `XGrabKey` cutover | 2026-09-13 | Historical `./scripts/smoke.sh x11` |
| TUI lifecycle (incoming GNOME) | blocking - pass | Same Synthetic and seated GNOME configurations above | Launch, `reload`, `restart`, `status`, `dispatch HOTKEY`, `quit` in both halves; seated dashboard reports dispatched binding | 2026-09-13 (synthetic), 2026-09-11 (seated) | Historical `./scripts/smoke.sh gnome` and `gnome-live` |
| TUI lifecycle (incoming X11) | blocking - pass | Same incoming Xvfb/Openbox configuration above | Launch, `reload`, `restart`, `status`, `dispatch HOTKEY`, `quit` | 2026-09-13 | Historical `./scripts/smoke.sh x11` |
| GNOME Wayland (local 51 upgrade, automated snapshot) | blocking - fail | Isolated headless Shell with two virtual monitors; seated automated run before user confirmation found cached GNOME-50-only metadata, `OUT OF DATE` | CachyOS, kernel 7.2.9-2-cachyos; GNOME Shell/Mutter 51.0; Text Editor 51.0. Synthetic: 17 passed, 6 input-dependent skips, 0 failures. Seated: 2 activation prerequisite failures; dependent checks skipped, extension settings unchanged | 2026-10-11 | Historical `./scripts/smoke.sh --keep-logs gnome` and `gnome-live`; repeat the full seated gate for the combined revision |
| GNOME Wayland (local 51 interactive snapshot before restoration) | pending | Isolated Shell/Mutter, unequal virtual monitors, native GTK Wayland/XWayland windows, RemoteDesktop input and ScreenCast frames; earlier seated activation `OUT OF DATE` | CachyOS kernel 7.2.9-2-cachyos, GNOME/Mutter 51.0, GTK 4.24.1, PyGObject 3.58.1, GStreamer 1.28.8, PipeWire 1.6.9, AppIndicator 66. 150 assertions passed with debug and installed release binaries; no skips. Default `auto run`/`auto tui` handle Ctrl+Super arrows. Physical input and seated lock/unlock not covered | 2026-10-11 | Local `db92c30` snapshot; current driver includes later restoration checks |
| GNOME Wayland (local 51 maximized/tiled placement) | pending | Same local GNOME 51 Synthetic configuration and native clients/input/captures; seated extension `ACTIVE`, changed JavaScript awaiting fresh login | 207 GNOME assertions passed without skips. Maximized/left/right-tiled clients move to built-in/custom zones and both display directions; full/half layouts survive unequal monitors. GTK confirms state cleared. Default Ctrl+Super+Left and Down/Up exercise maximized placement; fullscreen/fixed-size rejection covered | 2026-10-11 | Local `./scripts/test.sh && ./scripts/smoke.sh --keep-logs all`: 254 smoke assertions, 113 Rust tests passed |
| GNOME Wayland (local 51 transitions/profiling) | pending | Same local GNOME/Mutter 51.0 Synthetic configuration, unequal monitors, GTK Wayland/XWayland clients, native input and rendered frames; seated upgrade awaiting fresh login | Local `8ce0fcc` work: 253 native assertions, no failures/skips — opposite-half directional crossing/outer-edge stability, Right presses 30 ms apart, animation completion, restored states, disabled animations/reduced motion, actual process memory, logger restart/failure, caller timing isolation. 106 library and 16 CLI tests passed; X11 portion 46 checks | 2026-10-11 | `/usr/bin/python3 scripts/gnome-smoke.py --binary "$PWD/target/release/window_zones" --keep-logs`; local `./scripts/test.sh` and `./scripts/smoke.sh --keep-logs x11` |
| Linux X11 (local 51-era record) | blocking - pass | Xvfb 3520x1080 split into logical monitors 1920x1080 and 1600x1080, Openbox, Text Editor | CachyOS, kernel 7.2.9-2-cachyos; X.Org X Server 1.21.1.25; Openbox 3.6.1; xdotool 4.20260303.1; Text Editor 51.0. 46 assertions passed, no failures/skips; predates incoming expanded identified-move and `XGrabKey` checks | 2026-10-11 | Local `./scripts/smoke.sh --keep-logs all`, X11 portion |
| TUI lifecycle (local X11) | blocking - pass | Same local Xvfb/Openbox configuration above | Launch, `reload`, `restart`, `status`, `dispatch HOTKEY`, `quit` | 2026-10-11 | Local `./scripts/smoke.sh --keep-logs all` |
| KDE Plasma Wayland (local earlier record) | deferred - unverified | Nested KWin `--virtual`, two 1200x900 outputs, private XDG/bus, repository script/service, XWayland Text Editor, independent `MaximizeArea` observer, XTEST-to-EIS input; no seated run | CachyOS, kernel 7.2.4-1-cachyos; KWin 6.7.5; Xwayland 24.1.13; Qt 6.11.2. Originally reported 138 assertions, 0 failures, twice, with seated capture/panels/negative outputs skipped. Incoming re-recording below found the earlier script was not QJSEngine-parseable; this original report is not reliable proof of that contract | 2026-09-12 | Historical `./scripts/smoke.sh --keep-logs kde-live`; seated gate remains unverified |
| KDE Plasma Wayland (incoming corrected record) | deferred - unverified | Same nested KWin fixture, protocol/service script, independent output observer; no seated run | CachyOS, kernel 7.2.5-1-cachyos; KWin 6.7.5; Xwayland 24.1.13; Qt 6.11.2; commit `3eff23c`. 142 assertions, 0 failures, twice — handshake/capabilities/native routing, per-output areas, focus/zones, wrapping moves to `1200,0,800,900`, input, KGlobalAccel `action(int)` refusal of `alt+ctrl+f25`, conflict refusal, script/service recovery with same App PID, retention/TUI. Seated capture, panel exclusion, negative outputs skipped. Corrects the 138-assertion record whose script QJSEngine could not parse; `qmllint` now checks this syntax when installed | 2026-09-13 | Historical `./scripts/smoke.sh --keep-logs kde-live`; only a seated KWin 6.x run closes the gate |
| Windows | deferred - unverified | Windows desktop session | `windows` CI job compiles and unit-tests `src/windows_window_system.rs` against `windows` 0.58; the Release gate closes with the manual run in `docs/runbooks/windows-smoke.md`. | 2026-09-11 | `windows` job in `.github/workflows/ci.yml`; run `docs/runbooks/windows-smoke.md` to close the Release gate. |

### Earlier GNOME 51 startup/tray checks (2026-10-11)

The following three sections are local pre-merge evidence from the GNOME 51
work through `8ce0fcc`, on CachyOS kernel 7.2.9-2-cachyos with GNOME/Mutter
51.0, GTK 4.24.1, PyGObject 3.58.1, GStreamer 1.28.8, PipeWire 1.6.9, and
AppIndicator 66. References to installed files or seated extension state are
observations at those snapshots, not verification of this combined tree.

These historical observations precede the requested automatic restoration of maximized/tiled windows:

- `./scripts/test.sh`: 98 library tests and 15 CLI tests passed; Rust formatting, Bash/JavaScript/Python syntax checks, doc tests, and Clippy with warnings denied passed.
- `./scripts/smoke.sh --keep-logs all`: 197 assertions, zero failures — 150 native GNOME interactive checks, the Cargo build check, and 46 X11 checks. The installed release binary independently passed the same 150 GNOME checks.
- Every built-in zone and a custom percentage zone were checked against both Mutter's frame and the independently reported GTK allocation. Actual native keyboard input exercised all bindings; actual pointer input clicked the GTK button. Native Wayland and XWayland windows were rendered and captured.
- Monitor movement covers both directions, wraparound, built-in-zone preservation, and proportional custom geometry. The larger-monitor regression failed before the fix (requested height 1048, observed 900) and passed after `move_to_monitor` updated destination bounds before resizing.
- Tray status updates previously accumulated six icons. The fixed runtime retains one icon, updates the actual menu labels, exposes config errors, clears them on recovery, and preserves functioning bindings. Show status/Reload/Restart/Quit and clickable status rows were exercised through the real AppIndicator menu service; the panel icon and mouse interaction were visually checked in compositor screenshots.
- Configuration reload covers changed actions, malformed TOML, out-of-range custom zones, duplicate normalized hotkeys, deletion, restoration, and last-valid rollback. Companion tests cover disable/re-enable recovery without restarting the controller, Busy rejection, atomic accelerator rollback, and releasing newly grabbed keys after a failed transaction.
- At this earlier snapshot, maximized/tiled placement was deliberately rejected; that restriction is superseded by the current restoration behavior below. Fullscreen, fixed-size, overview, and no-focused-window safety checks, dry-run isolation, explicit X11 rejection inside Wayland, and the D-Bus polling regression passed.
- Rebuilt and installed the release binary at `~/.local/bin/window_zones` and the updated GNOME 50/51 companion. Installed four default bindings in `~/.config/window_zones/config.toml`: Ctrl+Super+Left/Right for halves, Down/Up for next/previous monitor. The default launcher and both automatic interactive modes were exercised with that same configuration in isolation.
- Evidence: `/tmp/window_zones-smoke.171049/assertions.log` and its `gnome/` screenshots; installed release: `/tmp/window-zones-native-gnome-9x9k212m/results.json`, `native-tray-and-mouse.png`, and `native-and-xwayland-windows.png`. These temporary paths are retained local evidence, not portable fixtures.
- After installation, seated `status` selects `gnome-wayland` and loads all four bindings, but Shell still reports the companion `Enabled: Yes`, `State: OUT OF DATE`. The error now explicitly requests logout/login after an upgrade. The user chose to restart the login after isolated tests; full seated placement, physical hotkeys, monitor hotplug/scaling, and lock/unlock remain unverified until then. No `/dev/uinput` permissions, version-validation bypass, or other seated extension settings were changed.

### Maximized/tiled placement checks (2026-10-11)

- The new public CLI regression failed before the companion change: `zone-from-maximized dispatch` returned the reported maximized/tiled rejection. Evidence: `/tmp/window-zones-native-gnome-m2mlosjq/zone-from-maximized-dispatch.log`.
- The shared companion move boundary now restores native maximization/side tiling after validating the request and intrinsic client resize capability. Display targets are calculated from the original full/half frame, so moving a maximized window fills the destination usable area rather than reverting to its saved normal size.
- Real Wayland and XWayland GTK clients passed built-in-zone placement from maximization, custom-zone placement from tiling, and next/previous display moves from both states. Independent GTK toplevel state confirms restoration; compositor and client geometry agree. Actual default automatic CLI/TUI shortcuts also passed from maximization.
- Full verification: 98 library and 15 CLI tests; formatting, Bash/JavaScript/Python syntax, doc tests, and Clippy passed. The combined smoke run passed 254 assertions: 207 GNOME, 46 X11, and the build check.
- Final evidence: `/tmp/window_zones-smoke.235246/assertions.log`, `gnome/results.json`, and `gnome/native-and-xwayland-windows.png`. The installed release binary separately exercised the changed companion with 203 assertions before the additional four default monitor-hotkey state assertions.
- Installed the updated companion JavaScript. The seated extension reports `Enabled: Yes`, `State: ACTIVE`; a fresh login is still required to load changed source. Full seated proof of the new behavior, physical input, scaling/hotplug, and lock/unlock remains unverified. No source reload bypass, version-validation bypass, or input-device permission changes were used.

### Directional transitions and profiling checks (2026-10-11)

- Native Wayland and XWayland half placement crosses in both physical directions, lands on the nearest half of the destination, handles unequal sizes, and remains at the outer edge. Explicit next/previous display wraparound and all existing zone/config/CLI/TUI/tray checks still pass.
- Two actual Ctrl+Super+Right presses 30 ms apart produce two completed moves and two successful JSONL records. Request timing is separate from frame/animation completion; maximized placement waits for the finished visual transition. Disabled animations and reduced motion avoid the custom motion delay.
- Waiting for completion exposed a late Wayland restore acknowledgement overwriting a larger monitor's target: requested 1920x1048, observed 1600x900. The companion now waits for restoration before requesting the destination frame, holds the original snapshot, animates once, and validates final geometry. Native maximized/tiled cases pass in both monitor directions.
- Session/dispatch rows contain real Linux App-process RSS/peak RSS; runtime restart keeps the log open. Failed fullscreen moves have no stale success timings, and an independent caller cannot read another controller's timing record. Rust checks cover append behavior, startup log-open failure, and late write failure without breaking movement.
- Verification: 106 library and 16 CLI tests, formatting, doc tests, Clippy, and syntax checks passed. Final release-binary native driver: 253 assertions, zero failures/skips. X11: 46 assertions passed in the combined run's X11 portion; its earlier GNOME animation snapshot failed and was corrected before the final native run.
- Evidence: `/tmp/window-zones-native-gnome-drtq7e8v/results.json`, `transitions.jsonl`, `animation-00.png` through `animation-15.png`, and `native-and-xwayland-windows.png`; X11 evidence is in `/tmp/window_zones-smoke.281559/`. Intermediate and final compositor surfaces were visually inspected. These are retained local evidence, not portable fixtures.
- The binary and companion are updated on disk; new JavaScript still requires logout/login. Physical-key delivery, seated monitor scaling/hotplug, and lock/unlock remain unverified for this source revision. Profiling excludes shared GNOME Shell/GPU memory and hotkey queue latency; it is not a whole-desktop or key-to-photon benchmark.

### Minimum verification

If you only need fast feedback:

```bash
cargo test --locked
```
