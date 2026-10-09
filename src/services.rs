use anyhow::Result;
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::modem::Modem,
    nvs::EspDefaultNvsPartition,
    sntp::EspSntp,
    wifi::{AccessPointConfiguration, AuthMethod, ClientConfiguration, Configuration, EspWifi},
};
use node_core::{health::Service, runtime::Runtime};
use serde_json::json;
use std::{
    sync::{atomic::Ordering, Arc},
    thread,
    time::{Duration, Instant},
};

pub fn start(
    modem: Modem,
    runtime: Arc<Runtime>,
    nvs: EspDefaultNvsPartition,
) -> Result<thread::JoinHandle<()>> {
    Ok(thread::Builder::new()
        .name("network".into())
        .stack_size(16384)
        .spawn(move || {
            let _guard = runtime
                .health
                .watch(Service::Network, (runtime.now_ms() / 1000) as u32);
            if let Err(error) = run(modem, runtime.clone(), nvs) {
                log::error!("Network services stopped: {error}");
            }
        })?)
}
fn run(modem: Modem, runtime: Arc<Runtime>, nvs: EspDefaultNvsPartition) -> Result<()> {
    let config = runtime.config.lock().unwrap().clone();
    if !config.management.enabled || config.management.password.len() < 8 {
        return Err(anyhow::anyhow!(
            "network services require explicit activation and configured credentials"
        ));
    }
    let sysloop = EspSystemEventLoop::take()?;
    let mut wifi = EspWifi::new(modem, sysloop, Some(nvs.clone()))?;
    let ap_name = format!("SmartConfig-{}", runtime.node_id);
    let mut ap_password = config.management.password.clone();
    ap_password.truncate(63);
    let ap = AccessPointConfiguration {
        ssid: ap_name
            .as_str()
            .try_into()
            .map_err(|_| anyhow::anyhow!("AP SSID too long"))?,
        password: ap_password
            .as_str()
            .try_into()
            .map_err(|_| anyhow::anyhow!("AP password too long"))?,
        auth_method: AuthMethod::WPA2Personal,
        max_connections: 2,
        ..Default::default()
    };
    let station = ClientConfiguration {
        ssid: config
            .network
            .ssid
            .as_str()
            .try_into()
            .map_err(|_| anyhow::anyhow!("SSID too long"))?,
        password: config
            .network
            .wifi_password
            .as_str()
            .try_into()
            .map_err(|_| anyhow::anyhow!("Wi-Fi password too long"))?,
        auth_method: if config.network.wifi_password.is_empty() {
            AuthMethod::None
        } else {
            AuthMethod::WPA2Personal
        },
        ..Default::default()
    };
    wifi.set_configuration(&if config.network.ssid.is_empty() {
        Configuration::AccessPoint(ap)
    } else {
        Configuration::Mixed(station, ap)
    })?;
    wifi.start()?;
    log::info!("Setup Wi-Fi: {ap_name}; credentials are in the private local bootstrap file");
    let _filesystem = crate::filesystem::mount(&runtime)
        .map_err(|error| {
            log::error!(
                "LittleFS mount failed; flash the filesystem image. APIs remain available: {error}"
            );
            error
        })
        .ok();
    let _http = crate::http::start(runtime.clone(), nvs.clone())?;
    runtime
        .network_ready
        .store(_filesystem.is_some(), Ordering::Release);
    let ap_ip = wifi.ap_netif().get_ip_info()?;
    log::info!("Web UI available on setup network: http://{}/", ap_ip.ip);
    // Optional protocol failures must not drop Wi-Fi and the recovery web UI.
    let mut modbus_revision = runtime.modbus_revision.load(Ordering::Acquire);
    let mut modbus = crate::modbus::start(runtime.clone(), modbus_revision)
        .map_err(|error| {
            log::error!("Modbus startup failed; web UI remains available: {error}");
            error
        })
        .ok();
    let mut mqtt_revision = runtime.mqtt_revision.load(Ordering::Acquire);
    let mut mqtt = if runtime.config.lock().unwrap().network.mqtt_enabled {
        crate::mqtt::start(runtime.clone(), nvs.clone(), mqtt_revision)
            .map_err(|error| {
                log::error!("MQTT startup failed; web UI remains available: {error}");
                error
            })
            .ok()
    } else {
        None
    };
    let mut timeserver = config.network.timeserver.clone();
    let servers = [timeserver.as_str()];
    let mut sntp = if timeserver.is_empty() {
        None
    } else {
        EspSntp::new(&esp_idf_svc::sntp::SntpConf {
            servers,
            ..Default::default()
        })
        .map_err(|error| {
            log::error!("SNTP startup failed; web UI remains available: {error}");
            error
        })
        .ok()
    };
    let mut retry_at = Instant::now();
    let mut backoff = 1u64;
    let mut last_ip = None;
    let mut config_revision = 0;
    loop {
        if let Some(updated) = runtime.config_update(&mut config_revision) {
            if updated.network.timeserver != timeserver {
                drop(sntp.take());
                timeserver = updated.network.timeserver.clone();
                if !timeserver.is_empty() {
                    sntp = EspSntp::new(&esp_idf_svc::sntp::SntpConf {
                        servers: [timeserver.as_str()],
                        ..Default::default()
                    })
                    .map_err(|e| log::error!("SNTP reconfiguration failed: {e}"))
                    .ok();
                }
            }
        }
        // Join retired workers before replacing them: no overlapping listeners,
        // stale callbacks, or leaked task stacks after repeated saves.
        let desired = runtime.modbus_revision.load(Ordering::Acquire);
        if desired != modbus_revision
            && modbus
                .as_ref()
                .is_none_or(|tasks| tasks.iter().all(|t| t.is_finished()))
        {
            if let Some(tasks) = modbus.take() {
                for task in tasks {
                    let _ = task.join();
                }
            }
            runtime.peer_status.lock().unwrap().clear();
            modbus_revision = desired;
            modbus = crate::modbus::start(runtime.clone(), desired)
                .map_err(|e| log::error!("Modbus reconfiguration failed: {e}"))
                .ok();
        }
        let desired = runtime.mqtt_revision.load(Ordering::Acquire);
        if desired != mqtt_revision && mqtt.as_ref().is_none_or(|task| task.is_finished()) {
            if let Some(task) = mqtt.take() {
                let _ = task.join();
            }
            mqtt_revision = desired;
            if runtime.config.lock().unwrap().network.mqtt_enabled {
                mqtt = crate::mqtt::start(runtime.clone(), nvs.clone(), desired)
                    .map_err(|e| log::error!("MQTT reconfiguration failed: {e}"))
                    .ok();
            }
        }
        let connected = !config.network.ssid.is_empty() && wifi.is_connected().unwrap_or(false);
        if !connected && !config.network.ssid.is_empty() && Instant::now() >= retry_at {
            let _ = wifi.connect();
            retry_at = Instant::now()
                + Duration::from_millis(
                    backoff * 1000 + u64::from(runtime.node_id.as_bytes()[0]) * 3,
                );
            backoff = (backoff * 2).min(60);
        }
        if connected {
            backoff = 1;
            let ip = wifi.sta_netif().get_ip_info()?;
            if !ip.ip.is_unspecified() && last_ip != Some(ip.ip) {
                log::info!("Wi-Fi connected; web UI: http://{}/", ip.ip);
            }
            last_ip = if ip.ip.is_unspecified() {
                None
            } else {
                Some(ip.ip)
            };
            *runtime.wifi_status.lock().unwrap() = json!({"connected":!ip.ip.is_unspecified(),"status":"connected","ip":ip.ip.to_string(),"gateway":ip.subnet.gateway.to_string(),"dns1":ip.dns.map(|v|v.to_string()).unwrap_or_default(),"dns2":ip.secondary_dns.map(|v|v.to_string()).unwrap_or_default(),"setup_ssid":ap_name,"setup_ip":ap_ip.ip.to_string()});
        } else {
            last_ip = None;
            *runtime.wifi_status.lock().unwrap() = json!({"connected":false,"status":"disconnected","ip":"0.0.0.0","gateway":"0.0.0.0","dns1":"0.0.0.0","dns2":"0.0.0.0","setup_ssid":ap_name,"setup_ip":ap_ip.ip.to_string()});
        }
        if runtime.scan_requested.swap(false, Ordering::Relaxed) {
            *runtime.wifi_scan.lock().unwrap() = json!({"scanning":true,"count":0,"networks":[]});
            match wifi.scan_n::<16>() {
                Ok((networks, _)) => {
                    let entries:Vec<_>=networks.into_iter().map(|n|json!({"ssid":n.ssid.as_str(),"rssi":n.signal_strength,"channel":n.channel})).collect();
                    *runtime.wifi_scan.lock().unwrap() =
                        json!({"scanning":false,"count":entries.len(),"networks":entries});
                }
                Err(_) => {
                    *runtime.wifi_scan.lock().unwrap() =
                        json!({"scanning":false,"count":0,"networks":[],"message":"Scan failed"})
                }
            }
        }
        runtime
            .health
            .progress(Service::Network, (runtime.now_ms() / 1000) as u32);
        thread::sleep(Duration::from_millis(250));
    }
}
