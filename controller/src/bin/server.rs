use std::collections::HashMap;
use std::io;
use std::fs::File;
use zeromq::{RouterSocket, SocketRecv, SocketSend, ZmqMessage, Socket};
use tokio::sync::mpsc;
use log::{info, error};
use std::sync::atomic::{AtomicU32, Ordering};


use controller::srv::*;
use controller::tui::*;
use controller::tlv::ESPEvent;
use controller::ipc_publisher::IPCPublisher;
use controller::config::ServerConfig;
use controller::esp_io::{OutputFiles, write_event};

static CSI_COUNTER: AtomicU32 = AtomicU32::new(0);


async fn send_to_client(
    socket: &mut RouterSocket,
    client_id: &ClientId,
    message: &str
) -> Result<(), Box<dyn std::error::Error>> {
    let mut msg = ZmqMessage::from(client_id.clone());
    msg.push_back(vec![].into());
    msg.push_back(message.as_bytes().to_vec().into());
    socket.send(msg).await?;
    Ok(())
}

async fn run_server(
    mut cmd_rx: mpsc::Receiver<Command>,
    event_tx: mpsc::Sender<ServerEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    let bind_addr = "tcp://0.0.0.0:5555";
    event_tx.send(ServerEvent::Log(format!("Starting ZMQ server on {}", bind_addr))).await.ok();

    let mut socket = RouterSocket::new();
    socket.bind(bind_addr).await?;
    
    let mut clients: HashMap<ClientId, String> = HashMap::new();

    event_tx.send(ServerEvent::Log("Server listening...".to_string())).await.ok();

    ServerConfig::init().unwrap();
    let cfg = ServerConfig::get();

    let mut out_files: Option<OutputFiles> = {
        if let Some(output_path) = cfg.output_dir.clone() {
            let out_file_path = output_path.join("raw.dat");
            info!("Logging raw data in {} ...", out_file_path.display());
            Some(OutputFiles {
                raw_file: File::create(output_path.join("raw.dat")).unwrap(),
                ftm_file: File::create(output_path.join("ftm.csv")).unwrap(),
                csi_file: File::create(output_path.join("csi.csv")).unwrap(),
		dbg_file: File::create(output_path.join("dbg.csv")).unwrap(),
		range_file: File::create(output_path.join("range.csv")).unwrap(),
            })
        } else {
            None
        }
    };


    let (ipc_publisher, _ipc_handle) = IPCPublisher::new(1024);
    loop {
        tokio::select! {
	    msg_result = socket.recv() => {
		match msg_result {
                    Ok(msg) => {
			let parts: Vec<Vec<u8>> = msg.into_vec()
			    .into_iter()
			    .map(|bytes| bytes.to_vec())
			    .collect();
                        if parts.len() < 2 {
                            continue;
                        }
                        let client_id = parts[0].clone();
                        let client_id_str = String::from_utf8_lossy(&client_id).to_string();
                        let payload: ESPEvent = {
			    match rmp_serde::from_slice::<ESPEvent>(&parts[1]) {
				Ok(event) => {
				    // event_tx.send(
				    // 	ServerEvent::Log(
				    // 	    format!("[{}] Received event: {}",
				    // 		    client_id_str,
				    // 		    event.id_str()))).await.ok();
				    event
				}
				Err(e) => {
				    event_tx.send(
					ServerEvent::Log(
					    format!("Error deserializing: {}", e))).await.ok();
				    continue;
				}
			    }
			};

                        if !clients.contains_key(&client_id) {
                            clients.insert(client_id.clone(), client_id_str.clone());
                            
                            event_tx.send(ServerEvent::ClientConnected(client_id_str.clone())).await.ok();
                            event_tx.send(ServerEvent::Log(format!(
                                "[{}] Connected (total: {})", 
                                client_id_str, 
                                clients.len()
                            ))).await.ok();
                        }
			
			ipc_publisher.send(payload.clone()).await.ok();
			match payload {
			    ESPEvent::CSI(_) => {
				CSI_COUNTER.fetch_add(1, Ordering::Relaxed);
			    }
			    _ => {

			    }
			}
		
			if let Some(ref mut out_files) = out_files {
			    let _ = write_event(&payload, out_files);
			}
		    }
                    Err(e) => {
                        event_tx.send(ServerEvent::Log(format!("Error: {}", e))).await.ok();
                        break;
                    }
                }
            }

            // Handle commands from TUI
            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    Command::ListClients => {
                        let client_list: Vec<String> = clients.values().cloned().collect();
                        event_tx.send(ServerEvent::ClientList(client_list)).await.ok();
                    }
                    Command::SendBroadcast(msg) => {
                        for client_id in clients.keys() {
                            send_to_client(&mut socket, client_id, &msg).await?;
                        }
                        event_tx.send(ServerEvent::Log(format!("Broadcast: {}", msg))).await.ok();
                    }
                    Command::SendToClient(client_name, msg) => {
                        if let Some((client_id, _)) = clients.iter().find(|(_, name)| **name == client_name) {
                            send_to_client(&mut socket, client_id, &msg).await?;
                            event_tx.send(ServerEvent::Log(format!("Sent to {}: {}", client_name, msg))).await.ok();
                        }
                    }
                    Command::Shutdown => {
                        event_tx.send(ServerEvent::Log("Server shutting down...".to_string())).await.ok();
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}


#[tokio::main]
async fn main() -> io::Result<()> {
    let (cmd_tx, cmd_rx) = mpsc::channel(1024);
    let (event_tx, event_rx) = mpsc::channel(1024);

    init_tui_logger(event_tx.clone(), log::LevelFilter::Debug);

    tokio::spawn(csi_stats_task());

    let server_handle = tokio::spawn(async move {
        if let Err(e) = run_server(cmd_rx, event_tx).await {
            error!("Server error: {}", e);
        }
    });

    let tui_result = run_tui(cmd_tx, event_rx).await;

    server_handle.abort();
    
    tui_result
}

async fn csi_stats_task() {
    use tokio::time::{interval, Duration};
    
    let mut interval = interval(Duration::from_secs(10));
    interval.tick().await; // First tick completes immediately, skip it
    
    loop {
        interval.tick().await;
        
        let count = CSI_COUNTER.swap(0, Ordering::Relaxed);
        info!("CSI packets in last 10s: {} ({:.1}/sec)", count, count as f32 / 10.0);
    }
}
