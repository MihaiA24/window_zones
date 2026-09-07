# Branch Notes: `feat/global-hotkey-backend`

## Branch
- Name: `feat/global-hotkey-backend`
- Latest commit: `645f9e6`
- Remote: `origin/feat/global-hotkey-backend`

## Summary
- Added native Wayland hotkey integration (Sway path) and runtime lifecycle handling.
- Added runtime fallback from native listener to CLI mode on startup and dispatch-time failures.
- Added support file `src/wayland_hotkey_system.rs` and native hotkey dispatcher (`src/native_hotkey_system.rs`).
- Updated docs/runbook to explain Wayland async/native behavior and fallback.

## Current PR status
- Pull request not yet created in this environment due API auth limits.
- Head branch is already pushed and ready.

### Create PR
1. Open: https://github.com/MihaiA24/window_zones/compare/main...feat/global-hotkey-backend?expand=1
2. Title: `feat: add native wayland hotkey backend with lifecycle fallback`
3. Description body:

```text
## Summary
- Add native Wayland hotkey backend support via Sway (`swaymsg`), including bind registration and event dispatch.
- Add listener lifecycle handling in Wayland hotkey adapter (process/thread shutdown + binding cleanup on drop).
- Add atomic hotkey re-registration with rollback and start/fallback handling for live dispatch mode.
- Wire CLI fallback if native listener becomes unavailable.
- Update runbook documentation for Wayland/native behavior.

## Verification
- `cargo test -- --nocapture`

## Notes
- Native Hyprland support remains intentionally unsupported in this branch with an explicit error message.
```

## Follow-up
- Track broader Wayland compositor support, especially Hyprland, as a follow-up task/issue.
