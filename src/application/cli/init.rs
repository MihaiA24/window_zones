use std::env;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use window_zones::{default_config_path, parse_config, starter_config};

/// Default modifier chord for the starter config, chosen to avoid the desktop's
/// own bindings: GNOME uses Ctrl+Alt+arrows for workspaces and Super+arrows
/// for tiling; Plasma uses Ctrl+Meta+arrows for desktops.
fn default_chord(current_desktop: Option<&str>) -> &'static str {
    let desktop_is = |name: &str| {
        current_desktop.is_some_and(|desktops| {
            desktops
                .split(':')
                .any(|desktop| desktop.eq_ignore_ascii_case(name))
        })
    };
    if desktop_is("GNOME") {
        "Ctrl+Super"
    } else if desktop_is("KDE") {
        "Ctrl+Alt+Super"
    } else {
        "Ctrl+Alt"
    }
}

#[derive(Debug, PartialEq, Eq)]
enum InitOutcome {
    Written,
    AlreadyExists,
}

fn write_starter_config(path: &Path, chord: &str, force: bool) -> Result<InitOutcome, String> {
    if path.exists() && !force {
        return Ok(InitOutcome::AlreadyExists);
    }
    let contents = starter_config(chord);
    parse_config(&contents)
        .map_err(|error| format!("--chord `{chord}` does not form valid hotkeys: {error}"))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let mut file = match File::options()
        .write(true)
        .create(force)
        .create_new(!force)
        .truncate(force)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if !force && error.kind() == io::ErrorKind::AlreadyExists => {
            return Ok(InitOutcome::AlreadyExists);
        }
        Err(error) => return Err(format!("cannot write {}: {error}", path.display())),
    };
    file.write_all(contents.as_bytes())
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    Ok(InitOutcome::Written)
}

pub(in crate::application) fn execute_init(
    config_path: Option<PathBuf>,
    chord: Option<&str>,
    force: bool,
) -> ExitCode {
    let path = match config_path.map_or_else(default_config_path, Ok) {
        Ok(path) => path,
        Err(error) => {
            window_zones::logging::diagnostic(format_args!("Error: {error}"));
            return ExitCode::FAILURE;
        }
    };
    let current_desktop = env::var("XDG_CURRENT_DESKTOP").ok();
    let chord = chord.unwrap_or_else(|| default_chord(current_desktop.as_deref()));

    match write_starter_config(&path, chord, force) {
        Ok(InitOutcome::Written) => {
            println!("Wrote starter config to {}", path.display());
            println!(
                "Hotkeys use {chord}+<key>: arrows for halves (repeat Left/Right to cross to an adjacent display), U/I/J/K corners, D/F/G thirds, E/T two-thirds, Return maximize, C center, Backspace restore, Shift+Left/Right other display."
            );
            println!("Edit it any time; a running window_zones applies changes automatically.");
            ExitCode::SUCCESS
        }
        Ok(InitOutcome::AlreadyExists) => {
            println!(
                "Config already exists at {}; pass --force to replace it",
                path.display()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            window_zones::logging::diagnostic(format_args!("Error: {error}"));
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use window_zones::parse_config;

    use super::{InitOutcome, default_chord, write_starter_config};
    use crate::application::test_support::temp_path;

    #[test]
    fn init_writes_a_valid_config_once_and_rejects_bad_chords() {
        let directory = temp_path("init");
        let path = directory.join("nested").join("config.toml");

        let written = write_starter_config(&path, "Ctrl+Super", false);
        fs::write(&path, "# user edits\n").unwrap();
        let kept = write_starter_config(&path, "Ctrl+Super", false);
        let kept_contents = fs::read_to_string(&path).unwrap();
        let bad_chord = write_starter_config(&path, "Ctrl+Hyper", true);
        let forced = write_starter_config(&path, "Ctrl+Alt", true);
        let forced_config = parse_config(&fs::read_to_string(&path).unwrap()).unwrap();
        fs::remove_dir_all(&directory).unwrap();

        assert_eq!(written, Ok(InitOutcome::Written));
        assert_eq!(kept, Ok(InitOutcome::AlreadyExists));
        assert_eq!(kept_contents, "# user edits\n");
        assert!(bad_chord.unwrap_err().contains("Ctrl+Hyper"));
        assert_eq!(forced, Ok(InitOutcome::Written));
        assert_eq!(forced_config.bindings[0].hotkey, "alt+ctrl+left");
    }

    #[test]
    fn default_chord_avoids_desktop_bindings() {
        assert_eq!(default_chord(Some("GNOME")), "Ctrl+Super");
        assert_eq!(default_chord(Some("ubuntu:GNOME")), "Ctrl+Super");
        assert_eq!(default_chord(Some("KDE")), "Ctrl+Alt+Super");
        assert_eq!(default_chord(Some("XFCE")), "Ctrl+Alt");
        assert_eq!(default_chord(None), "Ctrl+Alt");
    }
}
