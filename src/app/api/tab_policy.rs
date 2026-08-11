use super::responses::encode_error;

pub(super) const MULTI_TAB_UNSUPPORTED_MESSAGE: &str =
    "multi-tab mutations are not supported; use workspaces instead";

pub(super) fn multi_tab_unsupported(id: String) -> String {
    encode_error(id, "multi_tab_unsupported", MULTI_TAB_UNSUPPORTED_MESSAGE)
}
