#[cfg(target_os = "linux")]
fn main() {
    use window_zones::kwin_integration::KwinCompanionServiceError;

    if let Err(error) = window_zones::run_kwin_companion_service() {
        window_zones::logging::diagnostic(format_args!("KWin companion failed: {error}"));
        std::process::exit(match error {
            KwinCompanionServiceError::AlreadyRunning => 75,
            KwinCompanionServiceError::Failed(_) => 1,
        });
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    window_zones::logging::diagnostic(format_args!(
        "The KWin companion is only available on Linux."
    ));
    std::process::exit(1);
}
