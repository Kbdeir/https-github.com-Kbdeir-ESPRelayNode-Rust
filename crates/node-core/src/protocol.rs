use crate::command::{Action, Command, Source};
use crate::config::{RelayConfig, MAX_TIMER_SECONDS};

pub fn command(action: Action, source: Source) -> Command {
    Command {
        relay: 0,
        action,
        source,
        id: None,
        expires_at_ms: None,
    }
}
pub fn mqtt_command(
    config: &RelayConfig,
    topic: &str,
    payload: &[u8],
    retained: bool,
) -> Result<Command, String> {
    if retained {
        return Err("retained actuator commands are rejected".into());
    }
    if payload.len() > 512 {
        return Err("MQTT command exceeds 512 bytes".into());
    }
    let text = std::str::from_utf8(payload)
        .map_err(|_| "MQTT command must be UTF-8")?
        .trim();
    if !config.command_topic.is_empty() && topic == config.command_topic {
        let action = match text {
            "on" | "ON" => Action::On,
            "off" | "OFF" => Action::Off,
            "tog" | "TOG" => Action::Toggle,
            _ => return Err("invalid relay command".into()),
        };
        return Ok(command(action, Source::Mqtt));
    }
    if !config.ttl_command_topic.is_empty() && topic == config.ttl_command_topic {
        let seconds: u32 = text.parse().map_err(|_| "invalid TTL command")?;
        if seconds > MAX_TIMER_SECONDS {
            return Err("TTL exceeds seven days".into());
        }
        return Ok(command(Action::SetTtl(seconds), Source::Mqtt));
    }
    Err("unsubscribed command topic".into())
}
