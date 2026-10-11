# ADR 0007: V1 release gates and Synthetic session verification

Date: 2026-09-11

## Status

Accepted

## Context

ADR 0003 established compositor-specific routing for native Wayland support, but its original merge-ready gate list predates the completed verification work and treats every implemented integration as equally release-blocking. V1 needs an explicit distinction between integrations that block a release and integrations that ship implemented but unverified.

A user desktop session is also a poor verification fixture. The available GNOME session has one physical monitor, enabled tiling extensions can interfere with move and resize behavior, and Wayland extension changes require a logout because extensions cannot be reloaded in place.

## Decision

V1 has two blocking Release gates:

- GNOME Wayland.
- Linux X11.

KDE Plasma Wayland and Windows are implemented but deferred and explicitly non-blocking. The KDE Release gate closes when a Smoke run passes on a real KWin 6.x session. The Windows Release gate closes when the manual run in `docs/runbooks/windows-smoke.md` passes; the `windows-latest` GitHub Actions job holds the compile line in the meantime. macOS remains source-compatible and non-blocking, and Sway and Hyprland remain constrained integrations.

Verification uses Synthetic sessions for repeatable compositor checks and seated
sessions for the user's installed integration, physical displays, and input devices:

- Linux X11 runs entirely in a Synthetic session: `Xvfb` with `openbox`, with `xrandr --setmonitor` splitting the screen into two logical monitors.
- GNOME's Synthetic session uses `gnome-shell --headless`, two virtual monitors, private settings/data/runtime directories, and a private session bus. Mutter's supported RemoteDesktop API supplies virtual keyboard and pointer devices. Real GTK Wayland/XWayland clients, Companion geometry, and ScreenCast/PipeWire frames provide focus, placement, accelerator, lifecycle, and visual evidence. The seated gate still checks the user's actual installation and hardware.

The earlier headless harness created no virtual input devices and therefore could not focus clients or capture accelerators. GNOME 51 checks with compositor-provided virtual devices supersede that limitation; they are not physical-input evidence. The legacy Shell screenshot method remains access-controlled, so the Synthetic gate uses the supported ScreenCast API instead of enabling Shell unsafe mode.

Each Smoke run uses the real binary against the real compositor or window manager and asserts observable geometry with exact integer values. The out-of-band observer differs per gate: X11 uses `xdotool` against the X server plus an `ffmpeg`/`x11grab` capture, and the seated GNOME gate uses an XWayland test client observed through `xdotool`, because the GNOME Shell screenshot API refuses non-portal callers even in a seated session. A GTK client under XWayland owns its shadow, so the observer insets the X geometry by `_GTK_FRAME_EXTENTS` before comparing it with the frame the Companion reports.

The Synthetic GNOME gate injects through Mutter, not Xwayland XTEST. The existing seated gate uses physical key presses or an already-authorized `/dev/uinput` virtual keyboard. XTEST alone is not evidence of GNOME compositor accelerator delivery, and no gate should make an input device world-writable.

Together these evidence Companion protocol and capability behavior, X11 EWMH and RandR behavior, geometry and Usable area calculations, hotkey capture, and Companion disconnect and recovery. They cannot evidence display hardware beyond the two monitors on the verification host, nor compositor versions other than the recorded Verified configuration.

## Consequences

- The V1 release claim is end-to-end verification of the GNOME Wayland and Linux X11 blocking Release gates, X11 entirely in a Synthetic session and GNOME across a Synthetic session plus the seated live session, including the TUI lifecycle checks recorded in the matrix.
- V1 does not claim that KDE Plasma Wayland or Windows are verified; both ship as implemented, deferred, and non-blocking until their specified triggers pass.
- Synthetic sessions provide repeatable compositor and geometry evidence without claiming hardware coverage or full-desktop layout behavior.
- A nested `kwin_wayland` gate (`./scripts/smoke.sh kde-live`) exercises a real KWin against the real Companion, but a nested compositor runs `Session::Type::Noop` and does not own the login seat, so its result is Synthetic session evidence and leaves the KDE Release gate `deferred - unverified`.
- `docs/runbooks/testing.md` is the record of record for Release gate status, Verified configuration, dates, and reproduction commands.

## Rejected alternatives

- Installing full Plasma or maintaining a KDE VM was rejected because no KWin session is available on this machine and installing a separate desktop or VM solely for this verification was not accepted.
- Building a Windows VM or running the binary under wine was rejected because no Windows runtime is available and the verification contract requires a real Windows run; the manual runbook and `windows-latest` compile job are the interim path.
- Verifying only on the live GNOME session was rejected because its tiling extensions interfere with placement. The seated gate is run with `enabled-extensions` narrowed to the Companion, an operator step recorded in `docs/runbooks/gnome-wayland.md`; the gate itself asserts the extension state it found is the extension state it leaves.
