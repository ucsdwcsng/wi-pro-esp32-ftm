use tokio::sync::mpsc;
use zeromq::{PubSocket, SocketSend, ZmqMessage, Socket};
use log::{info, error};
use std::collections::HashMap;
use crate::tlv::ESPEvent;

/// Generic publisher for ESPEvent data over ZMQ PUB sockets
/// Routes different event types to different IPC endpoints
pub struct IPCPublisher {
    tx: mpsc::Sender<ESPEvent>,
}

impl IPCPublisher {
    /// Create a new publisher and spawn the publishing task
    /// Returns the publisher handle and a join handle for the task
    pub fn new(buffer_size: usize) -> (Self, tokio::task::JoinHandle<()>) {
        let (tx, rx) = mpsc::channel(buffer_size);

        let task_handle = tokio::spawn(async move {
            if let Err(e) = run_publisher(rx).await {
                error!("IPC publisher error: {}", e);
            }
        });

        (Self { tx }, task_handle)
    }

    /// Send an ESPEvent to be published
    /// Returns Ok(()) if queued successfully, Err if channel is full/closed
    pub async fn send(&self, event: ESPEvent) -> Result<(), mpsc::error::SendError<ESPEvent>> {
        self.tx.send(event).await
    }

    /// Try to send an ESPEvent without blocking
    /// Returns Ok(()) if queued successfully, Err if channel is full/closed
    pub fn try_send(&self, event: ESPEvent) -> Result<(), mpsc::error::TrySendError<ESPEvent>> {
        self.tx.try_send(event)
    }
}

/// Socket manager for different event types
struct SocketManager {
    sockets: HashMap<String, PubSocket>,
}

impl SocketManager {
    fn new() -> Self {
        Self {
            sockets: HashMap::new(),
        }
    }

    /// Get or create a socket for the given IPC address
    async fn get_socket(&mut self, ipc_path: &str) -> Result<&mut PubSocket, Box<dyn std::error::Error>> {
    if !self.sockets.contains_key(ipc_path) {
        // Remove stale socket file if it exists
        if let Some(file_path) = ipc_path.strip_prefix("ipc://") {
            if std::path::Path::new(file_path).exists() {
                std::fs::remove_file(file_path)?;
                info!("Removed stale IPC socket file: {}", file_path);
            }
        }

        let mut socket = PubSocket::new();
        socket.bind(ipc_path).await?;
        info!("IPC publisher: bound to {}", ipc_path);
        self.sockets.insert(ipc_path.to_string(), socket);
    }
    Ok(self.sockets.get_mut(ipc_path).unwrap())
    }

    /// Publish an event to the appropriate socket
    async fn publish(&mut self, event: &ESPEvent) -> Result<(), Box<dyn std::error::Error>> {
        let (ipc_path, serialized) = match event {
            ESPEvent::CSI(csi_event) => {
                let path = "ipc:///tmp/wipro_csi_data";
                let data = rmp_serde::to_vec(csi_event)?;
                (path, data)
            }
            ESPEvent::FTM(ftm_event) => {
                let path = "ipc:///tmp/wipro_ftm_data";
                let data = rmp_serde::to_vec(ftm_event)?;
                (path, data)
            }
	    ESPEvent::DBG(dbg_event) => {
                let path = "ipc:///tmp/wipro_dbg_data";
                let data = rmp_serde::to_vec(dbg_event)?;
                (path, data)
            }
            ESPEvent::Range(range_event) => {
                let path = "ipc:///tmp/wipro_range_data";
                let data = rmp_serde::to_vec(range_event)?;
                (path, data)
            }
        };

        let socket = self.get_socket(ipc_path).await?;
        let msg = ZmqMessage::from(serialized);
        socket.send(msg).await?;
        Ok(())
    }
}

/// Internal task that publishes ESPEvents to appropriate ZMQ PUB sockets
async fn run_publisher(mut rx: mpsc::Receiver<ESPEvent>) -> Result<(), Box<dyn std::error::Error>> {
    info!("IPC publisher started");
    info!("  CSI events  → ipc:///tmp/wipro_csi_data");
    info!("  FTM events  → ipc:///tmp/wipro_ftm_data");
    let mut socket_manager = SocketManager::new();

    while let Some(event) = rx.recv().await {
        if let Err(e) = socket_manager.publish(&event).await {
            error!("Error publishing {} event: {}", event.id_str(), e);
            // Continue processing other messages
        }
    }

    info!("IPC publisher shutting down");
    Ok(())
}

