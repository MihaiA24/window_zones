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
- `bash -n scripts/*.sh` (shell syntax, including the 2000-line Smoke harness)
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

This matrix is the record of record for V1 Release gate status. GNOME Wayland and TUI lifecycle (GNOME) remain `pending`; Linux X11 and TUI lifecycle (X11) are `blocking - pass`; KDE Plasma Wayland and Windows are `deferred - unverified`. Release gate status is exactly one of `pending`, `blocking - pass`, `blocking - fail`, or `deferred - unverified`.

Run the GNOME Wayland or Linux X11 Smoke run with:
```bash
./scripts/smoke.sh gnome       # headless Synthetic GNOME session: Companion load, Capability negotiation, display enumeration, usable-area math
./scripts/smoke.sh gnome-live  # seated live GNOME session: focused window, placement, real accelerator capture, Companion disconnect/recovery, TUI lifecycle
./scripts/smoke.sh x11
```
The TUI lifecycle Smoke runs cover launch, reload, dispatch, restart, and quit in each corresponding Synthetic or seated session. `gnome-live` needs the Companion already active on the live session bus: GNOME activates extensions only at session start, so install it and then log out and back in before running it.

| Gate | Status | Environment | Verified configuration | Date | How to reproduce |
|---|---|---|---|---|---|
| GNOME Wayland | pending | Synthetic session: `gnome-shell --headless` with virtual monitors 1920x1080 and 1600x900, isolated dconf/data/runtime dirs on a private session bus; seated live GNOME session for the seat-dependent half | Synthetic half passes on CachyOS, kernel 7.2.4-1-cachyos, GNOME Shell 50.4 (mutter 50.4): 23 assertions, 0 failures, run twice — companion bus ownership, `GetCapabilities`, both displays with the 32px panel excluded (`display-1 0,32,1920,1048`, `display-2 1920,0,1600,900`), config reload/retention/recovery. Six seat-dependent checks are recorded as skipped, not passed: focused window, zone placement, cross-display move, atomic hotkey registration, hotkey signal, companion disconnect/recovery. Seated half pending. | 2026-09-11 | `./scripts/smoke.sh gnome` and `./scripts/smoke.sh gnome-live` |
| Linux X11 | blocking - pass | Xvfb 3520x1080 split by `xrandr --setmonitor` into logical monitors 1920x1080 and 1600x1080, Openbox as EWMH window manager, `gnome-text-editor` as the test window | CachyOS, kernel 7.2.4-1-cachyos; X.Org X Server 1.21.1.24 (xorg-server 21.1.24-1.1); Openbox 3.6.1; xdotool 4.20260303.1 | 2026-09-11 | `./scripts/smoke.sh x11` — 42 assertions, 0 failures, run twice |
| TUI lifecycle (GNOME) | pending | Same Synthetic session; seated live GNOME session for hotkey-driven lifecycle | Synthetic half passes in the same run: TUI launch, `reload`, `restart`, `status`, `dispatch HOTKEY`, `quit`. Seated half pending. | 2026-09-11 | `./scripts/smoke.sh gnome` and `./scripts/smoke.sh gnome-live` |
| TUI lifecycle (X11) | blocking - pass | Same Xvfb + Openbox Synthetic session | Same as Linux X11; `reload`, `restart`, `status`, `dispatch HOTKEY`, `quit` all asserted | 2026-09-11 | `./scripts/smoke.sh x11` |
| KDE Plasma Wayland | deferred - unverified | KDE Plasma Wayland with KWin 6.x | unverified | — | Run a Smoke run on a real KWin 6.x session; this closes the Release gate. |
| Windows | deferred - unverified | Windows desktop session | `windows` CI job compiles and unit-tests `src/windows_window_system.rs` against `windows` 0.58; the Release gate closes with the manual run in `docs/runbooks/windows-smoke.md`. | 2026-09-11 | `windows` job in `.github/workflows/ci.yml`; run `docs/runbooks/windows-smoke.md` to close the Release gate. |

### Minimum verification

If you only need fast feedback:

```bash
cargo test --locked
```
