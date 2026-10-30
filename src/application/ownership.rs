use std::env;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Exit status when another instance already owns the hotkeys (EX_TEMPFAIL);
/// the systemd unit does not restart on it.
pub(super) const EXIT_ALREADY_RUNNING: u8 = 75;

/// Held for the lifetime of a `run`/`tui` session: only one App instance owns
/// the hotkeys at a time.
#[derive(Debug)]
pub(super) struct InstanceLock {
    _file: File,
    pub(super) pid_path: PathBuf,
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        // Keep the lock held while removing its sidecar; never unlink the lock
        // file, or another process could lock a different inode.
        let _ = fs::remove_file(&self.pid_path);
    }
}

#[derive(Debug)]
pub(super) enum InstanceLockError {
    AlreadyRunning(String),
    Io(String),
}

#[cfg(unix)]
pub(super) fn current_uid() -> u32 {
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    // SAFETY: geteuid takes no arguments and cannot fail.
    unsafe { geteuid() }
}

pub(super) fn instance_lock_path() -> Result<PathBuf, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt};

        let directory = env::temp_dir().join(format!("window_zones-{}", current_uid()));
        #[cfg(target_os = "linux")]
        let directory = env::var_os("XDG_RUNTIME_DIR")
            .filter(|value| !value.is_empty())
            .map_or(directory, PathBuf::from);
        let failure = |error: &str| {
            format!(
                "cannot prepare instance directory {}: {error}",
                directory.display()
            )
        };
        if !directory.is_absolute() {
            return Err(failure("an absolute path is required"));
        }
        match fs::DirBuilder::new().mode(0o700).create(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(failure(&error.to_string())),
        }
        let metadata =
            fs::symlink_metadata(&directory).map_err(|error| failure(&error.to_string()))?;
        if !metadata.is_dir() || metadata.uid() != current_uid() || metadata.mode() & 0o077 != 0 {
            return Err(failure(
                "must be an owned, non-symlink directory with mode 0700",
            ));
        }
        Ok(directory.join("window_zones.lock"))
    }
    #[cfg(not(unix))]
    {
        let user = env::var("USERNAME").unwrap_or_default();
        Ok(env::temp_dir().join(format!("window_zones-{user}.lock")))
    }
}

pub(super) fn acquire_instance_lock(path: &Path) -> Result<InstanceLock, InstanceLockError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() => {
            return Err(InstanceLockError::Io(format!(
                "cannot open instance lock {}: not a regular, non-symlink file",
                path.display()
            )));
        }
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            return Err(InstanceLockError::Io(format!(
                "cannot inspect instance lock {}: {error}",
                path.display()
            )));
        }
        _ => {}
    }
    let file = File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|error| {
            InstanceLockError::Io(format!(
                "cannot open instance lock {}: {error}",
                path.display()
            ))
        })?;
    // The pid sits beside the lock: a Windows lock also blocks reading the locked file.
    let pid_path = path.with_extension("pid");
    match file.try_lock() {
        Ok(()) => {
            // Recreate the informational sidecar without following a stale symlink.
            let _ = fs::remove_file(&pid_path);
            let _ = File::options()
                .write(true)
                .create_new(true)
                .open(&pid_path)
                .and_then(|mut pid| write!(pid, "{}", std::process::id()));
            Ok(InstanceLock {
                _file: file,
                pid_path,
            })
        }
        Err(fs::TryLockError::WouldBlock) => {
            let owner = fs::read_to_string(&pid_path).unwrap_or_default();
            let owner = owner.trim();
            Err(InstanceLockError::AlreadyRunning(format!(
                "another window_zones session{} already owns the hotkeys; stop it first (for the systemd service: `systemctl --user stop window-zones`) or use `window_zones dispatch <hotkey>`",
                if owner.is_empty() {
                    String::new()
                } else {
                    format!(" (pid {owner})")
                }
            )))
        }
        Err(fs::TryLockError::Error(error)) => Err(InstanceLockError::Io(format!(
            "cannot lock {}: {error}",
            path.display()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{InstanceLockError, acquire_instance_lock};
    use crate::application::test_support::temp_path;

    #[test]
    fn second_instance_lock_is_refused_until_the_first_is_released() {
        let path = temp_path("instance.lock");

        let first = acquire_instance_lock(&path).unwrap();
        let InstanceLockError::AlreadyRunning(second) = acquire_instance_lock(&path).unwrap_err()
        else {
            panic!("expected lock contention");
        };
        drop(first);
        assert!(!path.with_extension("pid").exists());
        let third = acquire_instance_lock(&path).unwrap();
        drop(third);
        assert!(!path.with_extension("pid").exists());
        fs::remove_file(&path).unwrap();

        assert!(
            second.contains(&format!("pid {}", std::process::id())),
            "{second}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn instance_pid_sidecar_never_overwrites_a_symlink_target() {
        let directory = temp_path("pid_symlink");
        fs::create_dir(&directory).unwrap();
        let path = directory.join("instance.lock");
        let unrelated = directory.join("unrelated.txt");
        fs::write(&unrelated, "keep this content").unwrap();
        std::os::unix::fs::symlink(&unrelated, path.with_extension("pid")).unwrap();

        let lock = acquire_instance_lock(&path).unwrap();
        drop(lock);
        let content = fs::read_to_string(&unrelated).unwrap();
        fs::remove_dir_all(directory).unwrap();

        assert_eq!(content, "keep this content");
    }

    #[test]
    fn instance_lock_io_failures_are_not_reported_as_contention() {
        let path = temp_path("missing_lock_parent").join("instance.lock");
        assert!(matches!(
            acquire_instance_lock(&path),
            Err(InstanceLockError::Io(message)) if message.contains("cannot open")
        ));
    }
}
