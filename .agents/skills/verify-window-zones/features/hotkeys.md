# Global hotkeys and dispatch

Users bind accelerators to positioning actions and trigger them from the desktop or a runtime command; failures and recovery must remain visible without silently changing backends.

## Sub-features

- `hotkey-global`: a real accelerator reaches the running app and moves the focused OS window.
- `hotkey-input`: one-shot dispatch, interactive `dispatch` and bare-hotkey input normalize equivalent hotkeys.
- `hotkey-registration`: startup/config changes register the new set atomically; conflicts/disconnects expose actionable errors and recover.
- `hotkey-unbound`: malformed/unbound input reports failure without moving a window.
- `hotkey-layout`: X11 grabs follow keyboard/modifier-map changes, including actual CapsLock/NumLock slots; failed remapping releases obsolete grabs.
- `hotkey-native-recovery`: Windows preserves AltGr typing, reconciles missed releases and replaces its single owned hook on restart; macOS waits for native tap readiness.

## How to get to it (user POV)

- Press the configured accelerator with `run`, `tui`, or tray runtime active.
- Invoke `scripts/run.sh … dispatch HOTKEY`.
- Type `dispatch HOTKEY` or the bare hotkey into the CLI loop/TUI; use modifier/case aliases supported by the config.
- Change bindings in TOML and reload; reconnect/re-enable a missing compositor companion on a supported real desktop.

## Driving it with verify

Preconditions:

- Baseline/current build; real accelerator proof needs a focused native test window and an input-capable compositor fixture.

- **One-shot dispatch.** `"$V" drive cli --expect 'Dispatch state: Succeeded' -- dispatch Ctrl+Alt+Left` exercises modifier normalization through the user command.
- **Interactive explicit/bare entries.** `"$V" drive session --mode run --command 'dispatch Ctrl+Alt+Left' --expect 'Dispatch state: Succeeded' --command 'Alt+Ctrl+Up' --expect 'Dispatch state: Succeeded'`. Repeat in TUI with `Last action: alt+ctrl+left` and `Last action: alt+ctrl+up` expectations. Every TUI dispatch success also requires a completed response/prompt without `Last error: dispatch:`; this applies to custom commands as well as built-in journeys. Include backend/config variants when affected.
- **Unbound/error.** `"$V" drive cli --expect-exit 0 --expect 'NoBindingForHotkey' -- dispatch Alt+Ctrl+F12`; `"$V" drive session --mode tui --command 'dispatch Alt+Ctrl+F12' --expect 'Last error: dispatch: no binding configured'` explicitly asserts the negative dispatch path. A `Last action` expectation must reject that same failed result. Capture unchanged native geometry when making the no-movement claim.
- **Empty.** `"$V" drive config empty`, then dispatch Left expecting `NoBindingForHotkey`. Restore with `drive config valid`; a new process must successfully dispatch Left again.
- **Real accelerator.** `"$V" drive smoke x11` injects XTEST input into its isolated X11 server and independently asserts geometry and `last action`. `"$V" drive smoke kde-live` exercises the native KWin companion in a synthetic session. A dispatch command alone is not evidence of global input capture.
- **Registration/config persistence.** `"$V" drive session --mode run --journey reload` observes replacement/retention/recovery; `drive tests` runs the repo's existing private-D-Bus contract tests as supplemental checks. Reopen with valid on-disk bindings and dispatch again.
- **Cancel.** Exit with each affected `--quit` variant before accelerator injection; verify owned runtime/input listeners end and later accelerators do not dispatch through the exited app.
- **GNOME controller isolation.** A second private-bus client may inspect but cannot replace another controller's grabs or receive its `HotkeyPressed` events. Disconnecting the controller releases its set; a new controller can then register.
- **GNOME atomic replacement.** The seated rollback fixture records controller-unicast events on its registering connection before and after rejecting a replacement; require an event in each phase and at least two total. An independent bus monitor is not the controller's receiver.
- **Compositor-bound dispatch.** On Sway/Hyprland, start `run`/`tui` with the same config pathname and compositor connection as its clients (`SWAYSOCK`, or `HYPRLAND_INSTANCE_SIGNATURE` plus runtime namespace). Verify shared Restore history, symlink retarget/reload/restart, refusal of another instance or replaced socket, and queued-action expiration. A replaced compositor connection requires closing and relaunching the session, not only in-process `restart`.
- **X11 mapping/work areas.** `check e2e` runs the ignored `x11_real_server*` regressions on its owned display. Move Alt/Super and NumLock to different modifier slots, switch US/German keyboard groups with distinct Y/Z bindings, and exercise shifted symbols only there; require intended accelerators and no stale exclusive grabs. A refused remap must release capture and report unavailability.
- **KWin reset/ownership.** On a private bus, reset the script during pending registration and require retry/recovery, not permanent rejection. Failed automatic restoration must report unavailable capture to the owning controller until registration succeeds; unrelated clients cannot claim ownership or consume recovery. A second companion must exit 75 without queueing for the service name.
- **Windows typing/recovery.** Follow the Windows native runbook: Polish/German AltGr input, secure-desktop return, ordinary editing keys, held-key release order and real hook removal followed by `restart`. Cross-target checks and synthetic event delivery do not prove these paths.

## Gotchas

- App dry-run uses no-op hotkey capture, so its sessions do not prove global input. The verifier still isolates display/bus and HOME/XDG because tray, config and logging remain separate side effects.
- The synthetic GNOME gate supplies native accelerators through Mutter's RemoteDesktop interface; it is not physical-input proof. Seated GNOME needs an enabled companion and authorized `/dev/uinput`; Xwayland XTEST is not equivalent. The verifier refuses the shared-session `gnome-live` route.
- No GNOME conflict/disconnect, tray runtime, Windows or macOS proof follows from an X11 pass. If affected, obtain those actual environments and user paths; record what is missing instead of using internal signals/setters.
- Dispatch/config error states may still exit 0. Match state and diagnostics, and observe side effects separately.
- TUI `Last action` records the requested hotkey even when dispatch fails. The verifier checks the completed response's `Last error`, not the action label alone; custom negative dispatch journeys must explicitly expect `Last error: dispatch: …`. Native success still requires independent post-TUI geometry.
