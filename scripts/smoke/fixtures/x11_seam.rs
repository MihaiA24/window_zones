use std::{env, process, thread, time::Duration};

use window_zones::config::normalize_hotkey;
use window_zones::{
    HotkeySystem, Rect, WindowId, WindowMove, WindowSystem, WindowSystemError, X11HotkeySystem,
    X11WindowSystem,
};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ClientMessageEvent, ConnectionExt, EventMask, PropMode};
use x11rb::wrapper::ConnectionExt as _;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["focused"] => match X11WindowSystem::new().focused_window() {
            Ok(Some(window)) => println!("{}", window.id),
            Ok(None) => println!("none"),
            Err(error) => println!("error: {error}"),
        },
        ["move", id, x, y, width, height] => {
            let target = Rect {
                x: x.parse().expect("x"),
                y: y.parse().expect("y"),
                width: width.parse().expect("width"),
                height: height.parse().expect("height"),
            };
            match X11WindowSystem::new().move_window(&WindowMove::new(WindowId::new(*id), target)) {
                Ok(()) => println!("ok"),
                Err(WindowSystemError::WindowGone(_)) => println!("gone"),
                Err(error) => println!("error: {error}"),
            }
        }
        ["state", window, action @ ("add" | "remove"), first, rest @ ..] if rest.len() <= 1 => {
            let (conn, screen) = x11rb::connect(None).expect("connect to DISPLAY");
            let root = conn.setup().roots[screen].root;
            let atom = |name: &str| {
                conn.intern_atom(false, name.as_bytes())
                    .expect("intern atom")
                    .reply()
                    .expect("intern atom reply")
                    .atom
            };
            let data = [
                u32::from(*action == "add"),
                atom(*first),
                rest.first().map_or(0, |name| atom(*name)),
                2,
                0,
            ];
            let window: u32 = window.parse().expect("window");
            let event = ClientMessageEvent::new(32, window, atom("_NET_WM_STATE"), data);
            conn.send_event(
                false,
                root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                event,
            )
            .expect("send _NET_WM_STATE")
            .check()
            .expect("_NET_WM_STATE request");
            println!("sent");
        }
        ["hints", window, backup, width @ ..] if width.len() <= 1 => {
            let (conn, _) = x11rb::connect(None).expect("connect to DISPLAY");
            let window: u32 = window.parse().expect("window");
            let current = conn
                .get_property(
                    false,
                    window,
                    AtomEnum::WM_NORMAL_HINTS,
                    AtomEnum::WM_SIZE_HINTS,
                    0,
                    u32::MAX,
                )
                .expect("get normal hints")
                .reply()
                .expect("normal hints reply");
            if let Some(width) = width.first() {
                let width: u32 = width.parse().expect("minimum width");
                assert!(width > 0);
                let mut hints = if current.type_ == u32::from(AtomEnum::NONE) {
                    Vec::new()
                } else {
                    assert_eq!(current.type_, u32::from(AtomEnum::WM_SIZE_HINTS));
                    assert_eq!(current.format, 32, "normal hints format");
                    current
                        .value32()
                        .expect("32-bit normal hints")
                        .collect::<Vec<_>>()
                };
                std::fs::write(backup, &current.value).expect("save original normal hints");
                hints.resize(hints.len().max(18), 0);
                if hints[0] & (1 << 4) == 0 {
                    hints[6] = 1;
                }
                hints[0] |= 1 << 4; // ICCCM PMinSize.
                hints[5] = width;
                conn.change_property32(
                    PropMode::REPLACE,
                    window,
                    AtomEnum::WM_NORMAL_HINTS,
                    AtomEnum::WM_SIZE_HINTS,
                    &hints,
                )
                .expect("set constrained normal hints")
                .check()
                .expect("constrained normal hints request");
                println!("constrained");
            } else {
                let saved = std::fs::read(backup).expect("read original normal hints");
                if saved.is_empty() {
                    conn.delete_property(window, AtomEnum::WM_NORMAL_HINTS.into())
                        .expect("delete temporary normal hints")
                        .check()
                        .expect("delete normal hints request");
                } else {
                    conn.change_property(
                        PropMode::REPLACE,
                        window,
                        AtomEnum::WM_NORMAL_HINTS,
                        AtomEnum::WM_SIZE_HINTS,
                        32,
                        (saved.len() / 4).try_into().expect("normal hints length"),
                        &saved,
                    )
                    .expect("restore original normal hints")
                    .check()
                    .expect("restore normal hints request");
                }
                let restored = conn
                    .get_property(
                        false,
                        window,
                        AtomEnum::WM_NORMAL_HINTS,
                        AtomEnum::WM_SIZE_HINTS,
                        0,
                        u32::MAX,
                    )
                    .expect("get restored normal hints")
                    .reply()
                    .expect("restored normal hints reply");
                assert_eq!(restored.value, saved, "original hints restored byte-for-byte");
                println!("restored");
            }
        }
        ["grab", hotkey] => {
            let hotkey = normalize_hotkey(hotkey).expect("hotkey");
            let mut hotkeys = X11HotkeySystem::new();
            if let Err(error) = hotkeys.register_hotkeys(std::slice::from_ref(&hotkey)) {
                println!("error: {error}");
                process::exit(1);
            }
            println!("GRABBED {hotkey}");
            loop {
                let _ = hotkeys.next_hotkey();
                thread::sleep(Duration::from_millis(100));
            }
        }
        _ => {
            eprintln!("usage: focused | move ID X Y WIDTH HEIGHT | state WINDOW add|remove ATOM [ATOM] | hints WINDOW BACKUP [MIN_WIDTH] | grab HOTKEY");
            process::exit(2);
        }
    }
}
