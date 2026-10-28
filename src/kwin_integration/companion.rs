use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use dbus_tree::MethodErr;

use super::protocol::{
    DisplayPayload, KWIN_PROTOCOL_MAJOR, NextRequestPayload, REQUEST_MOVE,
    REQUEST_REFRESH_DISPLAYS, REQUEST_REGISTER_HOTKEYS, method_error, sender_name,
};

const COMPANION_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(5);
const NEXT_REQUEST_WAIT: Duration = Duration::from_secs(1);

#[derive(Debug, PartialEq, Eq)]
enum CompanionRequest {
    Move {
        id: u32,
        window: String,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
    RefreshDisplays {
        id: u32,
    },
    RegisterHotkeys {
        id: u32,
        hotkeys: Vec<String>,
        controller: Option<String>,
    },
}

impl CompanionRequest {
    fn id(&self) -> u32 {
        match self {
            Self::Move { id, .. }
            | Self::RefreshDisplays { id }
            | Self::RegisterHotkeys { id, .. } => *id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequestResult {
    error_name: String,
    message: String,
}

#[derive(Debug)]
struct PendingNextRequest {
    call: dbus::Message,
    sender: String,
    deadline: Instant,
}

#[derive(Debug, Default)]
pub(super) struct CompanionState {
    script_sender: Option<String>,
    controller_sender: Option<String>,
    last_seen: Option<Instant>,
    pub(super) focused: Option<(String, i32, i32, u32, u32)>,
    pub(super) displays: Vec<DisplayPayload>,
    requests: VecDeque<CompanionRequest>,
    claimed_requests: HashSet<u32>,
    results: HashMap<u32, RequestResult>,
    last_request_id: u32,
    pending_next_request: Option<PendingNextRequest>,
    pub(super) registered_hotkeys: Vec<String>,
    pub(super) events: VecDeque<String>,
}

impl CompanionState {
    pub(super) fn mark_ready(
        &mut self,
        protocol_major: u32,
        script_sender: String,
        reset_state: bool,
    ) -> Result<Option<dbus::Message>, MethodErr> {
        if protocol_major != KWIN_PROTOCOL_MAJOR {
            return Err(method_error(
                "org.window_zones.KWin.Error.Incompatible",
                "unsupported KWin companion protocol major",
            ));
        }
        if !reset_state && self.script_sender.as_deref() == Some(script_sender.as_str()) {
            self.touch();
            return Ok(None);
        }

        let pending_reply = self.pending_next_request.take().map(|pending| {
            method_error(
                "org.window_zones.KWin.Error.Unavailable",
                "KWin script reset the request session",
            )
            .to_message(&pending.call)
        });
        let previous_hotkeys = std::mem::take(&mut self.registered_hotkeys);
        self.script_sender = Some(script_sender);
        self.last_seen = Some(Instant::now());
        self.focused = None;
        self.displays.clear();
        self.requests.clear();
        self.claimed_requests.clear();
        self.results.clear();
        self.events.clear();
        // A reconnecting script may still own keys after the companion lost its state.
        if !previous_hotkeys.is_empty() || !reset_state {
            self.enqueue_hotkeys(previous_hotkeys);
        }
        Ok(pending_reply)
    }

    pub(super) fn ensure_ready(&mut self) -> Result<(), MethodErr> {
        if self.script_sender.is_some()
            && self
                .last_seen
                .is_some_and(|last_seen| last_seen.elapsed() <= COMPANION_HEARTBEAT_TIMEOUT)
        {
            return Ok(());
        }

        self.script_sender = None;
        self.requests.clear();
        self.claimed_requests.clear();
        self.results.clear();
        self.registered_hotkeys.clear();
        self.events.clear();
        Err(method_error(
            "org.window_zones.KWin.Error.Unavailable",
            "KWin script is not connected; enable or upgrade the Window Zones KWin script to protocol 3",
        ))
    }

    pub(super) fn ensure_script(&mut self, sender: &str) -> Result<(), MethodErr> {
        self.ensure_ready()?;
        if self.script_sender.as_deref() == Some(sender) {
            return Ok(());
        }

        Err(method_error(
            "org.window_zones.KWin.Error.Denied",
            "request sender is not the connected KWin script",
        ))
    }

    pub(super) fn authorize_controller(&mut self, sender: String) -> Result<(), MethodErr> {
        self.is_controller(&sender)?;
        self.controller_sender = Some(sender);
        Ok(())
    }

    pub(super) fn is_controller(&self, sender: &str) -> Result<bool, MethodErr> {
        match self.controller_sender.as_deref() {
            Some(owner) if owner != sender => Err(method_error(
                "org.window_zones.KWin.Error.Busy",
                "another Window Zones process owns the KWin controller",
            )),
            Some(_) => Ok(true),
            None => Ok(false),
        }
    }

    pub(super) fn owner_lost(&mut self, owner: &str) {
        if self.script_sender.as_deref() == Some(owner) {
            self.script_sender = None;
            self.requests.clear();
            self.claimed_requests.clear();
            self.results.clear();
            self.registered_hotkeys.clear();
            self.events.clear();
        }
        if self.controller_sender.as_deref() == Some(owner) {
            self.controller_sender = None;
            self.requests.clear();
            self.claimed_requests.clear();
            self.results.clear();
            self.registered_hotkeys.clear();
            self.events.clear();
            if self.script_sender.is_some() {
                self.enqueue_hotkeys(Vec::new());
            }
        }
    }

    pub(super) fn touch(&mut self) {
        self.last_seen = Some(Instant::now());
    }

    fn next_request_id(&mut self) -> u32 {
        self.last_request_id = self.last_request_id.wrapping_add(1).max(1);
        self.last_request_id
    }
    pub(super) fn enqueue_move(
        &mut self,
        window: String,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> u32 {
        let id = self.next_request_id();
        self.requests.push_back(CompanionRequest::Move {
            id,
            window,
            x,
            y,
            width,
            height,
        });
        id
    }

    pub(super) fn enqueue_hotkeys(&mut self, hotkeys: Vec<String>) -> u32 {
        let id = self.next_request_id();
        self.requests.push_back(CompanionRequest::RegisterHotkeys {
            id,
            hotkeys,
            controller: self.controller_sender.clone(),
        });
        id
    }

    pub(super) fn enqueue_refresh_displays(&mut self) -> u32 {
        let id = self.next_request_id();
        self.requests
            .push_back(CompanionRequest::RefreshDisplays { id });
        id
    }

    fn next_request(&mut self, sender: &str) -> Result<NextRequestPayload, MethodErr> {
        self.ensure_script(sender)?;
        self.touch();

        let Some(request) = self
            .requests
            .iter()
            .find(|request| !self.claimed_requests.contains(&request.id()))
        else {
            return Ok((
                0,
                String::new(),
                String::new(),
                0,
                0,
                0,
                0,
                "[]".to_string(),
            ));
        };
        self.claimed_requests.insert(request.id());

        Ok(match request {
            CompanionRequest::Move {
                id,
                window,
                x,
                y,
                width,
                height,
            } => (
                *id,
                REQUEST_MOVE.to_string(),
                window.clone(),
                *x,
                *y,
                *width,
                *height,
                "[]".to_string(),
            ),
            CompanionRequest::RefreshDisplays { id } => (
                *id,
                REQUEST_REFRESH_DISPLAYS.to_string(),
                String::new(),
                0,
                0,
                0,
                0,
                "[]".to_string(),
            ),
            CompanionRequest::RegisterHotkeys { id, hotkeys, .. } => (
                *id,
                REQUEST_REGISTER_HOTKEYS.to_string(),
                String::new(),
                0,
                0,
                0,
                0,
                serde_json::to_string(hotkeys).map_err(|_| {
                    method_error(
                        "org.window_zones.KWin.Error.Operation",
                        "failed to encode hotkey request payload",
                    )
                })?,
            ),
        })
    }

    pub(super) fn poll_next_request(
        &mut self,
        call: &dbus::Message,
    ) -> Result<Vec<dbus::Message>, MethodErr> {
        let sender = sender_name(call)?;
        self.ensure_script(&sender)?;
        if self.pending_next_request.is_some() {
            return Err(method_error(
                "org.window_zones.KWin.Error.Busy",
                "KWin script already has an outstanding NextRequest",
            ));
        }
        let request = self.next_request(&sender)?;
        if request.0 != 0 {
            let mut reply = call.method_return();
            reply.append_all(request);
            return Ok(vec![reply]);
        }
        // dbus_message_copy deliberately resets the serial; a deferred reply must retain it.
        let mut deferred_call = call
            .duplicate()
            .map_err(|error| MethodErr::failed(&error))?;
        deferred_call.set_serial(call.get_serial().ok_or_else(|| {
            method_error(
                "org.window_zones.KWin.Error.Invalid",
                "NextRequest has no D-Bus serial",
            )
        })?);
        self.pending_next_request = Some(PendingNextRequest {
            call: deferred_call,
            sender,
            deadline: Instant::now() + NEXT_REQUEST_WAIT,
        });
        Ok(Vec::new())
    }

    pub(super) fn take_next_request_reply(&mut self, now: Instant) -> Option<dbus::Message> {
        let pending = self.pending_next_request.take()?;
        match self.next_request(&pending.sender) {
            Ok(request) if request.0 != 0 || now >= pending.deadline => {
                let mut reply = pending.call.method_return();
                reply.append_all(request);
                Some(reply)
            }
            Ok(_) => {
                self.pending_next_request = Some(pending);
                None
            }
            Err(error) => Some(error.to_message(&pending.call)),
        }
    }

    pub(super) fn complete_request(
        &mut self,
        id: u32,
        ok: bool,
        error_name: String,
        message: String,
    ) -> Result<(), MethodErr> {
        let Some(index) = self.requests.iter().position(|request| request.id() == id) else {
            return Err(method_error(
                "org.window_zones.KWin.Error.Invalid",
                "unknown KWin request id",
            ));
        };
        if !self.claimed_requests.remove(&id) {
            return Err(method_error(
                "org.window_zones.KWin.Error.Invalid",
                "KWin request was not claimed by the script",
            ));
        }

        let request = self.requests.remove(index).expect("request index exists");
        if ok
            && let CompanionRequest::RegisterHotkeys {
                hotkeys,
                controller,
                ..
            } = request
        {
            self.registered_hotkeys = hotkeys;
            if self.registered_hotkeys.is_empty() && self.controller_sender == controller {
                self.controller_sender = None;
            }
        }
        self.results.insert(
            id,
            RequestResult {
                error_name: if ok { String::new() } else { error_name },
                message: if ok { String::new() } else { message },
            },
        );
        Ok(())
    }

    pub(super) fn request_result(&mut self, id: u32) -> (bool, String, String) {
        if let Some(result) = self.results.remove(&id) {
            return (true, result.error_name, result.message);
        }
        if self.requests.iter().any(|request| request.id() == id) {
            return (false, String::new(), String::new());
        }
        (
            true,
            "org.window_zones.KWin.Error.Unavailable".to_string(),
            "KWin request result is no longer available; its session was reset or released"
                .to_string(),
        )
    }
}

#[cfg(test)]
#[path = "tests/companion.rs"]
mod tests;
