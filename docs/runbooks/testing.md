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

`cargo test --locked` passes 80 tests across the library and binary suites; the doc-test suite has no tests. The full `./scripts/test.sh` verification now passes formatting, tests, doc tests, and clippy with warnings denied.

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
