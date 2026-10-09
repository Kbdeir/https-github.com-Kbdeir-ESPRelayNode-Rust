use crate::{
    command::{Action, Outcome, Source},
    config::{Config, MAX_CONFIG_BYTES},
    management,
    protocol::command,
    runtime::Runtime,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use std::sync::{atomic::Ordering, Arc};

pub struct Reply {
    pub status: u16,
    pub mime: &'static str,
    pub body: Body,
    pub headers: Vec<(&'static str, String)>,
}
pub enum Body {
    Bytes(Vec<u8>),
    Asset(Box<AssetBody>),
    File(std::fs::File),
    FileList(Vec<crate::filesystem::Entry>, u64, u64),
    Backup(Box<crate::backup::Backup>),
}
pub struct AssetBody {
    file: std::fs::File,
    config: Config,
    slot: usize,
    state: crate::runtime::RuntimeSnapshot,
    clock: String,
}
impl Body {
    pub fn write_to(
        &self,
        mut write: impl FnMut(&[u8]) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        match self {
            Self::Backup(backup) => backup.write(write),
            Self::FileList(entries, total, used) => {
                write(format!("{{\"fsTotal\":{total},\"fsUsed\":{used},\"files\":[").as_bytes())?;
                for (index, entry) in entries.iter().enumerate() {
                    if index > 0 {
                        write(b",")?;
                    }
                    let bytes = serde_json::to_vec(entry).map_err(std::io::Error::other)?;
                    write(&bytes)?;
                }
                write(b"]}")
            }
            Self::Bytes(bytes) => {
                for chunk in bytes.chunks(768) {
                    write(chunk)?;
                }
                Ok(())
            }
            Self::File(file) => {
                use std::io::Read;
                let mut file = file;
                let mut buffer = [0; 768];
                loop {
                    let count = file.read(&mut buffer)?;
                    if count == 0 {
                        return Ok(());
                    }
                    write(&buffer[..count])?;
                }
            }
            Self::Asset(asset) => management::render_reader(
                &asset.file,
                &management::RenderContext {
                    config: &asset.config,
                    slot: asset.slot,
                    state: asset.state.relay,
                    time: &asset.clock,
                    uptime: asset.state.uptime_seconds,
                    active: asset.state.timers[asset.slot - 1],
                },
                write,
            ),
        }
    }
    pub fn into_bytes(self) -> Vec<u8> {
        let mut bytes = Vec::new();
        self.write_to(|chunk| {
            bytes.extend_from_slice(chunk);
            Ok(())
        })
        .unwrap();
        bytes
    }
}
impl Reply {
    pub fn json(status: u16, value: Value) -> Self {
        Self {
            status,
            mime: "application/json",
            body: Body::Bytes(serde_json::to_vec(&value).unwrap()),
            headers: Vec::new(),
        }
    }
    pub fn error(status: u16, message: impl AsRef<str>) -> Self {
        Self::json(status, json!({"ok":false,"message":message.as_ref()}))
    }
}
pub fn authorized(header: Option<&str>, config: &Config) -> bool {
    let Some(encoded) = header.and_then(|v| v.strip_prefix("Basic ")) else {
        return false;
    };
    if encoded.len() > 256 || config.management.password.is_empty() {
        return false;
    }
    let Ok(decoded) = STANDARD.decode(encoded) else {
        return false;
    };
    let expected = format!(
        "{}:{}",
        config.management.username, config.management.password
    );
    if decoded.len() != expected.len() {
        return false;
    }
    decoded
        .iter()
        .zip(expected.as_bytes())
        .fold(0u8, |d, (a, b)| d | (a ^ b))
        == 0
}
pub fn clock(config: &Config) -> String {
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |v| v.as_secs());
    if epoch < 1_704_067_200 {
        return "Not synchronized".into();
    }
    chrono::DateTime::from_timestamp(
        epoch as i64 + i64::from(config.network.utc_offset_hours) * 3600,
        0,
    )
    .map_or_else(
        || "Not synchronized".into(),
        |d| d.format("%Y-%m-%d %H:%M:%S").to_string(),
    )
}
pub fn handle<F>(
    runtime: &Arc<Runtime>,
    method: &str,
    uri: &str,
    body: &[u8],
    mut persist: F,
) -> Reply
where
    F: FnMut(&Config, &Config) -> Result<(), String>,
{
    if uri.len() > 4096 || body.len() > MAX_CONFIG_BYTES {
        return Reply::error(413, "request too large");
    }
    let (path, query) = uri.split_once('?').unwrap_or((uri, ""));
    let fields = match management::form(query.as_bytes()) {
        Ok(v) => v,
        Err(e) => return Reply::error(400, e),
    };
    if method == "POST" && runtime.maintenance.load(Ordering::Acquire) {
        return Reply::error(409, "Firmware maintenance in progress");
    }
    if path == "/api/backup" && method == "GET" {
        return match fields.get("includeWeb").map_or("0", String::as_str) {
            "0" => crate::backup::download(runtime, false),
            "1" if runtime.files.lock().unwrap().is_some() => {
                crate::backup::download(runtime, true)
            }
            "1" => Reply::error(503, "LittleFS unavailable"),
            _ => Reply::error(400, "Invalid includeWeb flag"),
        };
    }
    if path == "/api/restore" && method == "POST" {
        return crate::backup::restore(runtime, uri, body.len(), body, persist);
    }
    if path == "/api/resetconfig" && method == "POST" {
        return crate::backup::reset(runtime, persist);
    }
    if path == "/api/reboot" {
        if method != "POST" {
            return Reply::error(405, "Reboot requires POST");
        }
        let _config = runtime.config.lock().unwrap();
        let deadline = (runtime.now_ms() / 1000) as u32 + 3;
        let _ =
            runtime
                .restart_at
                .compare_exchange(0, deadline, Ordering::AcqRel, Ordering::Acquire);
        return Reply::json(
            200,
            json!({"ok":true,"restart":true,"message":"Rebooting with relay OFF."}),
        );
    }
    if path == "/EraseCoreDump.json" {
        return Reply::error(
            if method == "POST" { 501 } else { 405 },
            "Crash dump storage is not enabled",
        );
    }
    if matches!(
        path,
        "/api/restore" | "/api/resetconfig" | "/update" | "/updatefs"
    ) && method != "POST"
    {
        return Reply::error(405, "Maintenance changes require POST");
    }
    if path.starts_with("/api/files/") {
        return handle_files(runtime, method, path, &fields, body);
    }
    let state = runtime.read();
    if method == "GET" {
        let config = runtime.config.lock().unwrap().clone();
        if fields.contains_key("RELAYACTION")
            || matches!(
                path,
                "/RelayControl.json"
                    | "/TimerControl.json"
                    | "/Apply.html"
                    | "/ApplyRelay.html"
                    | "/ApplyIRMap.html"
                    | "/savetimer.html"
            )
        {
            return Reply::error(405, "actuator and settings changes require POST");
        }
        if let Some(reply) = serve_file(runtime, path, &fields, &config, &state) {
            return reply;
        }
        let value = match path {
            "/GetConfig.json" | "/InputsConfig.json" => management::legacy_config(
                &config,
                state.relay,
                state.uptime_seconds,
                &runtime.node_id,
                &runtime.mac,
                &clock(&config),
            ),
            "/SensorStatus.json" => {
                json!({"RSTATE0":if state.relay.on{"ON"}else{"OFF"},"RSTATE1":"NA","relays":[{"nb":0,"state":if state.relay.on{"ON"}else{"OFF"}}],"ATEMP1":"NA","ATEMP2":"NA","ACS1":"NA","ACS2":"NA","uptime":state.uptime_seconds,"time":clock(&config)})
            }
            "/RelayState.json" | "/api/relay/state" => {
                if management::number(&fields, "relay", 0).unwrap_or(u32::MAX) != 0 {
                    return Reply::error(404, "relay unavailable");
                }
                json!({"relay":0,"state":if state.relay.on{"ON"}else{"OFF"},"on":state.relay.on,"ttl":state.relay.ttl_remaining_seconds,"tta":state.relay.tta_remaining_seconds,"interlocked":state.relay.interlocked})
            }
            "/WifiStatus.json" => runtime.wifi_status.lock().unwrap().clone(),
            "/WifiScan.json" => {
                if fields.contains_key("start") {
                    runtime.scan_requested.store(true, Ordering::Relaxed);
                    json!({"scanning":true,"networks":[],"count":0})
                } else {
                    runtime.wifi_scan.lock().unwrap().clone()
                }
            }
            "/MqttStatus.json" => {
                json!({"enabled":config.network.mqtt_enabled,"connected":runtime.mqtt_ready.load(Ordering::Relaxed),"connecting":runtime.mqtt_connected.load(Ordering::Relaxed),"server":format!("{}:{}",config.network.mqtt_host,config.network.mqtt_port),"route":"WiFi","healthTopic":format!("smartconfig/{}/availability",runtime.node_id)})
            }
            "/WireGuardStatus.json" => {
                json!({"connected":false,"networkUp":false,"address":"NA","endpoint":"NA","dns":"NA","supported":false})
            }
            "/FeatureFlags.json" => {
                json!({"waterFlowSensor":false,"oled1306":false,"emonlib":false,"solarHeater":false,"ads1x15":false,"hst":false,"pressureSensor":false,"adsDCCurrent":false})
            }
            "/CoreDumpStatus.json" => json!({"hasDump":false,"supported":false}),
            "/api/status" => {
                json!({"node":runtime.node_id,"runtime":state,"health":runtime.health.status((runtime.now_ms()/1000) as u32),"queue_overflows":runtime.overflows.load(Ordering::Relaxed),"wifi":runtime.wifi_status.lock().unwrap().clone(),"mqtt_connected":runtime.mqtt_ready.load(Ordering::Relaxed),"restart_pending":runtime.restart_at.load(Ordering::Relaxed)>0})
            }
            "/api/modbus" => {
                json!({"config":config.modbus,"map_version":crate::modbus::MAP_VERSION,"peers":runtime.peer_status.lock().unwrap().clone()})
            }
            "/api/automation" => automation_status(runtime, &config, &state),
            "/TimerStatus.json" => {
                json!({"timers":timer_status(&config,&state,runtime.schedule_paused.load(Ordering::Relaxed))})
            }
            "/api/live" => {
                json!({"system":{"nodeId":runtime.node_id,"mac":runtime.mac,"location":config.location,"time":clock(&config),"uptime":state.uptime_seconds},"features":{"emonlib":false,"ads1x15":false,"hst":false,"waterFlowSensor":false,"pressureSensor":false},"relays":[{"id":0,"state":if state.relay.on{"ON"}else{"OFF"},"ttl":state.relay.ttl_remaining_seconds,"tta":state.relay.tta_remaining_seconds}],"temperature":{"t1":null,"t2":null},"timers":timer_status(&config,&state,runtime.schedule_paused.load(Ordering::Relaxed))})
            }
            _ => return Reply::error(404, "route not ported"),
        };
        return Reply::json(200, value);
    }
    if method != "POST" {
        return Reply::error(405, "method not allowed");
    }
    if matches!(
        path,
        "/api/config" | "/api/modbus" | "/api/relay" | "/InputsConfig.json"
    ) {
        if let Err(error) = crate::config::check_json_depth(body, 16) {
            return Reply::error(400, error);
        }
    }
    if path == "/TimerControl.json" {
        let f = match management::form(body) {
            Ok(f) => f,
            Err(e) => return Reply::error(400, e),
        };
        if management::number(&f, "relay", 0).unwrap_or(u32::MAX) != 0 {
            return Reply::error(404, "relay unavailable");
        }
        let paused = match f.get("action").map(String::as_str) {
            Some("pause") => true,
            Some("resume") => false,
            _ => return Reply::error(400, "invalid timer action"),
        };
        runtime.schedule_paused.store(paused, Ordering::Release);
        return Reply::json(200, json!({"ok":true,"relay":0,"paused":paused}));
    }
    if path == "/api/relay" || path == "/RelayControl.json" || path == "/RelayConfig.html" {
        let fields = match management::form(body) {
            Ok(v) => v,
            Err(e) => return Reply::error(400, e),
        };
        let (relay, action) = if path == "/api/relay" {
            let value: Value = match serde_json::from_slice(body) {
                Ok(v) => v,
                Err(_) => return Reply::error(400, "invalid command JSON"),
            };
            (
                value["relay"].as_u64().unwrap_or(0),
                value["action"].as_str().unwrap_or("").to_owned(),
            )
        } else {
            (
                management::number(
                    &fields,
                    if path == "/RelayConfig.html" {
                        "GETRELAYNB"
                    } else {
                        "relay"
                    },
                    0,
                )
                .unwrap_or(u32::MAX) as u64,
                fields
                    .get(if path == "/RelayConfig.html" {
                        "RELAYACTION"
                    } else {
                        "action"
                    })
                    .cloned()
                    .unwrap_or_default(),
            )
        };
        if relay != 0 {
            return Reply::error(404, "relay unavailable");
        }
        let action = match action.to_ascii_lowercase().as_str() {
            "on" => Action::On,
            "off" => Action::Off,
            "tog" | "toggle" => Action::Toggle,
            _ => return Reply::error(400, "invalid relay action"),
        };
        return match runtime.submit(command(action, Source::Http)) {
            Ok(Outcome::Applied) => Reply::json(
                200,
                json!({"ok":true,"relay":0,"state":if runtime.read().relay.on{"ON"}else{"OFF"}}),
            ),
            Ok(outcome) => Reply::error(409, format!("{outcome:?}")),
            Err(e) => Reply::error(503, e),
        };
    }
    let mut current = runtime.config.lock().unwrap();
    if runtime.restart_at.load(Ordering::Relaxed) > 0 {
        return Reply::error(409, "configuration restart already pending");
    }
    let candidate = match path {
        "/api/automation/rule" | "/api/automation/remote" => {
            management::form(body).and_then(|f| management::update_automation(&current, path, &f))
        }
        "/InputsConfig.json" => serde_json::from_slice(body)
            .map_err(|_| "invalid input JSON".into())
            .and_then(|v| management::update_inputs(&current, &v)),
        "/api/config" => Config::parse(body),
        "/api/modbus" => serde_json::from_slice(body)
            .map_err(|_| "invalid Modbus JSON".into())
            .and_then(|v| {
                let mut c = current.clone();
                c.modbus = v;
                c.validate()?;
                Ok(c)
            }),
        _ => management::form(body).and_then(|f| management::update_config(&current, path, &f)),
    };
    let candidate = match candidate {
        Ok(c) => c,
        Err(e) => return Reply::error(400, e),
    };
    if let Err(e) = persist(&candidate, &current) {
        return Reply::error(500, e);
    }
    let restart = candidate.wifi_changed(&current);
    runtime.config_saved(&candidate, &current);
    *current = candidate;
    if restart {
        runtime
            .restart_at
            .store((runtime.now_ms() / 1000) as u32 + 3, Ordering::Release);
    }
    Reply::json(
        200,
        json!({"ok":true,"message":if restart {"Wi-Fi saved. Restarting with relay OFF."} else {"Saved. Settings take effect without reboot."},"restart":restart}),
    )
}

fn file_error(error: std::io::Error) -> Reply {
    use std::io::ErrorKind;
    let status = match error.kind() {
        ErrorKind::NotFound => 404,
        ErrorKind::InvalidInput | ErrorKind::PermissionDenied => 400,
        ErrorKind::AlreadyExists => 409,
        _ => 507,
    };
    Reply::error(status, error.to_string())
}
pub fn upload_file(
    runtime: &Runtime,
    uri: &str,
    length: usize,
    reader: impl std::io::Read,
) -> Reply {
    if length > crate::filesystem::MAX_FILE_BYTES {
        return Reply::error(413, "File exceeds 256 KiB limit");
    }
    let fields = match management::form(uri.split_once('?').map_or("", |(_, q)| q).as_bytes()) {
        Ok(fields) => fields,
        Err(error) => return Reply::error(400, error),
    };
    let Some(path) = fields.get("path") else {
        return Reply::error(400, "File path required");
    };
    let Some(store) = runtime.files.lock().unwrap().clone() else {
        return Reply::error(503, "Filesystem not mounted");
    };
    match store.upload(path, length, reader) {
        Ok(()) => Reply::json(
            200,
            json!({"ok":true,"message":format!("{length} bytes saved to {path}")}),
        ),
        Err(error) => file_error(error),
    }
}
fn handle_files(
    runtime: &Runtime,
    method: &str,
    route: &str,
    query: &std::collections::BTreeMap<String, String>,
    body: &[u8],
) -> Reply {
    let Some(store) = runtime.files.lock().unwrap().clone() else {
        return Reply::error(503, "Filesystem not mounted");
    };
    match (method, route) {
        ("GET", "/api/files/list") => match store
            .list()
            .and_then(|files| store.info().map(|(total, used)| (files, total, used)))
        {
            Ok((files, total, used)) => Reply {
                status: 200,
                mime: "application/json",
                body: Body::FileList(files, total, used),
                headers: Vec::new(),
            },
            Err(error) => file_error(error),
        },
        ("GET", "/api/files/content") => match query.get("path") {
            Some(path) => match store.open(path) {
                Ok(file) => Reply {
                    status: 200,
                    mime: "application/octet-stream",
                    body: Body::File(file),
                    headers: Vec::new(),
                },
                Err(error) => file_error(error),
            },
            None => Reply::error(400, "File path required"),
        },
        ("POST", "/api/files/content") => {
            let Some(path) = query.get("path") else {
                return Reply::error(400, "File path required");
            };
            match store.upload(path, body.len(), body) {
                Ok(()) => Reply::json(200, json!({"ok":true,"message":"File saved"})),
                Err(error) => file_error(error),
            }
        }
        ("POST", "/api/files/delete" | "/api/files/rename") => {
            let fields = match management::form(body) {
                Ok(f) => f,
                Err(e) => return Reply::error(400, e),
            };
            let result = if route.ends_with("delete") {
                store.delete(fields.get("path").map_or("", String::as_str))
            } else {
                store.rename(
                    fields.get("from").map_or("", String::as_str),
                    fields.get("to").map_or("", String::as_str),
                )
            };
            match result {
                Ok(()) => Reply::json(200, json!({"ok":true,"message":"Done"})),
                Err(error) => file_error(error),
            }
        }
        _ => Reply::error(405, "Method not allowed"),
    }
}
fn serve_file(
    runtime: &Runtime,
    route: &str,
    fields: &std::collections::BTreeMap<String, String>,
    config: &Config,
    state: &crate::runtime::RuntimeSnapshot,
) -> Option<Reply> {
    let path = crate::filesystem::asset_path(route);
    if ![
        ".html", ".js", ".css", ".png", ".jpg", ".jpeg", ".ico", ".svg", ".gz",
    ]
    .iter()
    .any(|extension| path.ends_with(extension))
    {
        return None;
    }
    let Some(store) = runtime.files.lock().unwrap().clone() else {
        return Some(Reply::error(
            503,
            "Filesystem not mounted. Flash the LittleFS image.",
        ));
    };
    let (file, compressed) = match store.open(path) {
        Ok(file) => (file, path.ends_with(".gz")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !path.ends_with(".gz") => {
            match store.open(&format!("{path}.gz")) {
                Ok(file) => (file, true),
                Err(error) => return Some(file_error(error)),
            }
        }
        Err(error) => return Some(file_error(error)),
    };
    let mime = crate::filesystem::mime(path.strip_suffix(".gz").unwrap_or(path));
    let body = if path.ends_with(".html") && !compressed {
        let slot = match management::number(fields, "GetTimer", 1) {
            Ok(slot) if (1..=4).contains(&slot) => slot as usize,
            _ => return Some(Reply::error(400, "Timer must be 1-4")),
        };
        Body::Asset(Box::new(AssetBody {
            file,
            config: config.clone(),
            slot,
            state: state.clone(),
            clock: clock(config),
        }))
    } else {
        Body::File(file)
    };
    Some(Reply {
        status: 200,
        mime,
        body,
        headers: if compressed {
            vec![("Content-Encoding", "gzip".into())]
        } else {
            Vec::new()
        },
    })
}

fn automation_status(
    runtime: &Runtime,
    config: &Config,
    state: &crate::runtime::RuntimeSnapshot,
) -> Value {
    let now = runtime.now_ms();
    let readings = *runtime.remote_values.lock().unwrap();
    let writes = *runtime.remote_writes.lock().unwrap();
    let rules: Vec<_> = config.automation.iter().enumerate().map(|(i,r)| {
        let target = r.modbus.clone().unwrap_or_default();
        json!({"id":i+1,"enabled":r.enabled,"target":r.target,"action":r.action,"logic":r.logic,"trigger":r.trigger,"gate":r.gate,"hold":r.hold,"cooldown":r.cooldown,"output":u8::from(r.modbus.is_some()),"mbHost":target.host,"mbPort":target.port,"mbUnit":target.unit,"mbAddress":target.address,"mbClear":u8::from(target.send_clear),"conditions":r.conditions,"status":state.automation[i],"delivery":writes[i]})
    }).collect();
    let remotes: Vec<_> = config.remote_sensors.iter().enumerate().map(|(i,r)| {
        let m = &r.modbus; let reading = readings[i];
        json!({"id":i+1,"transport":r.transport,"topic":r.topic,"jsonKey":r.json_key,"label":r.label,"host":m.host,"port":m.port,"unit":m.unit,"function":m.function,"address":m.address,"valueType":m.value_type,"wordOrder":m.word_order,"poll":m.poll,"scale":m.scale,"offset":m.offset,"valid":r.configured()&&reading.valid(now),"value":reading.value,"age":reading.seen_ms.map_or(0,|t|now.saturating_sub(t)/1000)})
    }).collect();
    json!({"relayCount":1,"staleSeconds":300,"rules":rules,"remotes":remotes})
}

fn timer_status(
    config: &Config,
    state: &crate::runtime::RuntimeSnapshot,
    paused: bool,
) -> Vec<Value> {
    config.timers.iter().enumerate().map(|(i,t)|{
        let mut days=json!({});for(k,b)in ["Su","Mo","Tu","We","Th","Fr","Sa"].iter().zip(t.weekdays){days[*k]=json!(b);}
        let (on,off)=t.compile().ok().and_then(|c|crate::schedule::local_now(config.network.utc_offset_hours).map(|now|c.countdown(now))).unwrap_or((-1,-1));
        json!({"id":i+1,"exists":true,"enabled":t.enabled,"timerActive":state.timers[i]&&!paused,"timerPaused":paused,"relay":t.relay.unwrap_or(255),"relayState":if state.relay.on{"ON"}else{"OFF"},"type":t.kind,"typeName":match t.kind{1=>"Specific date",2=>"Daily",3=>"Weekly",_=>"Monthly"},"dateFrom":t.date_from,"dateTo":t.date_to,"timeFrom":t.time_from,"timeTo":t.time_to,"markHours":t.duration_minutes/60,"markMinutes":t.duration_minutes%60,"days":days,"monthDay":t.month_day,"secondsToOn":on,"secondsToOff":off})
    }).collect()
}
