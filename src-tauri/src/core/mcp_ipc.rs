use rmcp::ServiceExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncReadExt};
use tokio::net::UnixListener;

use crate::core::mcp_event::{get_event_bus, get_unix_socket_path, McpMutationEvent};
use crate::core::mcp_server::PenguinMcpServer;
use crate::core::repo::AppState;

pub static EMBEDDED_MCP_ENABLED: AtomicBool = AtomicBool::new(false);

/// Event emitted to frontend when an MCP tool mutates a repository.
pub const MCP_EVENT: &str = "mcp-event";
/// Repository changed event trigger for live GUI refresh.
pub const REPO_CHANGED_EVENT: &str = "repo-changed";

pub fn is_embedded_enabled() -> bool {
    EMBEDDED_MCP_ENABLED.load(Ordering::Relaxed)
}

pub fn set_embedded_enabled(enabled: bool) {
    EMBEDDED_MCP_ENABLED.store(enabled, Ordering::Relaxed);
}

/// Spawns the Unix domain socket listener and in-process broadcast channel listener for MCP events.
pub fn start_mcp_event_listeners(app_handle: AppHandle) {
    let app = Arc::new(app_handle);

    // 1. In-process broadcast listener (embedded mode)
    let app_broadcast = Arc::clone(&app);
    let mut rx = get_event_bus().subscribe();
    tauri::async_runtime::spawn(async move {
        while let Ok(event) = rx.recv().await {
            emit_mcp_event(&app_broadcast, &event.tool, &event.repo_path);
        }
    });

    // 2. Standalone IPC / Embedded MCP Server over Unix domain socket
    let app_socket = Arc::clone(&app);
    tauri::async_runtime::spawn(async move {
        // Clean up any existing socket file from previous runs
        let socket_path = get_unix_socket_path();
        let _ = std::fs::remove_file(&socket_path);

        if let Ok(listener) = UnixListener::bind(&socket_path) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(metadata) = std::fs::metadata(&socket_path) {
                    let mut perms = metadata.permissions();
                    perms.set_mode(0o600); // Read/write only for owner.
                    let _ = std::fs::set_permissions(&socket_path, perms);
                }
            }
            loop {
                if let Ok((stream, _)) = listener.accept().await {
                    let app_conn = Arc::clone(&app_socket);
                    tokio::spawn(async move {
                        let mut reader = tokio::io::BufReader::new(stream);
                        let mut line = String::new();
                        if reader.read_line(&mut line).await.is_ok() && !line.trim().is_empty() {
                            if !line.contains("\"jsonrpc\"") {
                                if let Ok(event) = serde_json::from_str::<McpMutationEvent>(&line) {
                                    emit_mcp_event(&app_conn, &event.tool, &event.repo_path);
                                    return;
                                }
                            }

                            if is_embedded_enabled() {
                                let remaining_buffered = reader.buffer().to_vec();
                                let stream = reader.into_inner();
                                let (read_half, write_half) = stream.into_split();
                                let chained_read = std::io::Cursor::new(line.into_bytes())
                                    .chain(std::io::Cursor::new(remaining_buffered))
                                    .chain(read_half);
                                let server = PenguinMcpServer::new();
                                let _ = server.serve((chained_read, write_half)).await;
                            }
                        }
                    });
                }
            }
        }
    });
}

fn emit_mcp_event(app: &AppHandle, tool: &str, repo_path: &str) {
    let payload = serde_json::json!({
        "tool": tool,
        "repo_path": repo_path,
        "toast": format!("MCP: committed via {}", tool),
    });

    let _ = app.emit(MCP_EVENT, &payload);

    let repo_id = if let Some(state) = app.try_state::<AppState>() {
        if let Ok(canon_path) = std::fs::canonicalize(repo_path) {
            let repos = state.list();
            repos
                .iter()
                .find(|r| {
                    std::fs::canonicalize(&r.path)
                        .map(|p| p == canon_path)
                        .unwrap_or(false)
                })
                .map(|r| r.id.0.clone())
                .unwrap_or_else(|| repo_path.to_string())
        } else {
            repo_path.to_string()
        }
    } else {
        repo_path.to_string()
    };

    let _ = app.emit(
        REPO_CHANGED_EVENT,
        &serde_json::json!({ "repoId": repo_id }),
    );
}
