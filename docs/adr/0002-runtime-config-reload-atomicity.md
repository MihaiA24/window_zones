# ADR 0002: Preserve last-known bindings on runtime config reload errors

Date: 2026-07-14

## Status

Accepted

## Context

Reloading configuration while the app is running is the first step toward live config updates and robust launcher integration.

When config reload is added, temporary user edits can introduce transient `read`, `parse`, or `validation` failures.

If each failure discards existing bindings, a single typo can turn a functioning app into an unusable state until manually restarted.

`App` already models startup outcomes with `ConfigState` (`Missing`/`Loaded`/`Error`), but startup and runtime refresh needed a deterministic contract for retries and safe transitions.

## Decision

Adopt all-or-nothing reload semantics for bindings:

- Keep `App::reload_config(&mut self) -> &ConfigState` as the runtime entry point for explicit refresh.
- Keep `start_at(path)` initialization simple: set path + `ConfigState::Missing`, then call `reload_config()`.
- Add a private `load_and_normalize_config(path)` helper that performs:
  - file read (or not-found handling),
  - TOML parse,
  - binding validation/normalization.
- On `Ok(Some(config))`, atomically replace `self.config` and set `ConfigState::Loaded`.
- On missing file, reset to `AppConfig::default()` and set `ConfigState::Missing`.
- On any `ConfigLoadError`, set `ConfigState::Error(...)` and **do not change `self.config`** (retain last valid bindings).

## Consequences

- Users can safely retry after malformed edits: the app stays usable with previous bindings.
- `config_state()` becomes the authoritative readout for the latest load attempt,
  while `config()` remains stable unless a full reload succeeds.
- The runtime contract is now safe for future watcher-driven integrations and retry loops.
- Unit tests now cover: successful reload replacement, and retaining last-known-good bindings after parse/validation failure.

## Rejected alternatives

- Wiping `config` on any reload error (reduces fault tolerance).
- Applying partial updates during parse/validation (causes inconsistent runtime behavior on malformed input).
- Returning `Result` from `reload_config` instead of updating `ConfigState` (would duplicate state exposure semantics).
