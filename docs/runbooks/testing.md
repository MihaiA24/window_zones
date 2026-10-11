# Testing Runbook

## Local gate

```bash
./scripts/test.sh
```

What this verifies:

- `cargo fmt --all -- --check` (formatting)
- `cargo test --locked` (unit + integration tests)
- `cargo test --locked --doc` (doc tests)
- `cargo clippy --locked --all-targets --all-features` (only if clippy is installed)
- `bash -n scripts/*.sh` (shell syntax, including the Smoke harness)
- `node --check gnome-extension/extension.js`
- `node --check kwin-script/contents/code/main.js`
- Python syntax for `scripts/gnome-smoke.py` (system Python, without generating cache files)

The repository test script covers formatting, unit/integration tests, doc tests,
and clippy when installed. The GNOME and KDE adapter contract tests use fake
compositor services on private `dbus-daemon` instances; they do not require a
running compositor session.

The compositor scripts can be syntax-checked with:

```bash
node --check gnome-extension/extension.js
node --check kwin-script/contents/code/main.js
```

## V1 verification matrix

This matrix preserves exact verified configurations rather than inferring seated results from an isolated compositor. The earlier GNOME 50.4 gate passed. The current GNOME 51 companion supports maximized/tiled placement, directional half repetition, animation, and optional transition metrics; the seated companion is `ACTIVE`, but a fresh login is required to import newly installed JavaScript before seated verification. Linux X11 passes on this device. KDE Plasma Wayland and Windows remain `deferred - unverified`. Release gate status is exactly one of `pending`, `blocking - pass`, `blocking - fail`, or `deferred - unverified`.

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

`gnome-live` has two prerequisites on the seated session:

- The Companion must be active: `window-zones@mihai-a24` in `org.gnome.shell enabled-extensions` **and** `org.gnome.shell disable-user-extensions` set to `false`. The master switch suppresses the extension even when it is listed, which reads exactly like an extension that refuses to load.
- Automated accelerator delivery needs a writable `/dev/uinput`; otherwise the harness tries XTEST and asks for physical input. Xwayland XTEST is not evidence of compositor accelerator capture. Never grant world-writable access: an administrator can authorize a temporary per-user ACL using the saved/restored procedure in [the GNOME runbook](gnome-wayland.md#manual-smoke-checklist). Without authorized access or a physical key press, record delivery as unverified.

`kde-live` needs the `kwin` package (`kwin_wayland`, `kpackagetool6`, `kwriteconfig6`) and nothing from the user's session: it builds its own private bus, XDG tree, KWin configuration, and compositor. It runs KWin on its `--virtual` framebuffer backend with two 1200x900 outputs; a nested windowed backend was tried first and rejected because the outer compositor resizes those output windows mid-run. It is Synthetic session evidence whatever it reports: KWin runs a `Session::Type::Noop` session and never owns the login seat, so it cannot close the KDE Release gate — only a Smoke run on a seated KWin session does that.

| Gate | Status | Environment | Verified configuration | Date | How to reproduce |
|---|---|---|---|---|---|
| GNOME Wayland | blocking - pass | Synthetic session: `gnome-shell --headless` with virtual monitors 1920x1080 and 1600x900, isolated dconf/data/runtime dirs on a private session bus. Seated session: the live GNOME session with two physical monitors (HDMI-1 2560x1440 at 0,0 and DP-1 3440x1440 at 2560,0), `gnome-text-editor` on XWayland as the test window, `xdotool` as the independent observer, and a `/dev/uinput` virtual keyboard as the input source | CachyOS, kernel 7.2.4-1-cachyos; GNOME Shell 50.4; mutter 50.4. Synthetic: 23 assertions, 0 failures, run twice — Companion bus ownership, `GetCapabilities`, both virtual displays with the 32px panel excluded, config reload/retention/recovery; six seat-dependent checks recorded as skipped. Seated: 72 assertions, 0 failures, run twice — focused-window discovery agreeing with `xdotool` to the pixel, left-half `0,0,1280,1440`, center-third `853,0,854,1440`, left-two-thirds `0,0,1707,1440`, cross-display move to `2560,32,2294,1408` on `display-2` and back, real accelerator capture through the kernel virtual keyboard, atomic registration with a rejected set preserving the previous one, Companion disconnect and recovery without an App restart, and invalid-config reload retaining the last valid bindings | 2026-09-11 | `./scripts/smoke.sh gnome` and `./scripts/smoke.sh gnome-live` |
| GNOME Wayland (51 upgrade, automated snapshot) | blocking - fail | Synthetic: isolated headless Shell with two virtual monitors. Seated automated run before user confirmation: Shell cached the companion's old GNOME-50-only metadata and reported `OUT OF DATE` | CachyOS, kernel 7.2.9-2-cachyos; GNOME Shell 51.0; Mutter 51.0; Text Editor 51.0. Synthetic: 17 passed, 6 seat-dependent skips, 0 failures. Seated: 2 activation prerequisite failures; dependent checks skipped, extension settings unchanged | 2026-10-11 | `./scripts/smoke.sh --keep-logs gnome` and `./scripts/smoke.sh --keep-logs gnome-live`; repeat the full seated gate to replace this automated snapshot |
| GNOME Wayland (51 interactive snapshot before restoration support) | pending | Isolated real Shell/Mutter, two virtual monitors, native GTK Wayland and XWayland windows, RemoteDesktop input and ScreenCast capture; earlier seated activation reported `OUT OF DATE` | CachyOS 7.2.9-2-cachyos, GNOME/Mutter 51.0, GTK 4.24.1, PyGObject 3.58.1, GStreamer 1.28.8, PipeWire 1.6.9, AppIndicator 66. 150 interactive assertions passed with both debug and installed release binaries; no skipped cases in that driver. Both default-config `auto run` and `auto tui` handle real Ctrl+Super arrow events. Physical input and seated lock/unlock are not covered | 2026-10-11 | Historical `db92c30` snapshot; the current gate includes the restoration checks below |
| GNOME Wayland (51 maximized/tiled placement) | pending | Same isolated real GNOME, GTK Wayland/XWayland clients, native input and rendered captures; seated extension `ACTIVE`, changed JavaScript awaiting fresh login | 207 GNOME assertions passed without skips. Native maximized and left/right-tiled clients move to built-in/custom zones and both display directions; destination full/half layouts survive different monitor sizes. GTK independently confirms maximization/tiling cleared. Actual default Ctrl+Super+Left and Down/Up shortcuts exercise maximized-window placement. Fullscreen and fixed-size rejection remains covered | 2026-10-11 | `./scripts/test.sh && ./scripts/smoke.sh --keep-logs all` — 254 smoke assertions passed, 113 Rust tests passed |
| GNOME Wayland (51 directional transitions/profiling) | pending | Same isolated real Shell/Mutter 51.0, unequal-size virtual displays, GTK Wayland/XWayland clients, native input and rendered captures; seated source upgrade awaiting fresh login | 253 native assertions passed without skips: directional half crossing/outer-edge stability, two Right presses 30 ms apart, animation completion, maximized/tiled restoration, disabled animations/reduced motion, real process memory, logger restart/failure rows, and per-caller timing isolation. 106 library and 16 CLI tests passed; the X11 portion passed 46 checks | 2026-10-11 | `/usr/bin/python3 scripts/gnome-smoke.py --binary "$PWD/target/release/window_zones" --keep-logs`; local checks with `./scripts/test.sh`; X11 with `./scripts/smoke.sh --keep-logs x11` |
| Linux X11 | blocking - pass | Xvfb 3520x1080 split by `xrandr --setmonitor` into logical monitors 1920x1080 and 1600x1080, Openbox as EWMH window manager, Text Editor as the test window | CachyOS, kernel 7.2.9-2-cachyos; X.Org X Server 1.21.1.25; Openbox 3.6.1; xdotool 4.20260303.1; Text Editor 51.0 | 2026-10-11 | `./scripts/smoke.sh --keep-logs all` — X11 portion: 46 assertions passed, no failures or skips |
| TUI lifecycle (GNOME) | blocking - pass | Same Synthetic session and same seated live GNOME session | TUI launch, `reload`, `restart`, `status`, `dispatch HOTKEY`, and `quit` asserted in both halves; the seated run also asserts the dashboard reports the dispatched binding | 2026-09-11 | `./scripts/smoke.sh gnome` and `./scripts/smoke.sh gnome-live` |
| TUI lifecycle (X11) | blocking - pass | Same current Xvfb + Openbox Synthetic session | Launch, `reload`, `restart`, `status`, `dispatch HOTKEY`, and `quit` exercised in the X11 gate | 2026-10-11 | `./scripts/smoke.sh --keep-logs all` |
| KDE Plasma Wayland | deferred - unverified | Synthetic session: KWin 6.7.5 on its `--virtual` backend with two 1200x900 outputs and its own Xwayland, the repository KWin script installed with `kpackagetool6` into a private XDG tree, the Rust companion started before KWin on a private bus, `gnome-text-editor` on Xwayland as the test window, an independent KWin observer script reading per-output `MaximizeArea`, and XTEST-to-EIS key injection. Seated KDE session: not run | Nested configuration only: CachyOS, kernel 7.2.4-1-cachyos; KWin 6.7.5; Xwayland 24.1.13; Qt 6.11.2. 138 assertions, 0 failures, run twice — protocol handshake, capability set, `--backend auto` resolving `kde-wayland`, per-output display enumeration cross-checked against the observer, focused window, three zones, next/previous cross-display moves with wrap, real accelerator capture, atomic registration with a rejected accelerator, conflict refusal, script and service disconnect/recovery with an unchanged App PID, config retention, and the TUI lifecycle. Three limitations are recorded as named skips: seated accelerator capture, Plasma panel exclusion, negative-coordinate output movement. Seated KDE remains unverified | 2026-09-12 | `./scripts/smoke.sh --keep-logs kde-live`; the Release gate still closes only on a seated KWin 6.x Smoke run |
| Windows | deferred - unverified | Windows desktop session | `windows` CI job compiles and unit-tests `src/windows_window_system.rs` against `windows` 0.58; the Release gate closes with the manual run in `docs/runbooks/windows-smoke.md`. | 2026-09-11 | `windows` job in `.github/workflows/ci.yml`; run `docs/runbooks/windows-smoke.md` to close the Release gate. |

### Earlier GNOME 51 startup/tray checks (2026-10-11)

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
