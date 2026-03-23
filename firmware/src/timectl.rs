use crate::wifi::get_mac_counter;
use portable_atomic::{AtomicI64, Ordering};

// This file is mostly unused for now.

// pub struct TimeState {
//     mac_ff_count: i32,
//     _mac_ff_err_est: i32,
//     _mac_ff_last: i64
// }

// pub static TIME_STATE: Mutex<CriticalSectionRawMutex, TimeState> = Mutex::new(TimeState {
//     mac_ff_count: 0,
//     _mac_ff_err_est: 0,
//     _mac_ff_last: 0
// });

pub static MAC_FF_OFFSET: AtomicI64 = AtomicI64::new(0);

pub static _INTERNAL_OFFSET: AtomicI64 = AtomicI64::new(0);


pub fn get_mac_ff_time() -> i64 {
    let mac_raw = get_mac_counter();
    mac_raw.wrapping_add(MAC_FF_OFFSET.load(Ordering::Relaxed))
}

pub fn get_mac_ff_offset() -> i64 {
    MAC_FF_OFFSET.load(Ordering::Relaxed)
}
