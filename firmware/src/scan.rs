use embassy_time::{Duration, Instant, Timer};
use esp_idf_svc::sys as esp_idf_sys;
use log::info;

use crate::config::CONFIG;
use crate::espnow;
use crate::ftm::PeerFtmStats;
use crate::peers::PeerContactStats;
use crate::peers::{print_all_peers, PeerInfo, PEER_LIST};

const MAX_APS: usize = 32;

async fn scan() {
    unsafe {
        let softap_prefix = CONFIG.lock(|config| config.borrow().softap_prefix);

        // Start scan
        let scan_config = esp_idf_sys::wifi_scan_config_t {
            ssid: core::ptr::null_mut(),
            bssid: core::ptr::null_mut(),
            channel: 0, // All channels
            show_hidden: false,
            scan_type: esp_idf_sys::wifi_scan_type_t_WIFI_SCAN_TYPE_ACTIVE,
            scan_time: esp_idf_sys::wifi_scan_time_t {
                active: esp_idf_sys::wifi_active_scan_time_t { min: 100, max: 300 },
                passive: 0,
            },
            home_chan_dwell_time: 0,
            channel_bitmap: esp_idf_sys::wifi_scan_channel_bitmap_t {
                ghz_2_channels: 0,
                ghz_5_channels: 0, // 5ghz not supported, will ignore this
            },
	    coex_background_scan: false
        };

        let result = esp_idf_sys::esp_wifi_scan_start(&scan_config as *const _, true);

        if result != esp_idf_sys::ESP_OK {
            info!("Scan start failed: {}", result);
            return;
        }

        // Get number of APs found
        let mut ap_count: u16 = 0;
        esp_idf_sys::esp_wifi_scan_get_ap_num(&mut ap_count as *mut _);

        if ap_count > 0 {
            let count = ap_count.min(MAX_APS as u16);
            let mut ap_records: [esp_idf_sys::wifi_ap_record_t; MAX_APS] = core::mem::zeroed();
            let mut actual_count = count;

            let result = esp_idf_sys::esp_wifi_scan_get_ap_records(
                &mut actual_count as *mut _,
                ap_records.as_mut_ptr(),
            );

            if result == esp_idf_sys::ESP_OK {
                let now = Instant::now();
                let mut peer_count = 0;

                // Lock the peer list for updating
                let mut peer_list = PEER_LIST.lock().await;

                info!("Scan results:");
                for i in 0..actual_count as usize {
                    let ap = &ap_records[i];

                    // Convert SSID from bytes to string
                    let ssid_len = ap
                        .ssid
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(ap.ssid.len());

                    if let Ok(ssid) = core::str::from_utf8(&ap.ssid[..ssid_len]) {
                        info!("  [{}] SSID: '{}', RSSI: {}, Channel: {}",
                              i + 1, ssid, ap.rssi, ap.primary);

                        // Check if this is one of our ESP devices
                        if ssid.starts_with(softap_prefix) {
                            let peer = PeerInfo {
                                bssid: ap.bssid,
                                ftm_stats: PeerFtmStats {
                                    next_contact_after: now,
                                    seq: 0,
                                    should_init: true,
                                },
                                contact_stats: PeerContactStats {
                                    last_seen: now,
                                    rssi: ap.rssi,
                                    channel: ap.primary,
                                },
                            };

                            match peer_list.add_or_update(peer) {
                                Ok(_) => {
                                    peer_count += 1;
                                    info!("Added/updated ESP peer: {}", ssid);
                                    let _ = espnow::add_peer(&ap.bssid, ap.primary);
                                }
                                Err(_) => {
                                    info!("Peer list full, cannot add: {}", ssid);
                                }
                            }
                        }
                    }
                }

                // Remove stale peers (not seen in last 60 seconds)
                if let Some(stale_cutoff) = now.checked_sub(Duration::from_secs(60)) {
                    peer_list.remove_stale(stale_cutoff);
                }

                info!(
                    "Total ESP peers tracked: {} (added/updated: {})",
                    peer_list.count(),
                    peer_count
                );
            } else {
                info!("Failed to get scan records: {}", result);
            }
        }

        // Clean up scan
        esp_idf_sys::esp_wifi_scan_stop();
    }
}

#[embassy_executor::task]
pub async fn scan_task() {
    info!("WiFi scan task started");

    loop {
        info!("Starting WiFi scan...");
        scan().await;
        print_all_peers().await;
        let num_peers = PEER_LIST.lock().await.count();
        if num_peers > 0 {
            Timer::after(Duration::from_secs(600)).await;
        } else {
            // if no peers keep rescanning
            Timer::after(Duration::from_secs(5)).await;
        }
    }
}
