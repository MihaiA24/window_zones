use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};

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
    if force {
        replace_config(path, contents.as_bytes())
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        return Ok(InitOutcome::Written);
    }
    let mut file = match File::options().write(true).create_new(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Ok(InitOutcome::AlreadyExists);
        }
        Err(error) => return Err(format!("cannot write {}: {error}", path.display())),
    };
    file.write_all(contents.as_bytes())
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    Ok(InitOutcome::Written)
}

fn replace_config(path: &Path, contents: &[u8]) -> io::Result<()> {
    let (target, permissions) = match path.canonicalize() {
        Ok(target) => {
            if !target.metadata()?.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "config must be a regular file",
                ));
            }
            // Replacing the directory entry must not bypass write permission on the config.
            let permissions = File::options()
                .write(true)
                .open(&target)?
                .metadata()?
                .permissions();
            (target, Some(permissions))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Ok(link) = fs::read_link(path) {
                return replace_config(&path.with_file_name(link), contents);
            }
            (path.to_owned(), None)
        }
        Err(error) => return Err(error),
    };

    static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if permissions.is_some() {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let (temporary, mut file) = loop {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let temporary = target.with_file_name(format!(
            ".window-zones-init-{}-{id}.tmp",
            std::process::id()
        ));
        match options.open(&temporary) {
            Ok(file) => break (temporary, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let staged = (|| {
        file.write_all(contents)?;
        if let Some(permissions) = permissions {
            file.set_permissions(permissions)?;
        }
        file.sync_all()
    })();
    drop(file);
    let result = staged.and_then(|()| fs::rename(&temporary, &target));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
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

    #[cfg(unix)]
    #[test]
    fn force_replaces_the_config_instead_of_overwriting_its_contents_in_place() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let directory = temp_path("atomic_init");
        fs::create_dir_all(&directory).unwrap();
        let target = directory.join("user.toml");
        let path = directory.join("config.toml");
        fs::write(&target, "# user edits\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
        symlink("user.toml", &path).unwrap();
        let previous = directory.join("previous.toml");
        fs::hard_link(&target, &previous).unwrap();

        assert_eq!(
            write_starter_config(&path, "Ctrl+Alt", true),
            Ok(InitOutcome::Written)
        );
        assert!(fs::symlink_metadata(&path).unwrap().is_symlink());
        assert_eq!(fs::read_to_string(&previous).unwrap(), "# user edits\n");
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o640,
        );
        assert_eq!(
            parse_config(&fs::read_to_string(&path).unwrap())
                .unwrap()
                .bindings[0]
                .hotkey,
            "alt+ctrl+left",
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 3);
        fs::remove_dir_all(directory).unwrap();
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
