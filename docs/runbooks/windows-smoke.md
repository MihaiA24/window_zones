# Windows Smoke Runbook

> **Windows is currently a DEFERRED, non-blocking V1 release gate.** The GitHub
> Actions Windows job proves the Windows code compiles and unit-tests. Run this
> procedure later on a real Windows desktop before changing the matrix to a
> blocking pass or fail.

This procedure uses the real `window_zones.exe`, real Win32 windows, the real
monitor work areas, and real keyboard input. Do not use Wine or a cross-compile
as evidence. Keep screenshots outside the repository; screenshots are
out-of-band evidence and are not committed.

## Prerequisites

- A real Windows 10 or Windows 11 desktop. One monitor is enough for the first
  checks; two independently positioned monitors are required for the display
  check.
- Git, PowerShell, and `rustup`.
- The MSVC linker/build tools required by the
  `stable-x86_64-pc-windows-msvc` Rust toolchain.
- A normal movable, resizable foreground window. The built-in Notepad is used
  below as the example.

From the repository checkout, select the MSVC toolchain:

```powershell
rustup toolchain install stable-x86_64-pc-windows-msvc
rustup default stable-x86_64-pc-windows-msvc
rustc -vV
git --version
```

Record the output of `winver` (the version/build shown by its dialog) and the
exact monitor arrangement before testing:

```powershell
winver
Get-ComputerInfo -Property WindowsProductName,WindowsVersion,OsBuildNumber | Format-List
```

## Build and test configuration

Run these commands from the repository root. The configuration uses only action
names accepted by the current CLI: `move-to-zone`, `move-to-next-display`, and
`move-to-previous-display`.

```powershell
cargo build --release --locked --bin window_zones
$bin = (Resolve-Path ".\target\release\window_zones.exe").Path
$config = Join-Path (Get-Location) "windows-smoke.toml"
$configText = @'
[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "left-half" }

[[bindings]]
hotkey = "Ctrl+Alt+Right"
action = { type = "move-to-zone", zone = "right-half" }

[[bindings]]
hotkey = "Ctrl+Alt+Down"
action = { type = "move-to-next-display" }

[[bindings]]
hotkey = "Ctrl+Alt+Up"
action = { type = "move-to-previous-display" }
'@
[System.IO.File]::WriteAllText($config, $configText, [System.Text.UTF8Encoding]::new($false))
```

The exact CLI forms used below are:

```powershell
& $bin --backend auto --config $config status
& $bin --backend windows --config $config dispatch "Ctrl+Alt+Left"
& $bin --backend windows --config $config run
& $bin --backend windows --config $config tui
& $bin --backend windows --config $config --tray run
```

`dispatch` is useful for a one-shot action, but geometry evidence must be
collected while the target window is actually focused. The interactive `run`
session plus a physical key press is the primary zone/display and hotkey check.

## Geometry observer

Open a second PowerShell window so the application can keep its own console
running in the first window. Paste this observer once in the second window:

```powershell
Add-Type @'
using System;
using System.Runtime.InteropServices;

public static class WindowZonesProbe
{
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT
    {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);

    [DllImport("user32.dll")]
    public static extern IntPtr GetForegroundWindow();
}
'@
Add-Type -AssemblyName System.Windows.Forms

function Get-WzWindowRect([IntPtr] $Handle) {
    $rect = New-Object 'WindowZonesProbe+RECT'
    if (-not [WindowZonesProbe]::GetWindowRect($Handle, [ref] $rect)) {
        throw "GetWindowRect failed for $Handle"
    }
    [pscustomobject]@{
        X = $rect.Left
        Y = $rect.Top
        Width = $rect.Right - $rect.Left
        Height = $rect.Bottom - $rect.Top
    }
}

function Get-WzWorkAreas {
    [System.Windows.Forms.Screen]::AllScreens | ForEach-Object {
        $area = $_.WorkingArea
        [pscustomobject]@{
            DeviceName = $_.DeviceName
            X = $area.X
            Y = $area.Y
            Width = $area.Width
            Height = $area.Height
        }
    }
}
function Get-WzWindowWorkArea([IntPtr] $Handle) {
    $screen = [System.Windows.Forms.Screen]::FromHandle($Handle)
    $area = $screen.WorkingArea
    [pscustomobject]@{
        DeviceName = $screen.DeviceName
        X = $area.X
        Y = $area.Y
        Width = $area.Width
        Height = $area.Height
    }
}
```

Start and identify the test window in the observer PowerShell. Keep its process
ID and handle; querying that handle does not require the observer itself to be
focused:

```powershell
$targetProcess = Start-Process notepad.exe -PassThru
do {
    Start-Sleep -Milliseconds 200
    $target = Get-Process -Id $targetProcess.Id
} while ($target.MainWindowHandle -eq [IntPtr]::Zero)
$target | Select-Object Id,ProcessName,MainWindowTitle,MainWindowHandle
$layout = @(Get-WzWorkAreas)
$layout | Format-Table
$layout | ConvertTo-Json -Compress
Get-WzWindowWorkArea $target.MainWindowHandle
Get-WzWindowRect $target.MainWindowHandle
```

`WorkingArea` is the Win32 work area used by the Windows adapter, excluding
reserved taskbar/dock space. `GetWindowRect` reports the full window frame, the
same rectangle that the adapter reads and moves. Record all `DeviceName`, `X`,
`Y`, `Width`, and `Height` values exactly, including negative coordinates.

For a work area `(x, y, w, h)`, the exact expected built-in zone rectangles
are:

- left half: `(x, y, floor(w / 2), h)`;
- right half: `(x + w - floor(w / 2), y, floor(w / 2), h)`;
- maximize: `(x, y, w, h)`.

Record both the expected rectangle calculated from the recorded work area and
the observed rectangle from `Get-WzWindowRect`. Capture a screenshot after each
successful geometry check, outside the repository.

## Numbered release-gate checks

1. **Startup and backend resolution.** Run:

   ```powershell
   & $bin --backend auto --config $config status
   ```

   The first line must be `Using runtime window backend: windows`. Record the
   complete output, the `winver` result, the Rust toolchain output, and the
   commit or tag tested. A failure to resolve `windows` is a release-gate
   failure.

2. **Capability and status output.** Run the explicit backend status command:

   ```powershell
   & $bin --backend windows --config $config status
   ```

   Record the complete output. It must identify `windows`, show `binding count:
   4`, `config state: Loaded`, and a resolved config path. The Windows adapter
   has no separate compositor capability query; focused-window, display
   enumeration, move/resize, and global-hotkey capability are proven by checks
   3--5 and by the runtime's observable `Registered` hotkey state.

3. **Zone move and exact geometry.** Start the interactive real backend:

   ```powershell
   & $bin --backend windows --config $config run
   ```

   Wait for `Interactive session started`; a successful initial registration has
   no success log line, so record the absence of a `Hotkey registration
   initially failed` diagnostic. In the observer window, focus Notepad and
   capture its current monitor work area and frame rectangle. While Notepad is
   focused, press the real `Ctrl+Alt+Left` keys. After each zone key press, run
   these observer commands:

   ```powershell
   Get-WzWindowWorkArea $target.MainWindowHandle
   Get-WzWindowRect $target.MainWindowHandle
   ```

   The observed frame must equal the current monitor's left-half rectangle,
   including exact integer `X`, `Y`, `Width`, and `Height`. Repeat with
   `Ctrl+Alt+Right` and compare with the right-half formula. Record the before,
   expected, and observed rectangles and retain an external screenshot for each
   zone.

4. **Move to another display with two monitors.** Arrange two monitors in
   Windows Display Settings and record the exact work-area layout from
   `Get-WzWorkAreas`, including each device name and any negative global
   coordinates. Focus Notepad on one monitor and first put it in the left half
   with `Ctrl+Alt+Left`. Press the real `Ctrl+Alt+Down` key. The window must
   move to the next enumerated display and remain in that display's left-half
   rectangle; identify the target display from the new `Screen.FromHandle`
   work area and compare exact integers. Press `Ctrl+Alt+Up` to return and
   record the source/target displays and both expected/observed rectangles.
   In the observer PowerShell, run the same two commands again to record the
   target display and the observed frame:

   ```powershell
   Get-WzWindowWorkArea $target.MainWindowHandle
   Get-WzWindowRect $target.MainWindowHandle
   ```

   This proves display movement against two real monitor work areas, rather
   than relying on the companion's self-reported geometry.

5. **Global hotkey registration and a real key press.** In the `run` output,
   record the absence of an initial registration failure. With Notepad focused,
   press each configured combination physically (at minimum `Ctrl+Alt+Left` and
   `Ctrl+Alt+Down`), not by typing the `dispatch` command. Each press must
   produce the configured move and the session must remain alive. Record the
   exact key sequence, the resulting `Dispatch state: Succeeded` output, and
   the observed rectangle. Return to the application console and type `status`
   to record the runtime status, including `hotkey state: Registered`.

6. **Config reload atomicity with an invalid config.** Leave `run` active. In
   the observer PowerShell, replace the file with invalid TOML and wait at least
   one second for the file watcher:

   ```powershell
   [System.IO.File]::WriteAllText($config, "[bindings", [System.Text.UTF8Encoding]::new($false))
   Start-Sleep -Seconds 1
   ```

   Return to the application console and type `reload`, then `status`. Record
   the `Config reload error`/`Config state: Error(...)` output and confirm that
   the binding count and last valid bindings remain unchanged. Refocus Notepad
   and press the previously valid `Ctrl+Alt+Right`; it must still dispatch.
   Restore the original `$configText`, wait one second, type `reload`, and type
   `status`:

   ```powershell
   [System.IO.File]::WriteAllText($config, $configText, [System.Text.UTF8Encoding]::new($false))
   Start-Sleep -Seconds 1
   ```

   Record the transition back to `Config state: Loaded` and successful hotkey
   registration. This check fails if an invalid edit discards the last valid
   bindings or leaves a partially applied set.

7. **TUI lifecycle.** Type `quit` in the `run` session and start the TUI with:

   ```powershell
   & $bin --backend windows --config $config tui
   ```

   Record the initial dashboard and then enter these exact runtime commands,
   one per line:

   ```text
   reload
   restart
   status
   dispatch Ctrl+Alt+Left
   quit
   ```

   The dashboard must stay alive through `reload`, `restart`, `status`, and
   `dispatch Ctrl+Alt+Left`, update its config/hotkey/last-action fields, and
   close cleanly with `Session closed.` after `quit`. Record the terminal
   transcript. The `dispatch` line checks the TUI command path; geometry proof
   comes from the focused-window hotkey checks above.

8. **Tray behavior.** Start the tray surface:

   ```powershell
   & $bin --backend windows --config $config --tray run
   ```

   Record `Tray menu started`. Open the Window Zones tray menu and exercise
   `Show status`, `Reload`, `Restart`, and `Quit`. Confirm status output reflects
   the current config, reload and restart return to a live tray session, and
   `Quit` prints `Session closed.` and exits. Record whether the icon is visible,
   the exact menu labels, each observed result, and any Windows notification or
   shell error.

When a check fails, preserve the command output, the exact monitor layout,
the observed/expected rectangles, and an external screenshot. Do not change
product code as part of this run; report the defect instead.

## Results to paste into `docs/runbooks/testing.md`

Replace the placeholders after the manual run. Keep `deferred - unverified`
until all required evidence is captured; use `blocking - pass` or
`blocking - fail` only after the run is complete.

```markdown
| Gate | Status | Environment | Verified configuration | Date | How to reproduce |
| Windows | deferred - unverified | Windows <10/11 build from winver>; <one/two-monitor layout> | stable-x86_64-pc-windows-msvc; release binary; <commit/tag>; <observed geometry and hotkey evidence> | <YYYY-MM-DD> | Follow `docs/runbooks/windows-smoke.md`; `cargo build --release --locked --bin window_zones`, then the exact commands in the checklist |
```
