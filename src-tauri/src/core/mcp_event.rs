use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpMutationEvent {
    pub tool: String,
    pub repo_path: String,
}

pub fn get_mcp_socket_path() -> std::path::PathBuf {
    use std::fs::DirBuilder;
    #[cfg(unix)]
    use std::os::unix::fs::DirBuilderExt;

    // 1. Try XDG_RUNTIME_DIR
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        let path = std::path::PathBuf::from(runtime_dir);
        if path.is_absolute() {
            let socket_dir = path.join("penguingit");
            let mut builder = DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            builder.mode(0o700);
            if builder.create(&socket_dir).is_ok() {
                return socket_dir.join("penguingit-mcp.sock");
            }
        }
    }

    // 2. Fallback to ~/.config/penguingit
    let config_dir = std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .unwrap_or_else(|_| std::env::temp_dir())
        .join("penguingit");

    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    let _ = builder.create(&config_dir);

    config_dir.join("mcp.sock")
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
    let socket_path = get_mcp_socket_path();
    if let Ok(mut stream) = tokio::net::UnixStream::connect(&socket_path).await {
        use tokio::io::AsyncWriteExt;
        if let Ok(json) = serde_json::to_string(&event) {
            let mut data = json.into_bytes();
            data.push(b'\n');
            let _ = stream.write_all(&data).await;
        }
    }
}
