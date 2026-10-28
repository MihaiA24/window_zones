use dbus::blocking::LocalConnection;
use dbus_tree::MethodErr;

use super::protocol::{CALL_TIMEOUT, method_error};

// KWin hosts KGlobalAccel in-process and registers script actions under the "kwin" component.
pub(super) const KGLOBALACCEL_SERVICE: &str = "org.kde.kglobalaccel";
pub(super) const KGLOBALACCEL_PATH: &str = "/kglobalaccel";
pub(super) const KGLOBALACCEL_INTERFACE: &str = "org.kde.KGlobalAccel";
pub(super) const KGLOBALACCEL_COMPONENT: &str = "kwin";

pub(super) fn set_shortcut(
    connection: &LocalConnection,
    hotkey: String,
    key: i32,
) -> Result<(), MethodErr> {
    if crate::config::normalize_hotkey(&hotkey).as_ref() != Ok(&hotkey) {
        return Err(method_error(
            "org.window_zones.KWin.Error.Invalid",
            "SetShortcut requires a canonical hotkey",
        ));
    }
    let action_id = vec![
        KGLOBALACCEL_COMPONENT.to_string(),
        format!("Window Zones Hotkey {hotkey}"),
        String::new(),
        String::new(),
    ];
    // QJSEngine callDBus cannot marshal JS arrays as a(ai).
    // setForeignShortcutKeys also updates KWin's QAction cache, so an
    // accelerator restart cannot resurrect a removed grab.
    let keys: Vec<(Vec<i32>,)> = if key == 0 {
        Vec::new()
    } else {
        // QKeySequence's D-Bus form always contains four key slots.
        vec![(vec![key, 0, 0, 0],)]
    };
    let accel = connection.with_proxy(KGLOBALACCEL_SERVICE, KGLOBALACCEL_PATH, CALL_TIMEOUT);
    accel
        .method_call::<(), _, _, _>(
            KGLOBALACCEL_INTERFACE,
            "setForeignShortcutKeys",
            (&action_id, &keys),
        )
        .map_err(|error| {
            MethodErr::from((
                "org.window_zones.KWin.Error.Unavailable",
                format!("KGlobalAccel could not update '{hotkey}': {error}"),
            ))
        })?;
    let (assigned,): (Vec<(Vec<i32>,)>,) = accel
        .method_call(KGLOBALACCEL_INTERFACE, "shortcutKeys", (&action_id,))
        .map_err(|error| {
            MethodErr::from((
                "org.window_zones.KWin.Error.Unavailable",
                format!("KGlobalAccel could not verify '{hotkey}': {error}"),
            ))
        })?;
    if assigned != keys {
        return Err(method_error(
            "org.window_zones.KWin.Error.Unavailable",
            "KGlobalAccel did not apply the requested shortcut",
        ));
    }
    if key != 0 {
        let (winner,): (Vec<String>,) = accel
            .method_call(KGLOBALACCEL_INTERFACE, "action", (key,))
            .map_err(|error| {
                MethodErr::from((
                    "org.window_zones.KWin.Error.Unavailable",
                    format!("KGlobalAccel could not verify the owner of '{hotkey}': {error}"),
                ))
            })?;
        if winner.first() != action_id.first() || winner.get(1) != action_id.get(1) {
            return Err(MethodErr::from((
                "org.window_zones.KWin.Error.Unavailable",
                format!(
                    "KGlobalAccel did not assign '{hotkey}' to Window Zones; current owner: {}",
                    winner.get(1).map(String::as_str).unwrap_or("none")
                ),
            )));
        }
    }
    Ok(())
}
