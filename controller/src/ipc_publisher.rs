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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tlv::{CSIEvent, FTMEvent, FTMReport, PPSEvent, TimeEvent};

    #[tokio::test]
    async fn test_ipc_publisher_creation() {
        let (publisher, _handle) = IPCPublisher::new(10);

        let csi_event = CSIEvent {
            t_ms: 1000,
            seq: 1,
            own_mac: "AA:BB:CC:DD:EE:FF".to_string(),
            tgt_mac: "11:22:33:44:55:66".to_string(),
            timestamp: 12345,
            channel: 1,
            channel2: 0,
            rssi: -50,
            payload_b64: "test".to_string(),
        };

        // Should successfully send
        assert!(publisher.send(ESPEvent::CSI(csi_event)).await.is_ok());
    }

    #[tokio::test]
    async fn test_all_event_types() {
        let (publisher, _handle) = IPCPublisher::new(100);

        // CSI
        let csi = ESPEvent::CSI(CSIEvent {
            t_ms: 1000,
            seq: 1,
            own_mac: "AA:BB:CC:DD:EE:FF".to_string(),
            tgt_mac: "11:22:33:44:55:66".to_string(),
            timestamp: 12345,
            channel: 1,
            channel2: 0,
            rssi: -50,
            payload_b64: "test".to_string(),
        });
        assert!(publisher.send(csi).await.is_ok());

        // FTM
        let ftm = ESPEvent::FTM(FTMEvent {
            t_ms: 1000,
            own_mac: "AA:BB:CC:DD:EE:FF".to_string(),
            tgt_mac: "11:22:33:44:55:66".to_string(),
            seq: 1,
            reports: vec![FTMReport {
                own_mac: "AA:BB:CC:DD:EE:FF".to_string(),
                tgt_mac: "11:22:33:44:55:66".to_string(),
                dlog_token: 1,
                rssi: -45,
                t1: 100,
                t2: 200,
                t3: 300,
                t4: 400,
            }],
        });
        assert!(publisher.send(ftm).await.is_ok());

        // PPS
        let pps = ESPEvent::PPS(PPSEvent {
            own_mac: "AA:BB:CC:DD:EE:FF".to_string(),
            t_ms: 1000,
            timestamp_esp: 12345,
            timestamp_mac: 12346,
            internal_offset: 10,
            compensated_mac_time: 12355,
            frac: 500,
        });
        assert!(publisher.send(pps).await.is_ok());

        // Time
        let time = ESPEvent::Time(TimeEvent {
            own_mac: "AA:BB:CC:DD:EE:FF".to_string(),
            tgt_mac: "11:22:33:44:55:66".to_string(),
            t_ms: 1000,
            t1: 100,
            t2: 200,
            t3: 300,
            t4: 400,
        });
        assert!(publisher.send(time).await.is_ok());
    }
}
