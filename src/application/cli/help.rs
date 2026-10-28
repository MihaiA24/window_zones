pub(in crate::application) fn print_help() {
    println!("window_zones: move and resize the focused window into zones with hotkeys");
    println!("Usage:");
    println!(
        "  window_zones [--config <path>] [--backend <auto|x11|wayland|windows|macos|dry-run>] status"
    );
    println!(
        "  window_zones [--perf-log <path>] [--config <path>] [--backend <...>] dispatch <HOTKEY>"
    );
    println!("  window_zones [--tray] [--perf-log <path>] [--config <path>] [--backend <...>] run");
    println!("  window_zones [--perf-log <path>] [--config <path>] [--backend <...>] tui");
    println!("  window_zones [--config <path>] init [--chord <MODIFIERS>] [--force]");
    println!("Commands:");
    println!("  init             write a starter config (default chord depends on the desktop)");
    println!("  status           print runtime state and exit");
    println!("  dispatch         run the action bound to HOTKEY once and print its state");
    println!("                   Sway/Hyprland dispatch requires an existing run/tui session");
    println!("  run              serve hotkeys until quit; keeps running without a terminal");
    println!("  run --tray       serve hotkeys with a tray status menu");
    println!("  tui              serve hotkeys with the ANSI dashboard");
    println!("Session commands: status, reload, restart, dispatch HOTKEY, help, q/quit/exit");
    println!("Options:");
    println!("  --log-file PATH  append best-effort timestamped session events");
    println!("  --perf-log PATH  append JSONL dispatch timings and app-process memory");
    println!(
        "                   Linux RSS/peak RSS exclude GNOME Shell; unavailable values are null"
    );
    println!("                   Sway/Hyprland logging belongs to run/tui, not dispatch clients");
}
