use crate::config::CONFIG;
use crate::csi;
use core::fmt::Write;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::sys as esp_idf_sys;
use esp_idf_svc::wifi::{
    AccessPointConfiguration, AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi,
};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use heapless::String;
use log::{info, debug};
use embassy_time::{Duration, Timer};
use core::sync::atomic::{AtomicU32, Ordering};

pub struct WifiState {
    pub channel: u8,
    pub use_ht40: bool,
}

pub static WIFI_STATE: Mutex<CriticalSectionRawMutex, WifiState> = Mutex::new(WifiState {
    channel: 6,      // default
    use_ht40: true, // default
});


unsafe extern "C" fn promiscuous_rx_callback(
    buf: *mut core::ffi::c_void,
    _typ: esp_idf_sys::wifi_promiscuous_pkt_type_t,
) {
    if buf.is_null() {
        return;
    }

    // let pkt = buf as *const esp_idf_sys::wifi_promiscuous_pkt_t;
    // let rx_ctrl = &(*pkt).rx_ctrl;
    
    // // Get the payload (802.11 frame)
    // let payload = core::slice::from_raw_parts(
    //     (*pkt).payload.as_ptr(),
    //     rx_ctrl.sig_len() as usize
    // );
    
    // if payload.len() >= 16 {  // Need at least 16 bytes for FC + Duration + Addr1 + start of Addr2
    //     let frame_control = u16::from_le_bytes([payload[0], payload[1]]);
    //     let frame_type = (frame_control >> 2) & 0x0003;
    //     let frame_subtype = (frame_control >> 4) & 0x000F;
        
    //     // Extract MAC addresses
    //     // Address 1 (receiver): bytes 4-9
    //     let addr1 = &payload[4..10];
    //     // Address 2 (transmitter): bytes 10-15
    //     let addr2 = &payload[10..16];
        
    //     info!("FC: 0x{:04x}, Type: {}, Subtype: {}", 
    //           frame_control, frame_type, frame_subtype);
    //     info!("  Addr1 (RX): {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
    //           addr1[0], addr1[1], addr1[2], addr1[3], addr1[4], addr1[5]);
    //     info!("  Addr2 (TX): {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
    //           addr2[0], addr2[1], addr2[2], addr2[3], addr2[4], addr2[5]);
    // }
}

pub fn set_promi(promi_en: bool) {
    unsafe {
    	let filter = esp_idf_sys::wifi_promiscuous_filter_t {
	    filter_mask: esp_idf_sys::WIFI_PROMIS_FILTER_MASK_DATA 
	};
	esp_idf_sys::esp_wifi_set_promiscuous_filter(&filter as *const _);
        esp_idf_sys::esp_wifi_set_promiscuous(promi_en);
        esp_idf_sys::esp_wifi_set_promiscuous_rx_cb(Some(promiscuous_rx_callback));
    }
}

/// Sets the WiFi channel with optional HT40 mode
pub async fn set_channel(channel: u8, use_ht40: bool) -> Result<(), esp_idf_sys::EspError> {
    if channel != 1 && channel != 6 && channel != 11 {
        log::error!("Invalid channel: {}. Use 1, 6, or 11", channel);
        return Err(esp_idf_sys::EspError::from(esp_idf_sys::ESP_ERR_INVALID_ARG as i32).unwrap());
    }
    unsafe {
        let secondary_chan = if use_ht40 {
            if channel <= 7 {
                esp_idf_sys::wifi_second_chan_t_WIFI_SECOND_CHAN_ABOVE
            } else {
                esp_idf_sys::wifi_second_chan_t_WIFI_SECOND_CHAN_BELOW
            }
        } else {
            esp_idf_sys::wifi_second_chan_t_WIFI_SECOND_CHAN_NONE
        };

        let result = esp_idf_sys::esp_wifi_set_channel(channel, secondary_chan);

        if result == esp_idf_sys::ESP_OK {
            // Update the global state
	    let mut state = WIFI_STATE.lock().await;
	    state.channel = channel;
	    state.use_ht40 = use_ht40;
            
            Ok(())
        } else {
            Err(esp_idf_sys::EspError::from(result).unwrap())
        }
    }
}

pub async fn setup(
    modem: Modem<'static>,
    sys_loop: &EspSystemEventLoop,
    nvs: Option<EspDefaultNvsPartition>,
) -> Result<BlockingWifi<EspWifi<'static>>, Box<dyn std::error::Error>> {
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(modem, sys_loop.clone(), nvs)?,
        sys_loop.clone(),
    )?;

    let mut mac = [0u8; 6];
    unsafe {
        esp_idf_sys::esp_wifi_get_mac(esp_idf_sys::wifi_interface_t_WIFI_IF_AP, mac.as_mut_ptr());
    }

    let (softap_prefix, promi_en) = CONFIG.lock(|cell| {
        let config = cell.borrow();
        (config.softap_prefix, config.promi_en)
    });

    let softap_channel = WIFI_STATE.lock().await.channel;

    let mut ssid = String::<32>::new();
    write!(
        &mut ssid,
        "{}",
        softap_prefix
    )
    .unwrap();

    let ap_config = AccessPointConfiguration {
        ssid: ssid.as_str().try_into().unwrap(),
        ssid_hidden: false,
        password: "password".try_into().unwrap(),
        auth_method: AuthMethod::default(),
        channel: softap_channel,
        max_connections: 8,
        ..Default::default()
    };

    let sta_config = ClientConfiguration {
	..Default::default()
    };

    wifi.set_configuration(&Configuration::Mixed(sta_config, ap_config))?;
    wifi.start()?;

    
    info!("WiFi AP started!");
    let ip_info = wifi.wifi().ap_netif().get_ip_info()?;
    info!("AP IP address: {:?}", ip_info.ip);
    info!("AP SSID: '{}', Password: '{}'", ssid, "password");

    unsafe {
        let result = esp_idf_sys::esp_wifi_set_bandwidth(
            esp_idf_sys::wifi_interface_t_WIFI_IF_AP,
            esp_idf_sys::wifi_bandwidth_t_WIFI_BW_HT40,
        );

        if result == esp_idf_sys::ESP_OK {
            info!("AP bandwidth set to 40MHz (HT40)");
        } else {
            log::error!("Failed to set AP bandwidth: {}", result);
        }
    }

    // rust lib doesn't have the ftm_responder field in ap_config, need to use C api
    unsafe {
        let mut wifi_config: esp_idf_sys::wifi_config_t = core::mem::zeroed();
        esp_idf_sys::esp_wifi_get_config(
            esp_idf_sys::wifi_interface_t_WIFI_IF_AP,
            &mut wifi_config as *mut _,
        );

        wifi_config.ap.ftm_responder = true;

        let result = esp_idf_sys::esp_wifi_set_config(
            esp_idf_sys::wifi_interface_t_WIFI_IF_AP,
            &mut wifi_config as *mut _,
        );

        if result == esp_idf_sys::ESP_OK {
            info!("FTM Responder enabled on AP");
        } else {
            log::error!("Failed to enable FTM responder: {}", result);
        }
    }

    unsafe {
        extern "C" fn ftm_event_handler(
            _arg: *mut core::ffi::c_void,
            _event_base: esp_idf_sys::esp_event_base_t,
            event_id: i32,
            event_data: *mut core::ffi::c_void,
        ) {
            if event_id == esp_idf_sys::wifi_event_t_WIFI_EVENT_FTM_REPORT as i32 {
                unsafe {
                    crate::ftm::handle_ftm_report_event(
                        event_data as *const esp_idf_sys::wifi_event_ftm_report_t,
                    );
                }
            }
        }

        let result = esp_idf_sys::esp_event_handler_register(
            esp_idf_sys::WIFI_EVENT,
            esp_idf_sys::wifi_event_t_WIFI_EVENT_FTM_REPORT as i32,
            Some(ftm_event_handler),
            core::ptr::null_mut(),
        );

        if result == esp_idf_sys::ESP_OK {
            info!("FTM event handler registered");
        } else {
            log::error!("Failed to register FTM event handler: {}", result);
        }
    }

    set_promi(promi_en);

    unsafe {
        let csi_config = esp_idf_sys::wifi_csi_config_t {
            lltf_en: true,
            htltf_en: true,
            stbc_htltf2_en: true,
            ltf_merge_en: false,
            channel_filter_en: true,
            manu_scale: false,
            shift: 0,
            dump_ack_en: false,
        };

        esp_idf_sys::esp_wifi_set_csi_rx_cb(Some(csi::csi_rx_callback), core::ptr::null_mut());
        esp_idf_sys::esp_wifi_set_csi_config(&csi_config as *const _);
        esp_idf_sys::esp_wifi_set_csi(true);
        info!("CSI enabled");
    }

    Ok(wifi)
}


extern "C" {
    fn esp_wifi_internal_get_mac_clock_time() -> i64;
}

static MAC_UPPER_BITS: AtomicU32 = AtomicU32::new(0);
static MAC_LAST_LOWER: AtomicU32 = AtomicU32::new(0);

pub fn get_mac_counter() -> i64 {
    let raw = unsafe { esp_wifi_internal_get_mac_clock_time() };
    let hardware_lower_32 = raw as u32;
    
    // Simple overflow detection with 32-bit atomics (always lock-free on ESP32)
    let last_lower = MAC_LAST_LOWER.load(Ordering::Relaxed);
    
    // Update last_lower optimistically
    MAC_LAST_LOWER.store(hardware_lower_32, Ordering::Relaxed);
    
    // If we wrapped, increment upper bits
    if hardware_lower_32 < last_lower {
        MAC_UPPER_BITS.fetch_add(1, Ordering::Relaxed);
    }
    
    let upper = MAC_UPPER_BITS.load(Ordering::Relaxed);
    ((upper as u64) << 32 | hardware_lower_32 as u64) as i64
}

#[embassy_executor::task]
pub async fn mac_counter_keepalive() {
    loop {
        // Call get_mac_counter to update overflow tracking
        let counter = get_mac_counter();
        debug!("MAC counter keepalive: {:016x}", counter);
        
        // Wait 60 seconds
        Timer::after(Duration::from_secs(60)).await;
    }
}

pub async fn get_channel() -> u8{
    return WIFI_STATE.lock().await.channel;
}

#[allow(dead_code)]
pub async fn get_ht40() -> bool{
    return WIFI_STATE.lock().await.use_ht40;
}

// Add this helper function
pub fn is_sta_connected() -> bool {
    unsafe {
        let mut ap_info: esp_idf_sys::wifi_ap_record_t = core::mem::zeroed();
        let result = esp_idf_sys::esp_wifi_sta_get_ap_info(&mut ap_info as *mut _);
        result == esp_idf_sys::ESP_OK
    }
}

pub fn sta_get_ip() -> Option<std::net::Ipv4Addr> {
    unsafe {
        let key = b"WIFI_STA_DEF\0";
        let netif = esp_idf_sys::esp_netif_get_handle_from_ifkey(key.as_ptr() as *const _);
        if netif.is_null() {
            return None;
        }
        let mut ip_info: esp_idf_sys::esp_netif_ip_info_t = core::mem::zeroed();
        if esp_idf_sys::esp_netif_get_ip_info(netif, &mut ip_info as *mut _) != esp_idf_sys::ESP_OK {
            return None;
        }
        if ip_info.ip.addr == 0 {
            return None;
        }
        // LwIP stores addr in network byte order; to_ne_bytes() on a little-endian
        // ESP32-S3 yields octets in the correct on-wire order.
        let b = ip_info.ip.addr.to_ne_bytes();
        Some(std::net::Ipv4Addr::new(b[0], b[1], b[2], b[3]))
    }
}


pub fn try_connect() {
    if !is_sta_connected() {
        info!("STA not connected, attempting reconnect...");
        
        let (ssid, password) = CONFIG.lock(|cell| {
            let config = cell.borrow();
            (config.softap_prefix, "password")
        });
        
        unsafe {
            // Set STA configuration before connecting
            let mut wifi_config: esp_idf_sys::wifi_config_t = core::mem::zeroed();
            
            // Copy SSID
            let ssid_bytes = ssid.as_bytes();
            let ssid_len = ssid_bytes.len().min(32);
            wifi_config.sta.ssid[..ssid_len].copy_from_slice(&ssid_bytes[..ssid_len]);
            
            // Copy password
            let pwd_bytes = password.as_bytes();
            let pwd_len = pwd_bytes.len().min(64);
            wifi_config.sta.password[..pwd_len].copy_from_slice(&pwd_bytes[..pwd_len]);
            
            // Set the config
            esp_idf_sys::esp_wifi_set_config(
                esp_idf_sys::wifi_interface_t_WIFI_IF_STA,
                &mut wifi_config as *mut _,
            );
            
            // Now try to connect
            let result = esp_idf_sys::esp_wifi_connect();
            if result == esp_idf_sys::ESP_OK {
                info!("Reconnect attempt initiated");
            } else {
                info!("Reconnect attempt failed: {}", result);
            }
        }
    }
}

pub fn disconnect() {
    unsafe {
        // Disconnect first
        let result = esp_idf_sys::esp_wifi_disconnect();
        if result == esp_idf_sys::ESP_OK {
            info!("Disconnected from AP");
        } else {
            info!("Disconnect failed: {}", result);
        }
        
        // Clear STA configuration to prevent auto-reconnect
        let mut wifi_config: esp_idf_sys::wifi_config_t = core::mem::zeroed();
        // SSID and password already zeroed, just set it
        esp_idf_sys::esp_wifi_set_config(
            esp_idf_sys::wifi_interface_t_WIFI_IF_STA,
            &mut wifi_config as *mut _,
        );
        
        info!("STA config cleared");
    }
}
