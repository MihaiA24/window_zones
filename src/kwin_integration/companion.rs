use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use dbus_tree::MethodErr;

use super::protocol::{
    CALL_TIMEOUT, DisplayPayload, KWIN_PROTOCOL_MAJOR, NextRequestPayload, REQUEST_MOVE,
    REQUEST_REFRESH_DISPLAYS, REQUEST_REGISTER_HOTKEYS, method_error, sender_name,
};

const COMPANION_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(5);
const NEXT_REQUEST_WAIT: Duration = Duration::from_secs(1);

#[derive(Debug)]
enum RequestKind {
    Move {
        window: String,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
    RefreshDisplays,
    RegisterHotkeys(Vec<String>),
}

#[derive(Debug)]
struct CompanionRequest {
    id: u32,
    sender: Option<String>,
    deadline: Option<Instant>,
    claimed: bool,
    kind: RequestKind,
}

#[derive(Debug)]
struct RequestResult {
    sender: String,
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
                "unsupported KWin companion protocol major; upgrade the KWin script and companion service together to protocol 4",
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
        self.results.clear();
        self.events.clear();
        // A reconnecting script may still own keys after the companion lost its state.
        if !previous_hotkeys.is_empty() || !reset_state {
            self.push_request(
                self.controller_sender.clone(),
                None,
                RequestKind::RegisterHotkeys(previous_hotkeys),
            );
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
        self.results.clear();
        self.registered_hotkeys.clear();
        self.events.clear();
        Err(method_error(
            "org.window_zones.KWin.Error.Unavailable",
            "KWin script is not connected; enable or upgrade the Window Zones KWin script to protocol 4",
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
            self.results.clear();
            self.registered_hotkeys.clear();
            self.events.clear();
        }
        self.requests
            .retain(|request| request.sender.as_deref() != Some(owner));
        self.results.retain(|_, result| result.sender != owner);
        if self.controller_sender.as_deref() == Some(owner) {
            self.controller_sender = None;
            self.registered_hotkeys.clear();
            self.events.clear();
            if self.script_sender.is_some() {
                self.push_request(None, None, RequestKind::RegisterHotkeys(Vec::new()));
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
        sender: String,
        window: String,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> (u32, u64) {
        self.enqueue_request(
            sender,
            RequestKind::Move {
                window,
                x,
                y,
                width,
                height,
            },
        )
    }

    pub(super) fn enqueue_hotkeys(
        &mut self,
        sender: String,
        hotkeys: Vec<String>,
    ) -> Result<(u32, u64), MethodErr> {
        self.authorize_controller(sender.clone())?;
        Ok(self.enqueue_request(sender, RequestKind::RegisterHotkeys(hotkeys)))
    }

    pub(super) fn enqueue_refresh_displays(&mut self, sender: String) -> (u32, u64) {
        self.enqueue_request(sender, RequestKind::RefreshDisplays)
    }

    fn enqueue_request(&mut self, sender: String, kind: RequestKind) -> (u32, u64) {
        let now = Instant::now();
        self.drop_expired_requests(now);
        // Claimed requests stay queued until completion; retained results do not.
        let slots = u32::try_from(self.requests.len()).unwrap_or(u32::MAX);
        let budget = CALL_TIMEOUT * slots.saturating_add(1);
        let id = self.push_request(Some(sender), Some(now + budget), kind);
        (id, budget.as_millis() as u64)
    }

    fn push_request(
        &mut self,
        sender: Option<String>,
        deadline: Option<Instant>,
        kind: RequestKind,
    ) -> u32 {
        let id = self.next_request_id();
        self.requests.push_back(CompanionRequest {
            id,
            sender,
            deadline,
            claimed: false,
            kind,
        });
        id
    }

    fn drop_expired_requests(&mut self, now: Instant) {
        self.requests
            .retain(|request| request.claimed || request.deadline.is_none_or(|end| now < end));
    }

    fn next_request(&mut self, sender: &str) -> Result<NextRequestPayload, MethodErr> {
        self.ensure_script(sender)?;
        self.touch();
        self.drop_expired_requests(Instant::now());

        let Some(request) = self.requests.iter_mut().find(|request| !request.claimed) else {
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
        request.claimed = true;

        let id = request.id;
        Ok(match &request.kind {
            RequestKind::Move {
                window,
                x,
                y,
                width,
                height,
            } => (
                id,
                REQUEST_MOVE.to_string(),
                window.clone(),
                *x,
                *y,
                *width,
                *height,
                "[]".to_string(),
            ),
            RequestKind::RefreshDisplays => (
                id,
                REQUEST_REFRESH_DISPLAYS.to_string(),
                String::new(),
                0,
                0,
                0,
                0,
                "[]".to_string(),
            ),
            RequestKind::RegisterHotkeys(hotkeys) => (
                id,
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
        let sender = sender_name(call)?.to_string();
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
        let Some(index) = self.requests.iter().position(|request| request.id == id) else {
            return Err(method_error(
                "org.window_zones.KWin.Error.Invalid",
                "unknown KWin request id",
            ));
        };
        if !self.requests[index].claimed {
            return Err(method_error(
                "org.window_zones.KWin.Error.Invalid",
                "KWin request was not claimed by the script",
            ));
        }

        let request = self.requests.remove(index).expect("request index exists");
        if ok && let RequestKind::RegisterHotkeys(hotkeys) = request.kind {
            self.registered_hotkeys = hotkeys;
            if self.registered_hotkeys.is_empty() && self.controller_sender == request.sender {
                self.controller_sender = None;
            }
        }
        if request.deadline.is_some()
            && let Some(sender) = request.sender
        {
            self.results.insert(
                id,
                RequestResult {
                    sender,
                    error_name: if ok { String::new() } else { error_name },
                    message: if ok { String::new() } else { message },
                },
            );
        }
        Ok(())
    }

    pub(super) fn request_result(&mut self, sender: &str, id: u32) -> (bool, String, String) {
        if let std::collections::hash_map::Entry::Occupied(result) = self.results.entry(id)
            && result.get().sender == sender
        {
            let result = result.remove();
            return (true, result.error_name, result.message);
        }
        if self
            .requests
            .iter()
            .any(|request| request.id == id && request.sender.as_deref() == Some(sender))
        {
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
