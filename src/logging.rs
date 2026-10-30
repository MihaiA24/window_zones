//! Append-only session log: one timestamped line per event, written immediately.

use std::cell::RefCell;
use std::fmt::{self, Write as _};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Writes a diagnostic best-effort; an unavailable stderr must not panic or
/// replace the operation's actual exit status.
pub fn diagnostic(message: fmt::Arguments<'_>) {
    let _ = writeln!(io::stderr().lock(), "{message}");
}

/// Session event sink. Sink failures warn once on stderr and never stop the app.
/// Creating a `Logger` records the session start; dropping it records the session end.
#[derive(Debug)]
pub struct Logger {
    file: RefCell<Option<File>>,
    session: &'static str,
}

impl Logger {
    /// Opens an explicit append-only destination and records the session start.
    pub fn new(path: &Path, session: &'static str) -> Self {
        let logger = Self {
            file: RefCell::new(open_log_file(path)),
            session,
        };
        logger.log(format_args!("session start: {session}"));
        logger
    }

    /// Whether formatting a record can reach a sink.
    pub fn is_active(&self) -> bool {
        self.file.borrow().is_some()
    }

    /// Appends `<RFC 3339 UTC timestamp> <message>` as one line; control characters are
    /// escaped. Formats nothing when no sink is active.
    pub fn log(&self, message: fmt::Arguments<'_>) {
        if !self.is_active() {
            return;
        }

        let mut line = String::with_capacity(96);
        write_timestamp(&mut line, SystemTime::now());
        line.push(' ');
        let _ = EscapeControls(&mut line).write_fmt(message);
        line.push('\n');

        let mut file = self.file.borrow_mut();
        if let Some(Err(error)) = file
            .as_mut()
            .map(|handle| handle.write_all(line.as_bytes()))
        {
            *file = None;
            let _ = writeln!(
                io::stderr(),
                "Failed to write log file: {error}; file logging disabled."
            );
        }
    }
}

impl Drop for Logger {
    fn drop(&mut self) {
        self.log(format_args!("session end: {}", self.session));
    }
}

fn open_log_file(path: &Path) -> Option<File> {
    let opened = path
        .parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| OpenOptions::new().create(true).append(true).open(path));
    match opened {
        Ok(file) => Some(file),
        Err(error) => {
            let _ = writeln!(
                io::stderr(),
                "Failed to open log file {}: {error}; file logging disabled.",
                path.display()
            );
            None
        }
    }
}

/// Writes formatted text with control characters (newlines included) escaped.
struct EscapeControls<'a>(&'a mut String);

impl fmt::Write for EscapeControls<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for character in text.chars() {
            if character.is_control() {
                self.0.extend(character.escape_debug());
            } else {
                self.0.push(character);
            }
        }
        Ok(())
    }
}

/// RFC 3339 UTC with milliseconds, e.g. `2026-10-27T08:15:00.123Z`. Pre-epoch clocks clamp to
/// the epoch.
fn write_timestamp(out: &mut String, time: SystemTime) {
    let elapsed = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = elapsed.as_secs();
    let (year, month, day) = civil_from_days(seconds / 86_400);
    let _ = write!(
        out,
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        seconds / 3_600 % 24,
        seconds / 60 % 60,
        seconds % 60,
        elapsed.subsec_millis()
    );
}

/// Howard Hinnant's `civil_from_days` for non-negative days since 1970-01-01.
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = era * 400 + year_of_era + u64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::{Logger, write_timestamp};
    use std::env;
    use std::fmt;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, UNIX_EPOCH};

    fn test_directory(name: &str) -> PathBuf {
        let path = env::temp_dir().join(format!(
            "window_zones_logging_{}_{}",
            std::process::id(),
            name
        ));
        let _ = fs::remove_dir_all(&path);
        path
    }

    fn timestamp(millis: u64) -> String {
        let mut out = String::new();
        write_timestamp(&mut out, UNIX_EPOCH + Duration::from_millis(millis));
        out
    }

    /// Messages of `contents` lines, checking each starts with an RFC 3339 UTC timestamp.
    fn messages(contents: &str) -> Vec<&str> {
        contents
            .lines()
            .map(|line| {
                let (stamp, message) = line.split_once(' ').unwrap();
                assert_eq!(stamp.len(), "1970-01-01T00:00:00.000Z".len(), "{line}");
                assert!(stamp.ends_with('Z') && &stamp[10..11] == "T", "{line}");
                message
            })
            .collect()
    }

    #[test]
    fn timestamps_are_rfc3339_utc_across_date_boundaries() {
        assert_eq!(timestamp(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(timestamp(951_868_799_999), "2000-02-29T23:59:59.999Z");
        assert_eq!(timestamp(951_868_800_000), "2000-03-01T00:00:00.000Z");
        assert_eq!(timestamp(1_704_067_199_000), "2023-12-31T23:59:59.000Z");
        assert_eq!(timestamp(1_704_067_200_000), "2024-01-01T00:00:00.000Z");
        assert_eq!(timestamp(4_107_542_399_000), "2100-02-28T23:59:59.000Z");
        assert_eq!(timestamp(4_107_542_400_000), "2100-03-01T00:00:00.000Z");
    }

    #[test]
    fn records_append_escaped_timestamped_lines_across_sessions() {
        let directory = test_directory("append");
        let path = directory.join("nested/window_zones.log");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "existing\n").unwrap();

        Logger::new(&path, "status").log(format_args!("first"));
        let log = Logger::new(&path, "run");
        log.log(format_args!("multi\nline\r\tbell\u{7} é"));

        let contents = fs::read_to_string(&path).unwrap();
        let appended = contents.strip_prefix("existing\n").unwrap();
        assert_eq!(
            messages(appended),
            [
                "session start: status",
                "first",
                "session end: status",
                "session start: run",
                r"multi\nline\r\tbell\u{7} é"
            ]
        );

        drop(log);
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn records_are_written_immediately_and_session_ends_on_drop() {
        let directory = test_directory("lifecycle");
        let path = directory.join("window_zones.log");

        let log = Logger::new(&path, "tray");
        log.log(format_args!("dispatch alt+left ok"));
        assert_eq!(
            messages(&fs::read_to_string(&path).unwrap()),
            ["session start: tray", "dispatch alt+left ok"]
        );

        drop(log);
        assert_eq!(
            messages(&fs::read_to_string(&path).unwrap()),
            [
                "session start: tray",
                "dispatch alt+left ok",
                "session end: tray"
            ]
        );

        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn open_failure_disables_sink_and_skips_formatting() {
        struct PanicsWhenFormatted;
        impl fmt::Display for PanicsWhenFormatted {
            fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
                panic!("formatted without an active sink");
            }
        }
        let directory = test_directory("blocked");
        fs::create_dir_all(&directory).unwrap();
        let blocker = directory.join("not_a_directory");
        fs::write(&blocker, "").unwrap();

        let log = Logger::new(&blocker.join("window_zones.log"), "run");
        log.log(format_args!("{}", PanicsWhenFormatted));

        assert_eq!(fs::read_to_string(&blocker).unwrap(), "");
        let _ = fs::remove_dir_all(&directory);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn write_failure_disables_file_sink() {
        let path = PathBuf::from("/dev/full");
        assert!(super::open_log_file(&path).is_some());

        // The session start record fails with ENOSPC and drops the file sink.
        let log = Logger::new(&path, "run");

        assert!(!log.is_active());
        log.log(format_args!("ignored"));
    }
}
