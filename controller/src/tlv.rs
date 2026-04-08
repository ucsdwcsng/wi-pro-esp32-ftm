use base64::{engine::general_purpose, Engine as _};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FTMReport {
    pub own_mac: String,
    pub tgt_mac: String,
    pub dlog_token: u8,
    pub rssi: i8,
    pub t1: u64,
    pub t2: u64,
    pub t3: u64,
    pub t4: u64,
    pub channel: u32,
    pub channel2: u32,
    pub payload_b64: String,
    pub mac_timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FTMEvent {
    pub t_ms: u64,
    pub own_mac: String,
    pub tgt_mac: String,
    pub seq: u32,
    pub reports: Vec<FTMReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CSIEvent {
    pub t_ms: u64,
    pub seq: i32,
    pub own_mac: String,
    pub tgt_mac: String,
    pub timestamp: u64,
    pub channel: u32,
    pub channel2: u32,
    pub rssi: i32,
    pub payload_b64: String,
    pub mac_timestamp: i64,
    pub sig_mode: u8
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DBGEvent {
    pub own_mac: String,
    pub data_type: String,
    pub len: usize,
    pub payload_b64: String
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RangeEvent {
    pub t_ms: u64,
    pub own_mac: String,
    pub tgt_mac: String,
    pub timestamp: u64,
    pub range: f32,
}

// Enum to hold either event type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ESPEvent {
    CSI(CSIEvent),
    FTM(FTMEvent),
    DBG(DBGEvent),
    Range(RangeEvent),
}

pub fn parse_ftm(msg: Vec<&[u8]>, own_mac: &str, t_ms: u64) -> Result<FTMEvent, String> {
    // Validate m<essage has correct number of fields
    if msg.len() < 4 {
        return Err(format!(
            "Invalid FTM message: expected at least 4 fields, got {}",
            msg.len()
        ));
    }

    let tgt_mac = str::from_utf8(msg[1])
        .map_err(|e| format!("Invalid UTF-8 in tgt_mac: {}", e))?
        .trim()
        .to_string();

    let _n_reports = str::from_utf8(msg[2])
        .map_err(|e| format!("Invalid UTF-8 in seq: {}", e))?
        .parse::<u32>()
        .map_err(|e| format!("Invalid seq: {}", e))? as usize;


    let mut ftm_reports = Vec::new();
    let mut ii = 4;
    while ii < msg.len() {
	let payload_b64 = msg[ii];

	if str::from_utf8(msg[ii]).map_err(|e| format!("Invalid UTF-8 in tgt_mac: {}", e))?.trim() == "CSI" {
	    ii += 1;
	    break;
	}
	
	// Decode base64 payload
	let payload = general_purpose::STANDARD
            .decode(payload_b64)
            .map_err(|e| format!("Error decoding FTM base64: {}", e))?;
	
	// Each FTM report is 40 bytes
	const FTM_REPORT_SIZE: usize = 40;
	
	if payload.len() % FTM_REPORT_SIZE != 0 {
            return Err(format!(
		"Incorrect FTM Size: {} (expected multiple of {})",
		payload.len(),
		FTM_REPORT_SIZE
            ));
	}
	
	// Parse each FTM report
	for chunk in payload.chunks_exact(FTM_REPORT_SIZE) {
            let dlog_token = chunk[0];
            let rssi = chunk[1] as i8;
            // chunk[2], chunk[3] are padding
            let t1 = u64::from_le_bytes(chunk[8..16].try_into().unwrap());
            let t2 = u64::from_le_bytes(chunk[16..24].try_into().unwrap());
            let t3 = u64::from_le_bytes(chunk[24..32].try_into().unwrap());
            let t4 = u64::from_le_bytes(chunk[32..40].try_into().unwrap());
	    
            ftm_reports.push(FTMReport {
		own_mac: own_mac.to_string(),
		tgt_mac: tgt_mac.clone(),
		dlog_token,
		rssi,
		t1,
		t2,
		t3,
		t4,
		channel: 0,
		channel2: 0,
		payload_b64: "".to_owned(),
		mac_timestamp: 0
            });
	}
	ii += 1;

    }
    while ii+5 <= msg.len() {
	let ch = str::from_utf8(msg[ii])
            .map_err(|e| format!("Invalid UTF-8 in seq: {}", e))?
            .parse::<u32>()
            .map_err(|e| format!("Invalid seq: {}", e))?;
	let ch2 = str::from_utf8(msg[ii+1])
            .map_err(|e| format!("Invalid UTF-8 in seq: {}", e))?
            .parse::<u32>()
            .map_err(|e| format!("Invalid seq: {}", e))?;
	let csi_b64 = str::from_utf8(msg[ii+2])
            .map_err(|e| format!("Invalid UTF-8 in tgt_mac: {}", e))?
            .trim()
            .to_string();

	let t_ps = (str::from_utf8(msg[ii+3])
            .map_err(|e| format!("Invalid UTF-8 in seq: {}", e))?
            .parse::<u64>()
            .map_err(|e| format!("Invalid seq: {}", e))? * 1000) as i64;

	let t_mac = str::from_utf8(msg[ii+4])
            .map_err(|e| format!("Invalid UTF-8 in seq: {}", e))?
            .parse::<u64>()
            .map_err(|e| format!("Invalid seq: {}", e))? as i64;
	for f in &mut ftm_reports {
	    let delta = t_ps - f.t2 as i64;
	    if delta > -2000 && delta < 2000 {
		f.channel = ch;
		f.channel2 = ch2;
		f.payload_b64 = csi_b64;
		f.mac_timestamp = t_mac;
		break;
	    }
	}
	ii += 5;
    }
    
    ftm_reports.retain(|f| !f.payload_b64.is_empty());
    
    Ok(FTMEvent {
        t_ms,
        own_mac: own_mac.to_string(),
        tgt_mac: tgt_mac.clone(),
        seq: 0,
        reports: ftm_reports
    })
}

pub fn parse_csi(msg: Vec<&[u8]>, own_mac: &str, t_ms: u64) -> Result<CSIEvent, String> {
    // Validate message has correct number of fields
    if msg.len() != 11 {
        return Err(format!(
            "Invalid CSI message: expected 10 fields, got {}",
            msg.len()
        ));
    }

    let mac_timestamp = u64::from_str_radix(
	str::from_utf8(msg[1])
            .map_err(|e| format!("Invalid UTF-8 in seq: {}", e))?
            .trim(),
	16
    ).map_err(|e| format!("Invalid hex field: {}", e))? as i64;

    let seq = str::from_utf8(msg[2])
        .map_err(|e| format!("Invalid UTF-8 in seq: {}", e))?
        .parse::<i32>()
        .map_err(|e| format!("Invalid seq: {}", e))?;

    let tgt_mac = str::from_utf8(msg[3])
        .map_err(|e| format!("Invalid UTF-8 in tgt_mac: {}", e))?
        .trim()
        .to_string();

    let timestamp = str::from_utf8(msg[4])
        .map_err(|e| format!("Invalid UTF-8 in timestamp: {}", e))?
        .parse::<u64>()
        .map_err(|e| format!("Invalid timestamp: {}", e))?;

    let exp_payload_size = str::from_utf8(msg[5])
        .map_err(|e| format!("Invalid UTF-8 in payload size: {}", e))?
        .parse::<usize>()
        .map_err(|e| format!("Invalid payload size: {}", e))?;

    let payload_b64 = msg[6];

    let channel = str::from_utf8(msg[7])
        .map_err(|e| format!("Invalid UTF-8: {}", e))?
        .parse::<u32>()
        .map_err(|e| format!("Invalid: {}", e))?;

    let channel2 = str::from_utf8(msg[8])
        .map_err(|e| format!("Invalid UTF-8: {}", e))?
        .parse::<u32>()
        .map_err(|e| format!("Invalid: {}", e))?;

    let rssi = str::from_utf8(msg[9])
        .map_err(|e| format!("Invalid UTF-8: {}", e))?
        .parse::<i32>()
        .map_err(|e| format!("Invalid: {}", e))?;

    let sig_mode = str::from_utf8(msg[10])
	.map_err(|e| format!("Invalid UTF-8: {}", e))?
	.parse::<u8>()
	.map_err(|e| format!("Invalid: {}", e))?;

    // Validate payload size
    if payload_b64.len() != exp_payload_size {
        return Err(format!(
            "Incorrect CSI Size: expected {}, got {}",
            exp_payload_size,
            payload_b64.len()
        ));
    }

    let payload_b64_str = str::from_utf8(payload_b64)
        .map_err(|e| format!("Invalid UTF-8 in payload: {}", e))?
        .to_string();

    Ok(CSIEvent {
        t_ms: t_ms,
        seq: seq,
        own_mac: own_mac.to_string(),
        tgt_mac: tgt_mac,
        timestamp: timestamp,
        channel: channel,
        channel2: channel2,
        rssi: rssi,
	sig_mode,
        payload_b64: payload_b64_str,
	mac_timestamp,
    })
}


pub fn parse_dbg(msg: Vec<&[u8]>, own_mac: &str, _t_ms: u64) -> Result<DBGEvent, String> {
    if msg.len() != 4 {
        return Err(format!(
            "Invalid DBG message: expected 4 fields, got {}",
            msg.len()
        ));
    }

    let data_type = str::from_utf8(msg[1])
        .map_err(|e| format!("Invalid UTF-8 in data_type: {}", e))?
        .trim()
        .to_string();

    let len = str::from_utf8(msg[2])
        .map_err(|e| format!("Invalid UTF-8 in len: {}", e))?
        .trim()
        .parse::<usize>()
        .map_err(|e| format!("Invalid len: {}", e))?;

    let payload_b64 = str::from_utf8(msg[3])
        .map_err(|e| format!("Invalid UTF-8 in payload: {}", e))?
        .trim()
        .to_string();

    Ok(DBGEvent {
        own_mac: own_mac.to_string(),
        data_type,
        len,
        payload_b64,
    })
}

pub fn parse_range(msg: Vec<&[u8]>, own_mac: &str, t_ms: u64) -> Result<RangeEvent, String> {
    if msg.len() != 4 {
        return Err(format!(
            "Invalid RANGE message: expected 4 fields, got {}",
            msg.len()
        ));
    }

    let tgt_mac = str::from_utf8(msg[1])
        .map_err(|e| format!("Invalid UTF-8 in tgt_mac: {}", e))?
        .trim()
        .to_string();

    let timestamp = str::from_utf8(msg[2])
        .map_err(|e| format!("Invalid UTF-8 in timestamp: {}", e))?
        .trim()
        .parse::<u64>()
        .map_err(|e| format!("Invalid timestamp: {}", e))?;

    let range = str::from_utf8(msg[3])
        .map_err(|e| format!("Invalid UTF-8 in range: {}", e))?
        .trim()
        .parse::<f32>()
        .map_err(|e| format!("Invalid range: {}", e))?;

    Ok(RangeEvent {
        t_ms,
        own_mac: own_mac.to_string(),
        tgt_mac,
        timestamp,
        range,
    })
}

pub fn parse(line: &str) -> Option<Vec<&[u8]>> {
    let bytes = line.as_bytes();

    // Find start marker (0x02)
    let msg_start_idx = bytes.iter().position(|&b| b == 0x02)?;

    // Find end marker (0x03) after the start
    let msg_end_idx = bytes[msg_start_idx..].iter().position(|&b| b == 0x03)?;

    // Extract content between markers (exclusive)
    let msg_content = &bytes[msg_start_idx + 1..msg_start_idx + msg_end_idx];

    // Split on field delimiter (0x01)
    let fields: Vec<&[u8]> = msg_content.split(|&b| b == 0x01).collect();

    Some(fields)
}

impl CSIEvent {
    pub fn to_csv(&self) -> String {
        format!(
            "{},{},{},{},{},{},{},{},{}\n",
            self.t_ms, self.timestamp, self.own_mac, self.tgt_mac, self.seq, self.payload_b64, self.mac_timestamp, self.rssi, self.sig_mode
        )
    }
}

impl FTMEvent {
    pub fn to_csv(&self) -> Vec<String> {
        // FTM returns multiple CSV lines (one per report)
        self.reports
            .iter()
            .map(|report| {
                format!(
                    "{},{},{},{},{},{},{},{},{},{},{}\n",
                    self.own_mac,
                    self.t_ms,
                    self.seq,
                    self.tgt_mac,
                    report.dlog_token,
                    report.rssi,
                    report.t1,
                    report.t2,
                    report.t3,
                    report.t4,
		    report.payload_b64,
                )
            })
            .collect()
    }
}

impl DBGEvent {
    pub fn to_csv(&self) -> String {
        format!(
            "{},{},{},{}\n",
            self.own_mac,
            self.data_type,
            self.len,
            self.payload_b64
        )
    }
}

impl RangeEvent {
    pub fn to_csv(&self) -> String {
        format!(
            "{},{},{},{},{}\n",
            self.t_ms,
            self.own_mac,
            self.tgt_mac,
            self.timestamp,
            self.range,
        )
    }
}


impl ESPEvent {
    pub fn to_csv(&self) -> Vec<String> {
        match self {
            ESPEvent::CSI(csi) => vec![csi.to_csv()],
            ESPEvent::FTM(ftm) => ftm.to_csv(),
	    ESPEvent::DBG(dbg) => vec![dbg.to_csv()],
            ESPEvent::Range(range) => vec![range.to_csv()],
        }
    }
    pub fn id_str(&self) -> String {
        match self {
            ESPEvent::CSI(_) => "CSI".to_string(),
            ESPEvent::FTM(_) => "FTM".to_string(),
	    ESPEvent::DBG(_) => "DBG".to_string(),
            ESPEvent::Range(_) => "RANGE".to_string(),
        }
    }
}
