use base64::{engine::general_purpose, Engine as _};
use core::cell::RefCell;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex;
use esp_idf_svc::sys as esp_idf_sys;
use log::info;
use heapless::String;
use core::sync::atomic::{AtomicU32, Ordering};

pub static CSI_COUNTER: AtomicU32 = AtomicU32::new(0);

use crate::timectl::get_mac_ff_offset;
use crate::wifi::get_mac_counter;

const MAX_CSI_LEN: usize = 384;
const MAX_BASE64_LEN: usize = ((MAX_CSI_LEN + 2) / 3) * 4 + 4;

#[allow(dead_code)]
#[derive(Clone, Copy)]
pub struct CsiData {
    pub mac: [u8; 6],
    pub rssi: i8,
    pub seq: u16,
    //pub rate: u32,
    pub channel: u8,
    pub channel2: u8, // fill me in
    pub timestamp_us: u32,
    pub timestamp_mac_raw: u64,
    pub rxstart_time_cyc: u8,
    pub rxstart_time_cyc_dec: u16,
    pub sig_mode: u8,
    pub stbc: bool,
    pub mcs: u8,
    pub len: usize,
    pub buf: [u8; MAX_CSI_LEN],
}

pub static CSI_CHANNEL: Channel<CriticalSectionRawMutex, CsiData, 10> = Channel::new();


pub struct FtmCsiState {
    /// When true, CSI packets whose MAC matches `mac` are saved to `buffer`
    /// instead of being dumped to serial.  Set to `true` by
    /// `reset_ftm_csi_buf` and back to `false` by `dump_csi_buf`.
    pub active: bool,
    /// MAC address to match while buffering is active.
    pub mac: [u8; 6],
    /// Maximum number of entries to accept before dropping.
    pub capacity: usize,
    /// Accumulated CSI packets.
    pub buffer: Vec<CsiData>,
}

impl FtmCsiState {
    const fn new() -> Self {
        Self {
            active: false,
            mac: [0; 6],
            capacity: 0,
            buffer: Vec::new(),
        }
    }
    pub fn _get_num_csi(self) -> usize {
	self.buffer.len()
    }
}

pub static FTM_CSI_STATE: Mutex<CriticalSectionRawMutex, RefCell<FtmCsiState>> =
    Mutex::new(RefCell::new(FtmCsiState::new()));

pub async fn reset_ftm_csi_buf(mac: &[u8; 6], size: usize) {
    let guard = FTM_CSI_STATE.lock().await;
    let mut state = guard.borrow_mut();
    state.buffer.clear();
    state.mac = *mac;
    state.capacity = size;
    state.active = true;
}

pub async fn dump_csi_buf(print_output: bool) {
    // Take the buffer out and flip the flag while holding the critical
    // section as briefly as possible — we don't want to block the CSI
    // task while we format and print.
    let entries: Vec<CsiData> = {
        let guard = FTM_CSI_STATE.lock().await;
        let mut state = guard.borrow_mut();
        state.active = false;
        // mem::take replaces buffer with an empty Vec (no alloc) and
        // hands us the old one.
        core::mem::take(&mut state.buffer)
    };

    if entries.is_empty() {
        info!("dump_csi_buf: buffer was empty");
        return;
    }

    if !print_output { return; }
    // Allocate formatting workspace once for the entire batch.
    let mut base64_buf = Box::new([0u8; MAX_BASE64_LEN]);
    let mut msg_buf = Box::new(String::<{ MAX_BASE64_LEN + 256 }>::new());
    use core::fmt::Write;
    let _ = write!(msg_buf, "CSI\x01");
    print!("{}",msg_buf);
    for csi_data in &entries {
	let csi_slice = &csi_data.buf[..csi_data.len];
        match general_purpose::STANDARD.encode_slice(csi_slice, &mut *base64_buf) {
            Ok(encoded_len) => {
                if let Ok(encoded_str) = core::str::from_utf8(&base64_buf[..encoded_len]) {
		    let mac_ts = (csi_data.timestamp_mac_raw as i64).wrapping_add(get_mac_ff_offset());
		    msg_buf.clear();
		    let _ = write!(msg_buf,
				   "{}\x01\
				    {}\x01\
				    {}\x01\
				    {}\x01\
				    {}\x01",
				   csi_data.channel,
				   csi_data.channel2,
				   encoded_str,
				   calculate_precise_timestamp_ns(&csi_data, true),
				   mac_ts
		    );
		    print!("{}",msg_buf);
                } else {
                    info!("Failed to convert base64 to UTF-8");
                }
            }
            Err(e) => {
                info!("Base64 encoding failed: {:?}", e);
            }
        }	
    }
    // entries (and its heap allocation) is dropped here.
}



pub fn calculate_precise_timestamp_ns(csi: &CsiData, is_ofdm: bool) -> u64 {
    if !is_ofdm {
        // For non-OFDM, fallback to microsecond timestamp
        return (csi.timestamp_us as u64) * 1000;
    }

    // Apply the WIFI_PKT_RX_TIMESTAMP_NSEC macro from the patch
    let cyc_dec_adjust = if csi.rxstart_time_cyc_dec >= 1024 {
        2048 - csi.rxstart_time_cyc_dec
    } else {
        csi.rxstart_time_cyc_dec
    };

    let timestamp_ns = ((csi.timestamp_us as u64) * 1000)
        + (((csi.rxstart_time_cyc as u64) * 12500) / 1000)
        + (((cyc_dec_adjust as u64) * 1562) / 1000)
        - 20800;

    timestamp_ns
}

pub unsafe extern "C" fn csi_rx_callback(
    _ctx: *mut core::ffi::c_void,
    data: *mut esp_idf_sys::wifi_csi_info_t,
) {
    if data.is_null() {
        return;
    }
    let csi_info = &*data;

    let mut csi_buffer = [0u8; MAX_CSI_LEN];
    let _len = if csi_info.len > 0 && !csi_info.buf.is_null() {
        let copy_len = (csi_info.len as usize).min(MAX_CSI_LEN);
        core::ptr::copy_nonoverlapping(
            csi_info.buf as *const u8,
            csi_buffer.as_mut_ptr(),
            copy_len,
        );
        copy_len
    } else {
        0
    };
    let rx_ctrl = &csi_info.rx_ctrl;

    let mac_timestamp: u64 = get_mac_counter() as u64;
    let rx_timestamp: u64 = rx_ctrl.timestamp() as u64;
    let mac_32 = mac_timestamp & 0x00000000ffffffff;
    let rx_timestamp_wrap = if mac_32 > rx_timestamp {
	// no overflow
	rx_timestamp | (mac_timestamp & 0xffffffff00000000)
    } else {
	// mac clock lower 32 bits overflowed between rx_timestamp record and get_mac_counter()
	rx_timestamp | ((mac_timestamp - 0x0000000100000000) & 0xffffffff00000000)
    };
    //rx_timestamp = rx_timestamp.wrapping_add(get_mac_ff_offset() as u64);
   
    let csi_data = CsiData {
        mac: csi_info.mac,
        rssi: csi_info.rx_ctrl.rssi() as i8,
        seq: csi_info.rx_seq,
        channel: csi_info.rx_ctrl.channel() as u8,
        channel2: csi_info.rx_ctrl.secondary_channel() as u8,
        timestamp_us: rx_ctrl.timestamp(),
	timestamp_mac_raw: rx_timestamp_wrap,
        rxstart_time_cyc: rx_ctrl.rxstart_time_cyc() as u8,
        rxstart_time_cyc_dec: rx_ctrl.rxstart_time_cyc_dec() as u16,
	sig_mode: rx_ctrl.sig_mode() as u8,
	stbc: rx_ctrl.stbc() != 0,
	mcs: rx_ctrl.mcs() as u8,
        len: csi_info.len as usize,
        buf: csi_buffer,
    };

    let _ = CSI_CHANNEL.try_send(csi_data);
}

#[embassy_executor::task]
pub async fn csi_processing_task() {
    info!("CSI processing task started");
    let mut base64_buffer = Box::new([0u8; MAX_BASE64_LEN]);
    let mut msg_buffer = Box::new(String::<{MAX_BASE64_LEN + 256}>::new());

    loop {
        let csi_data = CSI_CHANNEL.receive().await;

	CSI_COUNTER.fetch_add(1, Ordering::Relaxed);

	// add mac clock offset (can't call in ISR, needs a lock)
	let mac_timestamp = (csi_data.timestamp_mac_raw as i64).wrapping_add(get_mac_ff_offset());

	let buffered = {
            let guard = FTM_CSI_STATE.lock().await;
            let mut state = guard.borrow_mut();
            if state.active && state.mac == csi_data.mac {
                if state.buffer.len() < state.capacity {
                    state.buffer.push(csi_data);
                } else {
                    info!(
                        "FTM CSI buffer full (cap={}), dropping seq={}",
                        state.capacity, csi_data.seq
                    );
                }
                true
            } else {
                false
            }
        };
	if buffered {
	    continue;
	}
	
        // info!(
        //     "Processing CSI from MAC: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        //     csi_data.mac[0],
        //     csi_data.mac[1],
        //     csi_data.mac[2],
        //     csi_data.mac[3],
        //     csi_data.mac[4],
        //     csi_data.mac[5]
        // );
        // info!(
        //     "  RSSI: {}, Channel: {}, Length: {}",
        //     csi_data.rssi, csi_data.channel, csi_data.len
        // );

        if csi_data.len > 0 {
            let csi_slice = &csi_data.buf[..csi_data.len];

            match general_purpose::STANDARD.encode_slice(csi_slice, &mut *base64_buffer) {
                Ok(encoded_len) => {
                    if let Ok(encoded_str) = core::str::from_utf8(&base64_buffer[..encoded_len]) {
			msg_buffer.clear();
                        // Write everything to the buffer first
                        use core::fmt::Write;
                        let _ = write!(
                            msg_buffer,
                            "\x02\
					CSI\x01\
					{:16x}\x01\
					{}\x01\
					{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}\x01",
			    mac_timestamp,
                            csi_data.seq,
                            csi_data.mac[0],
                            csi_data.mac[1],
                            csi_data.mac[2],
                            csi_data.mac[3],
                            csi_data.mac[4],
                            csi_data.mac[5]
                        );
                        let _ = write!(
                            msg_buffer,
                            "{}\x01{}\x01",
                            calculate_precise_timestamp_ns(&csi_data, true),
                            encoded_len
                        );
                        let _ = write!(msg_buffer, "{}\x01", encoded_str);
                        let _ = write!(
                            msg_buffer,
                            "{}\x01\
			     {}\x01\
			     {}\x01\
			     {}\x03\r\n",
                            csi_data.channel, csi_data.channel2, csi_data.rssi, csi_data.sig_mode
                        );
                        print!("{}", msg_buffer);
                    } else {
                        info!("Failed to convert base64 to UTF-8");
                    }
                }
                Err(e) => {
                    info!("Base64 encoding failed: {:?}", e);
                }
            }
        } else {
            info!("No CSI data in buffer");
        }
    }
}

#[embassy_executor::task]
pub async fn stats_task() {
    use embassy_time::{Duration, Timer};
    
    loop {
        Timer::after(Duration::from_secs(10)).await;
        
        let count = CSI_COUNTER.swap(0, Ordering::Relaxed);
        info!("CSI packets in last 10s: {} ({:.1}/sec)", count, count as f32 / 10.0);
    }
}
