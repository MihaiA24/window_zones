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
- `bash -n scripts/*.sh` (shell syntax, including the 3800-line Smoke harness)
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

This matrix is the record of record for V1 Release gate status. Both blocking gates, GNOME Wayland and Linux X11, are `blocking - pass` together with their TUI lifecycle rows; KDE Plasma Wayland and Windows are `deferred - unverified`. Release gate status is exactly one of `pending`, `blocking - pass`, `blocking - fail`, or `deferred - unverified`. macOS, Sway, and Hyprland are ungated integrations and have no row. Every Smoke run ends with `Smoke assertions (<gate>): N; passed: P; failed: F; skipped: S`.

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
- `/dev/uinput` must be writable by the user running the gate, because real accelerator capture is evidenced with a kernel virtual keyboard. Xwayland XTEST injection is not delivered to compositor accelerator grabs on GNOME 50, so `xdotool` cannot carry this check; with `/dev/uinput` unavailable the gate fails immediately and prints the remediation. Grant it for the run with `sudo modprobe uinput && sudo chmod 0666 /dev/uinput`, or persistently with a udev rule such as `KERNEL=="uinput", MODE="0660", GROUP="input"` plus membership in `input`.

`kde-live` needs the `kwin` package (`kwin_wayland`, `kpackagetool6`, `kwriteconfig6`) and nothing from the user's session: it builds its own private bus, XDG tree, KWin configuration, and compositor. It runs KWin on its `--virtual` framebuffer backend with two 1200x900 outputs; a nested windowed backend was tried first and rejected because the outer compositor resizes those output windows mid-run. It is Synthetic session evidence whatever it reports: KWin runs a `Session::Type::Noop` session and never owns the login seat, so it cannot close the KDE Release gate — only a Smoke run on a seated KWin session does that.

The GNOME Wayland, TUI lifecycle (GNOME), and KDE Plasma Wayland rows below were recorded before ADR 0008 (executor-owned correlation, mandatory `/dev/uinput`, KWin accelerator preflight); re-run `gnome`, `gnome-live`, and `kde-live` on the seated host and re-record them before release.

| Gate | Status | Environment | Verified configuration | Date | How to reproduce |
|---|---|---|---|---|---|
| GNOME Wayland | blocking - pass | Synthetic session: `gnome-shell --headless` with virtual monitors 1920x1080 and 1600x900, isolated dconf/data/runtime dirs on a private session bus. Seated session: the live GNOME session with two physical monitors (HDMI-1 2560x1440 at 0,0 and DP-1 3440x1440 at 2560,0), `gnome-text-editor` on XWayland as the test window, `xdotool` as the independent observer, and a `/dev/uinput` virtual keyboard as the input source | CachyOS, kernel 7.2.4-1-cachyos; GNOME Shell 50.4; mutter 50.4. Synthetic: 23 assertions, 0 failures, run twice — Companion bus ownership, `GetCapabilities`, both virtual displays with the 32px panel excluded, config reload/retention/recovery; six seat-dependent checks recorded as skipped. Seated: 72 assertions, 0 failures, run twice — focused-window discovery agreeing with `xdotool` to the pixel, left-half `0,0,1280,1440`, center-third `853,0,854,1440`, left-two-thirds `0,0,1707,1440`, cross-display move to `2560,32,2294,1408` on `display-2` and back, real accelerator capture through the kernel virtual keyboard, atomic registration with a rejected set preserving the previous one, Companion disconnect and recovery without an App restart, and invalid-config reload retaining the last valid bindings | 2026-09-11 | `./scripts/smoke.sh gnome` and `./scripts/smoke.sh gnome-live` |
| Linux X11 | blocking - pass | Xvfb 3520x1080 split by `xrandr --setmonitor` into logical monitors 1920x1080 and 1600x1080, Openbox as EWMH window manager, `gnome-text-editor` as the test window, `xdotool` + `xprop _NET_FRAME_EXTENTS` as the frame observer | Debian 12 container (`rust:1-bookworm`, kernel 7.0.14-orbstack aarch64); Xvfb from xorg-server 21.1.7; Openbox 3.6.1; xdotool 3.20160805.1. 56 assertions, 0 failures — frame-geometry zones on monitor 1, next/previous display with wrap, dispatch from a decorated frame at `1930,10` on monitor 2 landing on `1920,0,800,1080`, real `rdev` accelerator capture, invalid-config retention and recovery, TUI lifecycle. Previous record (CachyOS, X.Org 21.1.24, 2026-09-11, 42 assertions) predates the frame-geometry and monitor-2 assertions | 2026-09-12 | `./scripts/smoke.sh x11` |
| TUI lifecycle (GNOME) | blocking - pass | Same Synthetic session and same seated live GNOME session | TUI launch, `reload`, `restart`, `status`, `dispatch HOTKEY`, and `quit` asserted in both halves; the seated run also asserts the dashboard reports the dispatched binding | 2026-09-11 | `./scripts/smoke.sh gnome` and `./scripts/smoke.sh gnome-live` |
| TUI lifecycle (X11) | blocking - pass | Same Xvfb + Openbox Synthetic session | Same as Linux X11; `reload`, `restart`, `status`, `dispatch HOTKEY`, `quit` all asserted | 2026-09-12 | `./scripts/smoke.sh x11` |
| KDE Plasma Wayland | deferred - unverified | Synthetic session: KWin 6.7.5 on its `--virtual` backend with two 1200x900 outputs and its own Xwayland, the repository KWin script installed with `kpackagetool6` into a private XDG tree, the Rust companion started before KWin on a private bus, `gnome-text-editor` on Xwayland as the test window, an independent KWin observer script reading per-output `MaximizeArea`, and XTEST-to-EIS key injection. Seated KDE session: not run | Nested configuration only: CachyOS, kernel 7.2.4-1-cachyos; KWin 6.7.5; Xwayland 24.1.13; Qt 6.11.2. 138 assertions, 0 failures, run twice — protocol handshake, capability set, `--backend auto` resolving `kde-wayland`, per-output display enumeration cross-checked against the observer, focused window, three zones, next/previous cross-display moves with wrap, real accelerator capture, atomic registration with a rejected accelerator, conflict refusal, script and service disconnect/recovery with an unchanged App PID, config retention, and the TUI lifecycle. Three limitations are recorded as named skips: seated accelerator capture, Plasma panel exclusion, negative-coordinate output movement. Seated KDE remains unverified | 2026-09-12 | `./scripts/smoke.sh --keep-logs kde-live`; the Release gate still closes only on a seated KWin 6.x Smoke run |
| Windows | deferred - unverified | Windows desktop session | `windows` CI job compiles and unit-tests `src/windows_window_system.rs` against `windows` 0.58; the Release gate closes with the manual run in `docs/runbooks/windows-smoke.md`. | 2026-09-11 | `windows` job in `.github/workflows/ci.yml`; run `docs/runbooks/windows-smoke.md` to close the Release gate. |

### Minimum verification

If you only need fast feedback:

```bash
cargo test --locked
```
