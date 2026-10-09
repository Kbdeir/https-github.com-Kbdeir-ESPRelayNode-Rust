use crate::{
    config::{Config, InputMode},
    engine::Snapshot,
    INPUT_GPIOS,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub type Fields = BTreeMap<String, String>;
pub fn update_automation(current: &Config, path: &str, f: &Fields) -> Result<Config, String> {
    use crate::automation::*;
    let mut c = current.clone();
    let id = number(f, "id", 0)? as usize;
    let small = |key: &str, default: u8| -> Result<u8, String> {
        number(f, key, u32::from(default))?
            .try_into()
            .map_err(|_| format!("invalid {key}"))
    };
    let word = |key: &str, default: u16| -> Result<u16, String> {
        number(f, key, u32::from(default))?
            .try_into()
            .map_err(|_| format!("invalid {key}"))
    };
    let text = |key: &str| f.get(key).cloned().unwrap_or_default();
    let value = |key: &str, default: f32| -> Result<f32, String> {
        f.get(key).map_or(Ok(default), |v| scalar(v))
    };
    if path == "/api/automation/rule" {
        if !(1..=4).contains(&id) {
            return Err("rule must be 1-4".into());
        }
        let output = small("output", 0)?;
        if output > 1 {
            return Err("invalid output protocol".into());
        }
        let condition = |i: usize| -> Result<Condition, String> {
            Ok(Condition {
                source: Source::try_from(small(&format!("c{i}source"), 0)?)?,
                op: Operator::try_from(small(&format!("c{i}op"), 0)?)?,
                value1: value(&format!("c{i}value1"), 0.0)?,
                value2: value(&format!("c{i}value2"), 0.0)?,
            })
        };
        c.automation[id - 1] = Rule {
            enabled: small("enabled", 0)? != 0,
            target: small("target", 0)?,
            action: RuleAction::try_from(small("action", 0)?)?,
            logic: Logic::try_from(small("logic", 0)?)?,
            trigger: Trigger::try_from(small("trigger", 0)?)?,
            gate: small("gate", 0)?,
            hold: word("hold", 0)?,
            cooldown: word("cooldown", 0)?,
            conditions: [condition(0)?, condition(1)?],
            modbus: if output == 1 {
                Some(CoilTarget {
                    host: text("mbHost"),
                    port: word("mbPort", 502)?,
                    unit: small("mbUnit", 1)?,
                    address: word("mbAddress", 0)?,
                    send_clear: small("mbClear", 1)? != 0,
                })
            } else {
                None
            },
        };
    } else if path == "/api/automation/remote" {
        if !(1..=8).contains(&id) {
            return Err("remote sensor must be 1-8".into());
        }
        let transport = Transport::try_from(small("transport", 0)?)?;
        c.remote_sensors[id - 1] = RemoteSensor {
            transport,
            label: text("label"),
            topic: if transport == Transport::Mqtt {
                text("topic")
            } else {
                String::new()
            },
            json_key: if transport == Transport::Mqtt {
                text("jsonKey")
            } else {
                String::new()
            },
            modbus: if transport == Transport::Modbus {
                RemoteModbus {
                    host: text("host"),
                    port: word("port", 502)?,
                    unit: small("unit", 1)?,
                    function: small("function", 3)?,
                    address: word("address", 0)?,
                    value_type: ValueType::try_from(small("valueType", 1)?)?,
                    word_order: small("wordOrder", 0)?,
                    poll: word("poll", 10)?,
                    scale: value("scale", 1.0)?,
                    offset: value("offset", 0.0)?,
                }
            } else {
                RemoteModbus::default()
            },
        };
    } else {
        return Err("unknown automation save route".into());
    }
    c.validate()?;
    Ok(c)
}
pub const INPUT_TOPICS: [&str; 6] = [
    "I12_STS_PTP",
    "I14_STS_PTP",
    "I03_STS_PTP",
    "I04_STS_PTP",
    "I05_STS_PTP",
    "I06_STS_PTP",
];
pub fn form(bytes: &[u8]) -> Result<Fields, String> {
    if bytes.len() > crate::config::MAX_CONFIG_BYTES {
        return Err("request too large".into());
    }
    let mut fields = Fields::new();
    for (key, value) in form_urlencoded::parse(bytes) {
        if fields
            .insert(key.into_owned(), value.into_owned())
            .is_some()
        {
            return Err("duplicate form field".into());
        }
    }
    Ok(fields)
}
pub fn number(fields: &Fields, key: &str, default: u32) -> Result<u32, String> {
    match fields.get(key).filter(|v| !v.is_empty()) {
        Some(v) => v.parse().map_err(|_| format!("invalid {key}")),
        None => Ok(default),
    }
}
fn set(fields: &Fields, key: &str, dest: &mut String) {
    if let Some(v) = fields.get(key) {
        *dest = v.clone();
    }
}
pub fn update_config(current: &Config, path: &str, fields: &Fields) -> Result<Config, String> {
    let mut c = current.clone();
    match path {
        "/Apply.html" => {
            set(fields, "PhyLoc", &mut c.location);
            set(fields, "ssid", &mut c.network.ssid);
            // Blank secret fields retain credentials; explicit clear is available in typed config API.
            if fields.get("pass").is_some_and(|v| !v.is_empty()) {
                set(fields, "pass", &mut c.network.wifi_password);
            }
            set(fields, "timeserver", &mut c.network.timeserver);
            c.network.utc_offset_hours = fields
                .get("ntptz")
                .map_or(Ok(c.network.utc_offset_hours), |v| {
                    v.parse().map_err(|_| "invalid UTC offset")
                })?;
            c.network.mqtt_enabled = fields.contains_key("MQTT_Active");
            set(fields, "MQTT_BROKER", &mut c.network.mqtt_host);
            c.network.mqtt_port = u16::try_from(number(fields, "MQTT_B_PRT", 1883)?)
                .map_err(|_| "invalid MQTT port")?;
            c.network.mqtt_keepalive_seconds =
                u16::try_from(number(fields, "MQTT_KeepAliveSeconds", 30)?)
                    .map_err(|_| "invalid keepalive")?;
            set(fields, "mqttUser", &mut c.network.mqtt_username);
            if fields.get("mqttPass").is_some_and(|v| !v.is_empty()) {
                set(fields, "mqttPass", &mut c.network.mqtt_password);
            }
            if fields.get("webPassword").is_some_and(|v| !v.is_empty()) {
                set(fields, "webPassword", &mut c.management.password);
            }
        }
        "/ApplyRelay.html" => {
            if number(fields, "RELAYNB", 0)? != 0 {
                return Err("only relay 0 exists".into());
            }
            c.relay.ttl_seconds = number(fields, "ttl", c.relay.ttl_seconds)?;
            c.relay.tta_seconds = number(fields, "tta", c.relay.tta_seconds)?;
            for (key, target) in [
                ("PUB_TOPIC1", &mut c.relay.command_topic),
                ("STATE_PUB_TOPIC", &mut c.relay.state_topic),
                ("ttl_PUB_TOPIC", &mut c.relay.ttl_topic),
                ("i_ttl_PUB_TOPIC", &mut c.relay.ttl_command_topic),
                ("CURR_TTL_PUB_TOPIC", &mut c.relay.elapsed_topic),
            ] {
                set(fields, key, target);
            }
        }
        "/ApplyIRMap.html" => {
            for input in &mut c.inputs {
                input.relay = None;
            }
            for slot in 1..=10 {
                let pin: i32 = fields
                    .get(&format!("I{slot}"))
                    .map_or(Ok(-1), |v| v.parse().map_err(|_| "invalid mapping GPIO"))?;
                let relay: i32 = fields
                    .get(&format!("R{slot}"))
                    .map_or(Ok(-1), |v| v.parse().map_err(|_| "invalid mapping relay"))?;
                if pin == -1 || relay == -1 {
                    continue;
                }
                if relay != 0 {
                    return Err("only relay 0 exists".into());
                }
                let index = INPUT_GPIOS
                    .iter()
                    .position(|v| i32::from(*v) == pin)
                    .ok_or("unavailable GPIO")?;
                if c.inputs[index].relay.is_some() {
                    return Err("duplicate input mapping".into());
                }
                c.inputs[index].relay = Some(0);
            }
        }
        "/savetimer.html" => {
            let slot = number(fields, "TNumber", 0)?;
            if !(1..=4).contains(&slot) {
                return Err("timer must be 1-4".into());
            }
            let timer = &mut c.timers[(slot - 1) as usize];
            timer.enabled = fields.contains_key("CEnabled");
            timer.kind =
                u8::try_from(number(fields, "TMTYPEedit", 2)?).map_err(|_| "invalid timer type")?;
            let relay = number(fields, "TRelay", 0)?;
            timer.relay = if relay == 255 {
                None
            } else {
                Some(u8::try_from(relay).map_err(|_| "invalid relay")?)
            };
            set(fields, "Dfrom", &mut timer.date_from);
            set(fields, "DTo", &mut timer.date_to);
            set(fields, "TFrom", &mut timer.time_from);
            set(fields, "TTo", &mut timer.time_to);
            timer.duration_minutes = number(fields, "Mark_Hours", 0)?
                .checked_mul(60)
                .and_then(|h| {
                    number(fields, "Mark_Minutes", 0)
                        .ok()
                        .and_then(|m| (m < 60).then_some(m))
                        .and_then(|m| h.checked_add(m))
                })
                .ok_or("invalid duration")?;
            timer.month_day =
                u8::try_from(number(fields, "MonthDay", 1)?).map_err(|_| "invalid monthly day")?;
            for (i, key) in [
                "CSunday",
                "CMonday",
                "CTuesday",
                "CWednesday",
                "CThursday",
                "CFriday",
                "CSaturday",
            ]
            .iter()
            .enumerate()
            {
                timer.weekdays[i] = fields.contains_key(*key);
            }
        }
        _ => return Err("unsupported configuration route".into()),
    }
    c.validate()?;
    Ok(c)
}
pub fn update_inputs(current: &Config, value: &Value) -> Result<Config, String> {
    if !value.is_object() {
        return Err("inputs must be an object".into());
    }
    let mut c = current.clone();
    for (i, topic_key) in INPUT_TOPICS.iter().enumerate() {
        let mode = value[format!("I{}MODE", i + 1)]
            .as_u64()
            .ok_or("invalid input mode")?;
        c.inputs[i].mode =
            InputMode::from_legacy(u32::try_from(mode).map_err(|_| "invalid input mode")?)?;
        if !matches!(
            c.inputs[i].mode,
            InputMode::RelayToggle | InputMode::CopyToRelay
        ) {
            c.inputs[i].relay = None;
        }
        c.inputs[i].topic = value[*topic_key]
            .as_str()
            .ok_or("invalid input topic")?
            .into();
    }
    c.validate()?;
    Ok(c)
}
pub fn mode_number(mode: &InputMode) -> u8 {
    match mode {
        InputMode::None => 0,
        InputMode::Toggle => 1,
        InputMode::Normal => 2,
        InputMode::RelayToggle => 3,
        InputMode::CopyToRelay => 4,
        InputMode::Temperature => 5,
    }
}
pub fn legacy_config(
    c: &Config,
    state: Snapshot,
    uptime: u32,
    node: &str,
    mac: &str,
    time: &str,
) -> Value {
    let mut value = json!({"NODEID":node,"MACADDR":mac,"systemtime":time,"uptime":format!("{}d {:02}:{:02}:{:02}", uptime/86400,(uptime/3600)%24,(uptime/60)%60,uptime%60),
        "PhyLoc":c.location,"ssid":c.network.ssid,"pass":"","timeserver":c.network.timeserver,"ntptz":c.network.utc_offset_hours,
        "MQTT_Active":u8::from(c.network.mqtt_enabled),"MQTT_BROKER":c.network.mqtt_host,"MQTT_B_PRT":c.network.mqtt_port,"MQTT_KeepAliveSeconds":c.network.mqtt_keepalive_seconds,"mqttUser":c.network.mqtt_username,"mqttPass":"",
        "RSTATE0":if state.on {"ON"} else {"OFF"},"RSTATE1":"NA","RELAYNB":0,"PUB_TOPIC1":c.relay.command_topic,"STATE_PUB_TOPIC":c.relay.state_topic,"TTL_PUB_TOPIC":c.relay.ttl_topic,"i_ttl_PUB_TOPIC":c.relay.ttl_command_topic,"CURR_TTL_PUB_TOPIC":c.relay.elapsed_topic,"ttl":c.relay.ttl_seconds,"tta":c.relay.tta_seconds,
        "ACS1":"NA","ACS2":"NA","ATEMP1":"NA","ATEMP2":"NA","WaterFlowSensor":0,"PressureSensor":0,"ADS1X15":0,"HST":0,"EmonLib":0,"OLED1306":0,"HA_Discovery_Present":0,"hasTempRelay":0,"TOGGLE_BTN_PUB_TOPIC":"","I0MODE":0});
    for (i, key) in INPUT_TOPICS.iter().enumerate() {
        value[format!("I{}MODE", i + 1)] = json!(mode_number(&c.inputs[i].mode));
        value[*key] = json!(c.inputs[i].topic);
    }
    value
}
pub fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
pub struct RenderContext<'a> {
    pub config: &'a Config,
    pub slot: usize,
    pub state: Snapshot,
    pub time: &'a str,
    pub uptime: u32,
    pub active: bool,
}

fn render_token(key: &str, context: &RenderContext<'_>) -> Option<String> {
    let c = context.config;
    let timer = &c.timers[context.slot.clamp(1, 4) - 1];
    let value = match key {
        "RELAYNB_OPTIONS" | "RELAY_CONFIG_OPTIONS" => "<option value=\"0\">Relay 0</option>".into(),
        "TNBT" => context.slot.to_string(),
        "TMTYPEedit" => timer.kind.to_string(),
        "Dfrom" => escape(&timer.date_from),
        "DTo" => escape(&timer.date_to),
        "TFrom" => escape(&timer.time_from),
        "TTo" => escape(&timer.time_to),
        "MONTHDAY" => timer.month_day.to_string(),
        "Mark_Hours" => (timer.duration_minutes / 60).to_string(),
        "Mark_Minutes" => (timer.duration_minutes % 60).to_string(),
        "CEnabled" => u8::from(timer.enabled).to_string(),
        "TIMER_RELAY_OPTIONS" => format!(
            "<option value=\"255\"{}>(none)</option><option value=\"0\"{}>Relay 0</option>",
            if timer.relay.is_none() {
                " selected"
            } else {
                ""
            },
            if timer.relay.is_some() {
                " selected"
            } else {
                ""
            }
        ),
        "TSTATE" => if context.active { "ACTIVE" } else { "IDLE" }.into(),
        "RSTATE" => if context.state.on { "ON" } else { "OFF" }.into(),
        "systemtime" => escape(context.time),
        "uptime" => format!("{}s", context.uptime),
        _ => {
            if let Some(index) = [
                "CSunday",
                "CMonday",
                "CTuesday",
                "CWednesday",
                "CThursday",
                "CFriday",
                "CSaturday",
            ]
            .iter()
            .position(|v| *v == key)
            {
                return Some(u8::from(timer.weekdays[index]).to_string());
            }
            let (kind, index) = key.split_at(1);
            if !matches!(kind, "I" | "R") {
                return None;
            }
            let slot: usize = index.strip_suffix("_OPT")?.parse().ok()?;
            if !(1..=10).contains(&slot) {
                return None;
            }
            let assigned = c
                .inputs
                .iter()
                .enumerate()
                .filter(|(_, v)| v.relay.is_some())
                .nth(slot - 1)
                .map(|(i, _)| INPUT_GPIOS[i]);
            let mut options = format!(
                "<option value=\"-1\"{}>(none)</option>",
                if assigned.is_none() { " selected" } else { "" }
            );
            if kind == "I" {
                for pin in INPUT_GPIOS {
                    options.push_str(&format!(
                        "<option value=\"{pin}\"{}>GPIO{pin}</option>",
                        if assigned == Some(pin) {
                            " selected"
                        } else {
                            ""
                        }
                    ));
                }
            } else {
                options.push_str(&format!(
                    "<option value=\"0\"{}>Relay 0</option>",
                    if assigned.is_some() { " selected" } else { "" }
                ));
            }
            options
        }
    };
    Some(value)
}

// Static spans stay in flash; only small replacement values allocate.
pub fn render_reader(
    reader: impl std::io::Read,
    context: &RenderContext<'_>,
    mut write: impl FnMut(&[u8]) -> std::io::Result<()>,
) -> std::io::Result<()> {
    use std::io::{BufReader, Read};
    let reader = BufReader::with_capacity(768, reader);
    let mut output = Vec::with_capacity(768);
    let mut token = Vec::with_capacity(64);
    let mut emit = |bytes: &[u8]| -> std::io::Result<()> {
        for &byte in bytes {
            output.push(byte);
            if output.len() == 768 {
                write(&output)?;
                output.clear();
            }
        }
        Ok(())
    };
    for byte in reader.bytes() {
        let byte = byte?;
        if !token.is_empty() {
            if byte == b'%' {
                let key = std::str::from_utf8(&token[1..]).unwrap_or("");
                if let Some(value) = render_token(key, context) {
                    emit(value.as_bytes())?;
                } else {
                    emit(&token)?;
                    emit(b"%")?;
                }
                token.clear();
                continue;
            }
            if (byte.is_ascii_alphanumeric() || byte == b'_') && token.len() < 63 {
                token.push(byte);
                continue;
            }
            emit(&token)?;
            token.clear();
        }
        if byte == b'%' {
            token.push(byte);
        } else {
            emit(&[byte])?;
        }
    }
    emit(&token)?;
    if !output.is_empty() {
        write(&output)?;
    }
    Ok(())
}

pub fn render_stream<E>(
    template: &str,
    context: &RenderContext<'_>,
    mut write: impl FnMut(&[u8]) -> Result<(), E>,
) -> Result<(), E> {
    let mut emit = |bytes: &[u8]| -> Result<(), E> {
        for chunk in bytes.chunks(768) {
            write(chunk)?;
        }
        Ok(())
    };
    let mut cursor = 0;
    let mut span = 0;
    while let Some(offset) = template[cursor..].find('%') {
        let start = cursor + offset;
        let tail = &template[start + 1..];
        let length = tail
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
            .count();
        if length > 0 && tail.as_bytes().get(length) == Some(&b'%') {
            if let Some(value) = render_token(&tail[..length], context) {
                emit(&template.as_bytes()[span..start])?;
                emit(value.as_bytes())?;
                cursor = start + length + 2;
                span = cursor;
                continue;
            }
        }
        cursor = start + 1;
    }
    emit(&template.as_bytes()[span..])
}

pub fn render(
    template: &str,
    c: &Config,
    slot: usize,
    state: Snapshot,
    time: &str,
    uptime: u32,
    active: bool,
) -> String {
    let context = RenderContext {
        config: c,
        slot,
        state,
        time,
        uptime,
        active,
    };
    let mut body = Vec::new();
    render_stream(template, &context, |chunk| {
        body.extend_from_slice(chunk);
        Ok::<_, std::convert::Infallible>(())
    })
    .unwrap();
    String::from_utf8(body).unwrap()
}
