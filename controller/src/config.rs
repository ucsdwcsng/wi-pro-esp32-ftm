use chrono::{DateTime, Utc};
use clap::{Arg, Command};
use get_if_addrs::get_if_addrs;
use once_cell::sync::OnceCell;
use std::net::IpAddr;

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

// Global static config instance
static CONFIG: OnceCell<Arc<Config>> = OnceCell::new();

#[derive(Debug, Clone)]
pub struct Config {
    pub serial_port: String,
    pub output_dir: Option<PathBuf>,
    pub server: Option<(String, u16)>,
}

impl Config {
    /// Parse command-line arguments and initialize the global config
    pub fn init() -> Result<(), Box<dyn std::error::Error>> {
        let matches = Command::new("Serial Port Tool")
            .version("1.0")
            .author("wshunter")
            .arg(
                Arg::new("port")
                    .short('p')
                    .long("port")
                    .value_name("PORT")
                    .help("Serial port to connect to (e.g., /dev/ttyUSB0, COM3)")
                    .required(true)
                    .value_parser(clap::value_parser!(String)),
            )
            .arg(
                Arg::new("output")
                    .short('o')
                    .long("output")
                    .value_name("DIR")
                    .help("Output directory for received data")
                    .required(false)
                    .value_parser(clap::value_parser!(PathBuf)),
            )
	    .arg(
		Arg::new("server")
		    .short('s')
		    .long("server")
		    .value_name("IP:PORT")
		    .help("Optional server IP and port to forward messages (e.g., 192.168.1.100:8080)")
		    .required(false)
		    .value_parser(clap::value_parser!(String)),
	    )
            .get_matches();

        let output_dir = {
	    if let Some(output_path) = matches.get_one::<PathBuf>("output") {
		let output_path_full = if output_path.exists() {
		    // Path exists, create timestamped subdirectory
		    let now: DateTime<Utc> = Utc::now();
		    let timestamp = now.format("%Y%m%d_%H%M%S").to_string();
		    let dirname = format!("log{}", timestamp);
		    output_path.join(dirname)
		} else {
		    // Path doesn't exist, use it directly
		    output_path.clone()
		};

		fs::create_dir_all(&output_path_full)?;
		Some(output_path_full)
	    } else {
		None
	    }
	};

	
        let server = matches.get_one::<String>("server").map(|s| {
            let parts: Vec<&str> = s.split(':').collect();
            if parts.len() != 2 {
                panic!("Invalid server format. Expected IP:PORT");
            }
            let ip = parts[0].to_string();
            let port = parts[1].parse::<u16>().expect("Invalid port number");
            (ip, port)
        });

        let config = Config {
            serial_port: matches.get_one::<String>("port").cloned().unwrap(),
            output_dir: output_dir,
            server: server,
        };

        CONFIG
            .set(Arc::new(config))
            .map_err(|_| "Config already initialized")?;

        Ok(())
    }
    pub fn get() -> Arc<Config> {
        CONFIG
            .get()
            .expect("Config not initialized. Call Config::init() first")
            .clone()
    }
}

pub fn get_client_identifier() -> String {
    // Try to get IP addresses
    if let Ok(interfaces) = get_if_addrs() {
        let mut ipv4_addresses: Vec<std::net::Ipv4Addr> = interfaces
            .iter()
            .filter_map(|iface| {
                if let IpAddr::V4(ipv4) = iface.addr.ip() {
                    // Ignore loopback (127.0.0.1)
                    if !ipv4.is_loopback() {
                        return Some(ipv4);
                    }
                }
                None
            })
            .collect();

        // Prefer IPs starting with 10.
        if let Some(ip) = ipv4_addresses.iter().find(|ip| ip.octets()[0] == 10) {
            return ip.to_string();
        }

        // Otherwise, find the IP with the lowest first byte
        ipv4_addresses.sort_by_key(|ip| ip.octets()[0]);
        if let Some(ip) = ipv4_addresses.first() {
            return ip.to_string();
        }
    }

    // Fall back to MAC address
    if let Ok(Some(mac)) = mac_address::get_mac_address() {
        return mac.to_string();
    }

    // Ultimate fallback
    "unknown".to_string()
}


static SERVER_CONFIG: OnceCell<Arc<ServerConfig>> = OnceCell::new();

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub output_dir: Option<PathBuf>,
}

impl ServerConfig {
    /// Parse command-line arguments and initialize the global server config
    pub fn init() -> Result<(), Box<dyn std::error::Error>> {
        let matches = Command::new("Serial Port Server")
            .version("1.0")
            .author("wshunter")
            .arg(
                Arg::new("output")
                    .short('o')
                    .long("output")
                    .value_name("DIR")
                    .help("Output directory for received data")
                    .required(false)
                    .value_parser(clap::value_parser!(PathBuf)),
            )
            .get_matches();

        let output_dir = {
            if let Some(output_path) = matches.get_one::<PathBuf>("output") {
                let output_path_full = if output_path.exists() {
                    let now: DateTime<Utc> = Utc::now();
                    let timestamp = now.format("%Y%m%d_%H%M%S").to_string();
                    let dirname = format!("log{}", timestamp);
                    output_path.join(dirname)
                } else {
                    output_path.clone()
                };

                println!("output dir {}", output_path_full.display());
                fs::create_dir_all(&output_path_full)?;
                Some(output_path_full)
            } else {
                None
            }
        };

        let config = ServerConfig { output_dir };

        SERVER_CONFIG
            .set(Arc::new(config))
            .map_err(|_| "ServerConfig already initialized")?;

        Ok(())
    }

    pub fn get() -> Arc<ServerConfig> {
        SERVER_CONFIG
            .get()
            .expect("ServerConfig not initialized. Call ServerConfig::init() first")
            .clone()
    }
}
