use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::INPUT_GPIOS;

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_CONFIG_BYTES: usize = 8192;
pub const MAX_TIMER_SECONDS: u32 = 604_800;
pub fn check_json_depth(bytes: &[u8], maximum: u8) -> Result<(), String> {
    let (mut depth, mut quoted, mut escaped) = (0u8, false, false);
    for byte in bytes {
        if quoted {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                quoted = false;
            }
        } else {
            match byte {
                b'"' => quoted = true,
                b'{' | b'[' => {
                    if depth >= maximum {
                        return Err("JSON nesting limit exceeded".into());
                    }
                    depth += 1;
                }
                b'}' | b']' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    #[default]
    None,
    Toggle,
    Normal,
    RelayToggle,
    CopyToRelay,
    Temperature,
}

impl InputMode {
    pub fn from_legacy(value: u32) -> Result<Self, String> {
        match value {
            0 => Ok(Self::None),
            1 => Ok(Self::Toggle),
            2 => Ok(Self::Normal),
            3 => Ok(Self::RelayToggle),
            4 => Ok(Self::CopyToRelay),
            5 => Ok(Self::Temperature),
            _ => Err("invalid legacy input mode".into()),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct InputConfig {
    pub mode: InputMode,
    pub relay: Option<u8>,
    pub topic: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct RelayConfig {
    pub ttl_seconds: u32,
    pub tta_seconds: u32,
    pub command_topic: String,
    pub state_topic: String,
    pub ttl_topic: String,
    pub ttl_command_topic: String,
    pub elapsed_topic: String,
}

#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkConfig {
    pub ssid: String,
    pub wifi_password: String,
    pub mqtt_enabled: bool,
    pub mqtt_host: String,
    pub mqtt_username: String,
    pub mqtt_password: String,
    pub mqtt_port: u16,
    pub mqtt_keepalive_seconds: u16,
    pub timeserver: String,
    pub utc_offset_hours: i8,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            ssid: String::new(),
            wifi_password: String::new(),
            mqtt_enabled: false,
            mqtt_host: String::new(),
            mqtt_username: String::new(),
            mqtt_password: String::new(),
            mqtt_port: 1883,
            mqtt_keepalive_seconds: 30,
            timeserver: "pool.ntp.org".into(),
            utc_offset_hours: 4,
        }
    }
}

#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub schema: u32,
    pub location: String,
    pub relay: RelayConfig,
    pub inputs: [InputConfig; 6],
    pub network: NetworkConfig,
    pub management: ManagementConfig,
    pub modbus: ModbusConfig,
    pub timers: [crate::schedule::Schedule; 4],
    pub automation: [crate::automation::Rule; 4],
    pub remote_sensors: [crate::automation::RemoteSensor; 8],
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema: SCHEMA_VERSION,
            location: "NA".into(),
            relay: RelayConfig::default(),
            inputs: std::array::from_fn(|_| InputConfig::default()),
            network: NetworkConfig {
                mqtt_port: 1883,
                mqtt_keepalive_seconds: 30,
                ..NetworkConfig::default()
            },
            management: ManagementConfig::default(),
            modbus: ModbusConfig::default(),
            timers: std::array::from_fn(|_| crate::schedule::Schedule::default()),
            automation: std::array::from_fn(|_| crate::automation::Rule::default()),
            remote_sensors: std::array::from_fn(|_| crate::automation::RemoteSensor::default()),
        }
    }
}

impl Config {
    pub fn rule_changed(&self, previous: &Self, index: usize) -> bool {
        self.automation[index] != previous.automation[index]
            || self.automation[index].conditions.iter().any(|condition| {
                let source = u8::from(condition.source);
                if !(20..28).contains(&source) {
                    return false;
                }
                let a = &self.remote_sensors[usize::from(source - 20)];
                let b = &previous.remote_sensors[usize::from(source - 20)];
                a.transport != b.transport
                    || a.topic != b.topic
                    || a.json_key != b.json_key
                    || a.modbus != b.modbus
            })
    }

    pub fn wifi_changed(&self, previous: &Self) -> bool {
        self.network.ssid != previous.network.ssid
            || self.network.wifi_password != previous.network.wifi_password
    }

    pub fn mqtt_changed(&self, previous: &Self) -> bool {
        let a = &self.network;
        let b = &previous.network;
        a.mqtt_enabled != b.mqtt_enabled
            || a.mqtt_host != b.mqtt_host
            || a.mqtt_port != b.mqtt_port
            || a.mqtt_username != b.mqtt_username
            || a.mqtt_password != b.mqtt_password
            || a.mqtt_keepalive_seconds != b.mqtt_keepalive_seconds
            || self.relay.command_topic != previous.relay.command_topic
            || self.relay.state_topic != previous.relay.state_topic
            || self.relay.ttl_topic != previous.relay.ttl_topic
            || self.relay.ttl_command_topic != previous.relay.ttl_command_topic
            || self.relay.elapsed_topic != previous.relay.elapsed_topic
            || self.inputs != previous.inputs
            || self.remote_sensors != previous.remote_sensors
    }

    pub fn modbus_changed(&self, previous: &Self) -> bool {
        self.modbus != previous.modbus
            || self.remote_sensors != previous.remote_sensors
            || self.automation != previous.automation
    }

    pub fn factory_defaults(node_id: &str) -> Result<Self, String> {
        if !matches!(node_id.len(), 6 | 12) || !node_id.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("factory defaults require a hardware controller ID".into());
        }
        let prefix = format!("/home/Controller{node_id}");
        // Match SmartConfig - AI's generators without changing fail-safe Default().
        let config = Self {
            location: "Not configured yet".into(),
            relay: RelayConfig {
                command_topic: format!("{prefix}/Coils/C0"),
                state_topic: format!("{prefix}/Coils/State/C0"),
                ttl_topic: format!("{prefix}/sts/VTTL0"),
                ttl_command_topic: format!("{prefix}/i/TTL0"),
                elapsed_topic: format!("{prefix}/sts/CURRVTTL0"),
                ..RelayConfig::default()
            },
            inputs: std::array::from_fn(|index| InputConfig {
                mode: InputMode::Normal,
                relay: None,
                topic: format!("{prefix}/INS/sts/IN{}", index + 1),
            }),
            network: NetworkConfig {
                mqtt_host: "192.168.1.1".into(),
                timeserver: "162.159.200.123".into(),
                utc_offset_hours: 2,
                ..NetworkConfig::default()
            },
            ..Self::default()
        };
        config.validate()?;
        Ok(config)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_CONFIG_BYTES {
            return Err("configuration exceeds 8192 bytes".into());
        }
        check_json_depth(bytes, 16)?;
        let config: Self = serde_json::from_slice(bytes)
            .map_err(|_| "invalid configuration JSON or unknown field".to_string())?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema != SCHEMA_VERSION {
            return Err("unsupported configuration schema".into());
        }
        bounded(&self.location, 64, "location")?;
        if self.relay.ttl_seconds > MAX_TIMER_SECONDS || self.relay.tta_seconds > MAX_TIMER_SECONDS
        {
            return Err("relay timer exceeds seven days".into());
        }
        let command_topics = [&self.relay.command_topic, &self.relay.ttl_command_topic];
        let state_topics = [
            &self.relay.state_topic,
            &self.relay.ttl_topic,
            &self.relay.elapsed_topic,
        ];
        for topic in command_topics.iter().chain(state_topics.iter()) {
            validate_topic(topic)?;
        }
        for (index, topic) in command_topics.iter().enumerate() {
            if !topic.is_empty()
                && (state_topics.contains(topic) || command_topics[..index].contains(topic))
            {
                return Err("command and state topics must be distinct".into());
            }
        }
        for (index, input) in self.inputs.iter().enumerate() {
            validate_topic(&input.topic)?;
            if input.relay.is_some_and(|relay| relay != 0) {
                return Err("single-relay profile only supports relay 0".into());
            }
            if input.mode == InputMode::Temperature && index > 1 {
                return Err("temperature mode is only supported on GPIO33/GPIO16".into());
            }
            if input.relay.is_some()
                && !matches!(input.mode, InputMode::RelayToggle | InputMode::CopyToRelay)
            {
                return Err("relay mapping requires relay_toggle or copy_to_relay mode".into());
            }
            if !input.topic.is_empty() && command_topics.contains(&&input.topic) {
                return Err("input telemetry cannot use a local relay command topic".into());
            }
        }
        bounded(&self.network.ssid, 32, "Wi-Fi SSID")?;
        bounded(&self.network.wifi_password, 64, "Wi-Fi password")?;
        if !self.network.wifi_password.is_empty()
            && !(8..=63).contains(&self.network.wifi_password.len())
        {
            return Err("Wi-Fi password must contain 8-63 bytes".into());
        }
        bounded(&self.network.mqtt_host, 128, "MQTT host")?;
        bounded(&self.network.mqtt_username, 64, "MQTT username")?;
        bounded(&self.network.mqtt_password, 128, "MQTT password")?;
        bounded(&self.network.timeserver, 128, "time server")?;
        if !(-12..=14).contains(&self.network.utc_offset_hours) {
            return Err("invalid UTC offset".into());
        }
        bounded(&self.management.username, 32, "web username")?;
        bounded(&self.management.password, 64, "web password")?;
        if self.management.username.is_empty()
            || (!self.management.password.is_empty() && self.management.password.len() < 8)
        {
            return Err("web password requires at least eight bytes".into());
        }
        if self.management.enabled && self.management.password.len() < 8 {
            return Err("enabled web management requires configured credentials".into());
        }
        self.modbus.validate()?;
        for timer in &self.timers {
            timer.validate()?;
        }
        for rule in &self.automation {
            rule.validate()?;
        }
        for (i, sensor) in self.remote_sensors.iter().enumerate() {
            sensor.validate()?;
            if sensor.transport == crate::automation::Transport::Mqtt
                && !sensor.topic.is_empty()
                && (command_topics.contains(&&sensor.topic)
                    || self.remote_sensors[..i].iter().any(|s| {
                        s.transport == crate::automation::Transport::Mqtt && s.topic == sensor.topic
                    }))
            {
                return Err("remote MQTT topic is duplicated or is a command topic".into());
            }
        }
        if self.network.mqtt_port == 0 || !(5..=300).contains(&self.network.mqtt_keepalive_seconds)
        {
            return Err("invalid MQTT port or keepalive".into());
        }
        if self.network.mqtt_enabled
            && (self.network.ssid.is_empty() || self.network.mqtt_host.is_empty())
        {
            return Err("MQTT requires a Wi-Fi SSID and broker host".into());
        }
        Ok(())
    }

    pub fn from_legacy(system: &Value, relay: &Value, mapping: &Value) -> Result<Self, String> {
        if !system.is_object() || !relay.is_object() || !mapping.is_object() {
            return Err("legacy configuration files must be JSON objects".into());
        }
        if number(relay, "RELAYNB", 0)? != 0 {
            return Err("only relay0 can be imported for the single-relay profile".into());
        }
        let mut config = Self {
            location: text(system, "PhyLoc"),
            relay: RelayConfig {
                ttl_seconds: number(relay, "ttl", 0)?,
                tta_seconds: number(relay, "tta", 0)?,
                command_topic: text(relay, "PUB_TOPIC1"),
                state_topic: text(relay, "STATE_PUB_TOPIC"),
                ttl_topic: text(relay, "ttl_PUB_TOPIC"),
                ttl_command_topic: text(relay, "i_ttl_PUB_TOPIC"),
                elapsed_topic: text(relay, "CURR_TTL_PUB_TOPIC"),
            },
            network: NetworkConfig {
                ssid: text(system, "ssid"),
                wifi_password: text(system, "pass"),
                mqtt_enabled: number(system, "MQTT_Active", 0)? == 1,
                mqtt_host: text(system, "MQTT_BROKER"),
                mqtt_username: credential(system, "mqttUser"),
                mqtt_password: credential(system, "mqttPass"),
                mqtt_port: u16::try_from(number(system, "MQTT_B_PRT", 1883)?)
                    .map_err(|_| "invalid legacy MQTT port")?,
                mqtt_keepalive_seconds: u16::try_from(number(system, "MQTT_KeepAliveSeconds", 30)?)
                    .map_err(|_| "invalid legacy MQTT keepalive")?,
                timeserver: if text(system, "timeserver").is_empty() {
                    "pool.ntp.org".into()
                } else {
                    text(system, "timeserver")
                },
                utc_offset_hours: i8::try_from(signed_number(system, "ntptz", 4)?)
                    .map_err(|_| "invalid legacy UTC offset")?,
            },
            ..Self::default()
        };
        let topic_keys = [
            "I12_STS_PTP",
            "I14_STS_PTP",
            "I03_STS_PTP",
            "I04_STS_PTP",
            "I05_STS_PTP",
            "I06_STS_PTP",
        ];
        for (index, topic_key) in topic_keys.iter().enumerate() {
            config.inputs[index].mode =
                InputMode::from_legacy(number(system, &format!("I{}MODE", index + 1), 0)?)?;
            config.inputs[index].topic = text(system, topic_key);
        }
        for slot in 1..=10 {
            let pin = signed_number(mapping, &format!("I{slot}"), -1)?;
            let relay = signed_number(mapping, &format!("R{slot}"), -1)?;
            if pin == -1 || relay == -1 {
                continue;
            }
            let index = INPUT_GPIOS
                .iter()
                .position(|gpio| i64::from(*gpio) == pin)
                .ok_or("legacy IRMAP references an unavailable GPIO")?;
            if relay != 0 {
                return Err("legacy IRMAP references an unavailable relay".into());
            }
            config.inputs[index].relay = Some(0);
        }
        config.validate()?;
        Ok(config)
    }
}

#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ManagementConfig {
    pub enabled: bool,
    pub username: String,
    pub password: String,
}
impl Default for ManagementConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            username: "user".into(),
            password: String::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ModbusConfig {
    pub enabled: bool,
    pub port: u16,
    pub unit: u8,
    pub allowed_clients: Vec<String>,
    pub peers: Vec<ModbusPeer>,
}
impl Default for ModbusConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: 502,
            unit: 1,
            allowed_clients: Vec::new(),
            peers: Vec::new(),
        }
    }
}
impl ModbusConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.port == 0
            || self.unit == 0
            || self.allowed_clients.len() > 8
            || self.peers.len() > 4
        {
            return Err("invalid Modbus server or peer bounds".into());
        }
        if self.enabled && self.allowed_clients.is_empty() {
            return Err("enabled Modbus server requires a client IPv4 allowlist".into());
        }
        for ip in &self.allowed_clients {
            if ip != "*" {
                ip.parse::<std::net::Ipv4Addr>()
                    .map_err(|_| "Modbus allowlist requires IPv4 addresses or explicit *")?;
            }
        }
        for peer in &self.peers {
            peer.host
                .parse::<std::net::Ipv4Addr>()
                .map_err(|_| "Modbus peers require IPv4 addresses")?;
            if peer.port == 0 || peer.unit == 0 || !(1000..=60000).contains(&peer.poll_ms) {
                return Err("invalid Modbus peer configuration".into());
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModbusPeer {
    pub host: String,
    pub port: u16,
    pub unit: u8,
    pub coil: u16,
    pub poll_ms: u32,
}

fn bounded(value: &str, max: usize, field: &str) -> Result<(), String> {
    if value.len() > max || value.contains('\0') {
        return Err(format!("invalid {field} length or embedded NUL"));
    }
    Ok(())
}

fn validate_topic(value: &str) -> Result<(), String> {
    bounded(value, 192, "MQTT topic")?;
    if value.contains(['#', '+']) {
        return Err("MQTT command/telemetry topics cannot contain wildcards".into());
    }
    Ok(())
}

fn text(value: &Value, field: &str) -> String {
    value[field].as_str().unwrap_or_default().to_owned()
}

fn credential(value: &Value, field: &str) -> String {
    let value = text(value, field);
    if value.eq_ignore_ascii_case("null") {
        String::new()
    } else {
        value
    }
}

fn number(value: &Value, field: &str, default: u32) -> Result<u32, String> {
    let value = &value[field];
    if value.is_null() || value.as_str() == Some("") {
        return Ok(default);
    }
    if let Some(text) = value.as_str() {
        return text.parse().map_err(|_| format!("invalid legacy {field}"));
    }
    value
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| format!("invalid legacy {field}"))
}

fn signed_number(value: &Value, field: &str, default: i64) -> Result<i64, String> {
    if value[field].is_null() {
        return Ok(default);
    }
    if let Some(text) = value[field].as_str() {
        return text.parse().map_err(|_| format!("invalid legacy {field}"));
    }
    value[field]
        .as_i64()
        .ok_or_else(|| format!("invalid legacy {field}"))
}
