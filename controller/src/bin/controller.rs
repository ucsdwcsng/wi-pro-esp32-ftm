use controller::config::Config;
use controller::esp_io;

use zeromq::{DealerSocket, SocketRecv, SocketSend, ZmqMessage, Socket};
use tokio::sync::mpsc;
use tokio::select;

use controller::tlv::ESPEvent;

#[tokio::main]
async fn main() {
    if let Err(e) = Config::init() {
        eprintln!("Error parsing arguments: {}", e);
        std::process::exit(1);
    }
    let cfg = Config::get();

    println!("Port: {}", cfg.serial_port);
    let mut handles = vec![];
    
    let (tx, mut msg_rx, h_ser_read, h_ser_write) = esp_io::connect_serial(cfg.serial_port.clone(), cfg.output_dir.clone()).await;
    esp_io::send_command(&tx, "help").await;
    handles.push(h_ser_read);
    handles.push(h_ser_write);

    let mut handles = vec![];

    // Connect to server if specified
    if let Some((server_ip, server_port)) = cfg.server.clone() {
        let cmd_tx = tx.clone();
        let h = tokio::spawn(async move {
            if let Err(e) = connect_and_forward_to_server(server_ip, server_port, msg_rx, cmd_tx).await {
                eprintln!("Server connection error: {}", e);
            }
        });
        handles.push(h);
    } else {
        let h = tokio::spawn(async move {
            while let Some(_) = msg_rx.recv().await {}
        });
        handles.push(h);
    }
    
    // Now you can spawn any async tasks you need
    let tx_user = tx.clone();
    let h = tokio::spawn(async move {
        esp_io::handle_user_input(tx_user).await;
    });
    handles.push(h);


    tokio::signal::ctrl_c().await.expect("Failed to listen for Ctrl+C");
    println!("Shutting down...");

    for handle in handles {
        handle.abort();
    }
    
}


async fn connect_and_forward_to_server(
    server_ip: String,
    server_port: u16,
    mut msg_rx: mpsc::Receiver<ESPEvent>,
    cmd_tx: mpsc::Sender<String>
) -> Result<(), Box<dyn std::error::Error>> {
    let server_addr = format!("tcp://{}:{}", server_ip, server_port);
    println!("Connecting to server at {}", server_addr);


    let mut socket = DealerSocket::new();
    socket.connect(&server_addr).await?;
    
    println!("Connected to server!");
    
    // Single task handling both send and receive
    tokio::spawn(async move {
        loop {
            select! {
                // Receive from server
		msg_result = socket.recv() => {
		    match msg_result {
                        Ok(msg) => {
			    let frames = msg.into_vec();
			    if let Some(payload_bytes) = frames.last() {
                                let payload = String::from_utf8_lossy(payload_bytes);
                                println!("Server says: {}", payload);
				esp_io::send_command(&cmd_tx, &payload).await;
                            }
                        }
                        Err(e) => {
                            eprintln!("Error receiving from server: {}", e);
                            break;
                        }
                    }
                }
		Some(esp_event) = msg_rx.recv() => {
		    match rmp_serde::to_vec(&esp_event) {
			Ok(msgpack_bytes) => {
			    let msg = ZmqMessage::from(msgpack_bytes);
			    if let Err(e) = socket.send(msg).await {
				eprintln!("Error sending to server: {}", e);
				break;
			    }
			}
			Err(e) => {
			    eprintln!("Error serializing event: {}", e);
			}
		    }
		}
	    }
	}
    });
    println!("Server communication task ended");
    Ok(())
}
