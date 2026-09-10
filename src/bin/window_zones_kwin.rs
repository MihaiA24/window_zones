#[cfg(target_os = "linux")]
fn main() {
    if let Err(error) = window_zones::run_kwin_companion_service() {
        eprintln!("KWin companion failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("The KWin companion is only available on Linux.");
    std::process::exit(1);
}
