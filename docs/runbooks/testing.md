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

This matrix is the record of record for V1 Release gate status. The four
Synthetic session rows remain `pending` until the parent records the Smoke run
results. Final Release gate status uses `blocking - pass`, `blocking - fail`,
or `deferred - unverified`.

Run the GNOME Wayland or Linux X11 Smoke run with:

```bash
./scripts/smoke.sh gnome
./scripts/smoke.sh x11
```
The TUI lifecycle Smoke runs cover launch, reload, dispatch, restart, and quit
in each corresponding Synthetic session.

| Gate | Status | Environment | Verified configuration | Date | How to reproduce |
|---|---|---|---|---|---|
| GNOME Wayland | pending | GNOME Shell Wayland Synthetic session | pending | pending | `./scripts/smoke.sh gnome` |
| Linux X11 | blocking - pass | Xvfb 3520x1080 split by `xrandr --setmonitor` into logical monitors 1920x1080 and 1600x1080, Openbox as EWMH window manager, `gnome-text-editor` as the test window | CachyOS, kernel 7.2.4-1-cachyos; X.Org X Server 1.21.1.24 (xorg-server 21.1.24-1.1); Openbox 3.6.1; xdotool 4.20260303.1 | 2026-09-11 | `./scripts/smoke.sh x11` — 42 assertions, 0 failures, run twice |
| TUI lifecycle (GNOME) | pending | GNOME Shell Wayland Synthetic session | pending | pending | `./scripts/smoke.sh gnome` |
| TUI lifecycle (X11) | blocking - pass | Same Xvfb + Openbox Synthetic session | Same as Linux X11; `reload`, `restart`, `status`, `dispatch HOTKEY`, `quit` all asserted | 2026-09-11 | `./scripts/smoke.sh x11` |
| KDE Plasma Wayland | deferred - unverified | KDE Plasma Wayland with KWin 6.x | unverified | — | Run a Smoke run on a real KWin 6.x session; this closes the Release gate. |
| Windows | deferred - unverified | Windows desktop session | unverified | — | Run the manual verification in `docs/runbooks/windows-smoke.md`; the `windows-latest` CI job holds the compile line meanwhile. This closes the Release gate. |

### Minimum verification

If you only need fast feedback:

```bash
cargo test --locked
```
