mod config;
mod console;
mod csi;
mod espnow;
mod ftm;
mod peers;
mod scan;
mod timectl;
mod wifi;
mod wipro;

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use log::info;
use embassy_futures::block_on;
use static_cell::StaticCell;


fn main() {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    info!("init log!");
    let peripherals = Peripherals::take().unwrap();
    
    
    let sys_loop = EspSystemEventLoop::take().unwrap();
    let nvs = EspDefaultNvsPartition::take().unwrap();

    let _wifi = block_on(wifi::setup(peripherals.modem, &sys_loop, Some(nvs)))
        .expect("Failed to initialize WiFi!");

    // module for communicating with other wi-pro boards
    espnow::init().expect("Failed to initialize ESP-NOW");
    // initialize fft matrix
    wipro::fft_init();

    static EXECUTOR: StaticCell<embassy_executor::Executor> = StaticCell::new();
    let executor = EXECUTOR.init(embassy_executor::Executor::new());
    
    // run all tasks
    executor.run(|spawner| {
	spawner.spawn(csi::csi_processing_task()).unwrap();
	spawner.spawn(csi::stats_task()).unwrap();
        spawner.spawn(scan::scan_task()).unwrap();
        spawner.spawn(ftm::contact_loop()).unwrap();
        spawner.spawn(ftm::ftm_notification_task()).unwrap();
        spawner.spawn(console::console_task()).unwrap();
        spawner.spawn(espnow::beacon_task()).unwrap();
	spawner.spawn(wifi::mac_counter_keepalive()).unwrap();
        info!("Embassy tasks spawned");
    });
}
