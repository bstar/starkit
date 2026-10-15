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
    pub fn push(&self, message: ClientMessage) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        ensure!(
            !state.closed,
            "Session input connection closed; reconnect before continuing"
        );
        if state
            .messages
            .back()
            .is_some_and(|old| supersedes(old, &message))
        {
            *state.messages.back_mut().unwrap() = message;
        } else {
            ensure!(
                state.messages.len() < LIMIT,
                "Session control queue is full; reconnect before continuing"
            );
            state.messages.push_back(message);
        }
        let _ = self.wake.try_send(());
        Ok(())
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
        _ => false,
    }
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
    #[test]
    fn control_queue_remains_bounded() {
        let (q, _) = Outbox::new();
        for _ in 0..LIMIT {
            q.push(ClientMessage::Ping).unwrap();
        }
        assert!(q.push(ClientMessage::Ping).is_err());
    }
}
