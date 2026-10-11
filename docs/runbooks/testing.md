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

This matrix is the record of record for V1 Release gate status and preserves the exact verified configurations. The earlier GNOME 50.4 gate passed; the last recorded automated GNOME 51 seated run was blocked on a new login, despite passing Synthetic checks. The user subsequently confirmed Linux/GNOME 51 is working; that manual confirmation does not replace the recorded full Smoke checklist. Linux X11 passes on the current device. KDE Plasma Wayland and Windows remain `deferred - unverified`. Release gate status is exactly one of `pending`, `blocking - pass`, `blocking - fail`, or `deferred - unverified`.

Run the GNOME Wayland or Linux X11 Smoke run with:
```bash
./scripts/smoke.sh gnome       # headless Synthetic GNOME session: Companion load, Capability negotiation, display enumeration, usable-area math
./scripts/smoke.sh gnome-live  # seated live GNOME session: focused window, placement, real accelerator capture, Companion disconnect/recovery, TUI lifecycle
./scripts/smoke.sh kde-live    # nested KWin 6.x Synthetic session: the same checks against a real KWin, never seated evidence
./scripts/smoke.sh x11
```
The TUI lifecycle Smoke runs cover launch, reload, dispatch, restart, and quit in each corresponding Synthetic or seated session.

`gnome-live` has two prerequisites on the seated session:

- The Companion must be active: `window-zones@mihai-a24` in `org.gnome.shell enabled-extensions` **and** `org.gnome.shell disable-user-extensions` set to `false`. The master switch suppresses the extension even when it is listed, which reads exactly like an extension that refuses to load.
- Automated accelerator delivery needs a writable `/dev/uinput`; otherwise the harness tries XTEST and asks for physical input. Xwayland XTEST is not evidence of compositor accelerator capture. Never grant world-writable access: an administrator can authorize a temporary per-user ACL using the saved/restored procedure in [the GNOME runbook](gnome-wayland.md#manual-smoke-checklist). Without authorized access or a physical key press, record delivery as unverified.

`kde-live` needs the `kwin` package (`kwin_wayland`, `kpackagetool6`, `kwriteconfig6`) and nothing from the user's session: it builds its own private bus, XDG tree, KWin configuration, and compositor. It runs KWin on its `--virtual` framebuffer backend with two 1200x900 outputs; a nested windowed backend was tried first and rejected because the outer compositor resizes those output windows mid-run. It is Synthetic session evidence whatever it reports: KWin runs a `Session::Type::Noop` session and never owns the login seat, so it cannot close the KDE Release gate — only a Smoke run on a seated KWin session does that.

| Gate | Status | Environment | Verified configuration | Date | How to reproduce |
|---|---|---|---|---|---|
| GNOME Wayland | blocking - pass | Synthetic session: `gnome-shell --headless` with virtual monitors 1920x1080 and 1600x900, isolated dconf/data/runtime dirs on a private session bus. Seated session: the live GNOME session with two physical monitors (HDMI-1 2560x1440 at 0,0 and DP-1 3440x1440 at 2560,0), `gnome-text-editor` on XWayland as the test window, `xdotool` as the independent observer, and a `/dev/uinput` virtual keyboard as the input source | CachyOS, kernel 7.2.4-1-cachyos; GNOME Shell 50.4; mutter 50.4. Synthetic: 23 assertions, 0 failures, run twice — Companion bus ownership, `GetCapabilities`, both virtual displays with the 32px panel excluded, config reload/retention/recovery; six seat-dependent checks recorded as skipped. Seated: 72 assertions, 0 failures, run twice — focused-window discovery agreeing with `xdotool` to the pixel, left-half `0,0,1280,1440`, center-third `853,0,854,1440`, left-two-thirds `0,0,1707,1440`, cross-display move to `2560,32,2294,1408` on `display-2` and back, real accelerator capture through the kernel virtual keyboard, atomic registration with a rejected set preserving the previous one, Companion disconnect and recovery without an App restart, and invalid-config reload retaining the last valid bindings | 2026-09-11 | `./scripts/smoke.sh gnome` and `./scripts/smoke.sh gnome-live` |
| GNOME Wayland (51 upgrade, automated snapshot) | blocking - fail | Synthetic: isolated headless Shell with two virtual monitors. Seated automated run before user confirmation: Shell cached the companion's old GNOME-50-only metadata and reported `OUT OF DATE` | CachyOS, kernel 7.2.9-2-cachyos; GNOME Shell 51.0; Mutter 51.0; Text Editor 51.0. Synthetic: 17 passed, 6 seat-dependent skips, 0 failures. Seated: 2 activation prerequisite failures; dependent checks skipped, extension settings unchanged | 2026-10-11 | `./scripts/smoke.sh --keep-logs gnome` and `./scripts/smoke.sh --keep-logs gnome-live`; repeat the full seated gate to replace this automated snapshot |
| Linux X11 | blocking - pass | Xvfb 3520x1080 split by `xrandr --setmonitor` into logical monitors 1920x1080 and 1600x1080, Openbox as EWMH window manager, Text Editor as the test window | CachyOS, kernel 7.2.9-2-cachyos; X.Org X Server 1.21.1.25; Openbox 3.6.1; xdotool 4.20260303.1; Text Editor 51.0 | 2026-10-11 | `./scripts/smoke.sh --keep-logs all` — X11 portion: 46 assertions passed, no failures or skips |
| TUI lifecycle (GNOME) | blocking - pass | Same Synthetic session and same seated live GNOME session | TUI launch, `reload`, `restart`, `status`, `dispatch HOTKEY`, and `quit` asserted in both halves; the seated run also asserts the dashboard reports the dispatched binding | 2026-09-11 | `./scripts/smoke.sh gnome` and `./scripts/smoke.sh gnome-live` |
| TUI lifecycle (X11) | blocking - pass | Same current Xvfb + Openbox Synthetic session | Launch, `reload`, `restart`, `status`, `dispatch HOTKEY`, and `quit` exercised in the X11 gate | 2026-10-11 | `./scripts/smoke.sh --keep-logs all` |
| KDE Plasma Wayland | deferred - unverified | Synthetic session: KWin 6.7.5 on its `--virtual` backend with two 1200x900 outputs and its own Xwayland, the repository KWin script installed with `kpackagetool6` into a private XDG tree, the Rust companion started before KWin on a private bus, `gnome-text-editor` on Xwayland as the test window, an independent KWin observer script reading per-output `MaximizeArea`, and XTEST-to-EIS key injection. Seated KDE session: not run | Nested configuration only: CachyOS, kernel 7.2.4-1-cachyos; KWin 6.7.5; Xwayland 24.1.13; Qt 6.11.2. 138 assertions, 0 failures, run twice — protocol handshake, capability set, `--backend auto` resolving `kde-wayland`, per-output display enumeration cross-checked against the observer, focused window, three zones, next/previous cross-display moves with wrap, real accelerator capture, atomic registration with a rejected accelerator, conflict refusal, script and service disconnect/recovery with an unchanged App PID, config retention, and the TUI lifecycle. Three limitations are recorded as named skips: seated accelerator capture, Plasma panel exclusion, negative-coordinate output movement. Seated KDE remains unverified | 2026-09-12 | `./scripts/smoke.sh --keep-logs kde-live`; the Release gate still closes only on a seated KWin 6.x Smoke run |
| Windows | deferred - unverified | Windows desktop session | `windows` CI job compiles and unit-tests `src/windows_window_system.rs` against `windows` 0.58; the Release gate closes with the manual run in `docs/runbooks/windows-smoke.md`. | 2026-09-11 | `windows` job in `.github/workflows/ci.yml`; run `docs/runbooks/windows-smoke.md` to close the Release gate. |

### Current-device checks (2026-10-11)

The observations below describe the automated review before the user's subsequent confirmation that Linux/GNOME 51 is working and usable; they are not a claim that the device is still blocked.

- `./scripts/test.sh`: 98 library tests and 15 CLI tests passed; Rust formatting, Bash/JavaScript syntax checks, doc tests, and Clippy with warnings denied passed.
- GNOME's D-Bus polling regression failed before the fix and passed after it: inspection connections no longer delay the next queued hotkey. The bus subscription now filters ownership changes to the companion service.
- Rebuilt and installed the release binary at `~/.local/bin/window_zones`; installed metadata admits GNOME 50/51 and the installed JavaScript matches the repository. The current Shell keeps cached metadata until a new login.
- On the seated GNOME 51 session, the installed binary selects `gnome-wayland`, reports the missing companion explicitly, and rejects `--backend x11`. TUI reload/restart/status/dispatch/quit and tray Show status/Reload/Restart/Quit remain usable in that unavailable state. The tray advertises the installed `view-grid-symbolic` icon; this checks the menu service and icon property, not a visual panel rendering assertion.
- Synthetic evidence: `/tmp/window_zones-smoke.38841/assertions.log` (final GNOME run), `/tmp/window_zones-smoke.33583/assertions.log` (combined GNOME/X11 run). Seated activation blocker: `/tmp/window_zones-smoke.36436/assertions.log`. These retained temporary paths are local evidence, not portable fixtures.
- Full seated GNOME 51 placement, physical hotkeys, and companion recovery remain unverified. `/dev/uinput` is root-only and unattended `sudo` is unavailable; no device permissions or other extension settings were changed. The default `~/.config/window_zones/config.toml` is absent; use the [GNOME runbook's example bindings](gnome-wayland.md#start-the-application) to configure actions before the seated run.

### Minimum verification

If you only need fast feedback:

```bash
cargo test --locked
```
