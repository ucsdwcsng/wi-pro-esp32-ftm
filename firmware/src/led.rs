use anyhow::Result;
use log::info;

use esp_idf_svc::hal::peripheral::Peripheral;
use esp_idf_svc::sys;
use ws2812_esp32_rmt_driver::Ws2812Esp32RmtDriver; // low-level bindings

fn mac_to_f64(mac: &[u8; 6]) -> f64 {
    // same "hash" idea: XOR all bytes -> angle
    let mut v: u8 = 0;
    for b in mac.iter() {
        v ^= *b;
	v ^= ((*b << 4) & 0xf0) & ((*b >> 4) & 0x0f);
    }
    (v as f64) * 6.28 / 255.0
}

fn compute_color_from_hue(hue: f64) -> (u8, u8, u8) {
    let mut r = 1.0 + hue.cos();
    let mut g = 1.0 + (hue + 2.094).cos();
    let mut b = 1.0 + (hue + 4.188).cos();

    // shift-subtract min
    let min = r.min(g).min(b);
    r -= min;
    g -= min;
    b -= min;

    // normalize length (avoid divide-by-zero)
    let norm = (r * r + g * g + b * b).sqrt().max(1e-6);
    r /= norm;
    g /= norm;
    b /= norm;

    // scale to 0..255
    let ri = (r * 255.0).clamp(0.0, 255.0) as u8;
    let gi = (g * 255.0).clamp(0.0, 255.0) as u8;
    let bi = (b * 255.0).clamp(0.0, 255.0) as u8;
    (ri, gi, bi)
}

fn read_mac() -> Result<[u8; 6]> {
    let mut mac = [0u8; 6];
    // ESP_MAC_WIFI_SOFTAP constant is available via esp-idf C header; in rust we can use the raw value via the generated bindings:
    // ESP_MAC_WIFI_SOFTAP is usually part of the esp-idf headers; use sys::esp_read_mac
    unsafe {
        // ESP_MAC_WIFI_SOFTAP value is defined in IDF headers; use the binding name:
        // If the binding name differs (platform-specific), consult docs; this is the common name:
        let mac_type = sys::esp_mac_type_t_ESP_MAC_WIFI_SOFTAP;
        let rc = sys::esp_read_mac(mac.as_mut_ptr() as *mut u8, mac_type);
        if rc != 0 {
            anyhow::bail!("esp_read_mac failed: {}", rc);
        }
    }
    Ok(mac)
}

pub fn print_hue() {
    let mac = read_mac().unwrap();
    let hue = mac_to_f64(&mac);
    let (r, g, b) = compute_color_from_hue(hue);
    info!("Computed hue {}: RGB -> r {} g {} b {}", hue, r, g, b);

}

pub fn led_init<'d, C, PIN>(
    channel: impl Peripheral<P = C> + 'd,
    pin: impl Peripheral<P = PIN> + 'd,
) where
    C: esp_idf_svc::hal::rmt::RmtChannel,
    PIN: esp_idf_svc::hal::gpio::OutputPin,
{
    // Read MAC
    let mac = read_mac().unwrap();
    info!(
        "MAC: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );

    let hue = mac_to_f64(&mac);
    let (r, g, b) = compute_color_from_hue(hue);
    info!("Computed RGB -> r {} g {} b {}", r, g, b);

    // Create the RMT-based WS2812 driver using the channel and pin
    // obtained from `Peripherals` in the caller (main.rs).
    let mut driver = Ws2812Esp32RmtDriver::new(channel, pin).unwrap();
    // For one LED, prepare GRB bytes (WS2812 expects GRB order)
    let grb = [g, r, b];

    // send (blocking) — the driver expects an iterator of u8, so provide an iterator (cloned bytes)
    driver.write_blocking(grb.iter().cloned()).unwrap();
}
