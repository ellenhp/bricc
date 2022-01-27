use crate::wifi::mpsc::{Receiver, Sender};
use embedded_svc::wifi::AccessPointConfiguration;
use embedded_svc::wifi::AuthMethod;
use embedded_svc::wifi::ClientConfiguration;
use embedded_svc::wifi::Configuration;
use embedded_svc::wifi::Wifi;
use esp_idf_svc::netif::EspNetifStack;
use esp_idf_svc::nvs::EspDefaultNvs;
use esp_idf_svc::sysloop::EspSysLoopStack;
use esp_idf_svc::wifi::*;
use esp_idf_sys::EspError;
use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::mpsc::SendError;
use std::sync::Arc;
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;

const WIFI_SCAN_PERIOD: Duration = Duration::from_secs(60);

pub type SSID = String;
pub type PSKKey = String;
pub type WifiSignalStrength = u8;

pub enum WifiCommand {
    ConnectWPA2PSK(SSID, PSKKey),
    CreateApWPA2PSK(SSID, PSKKey),
}

enum WifiStatus {
    Connected(SSID, WifiSignalStrength),
    ApOnly(SSID),
    Disconnected,
    Error(WifiError),
}

enum WifiError {
    Fatal(String),
    NetworkNotFound(SSID),
}

impl From<EspError> for WifiError {
    fn from(_: EspError) -> WifiError {
        WifiError::Fatal("Unknown error during wifi operation".into())
    }
}

struct WifiManagerConfig {
    client_configs: HashMap<SSID, ClientConfiguration>,
    ap_config: Option<AccessPointConfiguration>,
}

pub struct WifiManager {
    #[allow(unused)]
    connection_thread: JoinHandle<()>,
    command_sender: Sender<WifiCommand>,
    status_receiver: Receiver<WifiStatus>,
}

impl WifiManager {
    fn make_ap_wpa2_psk(
        mut configs: WifiManagerConfig,
        ssid: SSID,
        key: PSKKey,
    ) -> WifiManagerConfig {
        configs.ap_config = Some(AccessPointConfiguration {
            ssid,
            channel: 1,
            password: key,
            auth_method: AuthMethod::WPA2WPA3Personal,
            ..Default::default()
        });
        configs
    }
    fn connect_wpa2_psk(
        mut configs: WifiManagerConfig,
        ssid: SSID,
        key: PSKKey,
    ) -> WifiManagerConfig {
        let config = ClientConfiguration {
            ssid: ssid.clone().into(),
            password: key.into(),
            channel: None,
            ..Default::default()
        };
        configs.client_configs.insert(ssid, config);
        configs
    }
    fn reconfigure_wifi(
        config: &WifiManagerConfig,
        mut esp_wifi: EspWifi,
    ) -> (EspWifi, Result<WifiStatus, WifiError>) {
        if config.client_configs.is_empty() {
            let ap_config = config.ap_config.clone();
            if ap_config.is_some() {
                let is_error = esp_wifi
                    .set_configuration(&Configuration::AccessPoint(ap_config.clone().unwrap()))
                    .is_err();
                if !is_error {
                    return (esp_wifi, Ok(WifiStatus::ApOnly(ap_config.unwrap().ssid)));
                } else {
                    return (
                        esp_wifi,
                        Err(WifiError::Fatal("Failed to create AP".into())),
                    );
                }
            } else {
                (esp_wifi, Ok(WifiStatus::Disconnected))
            }
        } else {
            let scan_result = esp_wifi.scan();

            if scan_result.is_err() {
                return (esp_wifi, Err(scan_result.unwrap_err().into()));
            }

            let aps = scan_result.unwrap().into_iter();

            for ap in aps {
                let client_config = config.client_configs.get(&ap.ssid);
                if client_config.is_some() {
                    let ap_config = config.ap_config.clone();
                    let overall_config = if ap_config.is_some() && client_config.is_some() {
                        Configuration::Mixed(client_config.unwrap().clone(), ap_config.unwrap())
                    } else if ap_config.is_some() {
                        Configuration::AccessPoint(ap_config.unwrap())
                    } else if client_config.is_some() {
                        Configuration::Client(client_config.unwrap().clone())
                    } else {
                        return (esp_wifi, Ok(WifiStatus::Disconnected));
                    };

                    let is_error = esp_wifi.set_configuration(&overall_config).is_err();
                    if !is_error {
                        return (
                            esp_wifi,
                            Ok(WifiStatus::Connected(
                                client_config.unwrap().ssid.clone(),
                                ap.signal_strength,
                            )),
                        );
                    }
                }
            }
            (esp_wifi, Ok(WifiStatus::Disconnected))
        }
    }

    pub fn init() -> WifiManager {
        let (command_sender, command_receiver) = mpsc::channel::<WifiCommand>();
        let (status_sender, status_receiver) = mpsc::channel::<WifiStatus>();
        let thread_builder = thread::Builder::new().stack_size(16384);

        WifiManager {
            command_sender,
            status_receiver,
            connection_thread: thread_builder
                .spawn(move || {
                    let netif_stack = Arc::new(match EspNetifStack::new() {
                        Ok(stack) => stack,
                        Err(_) => panic!("Couldn't create EspNetifStack"),
                    });
                    let sys_loop_stack = Arc::new(match EspSysLoopStack::new() {
                        Ok(stack) => stack,
                        Err(_) => panic!("Couldn't create EspSysLoopStack"),
                    });
                    let default_nvs = Arc::new(match EspDefaultNvs::new() {
                        Ok(nvs) => nvs,
                        Err(_) => panic!("Couldn't create EspDefaultNvs"),
                    });

                    let mut esp_wifi =
                        EspWifi::new(netif_stack, sys_loop_stack, default_nvs).unwrap();

                    let mut configs: WifiManagerConfig = WifiManagerConfig {
                        client_configs: HashMap::new(),
                        ap_config: None,
                    };

                    loop {
                        let status = match command_receiver.recv_timeout(WIFI_SCAN_PERIOD) {
                            Ok(c) => match c {
                                WifiCommand::ConnectWPA2PSK(ssid, key) => {
                                    configs = WifiManager::connect_wpa2_psk(configs, ssid, key);
                                    let result = WifiManager::reconfigure_wifi(&configs, esp_wifi);

                                    esp_wifi = result.0;

                                    match result.1 {
                                        Ok(status) => status,
                                        Err(err) => WifiStatus::Error(err),
                                    }
                                }
                                WifiCommand::CreateApWPA2PSK(ssid, key) => {
                                    configs = WifiManager::make_ap_wpa2_psk(configs, ssid, key);
                                    let result = WifiManager::reconfigure_wifi(&configs, esp_wifi);

                                    esp_wifi = result.0;

                                    match result.1 {
                                        Ok(status) => status,
                                        Err(err) => WifiStatus::Error(err),
                                    }
                                }
                            },
                            Err(_) => WifiStatus::Disconnected,
                        };
                        status_sender.send(status).unwrap();
                    }
                })
                .unwrap(),
        }
    }

    pub fn add_network_wpa2_psk(
        &mut self,
        ssid: SSID,
        key: PSKKey,
    ) -> Result<(), SendError<WifiCommand>> {
        self.command_sender
            .send(WifiCommand::ConnectWPA2PSK(ssid, key))
    }

    pub fn set_ap_wpa2_psk(
        &mut self,
        ssid: SSID,
        key: PSKKey,
    ) -> Result<(), SendError<WifiCommand>> {
        self.command_sender
            .send(WifiCommand::CreateApWPA2PSK(ssid, key))
    }
}
