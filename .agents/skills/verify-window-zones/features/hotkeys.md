# Global hotkeys and dispatch

Users bind accelerators to positioning actions and trigger them from the desktop or a runtime command; failures and recovery must remain visible without silently changing backends.

## Sub-features

- `hotkey-global`: a real accelerator reaches the running app and moves the focused OS window.
- `hotkey-input`: one-shot dispatch, interactive `dispatch` and bare-hotkey input normalize equivalent hotkeys.
- `hotkey-registration`: startup/config changes register the new set atomically; conflicts/disconnects expose actionable errors and recover.
- `hotkey-unbound`: malformed/unbound input reports failure without moving a window.

## How to get to it (user POV)

- Press the configured accelerator with `run`, `tui`, or tray runtime active.
- Invoke `scripts/run.sh … dispatch HOTKEY`.
- Type `dispatch HOTKEY` or the bare hotkey into the CLI loop/TUI; use modifier/case aliases supported by the config.
- Change bindings in TOML and reload; reconnect/re-enable a missing compositor companion on a supported real desktop.

## Driving it with verify

Preconditions:

- Baseline/current build; real accelerator proof needs a focused native test window and an input-capable compositor fixture.

- **One-shot dispatch.** `"$V" drive cli --expect 'Dispatch state: Succeeded' -- dispatch Ctrl+Alt+Left` exercises modifier normalization through the user command.
- **Interactive explicit/bare entries.** `"$V" drive session --mode run --command 'dispatch Ctrl+Alt+Left' --expect 'Dispatch state: Succeeded' --command 'Alt+Ctrl+Up' --expect 'Dispatch state: Succeeded'`. Repeat in TUI with `Last action: alt+ctrl+left` and `Last action: alt+ctrl+up` expectations. Include backend/config variants when affected.
- **Unbound/error.** `"$V" drive cli --expect 'NoBindingForHotkey' -- dispatch Alt+Ctrl+F12`; `"$V" drive session --mode tui --command 'dispatch Alt+Ctrl+F12' --expect 'no binding configured'` must show an error. Capture unchanged native geometry when making the no-movement claim.
- **Empty.** `"$V" drive config empty`, then dispatch Left expecting `NoBindingForHotkey`. Restore with `drive config valid`; a new process must successfully dispatch Left again.
- **Real accelerator.** `"$V" drive smoke x11` injects XTEST input into its isolated X11 server and independently asserts geometry and `last action`. `"$V" drive smoke kde-live` exercises the native KWin companion in a synthetic session. A dispatch command alone is not evidence of global input capture.
- **Registration/config persistence.** `"$V" drive session --mode run --journey reload` observes replacement/retention/recovery; `drive tests` runs the repo's existing private-D-Bus contract tests as supplemental checks. Reopen with valid on-disk bindings and dispatch again.
- **Cancel.** Exit with each affected `--quit` variant before accelerator injection; verify owned runtime/input listeners end and later accelerators do not dispatch through the exited app.

## Gotchas

- Interactive app dry-run still constructs the global listener. The helper removes the seated display and bus, so dry-run sessions are not real hotkey-capture proof and must not be attached to the user's desktop.
- Seatless GNOME rejects accelerators and has no focused window; its skips are failed preconditions for a global-hotkey journey. Seated GNOME needs an enabled companion and writable `/dev/uinput`; Xwayland XTEST is not equivalent to compositor accelerator input. Follow `docs/runbooks/gnome-wayland.md` on an explicitly controlled real target; the verifier refuses the shared-session `gnome-live` route.
- No GNOME conflict/disconnect, tray runtime, Windows or macOS proof follows from an X11 pass. If affected, obtain those actual environments and user paths; record what is missing instead of using internal signals/setters.
- Dispatch/config error states may still exit 0. Match state and diagnostics, and observe side effects separately.
