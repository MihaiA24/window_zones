# Testing Runbook

```bash
./scripts/test.sh
```

What this verifies:

- `cargo fmt --all -- --check` (formatting)
- `cargo test --locked` (unit + integration tests)
- `cargo test --locked --doc` (doc tests)
- `cargo clippy --locked --all-targets --all-features` (only if clippy is installed)


## Current repository status

The repository test script covers formatting, unit/integration tests, doc tests,
and clippy when installed. The GNOME and KDE adapter contract tests start
private `dbus-daemon` instances; they do not require a running compositor
session.

The compositor scripts can be syntax-checked with:

```bash
node --check gnome-extension/extension.js
node --check kwin-script/contents/code/main.js
```
Merge-ready V1 additionally requires:

- D-Bus contract tests using fake compositor services;
- one manual GNOME Wayland smoke run;
- one manual KDE Plasma Wayland smoke run;
- one manual Windows and one manual X11 smoke run;
- TUI launch, reload, dispatch, restart, and quit smoke coverage.

### Minimum verification
If you only need fast feedback:

```bash
cargo test --locked
```
