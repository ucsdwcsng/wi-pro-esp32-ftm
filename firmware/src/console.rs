use crate::config::CONFIG;
use crate::espnow;
use crate::peers;
use crate::wifi::set_promi;
use log::info;
use std::io::ErrorKind;

#[embassy_executor::task]
pub async fn console_task() {
    use std::io::{BufRead, BufReader};
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut buffer = String::new();

    loop {
        // Read a line (this blocks but we're in a separate task)
        match reader.read_line(&mut buffer) {
            Ok(0) => {
                // EOF - shouldn't happen on serial, but just in case
                embassy_time::Timer::after(embassy_time::Duration::from_millis(100)).await;
                continue;
            }
            Ok(_) => {
                let input = buffer.trim();

                // Split command and arguments
                let (cmd, args) = if let Some(pos) = input.find(char::is_whitespace) {
                    let (c, a) = input.split_at(pos);
                    (c, a.trim())
                } else {
                    (input, "")
                };

                info!("Received command: '{}' args: '{}'", cmd, args);
                handle_command(cmd, args).await;
                buffer.clear();
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                // No data available, wait and retry
                embassy_time::Timer::after(embassy_time::Duration::from_millis(100)).await;
            }
            Err(e) => {
                info!("Error reading command: {:?}", e);
                embassy_time::Timer::after(embassy_time::Duration::from_millis(100)).await;
            }
        }
    }
}

async fn handle_command(cmd: &str, args: &str) {
    match cmd {
        "help" => {
            info!("Available commands:");
            info!("  help     - Show this help");
            info!("  peers    - List discovered peers");
            info!("  stats    - Show ESP-NOW statistics");
            info!("  id       - Show device MAC address");
            info!("  mute <1|0> - enable/disable FTM requests");
            info!("  beacon <ms>  - Set UDP beaconing period in milliseconds");
            info!("  interval <ms> - Set FTM request interval in milliseconds");
            info!("  promi <1|0> - enable promiscuous mode");
            info!("  channel <ch>/<bw> - Set WiFi channel (e.g., channel 6/40)");
	    info!("  burst <16|25|32|64> - Set number of FTMs to be sent");
        }
        "peers" => {
            peers::print_all_peers().await;
        }
        "stats" => {
            let (sent, received) = espnow::get_stats();
            info!("ESP-NOW: Sent={}, Received={}", sent, received);
        }
        "id" => {
            // Get and print MAC address
            let mut mac = [0u8; 6];
            unsafe {
                esp_idf_svc::sys::esp_wifi_get_mac(
                    esp_idf_svc::sys::wifi_interface_t_WIFI_IF_AP,
                    mac.as_mut_ptr(),
                );
            }
            info!(
                "\x02MAC\x01{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}\x03",
                mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
            );
            info!(
                "MAC: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
            );

            mac = [0u8; 6];
            unsafe {
                esp_idf_svc::sys::esp_wifi_get_mac(
                    esp_idf_svc::sys::wifi_interface_t_WIFI_IF_STA,
                    mac.as_mut_ptr(),
                );
            }
            info!(
                "MAC, STA: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
            );
        }
        "mute" => {
	    if args.is_empty() {
	    } else {
		if let Ok(mute) = args.parse::<u32>(){
		    crate::ftm::set_mute(mute != 0).await;
		}
	    }
        }
        "interval" => {
            if args.is_empty() {
                // Show current interval
                let current = CONFIG.lock(|config| config.borrow().contact_interval_ms);
                info!("Current FTM interval: {} ms", current);
            } else {
                // Parse and set new interval
                match args.parse::<u64>() {
                    Ok(new_interval) if new_interval > 0 => {
                        CONFIG
                            .lock(|config| config.borrow_mut().contact_interval_ms = new_interval);
                        info!("FTM interval set to {} ms", new_interval);
                    }
                    Ok(_) => {
                        info!("Error: Interval must be greater than 0");
                    }
                    Err(_) => {
                        info!("Error: Invalid interval value. Usage: interval <milliseconds>");
                    }
                }
            }
        }
        "promi" => {
            if args.is_empty() {
            } else {
                match args.parse::<u64>() {
                    Ok(val) => {
                        set_promi(val > 0);
                    }
                    Err(_) => {
                        info!("Error: Invalid interval value. Usage: promi (1|0)");
                    }
                }
            }
        }
        "channel" => {
            if args.is_empty() {
                // Show current channel
                unsafe {
                    let mut primary: u8 = 0;
                    let mut second: esp_idf_svc::sys::wifi_second_chan_t = 0;
                    esp_idf_svc::sys::esp_wifi_get_channel(
                        &mut primary as *mut _,
                        &mut second as *mut _,
                    );
                    let bandwidth =
                        if second == esp_idf_svc::sys::wifi_second_chan_t_WIFI_SECOND_CHAN_NONE {
                            "20MHz"
                        } else {
                            "40MHz"
                        };
                    info!("Current channel: {} ({})", primary, bandwidth);
                }
            } else {
                // Parse channel/bandwidth format (e.g., "6/40" or "11/20")
                let parts: Vec<&str> = args.split('/').collect();
                if parts.len() != 2 {
                    info!("Error: Invalid format. Use: channel <ch>/<bw>");
                    info!("Examples: channel 1/20, channel 6/40, channel 11/20");
                } else {
                    match (parts[0].parse::<u8>(), parts[1].parse::<u8>()) {
                        (Ok(ch), Ok(bw)) if (bw == 20 || bw == 40) => {
                            if ch != 1 && ch != 6 && ch != 11 {
                                info!("Error: Channel must be 1, 6, or 11");
                            } else {
                                match crate::wifi::set_channel(ch, bw == 40).await {
                                    Ok(_) => {
                                        info!("Channel set to {} (HT{})", ch, bw);
                                    }
                                    Err(e) => {
                                        info!("Failed to set channel: {:?}", e);
                                    }
                                }
                            }
                        }
                        (Ok(_), Ok(_)) => {
                            info!("Error: Bandwidth must be 20 or 40");
                        }
                        _ => {
                            info!("Error: Invalid channel or bandwidth value");
                            info!("Usage: channel <ch>/<bw>");
                            info!("Examples: channel 1/20, channel 6/40");
                        }
                    }
                }
            }
        }
	"beacon" => {
            if args.is_empty() {
            } else {
		match args.parse::<i32>() {
		    Ok(new_beacon) => {
			crate::espnow::set_beacon(new_beacon).await;
		    }
		    Err(_) => {
		    }
		}
            }
        }
	"burst" => {
            if args.is_empty() {
            } else {
                match args.parse::<i32>() {
                    Ok(new_burst) => {
			crate::ftm::set_burst(new_burst).await;
                    }
                    Err(_) => {
                    }
                }
            }

	}
        "" => {
            // Empty line, ignore
        }
        _ => {
            info!(
                "Unknown command: '{}'. Type 'help' for available commands.",
                cmd
            );
        }
    }
}
