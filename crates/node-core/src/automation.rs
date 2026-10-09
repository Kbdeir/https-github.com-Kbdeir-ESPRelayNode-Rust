use crate::{command::Action, config::Config, runtime::RuntimeSnapshot};
use serde::{Deserialize, Serialize};

pub const STALE_MS: u64 = 300_000;
fn finite_f32<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<f32, D::Error> {
    let value = f64::deserialize(deserializer)?;
    if !value.is_finite() || value.abs() > f64::from(f32::MAX) {
        return Err(serde::de::Error::custom("finite float required"));
    }
    Ok(value as f32)
}
macro_rules! numeric_enum {
    ($name:ident { $($variant:ident = $number:literal),+ }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
        #[serde(try_from = "u8", into = "u8")]
        pub enum $name { $($variant),+ }
        impl TryFrom<u8> for $name {
            type Error = String;
            fn try_from(value: u8) -> Result<Self, String> {
                match value { $($number => Ok(Self::$variant),)+ _ => Err(format!("invalid {}", stringify!($name))) }
            }
        }
        impl From<$name> for u8 { fn from(value: $name) -> u8 { match value { $($name::$variant => $number),+ } } }
    };
}
numeric_enum!(Source { None = 0, Temperature1 = 1, Temperature2 = 2, Current = 3, Input1 = 4, Input2 = 5, Input3 = 6, Relay1 = 7, Relay2 = 8, Relay3 = 9, Relay4 = 10, Remote1 = 20, Remote2 = 21, Remote3 = 22, Remote4 = 23, Remote5 = 24, Remote6 = 25, Remote7 = 26, Remote8 = 27 });
numeric_enum!(Operator { Greater = 0, Less = 1, Between = 2, Equal = 3 });
numeric_enum!(RuleAction { On = 0, Off = 1, Toggle = 2 });
numeric_enum!(Logic { And = 0, Or = 1 });
numeric_enum!(Trigger { Change = 0, Periodic = 1 });
numeric_enum!(Transport { Mqtt = 0, Modbus = 1 });
numeric_enum!(ValueType { Bool = 0, U16 = 1, I16 = 2, U32 = 3, I32 = 4, F32 = 5 });

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Condition {
    pub source: Source,
    pub op: Operator,
    #[serde(deserialize_with = "finite_f32")]
    pub value1: f32,
    #[serde(deserialize_with = "finite_f32")]
    pub value2: f32,
}
impl Default for Condition {
    fn default() -> Self {
        Self {
            source: Source::None,
            op: Operator::Greater,
            value1: 0.0,
            value2: 0.0,
        }
    }
}
impl Condition {
    fn validate(&self) -> Result<(), String> {
        if !self.value1.is_finite()
            || !self.value2.is_finite()
            || (self.op == Operator::Between && self.value2 < self.value1)
        {
            return Err("invalid comparison range".into());
        }
        Ok(())
    }
    fn matches(&self, value: f32) -> bool {
        match self.op {
            Operator::Greater => value > self.value1,
            Operator::Less => value < self.value1,
            Operator::Between => value >= self.value1 && value <= self.value2,
            Operator::Equal => (value - self.value1).abs() < 0.001,
        }
    }
}
#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct CoilTarget {
    pub host: String,
    pub port: u16,
    pub unit: u8,
    pub address: u16,
    pub send_clear: bool,
}
impl Default for CoilTarget {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 502,
            unit: 1,
            address: 0,
            send_clear: true,
        }
    }
}
pub fn validate_host(host: &str, port: u16, unit: u8) -> Result<(), String> {
    let ip: std::net::Ipv4Addr = host
        .parse()
        .map_err(|_| "Modbus requires an IPv4 address")?;
    if ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_loopback()
        || port == 0
        || !(1..=247).contains(&unit)
    {
        return Err("invalid Modbus destination".into());
    }
    Ok(())
}
#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Rule {
    pub enabled: bool,
    pub target: u8,
    pub action: RuleAction,
    pub logic: Logic,
    pub trigger: Trigger,
    pub gate: u8,
    pub hold: u16,
    pub cooldown: u16,
    pub conditions: [Condition; 2],
    pub modbus: Option<CoilTarget>,
}
impl Default for Rule {
    fn default() -> Self {
        Self {
            enabled: false,
            target: 0,
            action: RuleAction::On,
            logic: Logic::And,
            trigger: Trigger::Change,
            gate: 0,
            hold: 0,
            cooldown: 0,
            conditions: [Condition::default(); 2],
            modbus: None,
        }
    }
}
impl Rule {
    pub fn validate(&self) -> Result<(), String> {
        if self.target != 0
            || self.gate > 4
            || (self.enabled && self.conditions[0].source == Source::None)
        {
            return Err("invalid rule target, gate or first source".into());
        }
        if self.action == RuleAction::Toggle
            && (self.trigger == Trigger::Periodic || self.modbus.is_some())
        {
            return Err("periodic or remote Toggle is not allowed".into());
        }
        if self.trigger == Trigger::Periodic && self.cooldown == 0 {
            return Err("periodic rules need at least one second cooldown".into());
        }
        for c in &self.conditions {
            c.validate()?;
        }
        if let Some(target) = &self.modbus {
            validate_host(&target.host, target.port, target.unit)?;
        }
        Ok(())
    }
}
#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct RemoteModbus {
    pub host: String,
    pub port: u16,
    pub unit: u8,
    pub function: u8,
    pub address: u16,
    pub value_type: ValueType,
    pub word_order: u8,
    pub poll: u16,
    #[serde(deserialize_with = "finite_f32")]
    pub scale: f32,
    #[serde(deserialize_with = "finite_f32")]
    pub offset: f32,
}
impl Default for RemoteModbus {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 502,
            unit: 1,
            function: 3,
            address: 0,
            value_type: ValueType::U16,
            word_order: 0,
            poll: 10,
            scale: 1.0,
            offset: 0.0,
        }
    }
}
impl RemoteModbus {
    pub fn count(&self) -> u16 {
        if u8::from(self.value_type) >= 3 {
            2
        } else {
            1
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if !self.host.is_empty() {
            validate_host(&self.host, self.port, self.unit)?;
        }
        if !(1..=4).contains(&self.function)
            || self.word_order > 3
            || !(5..=300).contains(&self.poll)
            || !self.scale.is_finite()
            || !self.offset.is_finite()
            || (self.function <= 2 && self.value_type != ValueType::Bool)
            || (self.function > 2 && self.value_type == ValueType::Bool)
            || (self.count() == 2 && self.address == u16::MAX)
        {
            return Err("invalid Modbus register settings".into());
        }
        Ok(())
    }
    pub fn decode(&self, words: &[u16]) -> Result<f32, String> {
        if words.len() != usize::from(self.count()) {
            return Err("wrong register count".into());
        }
        let mut bytes = [0; 4];
        bytes[..2].copy_from_slice(&words[0].to_be_bytes());
        if words.len() == 2 {
            bytes[2..].copy_from_slice(&words[1].to_be_bytes());
        }
        if self.word_order & 1 != 0 {
            bytes.rotate_left(2);
        }
        if self.word_order & 2 != 0 {
            bytes.swap(0, 1);
            bytes.swap(2, 3);
        }
        let raw = u32::from_be_bytes(bytes);
        let value = match self.value_type {
            ValueType::Bool => u8::from(words[0] != 0) as f32,
            ValueType::U16 => f32::from(words[0]),
            ValueType::I16 => f32::from(words[0] as i16),
            ValueType::U32 => raw as f32,
            ValueType::I32 => (raw as i32) as f32,
            ValueType::F32 => f32::from_bits(raw),
        };
        self.scaled(value)
    }
    pub fn scaled(&self, value: f32) -> Result<f32, String> {
        let value = value * self.scale + self.offset;
        if value.is_finite() {
            Ok(value)
        } else {
            Err("non-finite remote value".into())
        }
    }
}
#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct RemoteSensor {
    pub transport: Transport,
    pub topic: String,
    pub json_key: String,
    pub label: String,
    pub modbus: RemoteModbus,
}
impl Default for RemoteSensor {
    fn default() -> Self {
        Self {
            transport: Transport::Mqtt,
            topic: String::new(),
            json_key: String::new(),
            label: String::new(),
            modbus: RemoteModbus::default(),
        }
    }
}
impl RemoteSensor {
    pub fn configured(&self) -> bool {
        match self.transport {
            Transport::Mqtt => !self.topic.is_empty(),
            Transport::Modbus => !self.modbus.host.is_empty(),
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.topic.len() > 63
            || self.json_key.len() > 31
            || self.label.len() > 31
            || self.topic.contains(['#', '+', '\0'])
            || self.label.contains('\0')
            || self.json_key.contains('\0')
        {
            return Err("invalid remote sensor field".into());
        }
        if self.transport == Transport::Modbus {
            self.modbus.validate()?;
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Default)]
pub struct RemoteReading {
    pub value: f32,
    pub seen_ms: Option<u64>,
}
impl RemoteReading {
    pub fn valid(&self, now: u64) -> bool {
        self.seen_ms
            .is_some_and(|seen| now >= seen && now - seen <= STALE_MS)
            && self.value.is_finite()
    }
}
pub fn scalar(text: &str) -> Result<f32, String> {
    let text = text.trim();
    let value = match text.to_ascii_lowercase().as_str() {
        "on" | "true" | "yes" => 1.0,
        "off" | "false" | "no" => 0.0,
        _ => text.parse::<f32>().map_err(|_| "invalid scalar")?,
    };
    if value.is_finite() {
        Ok(value)
    } else {
        Err("non-finite scalar".into())
    }
}
pub fn remote_payload(payload: &[u8], key: &str) -> Result<f32, String> {
    if payload.is_empty() || payload.len() > 512 {
        return Err("remote payload size".into());
    }
    let text = std::str::from_utf8(payload)
        .map_err(|_| "invalid UTF-8")?
        .trim();
    if !text.starts_with(['{', '[', '"']) {
        return scalar(text);
    }
    // Preflight depth bounds stack use; serde still performs all JSON validation.
    let (mut depth, mut quoted, mut escaped) = (0u8, false, false);
    for byte in text.bytes() {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else {
            match byte {
                b'"' => quoted = true,
                b'{' | b'[' => {
                    depth += 1;
                    if depth > 8 {
                        return Err("remote JSON too deep".into());
                    }
                }
                b'}' | b']' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    let value: serde_json::Value = serde_json::from_str(text).map_err(|_| "invalid remote JSON")?;
    let value = if key.is_empty() {
        &value
    } else {
        value.get(key).ok_or("JSON key missing")?
    };
    match value {
        serde_json::Value::Bool(v) => Ok(u8::from(*v) as f32),
        serde_json::Value::Number(v) => scalar(&v.to_string()),
        serde_json::Value::String(v) => scalar(v),
        _ => Err("remote value is not a scalar".into()),
    }
}

#[derive(Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleStatus {
    pub available: bool,
    pub gate_open: bool,
    pub matched: bool,
    pub stable_seconds: u32,
    pub seconds_since_fire: u32,
}
#[derive(Clone, Copy, Default)]
struct State {
    initialized: bool,
    result: bool,
    fired: bool,
    since: u64,
    last_fire: Option<u64>,
    status: RuleStatus,
}
#[derive(Clone, Copy, Default)]
pub struct Step {
    pub cancel_remote: bool,
    pub permit_remote: bool,
    pub action: Option<Action>,
}
#[derive(Default)]
pub struct Automation {
    states: [State; 4],
}
pub struct Sources<'a> {
    pub snapshot: &'a RuntimeSnapshot,
    pub remote: &'a [RemoteReading; 8],
    pub gates: [bool; 4],
}
impl Sources<'_> {
    fn read(&self, source: Source, now: u64) -> Option<f32> {
        match source {
            Source::Relay1 => Some(u8::from(self.snapshot.relay.on) as f32),
            Source::Input1 | Source::Input2 | Source::Input3 => {
                let i = usize::from(u8::from(source) - 4);
                (self.snapshot.input_valid_mask & (1 << i) != 0)
                    .then_some(u8::from(self.snapshot.inputs[i]) as f32)
            }
            Source::Remote1
            | Source::Remote2
            | Source::Remote3
            | Source::Remote4
            | Source::Remote5
            | Source::Remote6
            | Source::Remote7
            | Source::Remote8 => {
                let reading = self.remote[usize::from(u8::from(source) - 20)];
                reading.valid(now).then_some(reading.value)
            }
            _ => None,
        }
    }
}
impl Automation {
    pub fn configure(&mut self, config: &Config, previous: &Config) -> [bool; 4] {
        std::array::from_fn(|i| {
            let changed = config.rule_changed(previous, i);
            if changed {
                self.states[i] = State::default();
            }
            changed
        })
    }

    pub fn tick(&mut self, now: u64, config: &Config, sources: &Sources<'_>) -> [Step; 4] {
        std::array::from_fn(|i| {
            let rule = &config.automation[i];
            let state = &mut self.states[i];
            let gate = rule.gate == 0 || sources.gates[usize::from(rule.gate - 1)];
            let mut values = [0.0; 2];
            let mut available = rule.enabled;
            for (j, condition) in rule.conditions.iter().enumerate() {
                if j == 1 && condition.source == Source::None {
                    continue;
                }
                if u8::from(condition.source) >= 20
                    && !config.remote_sensors[usize::from(u8::from(condition.source) - 20)]
                        .configured()
                {
                    available = false;
                    continue;
                }
                match sources.read(condition.source, now) {
                    Some(v) => values[j] = v,
                    None => available = false,
                }
            }
            state.status.available = available;
            state.status.gate_open = gate;
            state.status.seconds_since_fire = state
                .last_fire
                .map_or(0, |v| (now.saturating_sub(v) / 1000) as u32);
            if !available {
                state.initialized = false;
                state.status.matched = false;
                state.status.stable_seconds = 0;
                return Step {
                    cancel_remote: true,
                    ..Default::default()
                };
            }
            let first = rule.conditions[0].matches(values[0]);
            let matched = gate
                && if rule.conditions[1].source == Source::None {
                    first
                } else {
                    let second = rule.conditions[1].matches(values[1]);
                    match rule.logic {
                        Logic::And => first && second,
                        Logic::Or => first || second,
                    }
                };
            let changed = !state.initialized || state.result != matched;
            if changed {
                state.initialized = true;
                state.result = matched;
                state.since = now;
                state.fired = false;
            }
            state.status.matched = matched;
            state.status.stable_seconds = (now.saturating_sub(state.since) / 1000) as u32;
            let ready = now.saturating_sub(state.since) >= u64::from(rule.hold) * 1000
                && state.last_fire.map_or(true, |v| {
                    now.saturating_sub(v) >= u64::from(rule.cooldown) * 1000
                });
            let action = if ready && (rule.trigger == Trigger::Periodic || !state.fired) {
                if rule
                    .modbus
                    .as_ref()
                    .is_some_and(|t| !matched && !t.send_clear)
                {
                    None
                } else {
                    Some(match rule.action {
                        RuleAction::Toggle => Action::Toggle,
                        RuleAction::On => {
                            if matched {
                                Action::On
                            } else {
                                Action::Off
                            }
                        }
                        RuleAction::Off => {
                            if matched {
                                Action::Off
                            } else {
                                Action::On
                            }
                        }
                    })
                }
            } else {
                None
            };
            Step {
                cancel_remote: changed
                    || rule
                        .modbus
                        .as_ref()
                        .is_some_and(|t| !matched && !t.send_clear),
                permit_remote: true,
                action,
            }
        })
    }
    pub fn accepted(&mut self, index: usize, now: u64) {
        self.states[index].last_fire = Some(now);
        self.states[index].fired = true;
    }
    pub fn status(&self) -> [RuleStatus; 4] {
        self.states.map(|s| s.status)
    }
}
