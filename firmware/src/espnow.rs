use crate::ftm::{handle_ftm_notification, FtmNotification};
use crate::timectl::get_mac_ff_time;
use crate::wifi::{try_connect, disconnect};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Instant, Timer};
use std::net::UdpSocket;


use core::sync::atomic::{AtomicU32, Ordering};
use esp_idf_svc::sys as esp_idf_sys;
use log::info;
const MAX_ESPNOW_DATA_LEN: usize = 250; // ESP-NOW max payload

static PACKETS_SENT: AtomicU32 = AtomicU32::new(0);
static PACKETS_RECEIVED: AtomicU32 = AtomicU32::new(0);
pub struct BeaconState {
    pub should_beacon: bool,
    pub beacon_rate_ms: i32
}

pub static BEACON_STATE: Mutex<CriticalSectionRawMutex, BeaconState> = Mutex::new(BeaconState {
    should_beacon: false,
    beacon_rate_ms: 200,
});

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct FtmSyncPacket {
    pub sequence: u32,
    pub is_initiator: u8, // Use u8 instead of bool for C compatibility
    pub mac_time: i64
}

impl FtmSyncPacket {
    pub fn new(sequence: u32, is_initiator: bool, mac_time: i64) -> Self {
        Self {
            sequence,
            is_initiator: if is_initiator { 1 } else { 0 },
	    mac_time
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        unsafe {
            core::slice::from_raw_parts(self as *const _ as *const u8, core::mem::size_of::<Self>())
        }
    }

    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < core::mem::size_of::<Self>() {
            return None;
        }

        unsafe { Some(core::ptr::read_unaligned(data.as_ptr() as *const Self)) }
    }
}

// Send FTM coordination packet to peer
pub fn send_ftm_notification(
    peer_mac: &[u8; 6],
    sequence: u32,
    is_initiator: bool,
) -> Result<(), i32> {
    let packet = FtmSyncPacket::new(sequence, is_initiator, get_mac_ff_time());
    send_to_peer(peer_mac, packet.as_bytes())
}


unsafe extern "C" fn espnow_send_cb(
    tx_info: *const esp_idf_sys::wifi_tx_info_t,
    status: esp_idf_sys::esp_now_send_status_t,
) {
    if status == esp_idf_sys::esp_now_send_status_t_ESP_NOW_SEND_SUCCESS {
        PACKETS_SENT.fetch_add(1, Ordering::Relaxed);
    } else {
        if !tx_info.is_null() {
            let info = &*tx_info;
	    let mac = core::slice::from_raw_parts(info.des_addr, 6);
            info!(
                "ESP-NOW send failed to {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
            );
        }
    }
}

unsafe extern "C" fn espnow_recv_cb(
    esp_now_info: *const esp_idf_sys::esp_now_recv_info_t,
    data: *const u8,
    data_len: i32,
) {
    let local_mac_time = get_mac_ff_time();
    if esp_now_info.is_null() || data.is_null() || data_len <= 0 {
        return;
    }

    let info = &*esp_now_info;
    let payload = core::slice::from_raw_parts(data, data_len as usize);

    PACKETS_RECEIVED.fetch_add(1, Ordering::Relaxed);

    let src_mac = core::slice::from_raw_parts(info.src_addr, 6);

    // Try to parse as FTM coordination packet
    if let Some(packet) = FtmSyncPacket::from_bytes(payload) {
        // Copy values out of packed struct to avoid unaligned reference
        //let seq = packet.sequence;
        //let is_init = packet.is_initiator();

        handle_ftm_notification(FtmNotification {
            peer_mac: src_mac.try_into().unwrap(),
            sync_info: FtmSyncPacket {
                sequence: packet.sequence,
                is_initiator: packet.is_initiator,
		mac_time: packet.mac_time,
            },
	    _rx_mac_time: local_mac_time
        });
    } else {
        info!(
            "ESP-NOW received {} bytes from {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            data_len, src_mac[0], src_mac[1], src_mac[2], src_mac[3], src_mac[4], src_mac[5]
        );

        if let Ok(msg) = core::str::from_utf8(payload) {
            info!("  Message: {}", msg);
        }
    }
}

// Initialize ESP-NOW
pub fn init() -> Result<(), i32> {
    unsafe {
        // Initialize ESP-NOW
        let result = esp_idf_sys::esp_now_init();
        if result != esp_idf_sys::ESP_OK {
            return Err(result);
        }

        // Register callbacks
        esp_idf_sys::esp_now_register_send_cb(Some(espnow_send_cb));
        esp_idf_sys::esp_now_register_recv_cb(Some(espnow_recv_cb));

        info!("ESP-NOW initialized");
        Ok(())
    }
}

// Add a peer to ESP-NOW
pub fn add_peer(peer_mac: &[u8; 6], channel: u8) -> Result<(), i32> {
    unsafe {
        // Check if peer already exists
        if esp_idf_sys::esp_now_is_peer_exist(peer_mac.as_ptr()) {
            return Ok(()); // Already added
        }

        let peer_info = esp_idf_sys::esp_now_peer_info_t {
            peer_addr: *peer_mac,
            lmk: [0u8; 16], // Local Master Key (not used if encrypt=false)
            channel,
            ifidx: esp_idf_sys::wifi_interface_t_WIFI_IF_AP,
            encrypt: false, // No encryption for simplicity
            priv_: core::ptr::null_mut(),
        };

        let result = esp_idf_sys::esp_now_add_peer(&peer_info as *const _);
        if result == esp_idf_sys::ESP_OK {
            info!(
                "Added ESP-NOW peer: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                peer_mac[0], peer_mac[1], peer_mac[2], peer_mac[3], peer_mac[4], peer_mac[5]
            );
            Ok(())
        } else {
            Err(result)
        }
    }
}

pub fn send_to_peer(peer_mac: &[u8; 6], data: &[u8]) -> Result<(), i32> {
    if data.len() > MAX_ESPNOW_DATA_LEN {
        return Err(-1); // Data too large
    }

    unsafe {
        let result = esp_idf_sys::esp_now_send(peer_mac.as_ptr(), data.as_ptr(), data.len());

        if result == esp_idf_sys::ESP_OK {
            Ok(())
        } else {
            Err(result)
        }
    }
}

pub fn get_stats() -> (u32, u32) {
    (
        PACKETS_SENT.load(Ordering::Relaxed),
        PACKETS_RECEIVED.load(Ordering::Relaxed),
    )
}

pub fn get_connected_bssid() -> Option<[u8; 6]> {
    unsafe {
        let mut ap_info: esp_idf_sys::wifi_ap_record_t = core::mem::zeroed();
        let result = esp_idf_sys::esp_wifi_sta_get_ap_info(&mut ap_info as *mut _);
        
        if result == esp_idf_sys::ESP_OK {
            Some(ap_info.bssid)
        } else {
            None
        }
    }
}

#[embassy_executor::task]
pub async fn beacon_task() {
    
    let mut socket: Option<UdpSocket> = None;
    let mut connected = false;
    
    loop {
        let now = Instant::now();
        let state = BEACON_STATE.lock().await;
        let should_beacon = state.should_beacon;
        let rate = state.beacon_rate_ms;
        drop(state);
        
        if should_beacon {
	    if let Some(_bssid) = get_connected_bssid() {
            } else {
                info!("Not connected to any AP yet");
		try_connect();
                Timer::at(now + Duration::from_millis(3000)).await;
                continue;
            }
            // Try to create socket if not connected
            if !connected {
                match UdpSocket::bind("0.0.0.0:0") {
                    Ok(sock) => {
                        // Set broadcast/multicast options
                        sock.set_broadcast(true).ok();
                        socket = Some(sock);
                        connected = true;
                        info!("UDP socket created for beaconing");
                    }
                    Err(e) => {
                        info!("Failed to create UDP socket (not connected yet?): {:?}", e);
                        Timer::at(now + Duration::from_millis(1000)).await;
                        continue;
                    }
                }
            }
            
            // Send beacon packet
            if let Some(ref sock) = socket {
                let beacon_data = b"BEACON";
                // Send to broadcast address
                match sock.send_to(beacon_data, "255.255.255.255:5000") {
                    Ok(_) => {
                        info!("Beacon sent via UDP");
                    }
                    Err(e) => {
                        info!("Failed to send beacon: {:?}", e);
                        // Connection lost, reset
                        socket = None;
                        connected = false;
                    }
                }
            }
            
            Timer::at(now + Duration::from_millis(rate.try_into().unwrap())).await;
        } else {
            // Not beaconing, clean up
            socket = None;
            connected = false;
            	    
            Timer::at(now + Duration::from_millis(4000)).await;
        }
    }
}

pub async fn set_beacon(beacon_rate: i32) {
    let mut state = BEACON_STATE.lock().await;
    if beacon_rate == 0 {
	state.should_beacon = false;
	state.beacon_rate_ms = 200;
	disconnect();
    } else {
	state.should_beacon = true;
	if beacon_rate > 0 {
	    state.beacon_rate_ms = beacon_rate;
	} else {
	    state.beacon_rate_ms = 200;
	}
	info!("Set beacon rate to {}", state.beacon_rate_ms);
    }
}

