use core::fmt::Write;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::Instant;
use heapless::{String, Vec};
use log::info;

use crate::ftm::PeerFtmStats;

const MAX_PEERS: usize = 64;

#[derive(Clone, Copy, Debug)]
pub struct PeerContactStats {
    pub last_seen: Instant,          // Last time we saw this peer
    pub rssi: i8,                    // Signal strength
    pub channel: u8,                 // WiFi channel
}


#[derive(Clone, Copy, Debug)]
pub struct PeerInfo {
    pub bssid: [u8; 6],
    pub ftm_stats: PeerFtmStats,
    pub contact_stats: PeerContactStats,
}

impl PeerInfo {
    pub fn bssid_str(&self) -> String<17> {
        let mut s = String::<17>::new();
        write!(
            &mut s,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.bssid[0],
            self.bssid[1],
            self.bssid[2],
            self.bssid[3],
            self.bssid[4],
            self.bssid[5]
        )
        .unwrap();
        s
    }
}

pub struct PeerList {
    peers: Vec<PeerInfo, MAX_PEERS>,
}

impl PeerList {
    const fn new() -> Self {
        Self { peers: Vec::new() }
    }

    pub fn add_or_update(&mut self, peer: PeerInfo) -> Result<&mut PeerInfo, ()> {
	// Check if peer already exists (by BSSID)
	let index = self.peers.iter().position(|p| p.bssid == peer.bssid);
	
	if let Some(idx) = index {
            // Update existing peer
            let existing = &mut self.peers[idx];
	    existing.contact_stats = peer.contact_stats;
            Ok(existing)
	} else {
            // Add new peer
            self.peers.push(peer).map_err(|_| ())?;
            Ok(self.peers.last_mut().unwrap())
	}
    }
    /// Get number of peers
    pub fn count(&self) -> usize {
        self.peers.len()
    }

    /// Get peer by index
    // pub fn get(&self, index: usize) -> Option<&PeerInfo> {
    //     self.peers.get(index)
    // }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut PeerInfo> {
        self.peers.get_mut(index)
    }

    /// Get all peers as a slice
    pub fn all(&self) -> &[PeerInfo] {
        &self.peers
    }

    /// Remove peers not seen since the given time
    pub fn remove_stale(&mut self, cutoff: Instant) {
        self.peers.retain(|peer| peer.contact_stats.last_seen >= cutoff);
    }

    //// Find peer by BSSID
    pub fn find_by_bssid(&self, bssid: &[u8; 6]) -> Option<&PeerInfo> {
        self.peers.iter().find(|p| &p.bssid == bssid)
    }
}

// Global static peer list protected by a mutex
pub static PEER_LIST: Mutex<CriticalSectionRawMutex, PeerList> = Mutex::new(PeerList::new());

pub async fn print_all_peers() {
    let peer_list = PEER_LIST.lock().await;
    info!("Peers:");
    for peer in peer_list.all() {
        info!(
            "{}: RSSI: {} Last Seen: {} Next Contact: {}",
            peer.contact_stats.rssi,
            peer.bssid_str(),
            peer.contact_stats.last_seen,
            peer.ftm_stats.next_contact_after
        );
    }
}
