//! One command definition supplies shortcuts, controls, menus and help.
use super::Id;
use crate::crossterm::event::KeyEvent;
use crate::keymap::KeySpec;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    Application,
    View(Id),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub id: Id,
    pub label: String,
    pub group: String,
    pub shortcuts: Vec<String>,
    pub scope: Scope,
    pub enabled: bool,
    pub checked: bool,
}
impl Command {
    pub fn new(id: impl Into<Id>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            group: String::new(),
            shortcuts: vec![],
            scope: Scope::Application,
            enabled: true,
            checked: false,
        }
    }
    pub fn shortcut(mut self, key: impl Into<String>) -> Self {
        self.shortcuts.push(key.into());
        self
    }
    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.group = group.into();
        self
    }
    pub fn scope(mut self, scope: Scope) -> Self {
        self.scope = scope;
        self
    }
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }
}

/// Common command identifiers and defaults. Apps implement their domain effects.
pub fn standard() -> Vec<Command> {
    [
        ("kit.help", "Help", "F1"),
        ("kit.theme", "Theme…", "alt+t"),
        ("kit.presentation", "Cells / Graphics", "F9"),
        ("kit.repaint", "Repaint", "ctrl+l"),
        ("kit.quit", "Quit", "ctrl+q"),
    ]
    .into_iter()
    .map(|(id, label, key)| {
        Command::new(id, label)
            .shortcut(key)
            .group("Application")
            .enabled(id != "kit.presentation" || cfg!(all(feature = "terminal-graphics", unix)))
    })
    .collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Registry {
    commands: BTreeMap<Id, Command>,
    keys: Vec<(KeySpec, Id)>,
}
impl Registry {
    /// Validate atomically: malformed keys, duplicate IDs and ambiguous bindings
    /// are errors, including conflicts with disabled commands.
    pub fn new(commands: Vec<Command>) -> anyhow::Result<Self> {
        let mut registry = Self::default();
        for command in commands {
            anyhow::ensure!(
                !command.id.0.is_empty() && command.id.0.len() <= 120,
                "Invalid command identity"
            );
            anyhow::ensure!(
                !registry.commands.contains_key(&command.id),
                "Duplicate command {}",
                command.id.0
            );
            for shortcut in &command.shortcuts {
                let key = KeySpec::parse(shortcut)
                    .ok_or_else(|| anyhow::anyhow!("Invalid shortcut {shortcut}"))?;
                for (other, id) in &registry.keys {
                    let scope = registry
                        .commands
                        .get(id)
                        .map(|c| &c.scope)
                        .unwrap_or(&command.scope);
                    let overlapping = *scope == command.scope
                        || matches!(scope, Scope::Application)
                        || matches!(command.scope, Scope::Application);
                    anyhow::ensure!(
                        !overlapping || *other != key,
                        "Shortcut {shortcut} conflicts with {}",
                        id.0
                    );
                }
                // Detect repeated keys inside the same definition too.
                anyhow::ensure!(
                    !registry
                        .keys
                        .iter()
                        .any(|(other, id)| *other == key && *id == command.id),
                    "Repeated shortcut {shortcut}"
                );
                registry.keys.push((key, command.id.clone()));
            }
            registry.commands.insert(command.id.clone(), command);
        }
        Ok(registry)
    }
    pub fn get(&self, id: &Id) -> Option<&Command> {
        self.commands.get(id)
    }
    pub fn iter(&self) -> impl Iterator<Item = &Command> {
        self.commands.values()
    }
    pub fn resolve(&self, key: KeyEvent, ancestry: &[Id]) -> Option<&Command> {
        self.keys
            .iter()
            .filter(|(binding, _)| binding.matches(key))
            .filter_map(|(_, id)| self.commands.get(id))
            .find(|command| {
                command.enabled
                    && match &command.scope {
                        Scope::Application => true,
                        Scope::View(id) => ancestry.contains(id),
                    }
            })
    }
    pub fn set_enabled(&mut self, id: &Id, enabled: bool) -> bool {
        if let Some(command) = self.commands.get_mut(id) {
            let changed = command.enabled != enabled;
            command.enabled = enabled;
            changed
        } else {
            false
        }
    }
}

impl Registry {
    pub(crate) fn validate_view(&self, view: &super::View) -> anyhow::Result<()> {
        fn visit(
            node: &super::Node,
            parents: &[Id],
            ancestry: &mut BTreeMap<Id, Vec<Id>>,
            registry: &Registry,
        ) -> anyhow::Result<()> {
            let mut path = parents.to_vec();
            path.push(node.id.clone());
            ancestry.insert(node.id.clone(), path.clone());
            if let super::Widget::Button { command } | super::Widget::Checkbox { command } =
                &node.widget
            {
                anyhow::ensure!(
                    registry.get(command).is_some(),
                    "Control {} refers to unknown command {}",
                    node.id.0,
                    command.0
                );
            }
            if let super::Widget::List {
                items, selected, ..
            } = &node.widget
            {
                anyhow::ensure!(items.len() <= 100_000, "List exceeds item limit");
                let mut ids = std::collections::HashSet::new();
                for item in items.iter() {
                    anyhow::ensure!(
                        ids.insert(&item.id),
                        "Duplicate item {} in list {}",
                        item.id.0,
                        node.id.0
                    );
                }
                anyhow::ensure!(
                    selected.as_ref().is_none_or(|id| ids.contains(id)),
                    "Selected item is absent from list {}",
                    node.id.0
                );
            }
            for child in &node.children {
                visit(child, &path, ancestry, registry)?;
            }
            Ok(())
        }
        let mut ancestry = BTreeMap::new();
        visit(&view.root, &[], &mut ancestry, self)?;
        if let Some(modal) = &view.modal {
            visit(modal, &[], &mut ancestry, self)?;
        }
        // A scoped command may be registered before its transient view exists.
        // Its shortcut cannot fire until that scope is mounted and focused.
        for (index, (key, id)) in self.keys.iter().enumerate() {
            for (other, other_id) in &self.keys[index + 1..] {
                if key == other {
                    if let (Scope::View(a), Scope::View(b)) =
                        (&self.commands[id].scope, &self.commands[other_id].scope)
                    {
                        let overlap = ancestry.get(a).is_some_and(|path| path.contains(b))
                            || ancestry.get(b).is_some_and(|path| path.contains(a));
                        anyhow::ensure!(
                            !overlap,
                            "Nested scopes {} and {} share a shortcut",
                            a.0,
                            b.0
                        );
                    }
                }
            }
        }
        Ok(())
    }
    pub(crate) fn same_targets(&self, other: &Self) -> bool {
        self.commands.len() == other.commands.len()
            && self.commands.iter().all(|(id, command)| {
                other.get(id).is_some_and(|other| {
                    command.enabled == other.enabled && command.scope == other.scope
                })
            })
    }
}
