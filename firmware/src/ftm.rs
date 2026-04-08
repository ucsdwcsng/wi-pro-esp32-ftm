use crate::config::CONFIG;
use crate::espnow::{send_ftm_notification, FtmSyncPacket};
use crate::peers::PeerContactStats;
use crate::peers::PEER_LIST;
use crate::csi::{reset_ftm_csi_buf, dump_csi_buf};
use crate::wipro::process_report;
use base64::{engine::general_purpose, Engine as _};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use esp_idf_svc::sys as esp_idf_sys;
use std::string::String;
use log::{info, warn};

pub struct FtmState {
    pub mute: bool,
    pub num_burst: i32,
    pub estimate_range: bool,
}

pub static FTM_STATE: Mutex<CriticalSectionRawMutex, FtmState> =
    Mutex::new(FtmState {
	mute: true,
	num_burst: 16,
	estimate_range: false
    });

#[derive(Clone, Copy, Debug)]
pub struct PeerFtmStats {
    pub next_contact_after: Instant, // Next time to do an FTM exchange to them
    pub seq: u32,
    pub should_init: bool,
}

const MAX_FTM_ENTRIES: usize = 64;

const MAX_FTM_BUF_LEN: usize = size_of::<esp_idf_sys::wifi_ftm_report_entry_t>() + 16;
const MAX_BASE64_LEN: usize = ((MAX_FTM_BUF_LEN + 2) / 3) * 4 + 4;

static mut FTM_CFG: esp_idf_sys::wifi_ftm_initiator_cfg_t = unsafe { core::mem::zeroed() };


#[derive(Copy, Clone)]
pub struct FtmReportMetadata {
    pub peer_mac: [u8; 6],
    pub _rtt_raw: u32,
    pub _rtt_est: u32,
    pub _dist_est: u32,
    pub _status: u32,
    pub num_entries: u8,
}

#[derive(Clone, Copy)]
pub struct FtmReport {
    pub meta: FtmReportMetadata,
    pub entries: [esp_idf_sys::wifi_ftm_report_entry_t; MAX_FTM_ENTRIES],
}


static mut FTM_REPORT_METADATA: Option<FtmReportMetadata> = None;
static mut FTM_ENTRIES_BUFFER: [esp_idf_sys::wifi_ftm_report_entry_t; MAX_FTM_ENTRIES] = 
    unsafe { core::mem::zeroed() };

pub struct FtmNotification {
    pub peer_mac: [u8; 6],
    pub sync_info: FtmSyncPacket,
    pub _rx_mac_time: i64
}

static FTM_RADIO_DONE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

pub static FTM_NOTIFY_CHANNEL: Channel<CriticalSectionRawMutex, FtmNotification, 8> =
    Channel::new();


#[allow(static_mut_refs)]
pub unsafe fn handle_ftm_report_event(event_data: *const esp_idf_sys::wifi_event_ftm_report_t) {
    if event_data.is_null() {
        FTM_RADIO_DONE.signal(());
        return;
    }
    let report = &*event_data;
    
    // Store metadata only
    
    FTM_REPORT_METADATA = Some(FtmReportMetadata {
        peer_mac: report.peer_mac,
        _rtt_raw: report.rtt_raw,
        _rtt_est: report.rtt_est,
        _dist_est: report.dist_est,
        _status: report.status,
        num_entries: report.ftm_report_num_entries.min(MAX_FTM_ENTRIES as u8),
    });
    
    FTM_RADIO_DONE.signal(());  // Wake waiting task
}



// FTM initiate function
pub async fn initiate_ftm(peer_bssid: [u8; 6], channel: u8, n_ftm: i32) -> Result<(), i32> {
    let mut base64_buffer = vec![0u8; MAX_BASE64_LEN];
    //let mut msg_buffer = vec![0u8; MAX_BASE64_LEN + 256];
    //let mut msg_buffer = String::<{MAX_BASE64_LEN + 256}>::new();
    let mut msg_buffer = String::new();
    unsafe {
        // Configure FTM session
	FTM_CFG = esp_idf_sys::wifi_ftm_initiator_cfg_t {
            resp_mac: peer_bssid,
            channel: channel,
            frm_count: n_ftm as u8,
            burst_period: 2,
            use_get_report_api: true,
        };

	reset_ftm_csi_buf(&peer_bssid, (n_ftm*2) as usize).await;
        let result = esp_idf_sys::esp_wifi_ftm_initiate_session(&raw mut FTM_CFG as *mut _);

        if result == esp_idf_sys::ESP_OK {
	    FTM_RADIO_DONE.wait().await;
	    #[allow(static_mut_refs)]
	    let report = if let Some(metadata) = FTM_REPORT_METADATA.take() {
                let result = esp_idf_sys::esp_wifi_ftm_get_report(
		    FTM_ENTRIES_BUFFER.as_mut_ptr(),
		    metadata.num_entries
                );
                
                if result == esp_idf_sys::ESP_OK {
		    // Build full report for processing
		    let mut report = FtmReport {
			meta: metadata,
                        entries: [core::mem::zeroed(); MAX_FTM_ENTRIES],
		    };
		    
		    report.entries[..metadata.num_entries as usize]
                        .copy_from_slice(&FTM_ENTRIES_BUFFER[..metadata.num_entries as usize]);
		    report
                } else {
		    warn!("esp_wifi_ftm_get_report failed: {}", result);
		    return Err(result);
                }
	    } else {
		warn!("couldn't find FTM report metadata");
		return Err(1);
	    };
	    
	    let run_wipro = FTM_STATE.lock().await.estimate_range;
	    if run_wipro {
		process_report(&report).await;
	    }
	    
            //get peer
            let peer_list = PEER_LIST.lock().await;
            if let Some(peer) = peer_list.find_by_bssid(&report.meta.peer_mac) {
                let _ = send_ftm_notification(
                    &peer.bssid,
                    peer.ftm_stats.seq,
                    peer.ftm_stats.should_init,
                );
            } else {
                // we FTM'ed a peer we don't know about? Shouldn't happen.
		info!("FTM'd peer not in list!")
            }
            drop(peer_list);

	    msg_buffer.clear();
	    use core::fmt::Write;
	    let _ = write!(msg_buffer,
			   "\x02FTM\x01\
			    {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}\x01\
			    {}\x01",
			   report.meta.peer_mac[0],
                           report.meta.peer_mac[1],
                           report.meta.peer_mac[2],
                           report.meta.peer_mac[3],
                           report.meta.peer_mac[4],
                           report.meta.peer_mac[5],
			   report.meta.num_entries
	    ); // switch to one large message
	    print!("{}", msg_buffer);
            for i in 0..report.meta.num_entries {
                let entry = &report.entries[i as usize];
                let entry_bytes = core::slice::from_raw_parts(
                    entry as *const _ as *const u8,
                    core::mem::size_of::<esp_idf_sys::wifi_ftm_report_entry_t>(),
                );
		
                match general_purpose::STANDARD.encode_slice(entry_bytes, &mut base64_buffer) {
                    Ok(encoded_len) => {
                        if let Ok(encoded_str) = core::str::from_utf8(&base64_buffer[..encoded_len])
                        {
			    msg_buffer.clear();
                            let _ = write!(msg_buffer, "{}\x01", encoded_str);
                            print!("{}", msg_buffer);
                        } else {
                        }
                    }
                    Err(_) => {}
                }
            }
	    drop(msg_buffer);
	    drop(base64_buffer);
	    dump_csi_buf(true).await;
	    print!("\x03\r\n");
	    
            Ok(())
        } else {
	    dump_csi_buf(false).await;
            Err(result)
        }
    }
}

pub fn handle_ftm_notification(packet: FtmNotification) {
    let _ = FTM_NOTIFY_CHANNEL.try_send(packet);
}

#[embassy_executor::task]
pub async fn contact_loop() {
    info!("Contact loop started");

    // Give system time to discover some peers first
    Timer::after(Duration::from_secs(5)).await;
    loop {
        //let releaser = FTM_RADIO_SEM.acquire(1).await.unwrap();

        let is_mute = FTM_STATE.lock().await.mute;
        // Lock the peer list
        let mut peer_list = PEER_LIST.lock().await;

        let peer_count = peer_list.count();

        let now = Instant::now();
        let mut t_to_next = Duration::from_millis(1000);
        let mut most_overdue_index: Option<usize> = None;
        let mut earliest_expired_time: Option<Instant> = None;

        // Find the most overdue peer
        for i in 0..peer_count {
            if let Some(peer) = peer_list.get_mut(i) {
                if now >= peer.ftm_stats.next_contact_after {
                    // This peer is overdue - check if it's the most overdue so far
                    if earliest_expired_time.is_none()
                        || peer.ftm_stats.next_contact_after < earliest_expired_time.unwrap()
                    {
                        earliest_expired_time = Some(peer.ftm_stats.next_contact_after);
                        most_overdue_index = Some(i);
                    }
                } else {
                    // Track when the next peer will be ready
                    let peer_next_contact =
                        peer.ftm_stats.next_contact_after - now + Duration::from_millis(1);
                    if peer_next_contact <= t_to_next {
                        t_to_next = peer_next_contact;
                    }
                }
            }
        }
        // Process the most overdue peer if one was found
        if let Some(index) = most_overdue_index {
            if let Some(peer) = peer_list.get_mut(index) {
                let peer_copy = *peer;
                let contact_interval_ms = CONFIG.lock(|config| config.borrow().contact_interval_ms);

                if peer.ftm_stats.should_init {
                    peer.ftm_stats.next_contact_after =
                        now + Duration::from_millis(contact_interval_ms);
                } else {
                    peer.ftm_stats.next_contact_after =
                        now + Duration::from_millis(contact_interval_ms * 10);
                }

                if !is_mute {
                    if peer.ftm_stats.should_init {
                        peer.ftm_stats.seq = peer.ftm_stats.seq.wrapping_add(1);
                    }
                    drop(peer_list);
		    let num_ftm = FTM_STATE.lock().await.num_burst;

                    match initiate_ftm(peer_copy.bssid, peer_copy.contact_stats.channel, num_ftm).await {
                        Ok(_) => {
                            info!("FTM completed for {}", peer_copy.bssid_str());
                        }
                        Err(e) => info!("FTM initiation failed: {}", e),
                    }
                } else {
                    drop(peer_list);
                }
                t_to_next = Duration::from_millis(1);
            }
        } else {
            drop(peer_list);
        }

	// can't do FTMs faster than this or things start to break
        if t_to_next < Duration::from_millis(300) {
            t_to_next = Duration::from_millis(300);
        }
        Timer::after(t_to_next).await;
    }
}

#[embassy_executor::task]
pub async fn ftm_notification_task() {
    info!("FTM notification task started");

    loop {
        let packet = FTM_NOTIFY_CHANNEL.receive().await;
        let wants_init = packet.sync_info.is_initiator != 0;
        let sequence = packet.sync_info.sequence as u32;
        let softap_channel = crate::wifi::get_channel().await;
        let mut peer_list = PEER_LIST.lock().await;
        let _ = peer_list
            .add_or_update(crate::peers::PeerInfo {
                bssid: packet.peer_mac,
                ftm_stats: PeerFtmStats {
                    next_contact_after: Instant::now(),
                    seq: packet.sync_info.sequence,
                    should_init: true,
                },
                contact_stats: PeerContactStats {
                    rssi: 0,
                    channel: softap_channel,
                    last_seen: Instant::now(),
                },
            })
            .unwrap();
	
        drop(peer_list);
        info!(
            "FTM notification received for {}: {}/{}",
            packet
                .peer_mac
                .iter()
                .map(|b| format!("{:02X}", b))
                .collect::<Vec<_>>()
                .join(":"),
            sequence,
            wants_init
        );
    }
}

pub async fn set_mute(mute: bool) {
    let mut state = FTM_STATE.lock().await;
    state.mute = mute;
    info!("Mute: {}", state.mute);
}

pub async fn set_burst(num_burst: i32) {
    let mut state = FTM_STATE.lock().await;
    if num_burst == 16 || num_burst == 24 || num_burst == 32 || num_burst == 64 {
	state.num_burst = num_burst;
	info!("Set bursts to {}", state.num_burst);
    } else {
	info!("Must set burst count to <16|24|32|64>");
    }
}

pub async fn set_run_wipro(run: bool) {
    let mut state = FTM_STATE.lock().await;
    state.estimate_range = run;
    info!("Run Wi-PRO: {}", state.estimate_range);
}
