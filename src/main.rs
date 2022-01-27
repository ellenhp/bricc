mod wifi;

use esp_idf_sys::{self as _}; // If using the `binstart` feature of `esp-idf-sys`, always keep this module imported
use std::time::Duration;

fn main() {
    esp_idf_sys::link_patches();

    println!("Bricc booted, starting wifi");

    #[allow(unused)]
    let mut wifi_manager = wifi::WifiManager::init();
    wifi_manager
        .set_ap_wpa2_psk("bricc".into(), "showscreen".into())
        .unwrap();
    loop {
        println!("Sitting around doing nothing.");
        std::thread::sleep(Duration::from_secs(10));
    }
}
