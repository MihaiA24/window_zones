use std::env;
use std::fs;
use std::path::PathBuf;

pub(super) fn temp_path(name: &str) -> PathBuf {
    let path = env::temp_dir().join(format!("window_zones_main_{}_{name}", std::process::id()));
    let _ = fs::remove_file(&path);
    let _ = fs::remove_dir_all(&path);
    path
}
