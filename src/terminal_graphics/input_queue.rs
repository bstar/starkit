//! Ordered frontend input. Superseded positions never queue behind a slow writer.
use super::{
    embedded_input::EmbeddedKind,
    protocol::{ClientMessage, Input},
};
use anyhow::{ensure, Result};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::{collections::VecDeque, sync::Mutex};

const LIMIT: usize = 64;
#[derive(Default)]
struct State {
    messages: VecDeque<ClientMessage>,
    closed: bool,
}
pub(super) struct Outbox {
    state: Mutex<State>,
    wake: Sender<()>,
}
impl Outbox {
    pub fn new() -> (Self, Receiver<()>) {
        let (wake, rx) = bounded(1);
        (
            Self {
                state: Mutex::new(State::default()),
                wake,
            },
            rx,
        )
    }
    pub fn push(&self, mut message: ClientMessage) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        ensure!(
            !state.closed,
            "Session input connection closed; reconnect before continuing"
        );
        let family = motion_family(&message);
        let replacement = state
            .messages
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, old)| {
                if supersedes(old, &message) {
                    Some(Ok(index))
                } else if family.is_some()
                    && (bookkeeping(old)
                        || motion_family(old).is_some_and(|old_family| {
                            let new_family = family.as_ref().unwrap();
                            old_family.1 == new_family.1 && old_family.0 != new_family.0
                        }))
                {
                    None
                } else {
                    Some(Err(()))
                }
            })
            .and_then(|result| result.ok());
        if let Some(index) = replacement {
            let old = state.messages.remove(index).unwrap();
            // Keep ordered controls in place; the newest position belongs at
            // the end so input IDs still increase after removing the old one.
            if let (
                ClientMessage::Input {
                    input: Input::Osc72 { text: old },
                    ..
                },
                ClientMessage::Input {
                    input: Input::Osc72 { text: new },
                    ..
                },
            ) = (&old, &mut message)
            {
                if new
                    .split_once(';')
                    .is_none_or(|(_, payload)| payload.is_empty())
                {
                    if let Some((_, payload)) = old.split_once(';') {
                        let header = new.split(';').next().unwrap();
                        *new = format!("{header};{payload}");
                    }
                }
            }
        } else {
            ensure!(state.messages.len() < LIMIT,
                "Session control queue is full; reconnect before continuing (queued: {}; incoming: {})",
                queue_summary(&state.messages), message_kind(&message));
        }
        state.messages.push_back(message);
        let _ = self.wake.try_send(());
        Ok(())
    }
    /// Stop reading more terminal/file events before reaching the hard bound.
    /// The remaining slots are reserved for presentation and protocol replies,
    /// which must continue flowing while the transport writer is backpressured.
    pub fn accepts_input(&self) -> bool {
        let state = self.state.lock().unwrap();
        !state.closed && state.messages.len() < LIMIT / 4
    }
    pub fn pop(&self) -> Option<ClientMessage> {
        self.state.lock().unwrap().messages.pop_front()
    }
    pub fn pending(&self) -> bool {
        !self.state.lock().unwrap().messages.is_empty()
    }
    pub fn closed(&self) -> bool {
        self.state.lock().unwrap().closed
    }
    pub fn close(&self) {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        state.messages.clear();
        let _ = self.wake.try_send(());
    }
}
fn bookkeeping(message: &ClientMessage) -> bool {
    matches!(
        message,
        ClientMessage::CanvasAck { .. }
            | ClientMessage::EffectAck { .. }
            | ClientMessage::Presented { .. }
    )
}
fn motion_family(message: &ClientMessage) -> Option<(String, u64)> {
    let ClientMessage::Input {
        generation, input, ..
    } = message
    else {
        return None;
    };
    let family = match input {
        Input::Osc72 { text } if drop_motion(text).is_some() => "drop".into(),
        Input::Pointer { action, .. } if matches!(action.as_str(), "move" | "drag") => {
            "pointer".into()
        }
        Input::Embedded { canvas, event } if matches!(&event.kind, EmbeddedKind::Pointer { action, .. } if matches!(action.as_str(), "move" | "drag")) =>
        {
            format!("embedded:{canvas}")
        }
        _ => return None,
    };
    Some((family, *generation))
}
fn message_kind(message: &ClientMessage) -> String {
    match message {
        ClientMessage::Input {
            input: Input::Osc72 { text },
            ..
        } => {
            let kind = text
                .split(';')
                .next()
                .unwrap_or("")
                .split(':')
                .find_map(|f| f.strip_prefix("t="))
                .unwrap_or("chunk");
            format!(
                "osc72_{}",
                kind.chars()
                    .filter(|c| c.is_ascii_alphanumeric())
                    .take(8)
                    .collect::<String>()
            )
        }
        ClientMessage::Input {
            input: Input::Embedded { event, .. },
            ..
        } => match &event.kind {
            EmbeddedKind::Pointer { .. } => "embedded_pointer".into(),
            EmbeddedKind::Key { .. } => "embedded_key".into(),
            _ => "embedded_other".into(),
        },
        ClientMessage::Input { .. } => "input_other".into(),
        ClientMessage::CanvasAck { .. } => "canvas_ack".into(),
        ClientMessage::EffectAck { .. } => "effect_ack".into(),
        ClientMessage::Presented { .. } => "presented".into(),
        ClientMessage::Ping => "ping".into(),
        _ => "control_other".into(),
    }
}
fn queue_summary(messages: &VecDeque<ClientMessage>) -> String {
    let mut counts = std::collections::BTreeMap::new();
    for message in messages {
        *counts.entry(message_kind(message)).or_insert(0usize) += 1;
    }
    counts
        .into_iter()
        .map(|(kind, count)| format!("{kind}={count}"))
        .collect::<Vec<_>>()
        .join(", ")
}
fn supersedes(old: &ClientMessage, new: &ClientMessage) -> bool {
    let (
        ClientMessage::Input {
            generation: og,
            input: oi,
            ..
        },
        ClientMessage::Input {
            generation: ng,
            input: ni,
            ..
        },
    ) = (old, new)
    else {
        return false;
    };
    if og != ng {
        return false;
    }
    let position = |action: &str| matches!(action, "move" | "drag");
    match (oi, ni) {
        (
            Input::Pointer {
                action: a,
                button: ab,
                modifiers: am,
                ..
            },
            Input::Pointer {
                action: b,
                button: bb,
                modifiers: bm,
                ..
            },
        ) => position(a) && a == b && ab == bb && am == bm,
        (
            Input::Embedded {
                canvas: a,
                event: ae,
            },
            Input::Embedded {
                canvas: b,
                event: be,
            },
        ) if a == b => match (&ae.kind, &be.kind) {
            (
                EmbeddedKind::Pointer {
                    action: a,
                    button: ab,
                    modifiers: am,
                    ..
                },
                EmbeddedKind::Pointer {
                    action: b,
                    button: bb,
                    modifiers: bm,
                    ..
                },
            ) => position(a) && a == b && ab == bb && am == bm,
            _ => false,
        },
        (Input::Osc72 { text: a }, Input::Osc72 { text: b }) => {
            matches!((drop_motion(a), drop_motion(b)), (Some(a), Some(b)) if a == b)
        }
        _ => false,
    }
}
// Only complete drag-hover messages may replace each other. Drop, data,
// error, chunk, client, and enter/leave boundaries retain their ordering.
fn drop_motion(text: &str) -> Option<(String, bool)> {
    let header = text.split(';').next()?;
    let mut fields = std::collections::HashMap::new();
    for field in header.split(':') {
        let (key, value) = field.split_once('=')?;
        if fields.insert(key, value).is_some() {
            return None;
        }
    }
    if fields.get("t") != Some(&"m") || fields.get("m").is_some_and(|v| *v != "0") {
        return None;
    }
    let x: i32 = fields.get("x")?.parse().ok()?;
    let y: i32 = fields.get("y")?.parse().ok()?;
    Some((
        fields.get("i").copied().unwrap_or("0").to_owned(),
        x >= 0 && y >= 0,
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn pointer(id: u64, action: &str) -> ClientMessage {
        ClientMessage::Input {
            id,
            revision: 1,
            generation: 1,
            input: Input::Pointer {
                action: action.into(),
                button: 0,
                modifiers: 0,
                pixel: Some([id as u32, 0]),
                x: 0,
                y: 0,
            },
        }
    }
    #[test]
    fn blocked_writer_keeps_latest_drag_and_all_boundaries() {
        let (q, _) = Outbox::new();
        q.push(pointer(1, "down")).unwrap();
        for id in 2..10002 {
            q.push(pointer(id, "drag")).unwrap();
        }
        q.push(ClientMessage::Ping).unwrap();
        q.push(pointer(10002, "drag")).unwrap();
        q.push(pointer(10003, "up")).unwrap();
        let messages: Vec<_> = std::iter::from_fn(|| q.pop()).collect();
        assert_eq!(messages.len(), 5);
        assert!(matches!(&messages[0], ClientMessage::Input { id: 1, .. }));
        assert!(matches!(
            &messages[1],
            ClientMessage::Input { id: 10001, .. }
        ));
        assert!(matches!(&messages[2], ClientMessage::Ping));
        assert!(matches!(
            &messages[4],
            ClientMessage::Input { id: 10003, .. }
        ));
        q.close();
        assert!(q.push(pointer(10004, "up")).is_err());
    }
    fn assert_message(actual: &ClientMessage, expected: ClientMessage) {
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    }
    fn osc(id: u64, text: &str) -> ClientMessage {
        ClientMessage::Input {
            id,
            revision: 1,
            generation: 1,
            input: Input::Osc72 { text: text.into() },
        }
    }
    #[test]
    fn blocked_writer_coalesces_drag_hover_without_losing_mime_offer() {
        let (q, _) = Outbox::new();
        q.push(osc(1, "t=m:x=1:y=2:i=1;text/uri-list image/png"))
            .unwrap();
        for id in 2..10002 {
            q.push(osc(id, &format!("t=m:x={id}:y=2:i=1"))).unwrap();
        }
        q.push(osc(10002, "t=M:x=42:y=2:i=1;text/uri-list"))
            .unwrap();
        q.push(osc(10003, "t=r:x=1:i=1;m=0")).unwrap();
        let messages: Vec<_> = std::iter::from_fn(|| q.pop()).collect();
        assert_eq!(messages.len(), 3);
        assert_message(
            &messages[0],
            osc(10001, "t=m:x=10001:y=2:i=1;text/uri-list image/png"),
        );
        assert_message(&messages[1], osc(10002, "t=M:x=42:y=2:i=1;text/uri-list"));
        assert_message(&messages[2], osc(10003, "t=r:x=1:i=1;m=0"));
    }
    #[test]
    fn drag_hover_preserves_protocol_boundaries_and_updated_offers() {
        let (q, _) = Outbox::new();
        for (id, text) in [
            (1, "t=m:x=1:y=2:i=1;image/png"),
            (2, "t=m:x=2:y=2:i=1;text/uri-list"),
            (3, "t=m:x=-1:y=-1:i=1"),
            (4, "t=m:x=3:y=2:i=1"),
            (5, "t=m:x=4:y=2:i=2"),
            (6, "t=m:x=5:y=2:i=2:m=1;text/"),
            (7, "m=0:i=2;uri-list"),
        ] {
            q.push(osc(id, text)).unwrap();
        }
        let messages: Vec<_> = std::iter::from_fn(|| q.pop()).collect();
        assert_eq!(messages.len(), 6);
        assert_message(&messages[0], osc(2, "t=m:x=2:y=2:i=1;text/uri-list"));
        assert_message(&messages[1], osc(3, "t=m:x=-1:y=-1:i=1"));
        let mut resized = osc(2, "t=m:x=2:y=2:i=1");
        if let ClientMessage::Input { generation, .. } = &mut resized {
            *generation = 2;
        }
        assert!(!supersedes(&osc(1, "t=m:x=1:y=2:i=1"), &resized));
    }
    #[test]
    fn mixed_hover_and_mouse_reports_coalesce_around_acknowledgments() {
        let (q, _) = Outbox::new();
        q.push(osc(1, "t=m:x=1:y=2:i=1;text/uri-list")).unwrap();
        for id in 2..10002 {
            q.push(pointer(id * 2, "move")).unwrap();
            q.push(osc(id * 2 + 1, "t=m:x=3:y=2:i=1")).unwrap();
            if id < 42 {
                q.push(ClientMessage::Presented {
                    revision: id,
                    generation: 1,
                })
                .unwrap();
            }
        }
        q.push(osc(20005, "t=M:x=3:y=2:i=1;text/uri-list")).unwrap();
        let messages: Vec<_> = std::iter::from_fn(|| q.pop()).collect();
        assert_eq!(messages.len(), 43);
        assert_eq!(
            messages
                .iter()
                .filter(|m| matches!(m, ClientMessage::Presented { .. }))
                .count(),
            40
        );
        assert_message(&messages[41], osc(20003, "t=m:x=3:y=2:i=1;text/uri-list"));
        assert_message(&messages[42], osc(20005, "t=M:x=3:y=2:i=1;text/uri-list"));
        let ids: Vec<_> = messages
            .iter()
            .filter_map(|m| match m {
                ClientMessage::Input { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    }
    #[test]
    fn control_queue_remains_bounded() {
        let (q, _) = Outbox::new();
        for _ in 0..LIMIT {
            q.push(ClientMessage::Ping).unwrap();
        }
        assert!(q.push(ClientMessage::Ping).is_err());
    }
}
