//! Deterministic presentation replay for agents, tests and bug reports.
//! Captures capability choices and solved geometry without application content,
//! paths, credentials, terminal probes or playback/device side effects.
use crate::{
    panes::{Focus, Layout, Node},
    presentation::{Profile, Selection, Tier},
    ratatui::layout::Rect,
};
use serde::{Deserialize, Serialize};
use std::hash::Hash;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action<P> {
    Resize { columns: u16, rows: u16 },
    Focus { pane: P },
    Step { forward: bool },
    OpenModal { panes: Vec<P> },
    CloseModal,
}
#[derive(Clone, Debug, Serialize)]
pub struct Snapshot<P> {
    pub sequence: u64,
    pub profile: Profile,
    pub presentation: Selection,
    pub layout: Layout<P>,
    pub focus: Focus<P>,
}
pub struct Replay<P> {
    viewport: Rect,
    profile: Profile,
    preferred: Tier,
    focus: Focus<P>,
    sequence: u64,
}
impl<P: Copy + Eq + Hash> Replay<P> {
    pub fn new(
        columns: u16,
        rows: u16,
        profile: Profile,
        preferred: Tier,
        initial: Option<P>,
    ) -> anyhow::Result<Self> {
        let viewport = viewport(columns, rows)?;
        Ok(Self {
            viewport,
            profile,
            preferred,
            focus: Focus::new(initial),
            sequence: 0,
        })
    }
    pub fn snapshot(&mut self, tree: &Node<P>) -> anyhow::Result<Snapshot<P>> {
        let layout = tree.layout(self.viewport)?;
        self.focus.reconcile(&layout);
        Ok(Snapshot {
            sequence: self.sequence,
            profile: self.profile.clone(),
            presentation: self.profile.choose(self.preferred),
            layout,
            focus: self.focus.clone(),
        })
    }
    pub fn apply(&mut self, tree: &Node<P>, action: Action<P>) -> anyhow::Result<Snapshot<P>> {
        // Validate before changing state; rejected actions never advance replay.
        let resized = if let Action::Resize { columns, rows } = action {
            Some(viewport(columns, rows)?)
        } else {
            None
        };
        let layout = tree.layout(resized.unwrap_or(self.viewport))?;
        match action {
            Action::Resize { .. } => self.viewport = resized.unwrap(),
            Action::Focus { pane } => {
                anyhow::ensure!(layout.pane(pane).is_some(), "Unknown focus pane");
                self.focus.request(pane);
            }
            Action::Step { forward } => {
                self.focus.step(&layout, forward);
            }
            Action::OpenModal { panes } => {
                anyhow::ensure!(
                    self.focus.scope_depth() < 32
                        && !panes.is_empty()
                        && panes.len() <= 4096
                        && panes.iter().all(|id| layout.can_focus(*id)),
                    "Invalid modal focus scope"
                );
                anyhow::ensure!(
                    self.focus.push_scope(panes),
                    "Cannot enter modal focus scope"
                );
            }
            Action::CloseModal => {
                anyhow::ensure!(self.focus.scope_depth() > 0, "No modal scope is open");
                self.focus.pop_scope(&layout);
            }
        }
        self.sequence += 1;
        self.snapshot(tree)
    }
}
fn viewport(columns: u16, rows: u16) -> anyhow::Result<Rect> {
    anyhow::ensure!(
        columns > 0
            && rows > 0
            && columns <= 4096
            && rows <= 4096
            && u32::from(columns) * u32::from(rows) <= 1_048_576,
        "Invalid replay viewport"
    );
    Ok(Rect::new(0, 0, columns, rows))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        panes::{Overflow, Size},
        presentation::{Environment, Observed},
    };
    #[test]
    fn replay_restores_hidden_focus_and_contains_modal_navigation() {
        let tree = Node::column(vec![
            Node::pane(1).height(Size::fill(5)).priority(10),
            Node::pane(2)
                .height(Size::fixed(4))
                .overflow(Overflow::Hide),
        ]);
        let profile = Profile::resolve(&Environment::default(), Observed::default());
        let actions = [
            Action::Resize {
                columns: 80,
                rows: 5,
            },
            Action::Resize {
                columns: 80,
                rows: 20,
            },
            Action::OpenModal { panes: vec![1] },
            Action::Focus { pane: 2 },
            Action::Step { forward: true },
            Action::CloseModal,
        ];
        let mut recordings = vec![];
        for _ in 0..2 {
            let mut replay = Replay::new(80, 20, profile.clone(), Tier::Native, Some(2)).unwrap();
            let mut recording = vec![];
            for (index, action) in actions.iter().cloned().enumerate() {
                let state = replay.apply(&tree, action).unwrap();
                assert_eq!(
                    state.focus.active(),
                    Some(if index == 1 || index == 5 { 2 } else { 1 })
                );
                recording.push(serde_json::to_value(state).unwrap());
            }
            recordings.push(recording);
        }
        assert_eq!(recordings[0], recordings[1]);
    }
    #[test]
    fn rejected_actions_leave_the_snapshot_unchanged() {
        let tree = Node::pane(1);
        let mut replay = Replay::new(
            80,
            24,
            Profile::resolve(&Environment::default(), Observed::default()),
            Tier::Cells,
            Some(1),
        )
        .unwrap();
        let before = serde_json::to_value(replay.snapshot(&tree).unwrap()).unwrap();
        for action in [
            Action::Resize {
                columns: 0,
                rows: 20,
            },
            Action::Resize {
                columns: 4096,
                rows: 4096,
            },
            Action::Focus { pane: 2 },
            Action::OpenModal { panes: vec![2] },
            Action::CloseModal,
        ] {
            assert!(replay.apply(&tree, action).is_err());
            assert_eq!(
                serde_json::to_value(replay.snapshot(&tree).unwrap()).unwrap(),
                before
            );
        }
    }
}
