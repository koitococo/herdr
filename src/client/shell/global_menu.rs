use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ClientGlobalMenuAction {
    Binding(crate::input::KeybindAction),
    WhatsNew,
    SortSpacesByName,
}

pub(super) fn global_menu_attention(snapshot: &ClientShellSnapshot) -> bool {
    snapshot.update_available.is_some() || snapshot.integration_updates_available
}

pub(super) fn global_menu_item_has_badge(
    snapshot: &ClientShellSnapshot,
    action: ClientGlobalMenuAction,
) -> bool {
    (action == ClientGlobalMenuAction::WhatsNew && snapshot.update_available.is_some())
        || (action == ClientGlobalMenuAction::Binding(crate::input::KeybindAction::Settings)
            && snapshot.integration_updates_available)
}

fn workspace_ids_sorted_by_name(snapshot: &ClientShellSnapshot) -> Vec<String> {
    let mut blocks = Vec::<Vec<usize>>::new();
    for entry in super::sidebar::workspace_entries(snapshot, &HashSet::new()) {
        if entry.indented {
            if let Some(block) = blocks.last_mut() {
                block.push(entry.index);
            } else {
                blocks.push(vec![entry.index]);
            }
        } else {
            blocks.push(vec![entry.index]);
        }
    }
    for block in &mut blocks {
        let Some((_, children)) = block.split_first_mut() else {
            continue;
        };
        children.sort_by(|left, right| {
            snapshot.workspaces[*left]
                .label
                .cmp(&snapshot.workspaces[*right].label)
        });
    }
    blocks.sort_by(|left, right| {
        snapshot.workspaces[left[0]]
            .label
            .cmp(&snapshot.workspaces[right[0]].label)
    });
    blocks
        .into_iter()
        .flatten()
        .filter_map(|index| snapshot.workspaces.get(index))
        .map(|workspace| workspace.workspace_id.clone())
        .collect()
}

pub(super) fn global_menu_items(
    snapshot: &ClientShellSnapshot,
) -> Vec<(&'static str, ClientGlobalMenuAction)> {
    let mut items = vec![
        (
            "settings",
            ClientGlobalMenuAction::Binding(crate::input::KeybindAction::Settings),
        ),
        (
            "keybinds",
            ClientGlobalMenuAction::Binding(crate::input::KeybindAction::Help),
        ),
        (
            "reload config",
            ClientGlobalMenuAction::Binding(crate::input::KeybindAction::ReloadConfig),
        ),
    ];
    if snapshot.update_available.is_some() || snapshot.latest_release_notes_available {
        items.push((
            if snapshot.update_available.is_some() {
                "update ready"
            } else {
                "what's new"
            },
            ClientGlobalMenuAction::WhatsNew,
        ));
    }
    items.push((
        "sort spaces by name",
        ClientGlobalMenuAction::SortSpacesByName,
    ));
    items.push((
        "detach",
        ClientGlobalMenuAction::Binding(crate::input::KeybindAction::Detach),
    ));
    items
}

impl ClientShellState {
    pub(super) fn toggle_global_menu(&mut self) {
        if matches!(self.overlay, Some(ClientShellOverlay::GlobalMenu(_))) {
            self.overlay = None;
        } else {
            self.overlay = Some(ClientShellOverlay::GlobalMenu(ClientGlobalMenuOverlay {
                highlighted: 0,
            }));
        }
    }

    pub(super) fn move_global_menu_selection(&mut self, delta: isize) {
        let item_count = self
            .snapshot
            .as_deref()
            .map(global_menu_items)
            .map_or(0, |items| items.len());
        let Some(ClientShellOverlay::GlobalMenu(menu)) = self.overlay.as_mut() else {
            return;
        };
        menu.highlighted = (menu.highlighted as isize + delta)
            .clamp(0, item_count.saturating_sub(1) as isize) as usize;
    }

    pub(super) fn activate_global_menu_item(
        &mut self,
        index: usize,
        outcome: &mut ClientShellInput,
    ) {
        let Some(action) = self.snapshot.as_deref().and_then(|snapshot| {
            global_menu_items(snapshot)
                .get(index)
                .map(|(_, action)| *action)
        }) else {
            return;
        };
        if action == ClientGlobalMenuAction::WhatsNew
            && self
                .snapshot
                .as_deref()
                .and_then(|snapshot| snapshot.release_notes.as_ref())
                .is_none()
        {
            return;
        }
        self.overlay = None;
        match action {
            ClientGlobalMenuAction::Binding(binding) => {
                self.record_binding(crate::input::KeybindMatch::Action(binding), outcome)
            }
            ClientGlobalMenuAction::SortSpacesByName => self.sort_spaces_by_name(outcome),
            ClientGlobalMenuAction::WhatsNew => self.open_release_notes(),
        }
        outcome.repaint = true;
    }

    fn sort_spaces_by_name(&mut self, outcome: &mut ClientShellInput) {
        let Some(snapshot) = self.snapshot.as_deref() else {
            return;
        };
        let workspace_ids = workspace_ids_sorted_by_name(snapshot);
        let current_ids = snapshot
            .workspaces
            .iter()
            .map(|workspace| workspace.workspace_id.clone())
            .collect::<Vec<_>>();
        if workspace_ids == current_ids {
            return;
        }
        self.push_endpoint_method(
            crate::api::schema::Method::WorkspaceMoveBlock(
                crate::api::schema::WorkspaceMoveBlockParams {
                    workspace_ids,
                    before_workspace_id: None,
                },
            ),
            outcome,
        );
    }
}
