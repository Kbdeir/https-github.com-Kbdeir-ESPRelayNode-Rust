use anyhow::{anyhow, Result};
use esp_idf_svc::nvs::{EspDefaultNvs, EspDefaultNvsPartition};
use node_core::config::{Config, MAX_CONFIG_BYTES};

static BOOTSTRAP: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/bootstrap.json"));

pub fn factory_wifi() -> Result<(String, String)> {
    let config = Config::parse(BOOTSTRAP).map_err(|error| anyhow!(error))?;
    Ok((config.network.ssid, config.network.wifi_password))
}

pub fn load() -> Result<Config> {
    let partition = EspDefaultNvsPartition::take()?;
    let nvs = EspDefaultNvs::new(partition, "relay_node", true)?;
    let mut found_invalid = false;
    for key in ["config", "backup"] {
        match read(&nvs, key) {
            Ok(Some(config)) => {
                log::info!("Configuration loaded from NVS {key}");
                return Ok(config);
            }
            Ok(None) => {}
            Err(_) => {
                found_invalid = true;
                log::warn!("NVS {key} is invalid; trying recovery configuration");
            }
        }
    }
    if found_invalid {
        log::warn!("Configuration recovery failed; using unmapped, relay-OFF defaults");
        return Ok(Config::default());
    }
    log::info!("Using validated build configuration");
    Config::parse(BOOTSTRAP).map_err(|error| anyhow!(error))
}

pub fn save(config: &Config, previous: &Config, partition: &EspDefaultNvsPartition) -> Result<()> {
    config.validate().map_err(|e| anyhow!(e))?;
    let bytes = serde_json::to_vec(config)?;
    let backup = serde_json::to_vec(previous)?;
    if bytes.len() > MAX_CONFIG_BYTES || backup.len() > MAX_CONFIG_BYTES {
        return Err(anyhow!("configuration exceeds NVS bounds"));
    }
    let mut nvs = EspDefaultNvs::new(partition.clone(), "relay_node", true)?;
    nvs.set_blob("backup", &backup)?;
    nvs.set_blob("config", &bytes)?;
    Ok(())
}

fn read(nvs: &EspDefaultNvs, key: &str) -> Result<Option<Config>> {
    let Some(length) = nvs.blob_len(key)? else {
        return Ok(None);
    };
    if length > MAX_CONFIG_BYTES {
        return Err(anyhow!("NVS configuration is oversized"));
    }
    let mut buffer = vec![0; length];
    let bytes = nvs
        .get_blob(key, &mut buffer)?
        .ok_or_else(|| anyhow!("NVS configuration disappeared"))?;
    Ok(Some(Config::parse(bytes).map_err(|error| anyhow!(error))?))
}
