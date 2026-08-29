use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpMutationEvent {
    pub tool: String,
    pub repo_path: String,
}

pub fn get_unix_socket_path() -> std::path::PathBuf {
    static PATH: OnceLock<std::path::PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
            let path = std::path::Path::new(&runtime_dir).join("penguingit-mcp.sock");
            return path;
        }

        let uid = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "default".to_string());

        std::path::PathBuf::from(format!("/tmp/penguingit-mcp-{}.sock", uid))
    })
    .clone()
}

static EVENT_BUS: OnceLock<broadcast::Sender<McpMutationEvent>> = OnceLock::new();

pub fn get_event_bus() -> &'static broadcast::Sender<McpMutationEvent> {
    EVENT_BUS.get_or_init(|| {
        let (tx, _rx) = broadcast::channel(100);
        tx
    })
}

/// Sends a mutation notification to both the in-process broadcast channel AND the Unix domain socket.
pub async fn notify_mcp_mutation(tool: &str, repo_path: &str) {
    let event = McpMutationEvent {
        tool: tool.to_string(),
        repo_path: repo_path.to_string(),
    };

    // 1. In-process broadcast bus
    let _ = get_event_bus().send(event.clone());

    // 2. Standalone IPC over Unix domain socket
    if let Ok(mut stream) = tokio::net::UnixStream::connect(get_unix_socket_path()).await {
        use tokio::io::AsyncWriteExt;
        if let Ok(json) = serde_json::to_string(&event) {
            let mut data = json.into_bytes();
            data.push(b'\n');
            let _ = stream.write_all(&data).await;
        }
    }
}
