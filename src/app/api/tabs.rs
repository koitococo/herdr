use std::path::PathBuf;

use crate::api::schema::{
    ResponseResult, TabCreateParams, TabListParams, TabMoveParams, TabRenameParams, TabTarget,
};
use crate::app::App;

use super::responses::{encode_error, encode_success};
use super::tab_policy::multi_tab_unsupported;

impl App {
    pub(super) fn handle_tab_list(&mut self, id: String, params: TabListParams) -> String {
        let tabs = if let Some(workspace_id) = params.workspace_id {
            let Some(ws_idx) = self.parse_workspace_id(&workspace_id) else {
                return workspace_not_found(id, &workspace_id);
            };
            let Some(_) = self.state.workspaces.get(ws_idx) else {
                return workspace_not_found(id, &workspace_id);
            };
            self.tab_list_info(ws_idx)
        } else {
            let mut tabs = Vec::new();
            for (ws_idx, ws) in self.state.workspaces.iter().enumerate() {
                for tab_idx in 0..ws.tabs.len() {
                    if let Some(tab) = self.tab_info(ws_idx, tab_idx) {
                        tabs.push(tab);
                    }
                }
            }
            tabs
        };

        encode_success(id, ResponseResult::TabList { tabs })
    }

    pub(super) fn handle_tab_get(&mut self, id: String, target: TabTarget) -> String {
        let Some((ws_idx, tab_idx)) = self.parse_tab_id(&target.tab_id) else {
            return tab_not_found(id, &target.tab_id);
        };
        let Some(tab) = self.tab_info(ws_idx, tab_idx) else {
            return tab_not_found(id, &target.tab_id);
        };

        encode_success(id, ResponseResult::TabInfo { tab })
    }

    pub(super) fn handle_tab_create(&mut self, id: String, params: TabCreateParams) -> String {
        let source_ws_idx = match params.workspace_id.as_deref() {
            Some(workspace_id) => match self
                .parse_workspace_id(workspace_id)
                .filter(|index| self.state.workspaces.get(*index).is_some())
            {
                Some(index) => index,
                None => return workspace_not_found(id, workspace_id),
            },
            None => match self
                .state
                .active
                .filter(|index| self.state.workspaces.get(*index).is_some())
            {
                Some(index) => index,
                None => {
                    return encode_error(id, "workspace_not_found", "no active workspace");
                }
            },
        };
        let cwd = params.cwd.map(PathBuf::from).unwrap_or_else(|| {
            self.resolve_new_terminal_cwd(self.focused_pane_cwd_in_workspace(source_ws_idx))
        });
        let extra_env = match super::env::normalize_launch_env(params.env) {
            Ok(env) => env,
            Err((code, message)) => return encode_error(id, &code, message),
        };
        match self.create_workspace_with_launch_env(cwd, params.focus, extra_env) {
            Ok(index) => {
                if let Some(label) = params.label {
                    let workspace = &mut self.state.workspaces[index];
                    workspace.set_custom_name(label.clone());
                    workspace.tabs[0].set_custom_name(label);
                    crate::logging::workspace_renamed(&workspace.id);
                }
                self.emit_workspace_open_events(index);
                encode_success(
                    id,
                    self.tab_created_result(index, 0)
                        .expect("new workspace should produce a complete tab create response"),
                )
            }
            Err(err) => encode_error(id, "tab_create_failed", err.to_string()),
        }
    }

    pub(super) fn handle_tab_focus(&mut self, id: String, target: TabTarget) -> String {
        let Some((ws_idx, _)) = self.parse_tab_id(&target.tab_id) else {
            return tab_not_found(id, &target.tab_id);
        };
        self.focus_workspace_index(ws_idx);
        let Some(active_tab_idx) = self
            .state
            .workspaces
            .get(ws_idx)
            .map(crate::workspace::Workspace::active_tab_index)
        else {
            return tab_not_found(id, &target.tab_id);
        };
        let Some(tab) = self.tab_info(ws_idx, active_tab_idx) else {
            return tab_not_found(id, &target.tab_id);
        };

        encode_success(id, ResponseResult::TabInfo { tab })
    }

    pub(super) fn handle_tab_rename(&mut self, id: String, _params: TabRenameParams) -> String {
        multi_tab_unsupported(id)
    }

    pub(super) fn handle_tab_move(&mut self, id: String, _params: TabMoveParams) -> String {
        multi_tab_unsupported(id)
    }

    pub(super) fn handle_tab_close(&mut self, id: String, target: TabTarget) -> String {
        let Some((ws_idx, _)) = self.parse_tab_id(&target.tab_id) else {
            return tab_not_found(id, &target.tab_id);
        };
        if let Err(response) = self
            .require_restored_group_close_ready(&id, &self.state.workspace_close_indices(ws_idx))
        {
            return response;
        }
        match self.close_workspace_index(ws_idx, false) {
            Ok(()) => encode_success(id, ResponseResult::Ok {}),
            Err(super::workspaces::WorkspaceCloseError::GroupRequired) => {
                super::workspaces::workspace_close_group_required(id)
            }
        }
    }

    fn tab_list_info(&self, ws_idx: usize) -> Vec<crate::api::schema::TabInfo> {
        self.state
            .workspaces
            .get(ws_idx)
            .map(|ws| {
                (0..ws.tabs.len())
                    .filter_map(|idx| self.tab_info(ws_idx, idx))
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn workspace_not_found(id: String, workspace_id: &str) -> String {
    encode_error(
        id,
        "workspace_not_found",
        format!("workspace {workspace_id} not found"),
    )
}

fn tab_not_found(id: String, tab_id: &str) -> String {
    encode_error(id, "tab_not_found", format!("tab {tab_id} not found"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        api::schema::{ErrorResponse, SuccessResponse},
        config::Config,
        workspace::Workspace,
    };

    fn app_with_workspace() -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state.workspaces = vec![Workspace::test_new("tabs")];
        app.state.active = Some(0);
        app.state.selected = 0;
        app
    }

    fn assert_multi_tab_unsupported(response: &str) {
        let error: ErrorResponse = serde_json::from_str(response).unwrap();
        assert_eq!(error.error.code, "multi_tab_unsupported");
        assert_eq!(
            error.error.message,
            "multi-tab mutations are not supported; use workspaces instead"
        );
    }

    fn tab_create_params(
        workspace_id: Option<String>,
        cwd: Option<String>,
        focus: bool,
        label: Option<String>,
        env: std::collections::HashMap<String, String>,
    ) -> TabCreateParams {
        TabCreateParams {
            workspace_id,
            cwd,
            focus,
            label,
            env,
        }
    }

    fn unique_test_dir(tag: &str) -> std::path::PathBuf {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "herdr-tab-create-{tag}-{}-{timestamp}",
            std::process::id()
        ))
    }

    fn assert_workspace_open_events(app: &App) {
        use crate::api::schema::EventKind;

        let events = app.event_hub.events_after(0);
        assert_eq!(
            events
                .iter()
                .map(|(_, event)| event.event)
                .collect::<Vec<_>>(),
            vec![
                EventKind::WorkspaceCreated,
                EventKind::TabCreated,
                EventKind::PaneCreated,
                EventKind::LayoutUpdated,
            ]
        );
    }

    #[test]
    fn api_tab_create_requires_a_valid_source_even_with_explicit_cwd() {
        let mut app = app_with_workspace();
        let cwd = std::env::temp_dir().display().to_string();

        let invalid_source = app.handle_tab_create(
            "invalid-source".into(),
            tab_create_params(
                Some("w_999".into()),
                Some(cwd.clone()),
                false,
                None,
                Default::default(),
            ),
        );
        let error: ErrorResponse = serde_json::from_str(&invalid_source).unwrap();
        assert_eq!(error.error.code, "workspace_not_found");

        app.state.active = None;
        let no_active_workspace = app.handle_tab_create(
            "no-active".into(),
            tab_create_params(None, Some(cwd), false, None, Default::default()),
        );
        let error: ErrorResponse = serde_json::from_str(&no_active_workspace).unwrap();
        assert_eq!(error.error.code, "workspace_not_found");
        assert_eq!(error.error.message, "no active workspace");
        assert_eq!(app.state.workspaces.len(), 1);
        assert_eq!(app.state.workspaces[0].tabs.len(), 1);
        assert!(!app.state.session_dirty);
        assert!(app.event_hub.events_after(0).is_empty());
    }

    #[test]
    fn api_tab_create_rejects_invalid_env_without_side_effects() {
        let mut app = app_with_workspace();

        let source_workspace_id = app.public_workspace_id(0);
        let response = app.handle_tab_create(
            "req".into(),
            tab_create_params(
                Some(source_workspace_id),
                Some(std::env::temp_dir().display().to_string()),
                false,
                None,
                std::collections::HashMap::from([("BAD=KEY".into(), "value".into())]),
            ),
        );

        let error: ErrorResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(error.error.code, "invalid_env");
        assert_eq!(app.state.workspaces.len(), 1);
        assert_eq!(app.state.workspaces[0].tabs.len(), 1);
        assert!(!app.state.session_dirty);
        assert!(app.event_hub.events_after(0).is_empty());
    }

    #[tokio::test]
    async fn api_tab_create_redirects_to_a_new_focused_workspace() {
        use super::super::test_support::{exiting_test_command, shutdown_test_runtimes};
        use crate::config::ShellModeConfig;

        let base = unique_test_dir("focused");
        let source_cwd = base.join("source");
        let requested_cwd = base.join("requested");
        std::fs::create_dir_all(&source_cwd).unwrap();
        std::fs::create_dir_all(&requested_cwd).unwrap();

        let mut app = app_with_workspace();
        app.state.default_shell = exiting_test_command().into();
        app.state.shell_mode = ShellModeConfig::NonLogin;
        app.state.ensure_test_terminals();
        let source_pane = app.state.workspaces[0].focused_pane_id().unwrap();
        let source_terminal = app.state.workspaces[0]
            .terminal_id(source_pane)
            .unwrap()
            .clone();
        app.state.terminals.get_mut(&source_terminal).unwrap().cwd = source_cwd;
        let source_workspace_id = app.public_workspace_id(0);
        let source_tab_id = app.public_tab_id(0, 0).unwrap();
        let source_pane_id = app.public_pane_id(0, source_pane).unwrap();

        let response = app.handle_tab_create(
            "req".into(),
            tab_create_params(
                Some(source_workspace_id.clone()),
                Some(requested_cwd.display().to_string()),
                true,
                Some("build logs".into()),
                Default::default(),
            ),
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::TabCreated { tab, root_pane } = success.result else {
            panic!("expected tab_created response");
        };
        assert_eq!(app.public_workspace_id(0), source_workspace_id);
        assert_eq!(app.public_tab_id(0, 0).unwrap(), source_tab_id);
        assert_eq!(app.public_pane_id(0, source_pane).unwrap(), source_pane_id);
        assert_eq!(app.public_tab_id(1, 0).unwrap(), tab.tab_id);
        assert_eq!(
            app.public_pane_id(1, app.state.workspaces[1].tabs[0].root_pane)
                .unwrap(),
            root_pane.pane_id
        );
        let new_workspace_id = app.public_workspace_id(1);
        assert_ne!(new_workspace_id, source_workspace_id);
        assert_ne!(tab.tab_id, source_tab_id);
        assert_ne!(root_pane.pane_id, source_pane_id);
        assert_eq!(tab.workspace_id, new_workspace_id);
        assert_eq!(root_pane.workspace_id, new_workspace_id);
        assert_eq!(root_pane.tab_id, tab.tab_id);
        assert_eq!(tab.number, 1);
        assert_eq!(tab.label, "build logs");
        assert!(tab.focused);
        assert!(root_pane.focused);
        assert_eq!(app.state.workspaces.len(), 2);
        assert_eq!(app.state.workspaces[0].tabs.len(), 1);
        assert_eq!(app.state.workspaces[1].tabs.len(), 1);
        assert_eq!(app.state.workspaces[1].identity_cwd, requested_cwd);
        assert_eq!(
            app.state.workspaces[1].custom_name.as_deref(),
            Some("build logs")
        );
        assert_eq!(
            app.state.workspaces[1].tabs[0].custom_name.as_deref(),
            Some("build logs")
        );
        assert_eq!(app.state.active, Some(1));
        assert_eq!(app.state.selected, 1);
        assert_workspace_open_events(&app);

        shutdown_test_runtimes(&mut app);
        let _ = std::fs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn api_tab_create_inherits_focused_pane_cwd_without_focusing_new_workspace() {
        use super::super::test_support::{exiting_test_command, shutdown_test_runtimes};
        use crate::config::ShellModeConfig;
        use ratatui::layout::Direction;

        let base = unique_test_dir("inherited");
        let root_cwd = base.join("root");
        let focused_cwd = base.join("focused");
        std::fs::create_dir_all(&root_cwd).unwrap();
        std::fs::create_dir_all(&focused_cwd).unwrap();

        let mut app = app_with_workspace();
        app.state.default_shell = exiting_test_command().into();
        app.state.shell_mode = ShellModeConfig::NonLogin;
        let focused_pane = app.state.workspaces[0].test_split(Direction::Horizontal);
        app.state.workspaces[0].tabs[0]
            .layout
            .focus_pane(focused_pane);
        app.state.ensure_test_terminals();
        let root_pane = app.state.workspaces[0].tabs[0].root_pane;
        let root_terminal = app.state.workspaces[0]
            .terminal_id(root_pane)
            .unwrap()
            .clone();
        let focused_terminal = app.state.workspaces[0]
            .terminal_id(focused_pane)
            .unwrap()
            .clone();
        app.state.terminals.get_mut(&root_terminal).unwrap().cwd = root_cwd;
        app.state.terminals.get_mut(&focused_terminal).unwrap().cwd = focused_cwd.clone();
        let source_workspace_id = app.public_workspace_id(0);
        let source_tab_id = app.public_tab_id(0, 0).unwrap();

        let response = app.handle_tab_create(
            "req".into(),
            tab_create_params(None, None, false, None, Default::default()),
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::TabCreated { tab, root_pane } = success.result else {
            panic!("expected tab_created response");
        };
        assert_eq!(app.public_workspace_id(0), source_workspace_id);
        assert_eq!(app.public_tab_id(0, 0).unwrap(), source_tab_id);
        assert_ne!(tab.workspace_id, source_workspace_id);
        assert_ne!(tab.tab_id, source_tab_id);
        assert!(!tab.focused);
        assert!(!root_pane.focused);
        assert_eq!(app.state.workspaces[1].identity_cwd, focused_cwd);
        assert_eq!(app.state.workspaces[0].tabs.len(), 1);
        assert_eq!(app.state.workspaces[0].tabs[0].panes.len(), 2);
        assert_eq!(app.state.active, Some(0));
        assert_eq!(app.state.selected, 0);
        assert_workspace_open_events(&app);

        shutdown_test_runtimes(&mut app);
        let _ = std::fs::remove_dir_all(base);
    }
    #[tokio::test]
    async fn api_tab_create_uses_explicit_source_workspace_for_inherited_cwd() {
        use super::super::test_support::{exiting_test_command, shutdown_test_runtimes};
        use crate::config::ShellModeConfig;
        use ratatui::layout::Direction;

        let base = unique_test_dir("explicit-source");
        let active_cwd = base.join("active");
        let source_root_cwd = base.join("source-root");
        let source_focused_cwd = base.join("source-focused");
        for cwd in [&active_cwd, &source_root_cwd, &source_focused_cwd] {
            std::fs::create_dir_all(cwd).unwrap();
        }

        let mut app = app_with_workspace();
        app.state.workspaces.push(Workspace::test_new("source"));
        app.state.default_shell = exiting_test_command().into();
        app.state.shell_mode = ShellModeConfig::NonLogin;
        let source_focused_pane = app.state.workspaces[1].test_split(Direction::Horizontal);
        app.state.workspaces[1].tabs[0]
            .layout
            .focus_pane(source_focused_pane);
        app.state.ensure_test_terminals();
        let active_pane = app.state.workspaces[0].focused_pane_id().unwrap();
        let active_terminal = app.state.workspaces[0]
            .terminal_id(active_pane)
            .unwrap()
            .clone();
        let source_root_pane = app.state.workspaces[1].tabs[0].root_pane;
        let source_root_terminal = app.state.workspaces[1]
            .terminal_id(source_root_pane)
            .unwrap()
            .clone();
        let source_focused_terminal = app.state.workspaces[1]
            .terminal_id(source_focused_pane)
            .unwrap()
            .clone();
        app.state.terminals.get_mut(&active_terminal).unwrap().cwd = active_cwd.clone();
        app.state
            .terminals
            .get_mut(&source_root_terminal)
            .unwrap()
            .cwd = source_root_cwd;
        app.state
            .terminals
            .get_mut(&source_focused_terminal)
            .unwrap()
            .cwd = source_focused_cwd.clone();
        let active_workspace_id = app.public_workspace_id(0);
        let source_workspace_id = app.public_workspace_id(1);
        let source_tab_id = app.public_tab_id(1, 0).unwrap();

        let response = app.handle_tab_create(
            "req".into(),
            tab_create_params(
                Some(source_workspace_id.clone()),
                None,
                false,
                None,
                Default::default(),
            ),
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::TabCreated { tab, .. } = success.result else {
            panic!("expected tab_created response");
        };
        assert_ne!(tab.workspace_id, active_workspace_id);
        assert_ne!(tab.workspace_id, source_workspace_id);
        assert_ne!(tab.tab_id, source_tab_id);
        assert_eq!(app.state.workspaces[2].identity_cwd, source_focused_cwd);
        assert_eq!(app.state.active, Some(0));
        assert_eq!(app.state.selected, 0);
        assert_eq!(app.state.workspaces[0].tabs.len(), 1);
        assert_eq!(app.state.workspaces[1].tabs.len(), 1);
        assert_eq!(app.state.workspaces[1].tabs[0].panes.len(), 2);
        assert_workspace_open_events(&app);

        shutdown_test_runtimes(&mut app);
        let _ = std::fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn api_tab_create_passes_env_to_initial_terminal() {
        use super::super::test_support::shutdown_test_runtimes;
        use crate::config::ShellModeConfig;
        use std::os::unix::fs::PermissionsExt;

        let base = unique_test_dir("env");
        std::fs::create_dir_all(&base).unwrap();
        let shell = base.join("capture-env.sh");
        let output = base.join("captured-env");
        std::fs::write(
            &shell,
            "#!/bin/sh\nprintf '%s' \"$HERDR_TEST_TAB_CREATE_ENV\" > \"$HERDR_TEST_TAB_CREATE_OUTPUT\"\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&shell).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&shell, permissions).unwrap();

        let mut app = app_with_workspace();
        app.state.default_shell = shell.display().to_string();
        app.state.shell_mode = ShellModeConfig::NonLogin;
        let source_workspace_id = app.public_workspace_id(0);
        let response = app.handle_tab_create(
            "req".into(),
            tab_create_params(
                Some(source_workspace_id),
                Some(base.display().to_string()),
                false,
                None,
                std::collections::HashMap::from([
                    ("HERDR_TEST_TAB_CREATE_ENV".into(), "passed-through".into()),
                    (
                        "HERDR_TEST_TAB_CREATE_OUTPUT".into(),
                        output.display().to_string(),
                    ),
                ]),
            ),
        );
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert!(matches!(success.result, ResponseResult::TabCreated { .. }));

        for _ in 0..500 {
            if std::fs::read_to_string(&output).is_ok_and(|value| value == "passed-through") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(std::fs::read_to_string(&output).unwrap(), "passed-through");

        shutdown_test_runtimes(&mut app);
        let _ = std::fs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn api_tab_create_spawn_failure_has_no_side_effects() {
        use super::super::test_support::shutdown_test_runtimes;
        use crate::config::ShellModeConfig;

        let base = unique_test_dir("spawn-failure");
        std::fs::create_dir_all(&base).unwrap();
        let mut app = app_with_workspace();
        app.state.default_shell = base.join("missing-shell").display().to_string();
        app.state.shell_mode = ShellModeConfig::NonLogin;
        let source_workspace_id = app.public_workspace_id(0);

        let response = app.handle_tab_create(
            "req".into(),
            tab_create_params(
                Some(source_workspace_id),
                Some(base.display().to_string()),
                false,
                None,
                Default::default(),
            ),
        );

        let error: ErrorResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(error.error.code, "tab_create_failed");
        assert_eq!(app.state.workspaces.len(), 1);
        assert_eq!(app.state.workspaces[0].tabs.len(), 1);
        assert_eq!(app.state.active, Some(0));
        assert!(!app.state.session_dirty);
        assert!(app.event_hub.events_after(0).is_empty());

        shutdown_test_runtimes(&mut app);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn api_tab_focus_preserves_owning_spaces_active_legacy_tab() {
        let mut app = app_with_workspace();
        let mut background = Workspace::test_new("background");
        let requested_tab = background.test_add_tab(Some("requested"));
        let active_tab = background.test_add_tab(Some("already active"));
        background.active_tab = active_tab;
        app.state.workspaces.push(background);
        app.state.session_dirty = false;
        let requested_tab_id = app.public_tab_id(1, requested_tab).unwrap();
        let active_tab_id = app.public_tab_id(1, active_tab).unwrap();

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req".into(),
            method: crate::api::schema::Method::TabFocus(TabTarget {
                tab_id: requested_tab_id.clone(),
            }),
        });

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::TabInfo { tab } = success.result else {
            panic!("expected tab info");
        };
        assert_eq!(tab.tab_id, active_tab_id);
        assert!(tab.focused);
        assert!(!app.tab_info(1, requested_tab).unwrap().focused);
        assert_eq!(app.state.active, Some(1));
        assert_eq!(app.state.selected, 1);
        assert_eq!(app.state.workspaces[1].active_tab_index(), active_tab);
        assert!(app.state.session_dirty);
    }

    #[test]
    fn api_tab_focus_invalid_id_has_no_navigation_side_effects() {
        let mut app = app_with_workspace();
        app.state.workspaces[0].test_add_tab(Some("legacy"));
        app.state.workspaces[0].active_tab = 0;
        app.state.session_dirty = false;
        let active_workspace = app.state.active;
        let selected_workspace = app.state.selected;
        let tab_ids = (0..app.state.workspaces[0].tabs.len())
            .map(|tab_index| app.public_tab_id(0, tab_index).unwrap())
            .collect::<Vec<_>>();

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req".into(),
            method: crate::api::schema::Method::TabFocus(TabTarget {
                tab_id: "missing-tab".into(),
            }),
        });

        let error: ErrorResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(error.error.code, "tab_not_found");
        assert_eq!(app.state.active, active_workspace);
        assert_eq!(app.state.selected, selected_workspace);
        assert_eq!(app.state.workspaces[0].active_tab_index(), 0);
        assert_eq!(
            (0..app.state.workspaces[0].tabs.len())
                .map(|tab_index| app.public_tab_id(0, tab_index).unwrap())
                .collect::<Vec<_>>(),
            tab_ids
        );
        assert!(!app.state.session_dirty);
        assert!(app.event_hub.events_after(0).is_empty());
    }

    #[test]
    fn api_tab_rename_is_rejected_without_events_or_session_dirty() {
        let mut app = app_with_workspace();
        let tab_id = app.public_tab_id(0, 0).unwrap();

        let response = app.handle_tab_rename(
            "req".into(),
            TabRenameParams {
                tab_id,
                label: "renamed".into(),
            },
        );

        assert_multi_tab_unsupported(&response);
        assert!(app.state.workspaces[0].tabs[0].custom_name.is_none());
        assert!(!app.state.session_dirty);
        assert!(app.event_hub.events_after(0).is_empty());
    }

    #[test]
    fn api_tab_move_is_rejected_without_events_or_session_dirty() {
        let mut app = app_with_workspace();
        app.state.workspaces[0].test_add_tab(Some("legacy"));
        let first_tab = app.state.workspaces[0].tabs[0].root_pane;
        let tab_id = app.public_tab_id(0, 0).unwrap();

        let response = app.handle_tab_move(
            "req".into(),
            TabMoveParams {
                tab_id,
                insert_index: 1,
            },
        );

        assert_multi_tab_unsupported(&response);
        assert_eq!(app.state.workspaces[0].tabs[0].root_pane, first_tab);
        assert!(!app.state.session_dirty);
        assert!(app.event_hub.events_after(0).is_empty());
    }

    #[test]
    fn api_tab_close_closes_owning_space_and_emits_workspace_closed_snapshot() {
        let mut app = app_with_workspace();
        app.state.workspaces[0].test_add_tab(Some("legacy"));
        app.state.workspaces.push(Workspace::test_new("survivor"));
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.session_dirty = false;
        let closed_workspace_id = app.public_workspace_id(0);
        let closed_workspace = app.workspace_info(0);
        let closed_pane_ids = app.state.pane_ids_for_workspace(0);
        let closed_terminal_ids = app.state.terminal_ids_for_workspace(0);
        let survivor_id = app.public_workspace_id(1);
        for pane_id in &closed_pane_ids {
            app.state.plugin_panes.insert(
                *pane_id,
                crate::app::state::PluginPaneRecord {
                    plugin_id: "example.pane".into(),
                    entrypoint: "board".into(),
                },
            );
        }
        let requested_tab_id = app.public_tab_id(0, 1).unwrap();

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req".into(),
            method: crate::api::schema::Method::TabClose(TabTarget {
                tab_id: requested_tab_id,
            }),
        });

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(success.result, ResponseResult::Ok {});
        assert_eq!(app.state.workspaces.len(), 1);
        assert_eq!(app.public_workspace_id(0), survivor_id);
        assert!(closed_terminal_ids
            .iter()
            .all(|terminal_id| !app.state.terminals.contains_key(terminal_id)));
        assert!(closed_pane_ids
            .iter()
            .all(|pane_id| !app.state.plugin_panes.contains_key(pane_id)));
        assert!(app.state.session_dirty);
        let events = app.event_hub.events_after(0);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0].1.data,
            crate::api::schema::EventData::WorkspaceClosed {
                workspace_id,
                workspace: Some(snapshot),
            } if workspace_id == &closed_workspace_id && snapshot == &closed_workspace
        ));
        assert_eq!(
            events[0].1.event,
            crate::api::schema::EventKind::WorkspaceClosed
        );
    }

    #[test]
    fn api_tab_close_invalid_id_has_no_close_side_effects() {
        let mut app = app_with_workspace();
        app.state.ensure_test_terminals();
        app.state.session_dirty = false;
        let workspace_ids = app
            .state
            .workspaces
            .iter()
            .map(|workspace| workspace.id.clone())
            .collect::<Vec<_>>();
        let terminal_ids = app.state.terminal_ids_for_workspace(0);
        let active_workspace = app.state.active;
        let selected_workspace = app.state.selected;

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req".into(),
            method: crate::api::schema::Method::TabClose(TabTarget {
                tab_id: "missing-tab".into(),
            }),
        });

        let error: ErrorResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(error.error.code, "tab_not_found");
        assert_eq!(app.state.active, active_workspace);
        assert_eq!(app.state.selected, selected_workspace);
        assert_eq!(
            app.state
                .workspaces
                .iter()
                .map(|workspace| workspace.id.clone())
                .collect::<Vec<_>>(),
            workspace_ids
        );
        assert!(terminal_ids
            .iter()
            .all(|terminal_id| app.state.terminals.contains_key(terminal_id)));
        assert!(!app.state.session_dirty);
        assert!(app.event_hub.events_after(0).is_empty());
    }

    #[test]
    fn api_tab_close_requires_explicit_group_for_parent_worktree_space() {
        let mut app = app_with_workspace();
        app.state.workspaces[0].worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
            key: "repo-key".into(),
            label: "herdr".into(),
            repo_root: "/repo/herdr".into(),
            checkout_path: "/repo/herdr".into(),
            is_linked_worktree: false,
        });
        let mut linked = Workspace::test_new("linked");
        linked.worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
            key: "repo-key".into(),
            label: "herdr".into(),
            repo_root: "/repo/herdr".into(),
            checkout_path: "/repo/herdr-linked".into(),
            is_linked_worktree: true,
        });
        app.state.workspaces.push(linked);
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.session_dirty = false;
        let workspace_ids = app
            .state
            .workspaces
            .iter()
            .map(|workspace| workspace.id.clone())
            .collect::<Vec<_>>();
        let terminal_ids = app.state.terminal_ids_for_workspace(0);
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        app.state.plugin_panes.insert(
            pane_id,
            crate::app::state::PluginPaneRecord {
                plugin_id: "example.pane".into(),
                entrypoint: "board".into(),
            },
        );
        let tab_id = app.public_tab_id(0, 0).unwrap();

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req".into(),
            method: crate::api::schema::Method::TabClose(TabTarget { tab_id }),
        });

        let error: ErrorResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(error.error.code, "workspace_group_close_required");
        assert_eq!(
            app.state
                .workspaces
                .iter()
                .map(|workspace| workspace.id.clone())
                .collect::<Vec<_>>(),
            workspace_ids
        );
        assert!(terminal_ids
            .iter()
            .all(|terminal_id| app.state.terminals.contains_key(terminal_id)));
        assert!(app.state.plugin_panes.contains_key(&pane_id));
        assert_eq!(app.state.active, Some(0));
        assert_eq!(app.state.selected, 0);
        assert!(!app.state.session_dirty);
        assert!(app.event_hub.events_after(0).is_empty());
    }
}
