use crate::core::mcp_event::get_mcp_socket_path;
use crate::core::mcp_ipc;
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStatus {
    pub embedded_enabled: bool,
    pub socket_path: String,
}

#[tauri::command]
pub fn get_mcp_status() -> McpStatus {
    McpStatus {
        embedded_enabled: mcp_ipc::is_embedded_enabled(),
        socket_path: get_mcp_socket_path().to_string_lossy().to_string(),
    }
}

#[tauri::command]
pub fn set_mcp_enabled(enabled: bool) -> McpStatus {
    mcp_ipc::set_embedded_enabled(enabled);
    get_mcp_status()
}
