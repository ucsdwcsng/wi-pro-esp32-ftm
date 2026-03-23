use core::cell::RefCell;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;

pub struct Config {
    pub softap_prefix: &'static str,
    pub promi_en: bool,
    pub contact_interval_ms: u64,
}

pub static CONFIG: Mutex<CriticalSectionRawMutex, RefCell<Config>> =
    Mutex::new(RefCell::new(Config {
        softap_prefix: "WI_PRO",
        promi_en: false,
        contact_interval_ms: 5000,
    }));
