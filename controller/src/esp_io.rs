use std::{
    fs::File,
    io::Write,
    path::PathBuf,
    sync::OnceLock,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tokio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio_serial::SerialPortBuilderExt;

static ESP_MAC: OnceLock<String> = OnceLock::new();

use crate::tlv;

pub struct OutputFiles {
    pub raw_file: File,
    pub ftm_file: File,
    pub csi_file: File,
    pub dbg_file: File,
}

pub async fn connect_serial(port_name: String, output_path: Option<PathBuf>) -> (mpsc::Sender<String>, mpsc::Receiver<tlv::ESPEvent>, tokio::task::JoinHandle<()>, tokio::task::JoinHandle<()> ) {
    let (tx, rx) = mpsc::channel::<String>(100);
    let (msg_tx, msg_rx) = mpsc::channel::<tlv::ESPEvent>(100);  // New channel for raw messages
    
    let mut out_files: Option<OutputFiles> = {
        if let Some(output_path) = output_path {
            let out_file_path = output_path.join("raw.dat");
            println!("Logging raw data in {} ...", out_file_path.display());
            Some(OutputFiles {
                raw_file: File::create(output_path.join("raw.dat")).unwrap(),
                ftm_file: File::create(output_path.join("ftm.csv")).unwrap(),
                csi_file: File::create(output_path.join("csi.csv")).unwrap(),
		dbg_file: File::create(output_path.join("dbg.csv")).unwrap(),
            })
        } else {
            None
        }
    };

    const BAUD_RATE: u32 = 3000000;
    let port = tokio_serial::new(port_name.clone(), BAUD_RATE)
        .timeout(Duration::from_millis(100))
        .open_native_async()
        .expect("Failed to open serial port");
    let (port_read, port_write) = tokio::io::split(port);

    // Spawn reader task
    let h_read = tokio::spawn(async move {
        let mut reader = BufReader::new(port_read);
        let mut line = String::new();

        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => {
                    println!("Serial connection closed");
                    break;
                }
                Ok(_) => {
		    if let Some(ref mut out_files) = out_files {
			out_files.raw_file.write_all(line.as_bytes()).expect("Failed to write!");
		    }
		    if let Some(fields) = tlv::parse(&line) {
			if let Some(event) = handle_message(fields, &mut out_files) {
			    if let Err(_e) = msg_tx.send(event).await {
				eprintln!("Error sending raw message!");
                            }
			}
		    } else {
			let line_trim = line.trim();
			if line_trim.len() > 4 {
			    println!("{}", line_trim);
			    
			}
		    }
		}
                Err(e) => {
                    eprintln!("Error reading: {}", e);
                    break;
                }
            }
        }
    });

    // Spawn writer task
    let h_write = tokio::spawn(async move {
        write_task(port_write, rx).await;
    });

    send_command(&tx, "id").await;

    (tx, msg_rx, h_read, h_write)
}

async fn write_task(
    mut port_write: tokio::io::WriteHalf<tokio_serial::SerialStream>,
    mut rx: mpsc::Receiver<String>,
) {
    while let Some(message) = rx.recv().await {
        println!("TX: {}", message.trim());
        if let Err(e) = port_write.write_all(message.as_bytes()).await {
            eprintln!("Error writing: {}", e);
            break;
        }
        if let Err(e) = port_write.flush().await {
            eprintln!("Error flushing: {}", e);
            break;
        }
    }
}

pub async fn handle_user_input(tx: mpsc::Sender<String>) {
    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut line = String::new();

    loop {
        line.clear();
        match stdin.read_line(&mut line).await {
            Ok(0) => break,
            Ok(_) => {
                if tx.send(line.clone()).await.is_err() {
                    break;
                }
            }
            Err(e) => {
                eprintln!("Error reading stdin: {}", e);
                break;
            }
        }
    }
}

// pub async fn handle_programmatic_input(tx: mpsc::Sender<String>) {
//     let mut interval = tokio::time::interval(Duration::from_secs(10));

//     loop {
//         interval.tick().await;
//         let message = "HEARTBEAT\n".to_string();
//         if tx.send(message).await.is_err() {
//             break;
//         }
//     }
// }

// Helper function - now you can have nice async APIs!
pub async fn send_command(tx: &mpsc::Sender<String>, cmd: &str) {
    let message = format!("{}\n", cmd);
    if let Err(e) = tx.send(message).await {
        eprintln!("Failed to send command: {}", e);
    }
}
fn handle_message(msg: Vec<&[u8]>, out_files: &mut Option<OutputFiles>) -> Option<tlv::ESPEvent> {
    let Some(msg_type_bytes) = msg.first() else {
        return None;
    };

    let host_time_ms: u64 = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .try_into()
        .unwrap();
    let msg_type = String::from_utf8_lossy(msg_type_bytes);

    if msg_type.as_ref() == "MAC" {
        let mac = str::from_utf8(msg[1]).unwrap().trim().to_owned();
        let _ = ESP_MAC.set(mac);
        return None;
    }

    let Some(own_mac) = ESP_MAC.get() else {
        println!("Ignoring '{}' message - MAC not yet known", msg_type);
        return None;
    };

    println!("GOT {}", msg_type);

    let event_result: Result<tlv::ESPEvent, String> = match msg_type.as_ref() {
        "CSI"     => tlv::parse_csi(msg, own_mac, host_time_ms).map(tlv::ESPEvent::CSI),
        "FTM"     => tlv::parse_ftm(msg, own_mac, host_time_ms).map(tlv::ESPEvent::FTM),
        "DBG"     => tlv::parse_dbg(msg, own_mac, host_time_ms).map(tlv::ESPEvent::DBG),
        _         => return None,
    };

    match event_result {
        Ok(event) => {
            if let Some(out_files) = out_files {
                if let Err(e) = write_event(&event, out_files) {
                    eprintln!("Error writing {} event: {}", msg_type, e);
                }
            }
            Some(event)
        }
        Err(e) => {
            eprintln!("Error parsing {} event: {}", msg_type, e);
            None
        }
    }
}
pub fn write_event(event: &tlv::ESPEvent, out_files: &mut OutputFiles) -> std::io::Result<()> {
    let file = match event {
        tlv::ESPEvent::CSI(_)     => &mut out_files.csi_file,
        tlv::ESPEvent::FTM(_)     => &mut out_files.ftm_file,
	tlv::ESPEvent::DBG(_) => &mut out_files.dbg_file
    };

    for line in event.to_csv() {
        file.write_all(line.as_bytes())?;
    }

    Ok(())
}
