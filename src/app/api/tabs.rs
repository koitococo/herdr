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

    pub(super) fn handle_tab_create(&mut self, id: String, _params: TabCreateParams) -> String {
        multi_tab_unsupported(id)
    }

    pub(super) fn handle_tab_focus(&mut self, id: String, target: TabTarget) -> String {
        let Some((ws_idx, tab_idx)) = self.parse_tab_id(&target.tab_id) else {
            return tab_not_found(id, &target.tab_id);
        };
        let Some(tab) = self.tab_info(ws_idx, tab_idx) else {
            return tab_not_found(id, &target.tab_id);
        };
        let is_current_active_tab = self.state.active == Some(ws_idx)
            && self
                .state
                .workspaces
                .get(ws_idx)
                .is_some_and(|ws| ws.active_tab_index() == tab_idx);
        if !is_current_active_tab {
            return multi_tab_unsupported(id);
        }

        encode_success(id, ResponseResult::TabInfo { tab })
    }

    pub(super) fn handle_tab_rename(&mut self, id: String, _params: TabRenameParams) -> String {
        multi_tab_unsupported(id)
    }

    pub(super) fn handle_tab_move(&mut self, id: String, _params: TabMoveParams) -> String {
        multi_tab_unsupported(id)
    }

    pub(super) fn handle_tab_close(&mut self, id: String, _target: TabTarget) -> String {
        multi_tab_unsupported(id)
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
            true,
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

    #[test]
    fn api_tab_create_is_rejected_without_events_or_session_dirty() {
        let mut app = app_with_workspace();

        let response = app.handle_tab_create(
            "req".into(),
            TabCreateParams {
                workspace_id: None,
                cwd: None,
                focus: false,
                label: None,
                env: Default::default(),
            },
        );

        assert_multi_tab_unsupported(&response);
        assert_eq!(app.state.workspaces[0].tabs.len(), 1);
        assert!(!app.state.session_dirty);
        assert!(app.event_hub.events_after(0).is_empty());
    }

    #[test]
    fn api_tab_focus_rejects_legacy_non_active_tab_without_switching() {
        let mut app = app_with_workspace();
        app.state.workspaces[0].test_add_tab(Some("legacy"));
        app.state.workspaces[0].active_tab = 0;
        let inactive_tab_id = app.public_tab_id(0, 1).unwrap();

        let response = app.handle_tab_focus(
            "req".into(),
            TabTarget {
                tab_id: inactive_tab_id,
            },
        );

        assert_multi_tab_unsupported(&response);
        assert_eq!(app.state.active, Some(0));
        assert_eq!(app.state.workspaces[0].active_tab, 0);
        assert!(!app.state.session_dirty);
        assert!(app.event_hub.events_after(0).is_empty());
    }

    #[test]
    fn api_tab_focus_rejects_background_workspace_tab_without_switching() {
        let mut app = app_with_workspace();
        app.state.workspaces.push(Workspace::test_new("background"));
        let background_tab_id = app.public_tab_id(1, 0).unwrap();

        let response = app.handle_tab_focus(
            "req".into(),
            TabTarget {
                tab_id: background_tab_id,
            },
        );

        assert_multi_tab_unsupported(&response);
        assert_eq!(app.state.active, Some(0));
        assert_eq!(app.state.workspaces[1].active_tab, 0);
        assert!(!app.state.session_dirty);
        assert!(app.event_hub.events_after(0).is_empty());
    }

    #[test]
    fn api_tab_focus_current_active_tab_succeeds_without_switching() {
        let mut app = app_with_workspace();
        let active_tab_id = app.public_tab_id(0, 0).unwrap();

        let response = app.handle_tab_focus(
            "req".into(),
            TabTarget {
                tab_id: active_tab_id.clone(),
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::TabInfo { tab } = success.result else {
            panic!("expected tab info");
        };
        assert_eq!(tab.tab_id, active_tab_id);
        assert_eq!(app.state.active, Some(0));
        assert_eq!(app.state.workspaces[0].active_tab, 0);
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
    fn api_tab_close_is_rejected_without_events_or_session_dirty() {
        let mut app = app_with_workspace();
        let tab_id = app.public_tab_id(0, 0).unwrap();

        let response = app.handle_tab_close("req".into(), TabTarget { tab_id });

        assert_multi_tab_unsupported(&response);
        assert_eq!(app.state.workspaces.len(), 1);
        assert_eq!(app.state.workspaces[0].tabs.len(), 1);
        assert!(!app.state.session_dirty);
        assert!(app.event_hub.events_after(0).is_empty());
    }
}
