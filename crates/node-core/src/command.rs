use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    LocalInput,
    Mqtt,
    Modbus,
    Http,
    Automation,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    On,
    Off,
    Toggle,
    DelayedOn,
    SetTtl(u32),
    EmergencyOff,
    ClearInterlock,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub relay: u8,
    pub action: Action,
    pub source: Source,
    pub id: Option<u64>,
    pub expires_at_ms: Option<u64>,
}

impl Command {
    pub fn local(action: Action) -> Self {
        Self {
            relay: 0,
            action,
            source: Source::LocalInput,
            id: None,
            expires_at_ms: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Applied,
    Duplicate,
    Expired,
    InvalidRelay,
    InvalidTimer,
    Interlocked,
    Unauthorized,
}
