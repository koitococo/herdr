use super::*;
use crate::api::schema::Method;
use crossterm::event::{MouseButton, MouseEventKind};

fn close_state(confirm: bool, tab_count: usize) -> ClientShellState {
    let mut projected = snapshot();
    for number in 2..=tab_count {
        let mut tab = projected.tabs[0].clone();
        tab.tab_id = format!("tab_{number}");
        tab.number = number;
        tab.focused = false;
        projected.tabs.push(tab);
    }
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.config.confirm_close = confirm;
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 24).unwrap();
    state
}

fn click(state: &mut ClientShellState, rect: Rect) -> ClientShellInput {
    state.handle_raw_events(vec![crate::raw_input::RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })])
}

fn request_close(state: &mut ClientShellState, menu: bool) -> ClientShellInput {
    if menu {
        state.open_tab_context_menu("tab_1".into(), 30, 1);
        state.compose(106, 24).unwrap();
        let close_row = state.hits.context_menu_rows[2].0;
        click(state, close_row)
    } else {
        let mut outcome = ClientShellInput::default();
        state.record_binding(
            crate::input::KeybindMatch::Action(crate::input::KeybindAction::CloseTab),
            &mut outcome,
        );
        outcome
    }
}

fn assert_no_close(outcome: &ClientShellInput) {
    assert!(outcome.requests.is_empty());
    assert!(outcome.actions.iter().all(|action| {
        !matches!(action, ClientShellAction::Endpoint { request, .. }
            if matches!(request.method, Method::TabClose(_) | Method::WorkspaceClose(_)))
    }));
}

fn assert_tab_close(outcome: &ClientShellInput) {
    let methods = outcome
        .actions
        .iter()
        .filter_map(|action| match action {
            ClientShellAction::Endpoint { request, .. } => Some(&request.method),
            _ => None,
        })
        .filter(|method| !matches!(method, Method::WorkspaceFocus(_)))
        .collect::<Vec<_>>();
    assert!(matches!(methods.as_slice(), [Method::TabClose(target)] if target.tab_id == "tab_1"));
}

#[test]
fn tab_close_waits_for_space_confirmation_across_legacy_tabs() {
    for tabs in [1, 2] {
        for menu in [false, true] {
            let mut state = close_state(true, tabs);
            let requested = request_close(&mut state, menu);
            assert_no_close(&requested);
            assert!(requested.repaint);
            let Some(ClientShellOverlay::ConfirmClose(confirm)) = state.overlay.as_ref() else {
                panic!("Space close confirmation");
            };
            assert_eq!(confirm.workspace_id, "ws_1");
            assert!(!confirm.close_group);
            assert!(matches!(
                confirm.tab_target.as_ref(),
                Some(target) if target.tab_id == "tab_1"
                    && target.workspace.workspace_id == "ws_1"
            ));

            state.compose(106, 24).unwrap();
            let accepted = if menu {
                let primary = state.hits.overlay_primary;
                click(&mut state, primary)
            } else {
                state.handle_input_bytes(b"\r")
            };
            assert_tab_close(&accepted);
            assert!(state.overlay.is_none());
        }
    }
}

#[test]
fn tab_close_confirmation_can_be_cancelled_for_legacy_multi_tab_space() {
    for mouse in [false, true] {
        let mut state = close_state(true, 2);
        assert_no_close(&request_close(&mut state, false));
        assert!(matches!(
            state.overlay.as_ref(),
            Some(ClientShellOverlay::ConfirmClose(confirm))
                if confirm.workspace_id == "ws_1" && !confirm.close_group
        ));
        state.compose(106, 24).unwrap();
        let cancelled = if mouse {
            let cancel = state.hits.overlay_cancel;
            click(&mut state, cancel)
        } else {
            state.handle_input_bytes(b"\x1b")
        };
        assert_no_close(&cancelled);
        assert!(state.overlay.is_none());
    }
}

#[test]
fn tab_close_stays_immediate_when_confirmation_is_disabled() {
    for tabs in [1, 2] {
        for menu in [false, true] {
            let mut state = close_state(false, tabs);
            assert_tab_close(&request_close(&mut state, menu));
            assert!(state.overlay.is_none());
        }
    }
}

#[test]
fn immediate_tab_close_requires_a_current_tab_and_owner() {
    for change in ["deleted", "missing_owner"] {
        let mut state = close_state(false, 1);
        let mut projected = state.snapshot.as_deref().unwrap().clone();
        match change {
            "deleted" => projected.tabs.clear(),
            "missing_owner" => projected.workspaces.clear(),
            _ => unreachable!(),
        }
        state.set_snapshot(Box::new(projected));

        let mut outcome = ClientShellInput::default();
        state.request_tab_close("tab_1".into(), &mut outcome);

        assert_no_close(&outcome);
    }
}

#[test]
fn last_tab_confirmation_preserves_target_across_focus_changes_and_new_tabs() {
    let mut state = close_state(true, 1);
    let mut projected = state.snapshot.as_deref().unwrap().clone();
    let mut other_workspace = projected.workspaces[0].clone();
    other_workspace.workspace_id = "ws_2".into();
    other_workspace.active_tab_id = "other_tab".into();
    other_workspace.focused = false;
    projected.workspaces.push(other_workspace);
    let mut other_tab = projected.tabs[0].clone();
    other_tab.workspace_id = "ws_2".into();
    other_tab.tab_id = "other_tab".into();
    other_tab.focused = false;
    projected.tabs.push(other_tab);
    state.set_snapshot(Box::new(projected.clone()));

    assert_no_close(&request_close(&mut state, false));
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::ConfirmClose(_))
    ));
    let mut new_tab = projected.tabs[0].clone();
    new_tab.tab_id = "new_tab".into();
    new_tab.focused = false;
    projected.tabs.push(new_tab);
    projected.focused_workspace_id = Some("ws_2".into());
    projected.focused_tab_id = Some("other_tab".into());
    for workspace in &mut projected.workspaces {
        workspace.focused = workspace.workspace_id == "ws_2";
    }
    for tab in &mut projected.tabs {
        tab.focused = tab.tab_id == "other_tab";
    }
    state.set_snapshot(Box::new(projected));
    assert_tab_close(&state.handle_input_bytes(b"\r"));
}

#[test]
fn close_confirmation_rejects_stale_endpoint_owner_or_tab_targets() {
    for change in ["deleted", "moved", "endpoint", "reconnected"] {
        let mut state = close_state(true, 1);
        assert_no_close(&request_close(&mut state, false));
        assert!(matches!(
            state.overlay,
            Some(ClientShellOverlay::ConfirmClose(_))
        ));
        let mut projected = state.snapshot.as_deref().unwrap().clone();
        match change {
            "deleted" => projected.tabs.clear(),
            "moved" => {
                let mut new_owner = projected.workspaces[0].clone();
                new_owner.workspace_id = "ws_2".into();
                new_owner.number = 2;
                new_owner.label = "moved".into();
                new_owner.focused = false;
                projected.workspaces.push(new_owner);
                projected.tabs[0].workspace_id = "ws_2".into();
            }
            "endpoint" => {
                state.active_endpoint_id = crate::client::endpoint::ClientEndpointId::Ssh(
                    crate::client::endpoint::ProfileId::parse("0123456789abcdef0123456789abcdef")
                        .unwrap(),
                );
            }
            "reconnected" => state.endpoints[0].snapshot_generation = Some(2),
            _ => unreachable!(),
        }
        if !matches!(change, "endpoint" | "reconnected") {
            state.set_snapshot(Box::new(projected));
        }
        assert_no_close(&state.handle_input_bytes(b"\r"));
        assert!(state.overlay.is_none());
    }
}

#[test]
fn tab_close_confirmation_does_not_promise_or_implicitly_close_worktree_groups() {
    for linked in [false, true] {
        let mut state = close_state(true, 1);
        let mut projected = state.snapshot.as_deref().unwrap().clone();
        projected.workspaces[0].worktree = Some(ClientShellWorktree {
            key: "repo".into(),
            label: "repo".into(),
            is_linked_worktree: linked,
        });
        let mut sibling = projected.workspaces[0].clone();
        sibling.workspace_id = "ws_2".into();
        sibling.active_tab_id = "tab_2".into();
        sibling.focused = false;
        sibling.worktree.as_mut().unwrap().is_linked_worktree = !linked;
        projected.workspaces.push(sibling);
        let mut tab = projected.tabs[0].clone();
        tab.tab_id = "tab_2".into();
        tab.workspace_id = "ws_2".into();
        tab.focused = false;
        projected.tabs.push(tab);
        state.set_snapshot(Box::new(projected));

        let requested = request_close(&mut state, false);
        assert_no_close(&requested);
        assert!(matches!(
            state.overlay.as_ref(),
            Some(ClientShellOverlay::ConfirmClose(confirm))
                if confirm.workspace_id == "ws_1" && !confirm.close_group
        ));
        let accepted = state.handle_input_bytes(b"\r");
        assert_tab_close(&accepted);

        if !linked {
            let [ClientShellAction::Endpoint { request, .. }] = accepted.actions.as_slice() else {
                panic!("tab close request");
            };
            let (_, rejected_actions) = state.handle_endpoint_result(
                "boot-1",
                &request.id,
                Err(ClientShellEndpointError {
                    code: Some("workspace_group_close_required".into()),
                    message: "closing this workspace requires an explicit group close".into(),
                }),
            );
            assert!(rejected_actions.is_empty());
            assert!(
                state.overlay.is_none(),
                "group close must not be auto-confirmed"
            );

            let mut explicit_group = ClientShellInput::default();
            state.request_workspace_close("ws_1".into(), Some(true), &mut explicit_group);
            assert_no_close(&explicit_group);
            assert!(matches!(state.overlay.as_ref(),
                Some(ClientShellOverlay::ConfirmClose(confirm))
                    if confirm.workspace_id == "ws_1" && confirm.close_group));
            let group_close = state.handle_input_bytes(b"\r");
            assert!(matches!(group_close.actions.as_slice(),
                [ClientShellAction::Endpoint { request, .. }]
                    if matches!(&request.method, Method::WorkspaceClose(params)
                        if params.workspace_id == "ws_1" && params.close_group)));
        }
    }
}

#[test]
fn tab_context_action_revalidates_deleted_or_moved_target() {
    for change in ["deleted", "moved"] {
        let mut state = close_state(false, 1);
        state.open_tab_context_menu("tab_1".into(), 30, 1);
        state.compose(106, 24).unwrap();
        let mut projected = state.snapshot.as_deref().unwrap().clone();
        match change {
            "deleted" => projected.tabs.clear(),
            "moved" => {
                let mut owner = projected.workspaces[0].clone();
                owner.workspace_id = "ws_2".into();
                owner.number = 2;
                owner.label = "moved".into();
                owner.active_tab_id = "tab_1".into();
                owner.focused = false;
                projected.workspaces.push(owner);
                projected.tabs[0].workspace_id = "ws_2".into();
                projected.tabs[0].focused = false;
            }
            _ => unreachable!(),
        }
        state.set_snapshot(Box::new(projected));
        assert!(matches!(
            state.overlay,
            Some(ClientShellOverlay::ContextMenu(_))
        ));
        state.compose(106, 24).unwrap();
        let close_row = state.hits.context_menu_rows[2].0;

        let selected = click(&mut state, close_row);

        assert!(
            selected.actions.is_empty(),
            "stale tab context target must not dispatch: {change}"
        );
    }
}
