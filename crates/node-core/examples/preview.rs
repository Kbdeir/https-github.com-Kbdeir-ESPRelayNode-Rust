use node_core::{
    config::Config,
    engine::Engine,
    runtime::{Runtime, RuntimeSnapshot},
    web,
};
use serde_json::json;
use std::io::Read;
use std::{
    sync::{atomic::Ordering, Arc},
    thread,
    time::Duration,
};
use tiny_http::{Header, Response, Server, StatusCode};

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut config = Config {
        location: "Rust Local Preview".into(),
        ..Config::default()
    };
    config.network.ssid = "Local preview".into();
    config.management.password = "preview-local".into();
    let (runtime, receiver) =
        Runtime::new(config.clone(), "000000".into(), "00:00:00:00:00:00".into());
    runtime.ota_confirmed.store(true, Ordering::Release);
    runtime.network_ready.store(true, Ordering::Release);
    let asset_directory =
        std::env::temp_dir().join(format!("smartconfig-preview-files-{}", std::process::id()));
    std::fs::create_dir_all(&asset_directory)?;
    for entry in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../web"))? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            std::fs::copy(entry.path(), asset_directory.join(entry.file_name()))?;
        }
    }
    *runtime.files.lock().unwrap() = Some(Arc::new(node_core::filesystem::FileStore::new(
        asset_directory,
        None,
    )));
    *runtime.wifi_status.lock().unwrap() = json!({"connected":true,"ip":"127.0.0.1","gateway":"127.0.0.1","dns1":"","dns2":"","simulated":true});
    let control = Arc::clone(&runtime);
    thread::spawn(move || {
        let mut engine = Engine::new(&config.relay);
        let mut schedules: Vec<_> = config.timers.iter().map(|v| v.compile().unwrap()).collect();
        let mut timer_states = [false; 4];
        let mut automation = node_core::automation::Automation::default();
        let mut was_on = false;
        let mut config_revision = 0;
        loop {
            let now = control.now_ms();
            if let Some(updated) = control.config_update(&mut config_revision) {
                if updated.relay.ttl_seconds != config.relay.ttl_seconds
                    || updated.relay.tta_seconds != config.relay.tta_seconds
                {
                    engine.configure_changed(&updated.relay, &config.relay, now);
                }
                if updated.automation != config.automation
                    || updated.remote_sensors != config.remote_sensors
                    || updated.inputs != config.inputs
                    || updated.timers != config.timers
                {
                    let changed = automation.configure(&updated, &config);
                    for (index, changed) in changed.into_iter().enumerate() {
                        if changed {
                            control.cancel_remote_write(index);
                        }
                    }
                }
                schedules = updated
                    .timers
                    .iter()
                    .map(|v| v.compile().unwrap())
                    .collect();
                config = updated;
            }
            engine.tick(now);
            let restarting = control.restart_at.load(Ordering::Acquire);
            if restarting > 0 {
                engine.apply(
                    &node_core::command::Command::local(node_core::command::Action::Off),
                    now,
                );
                if now / 1000 >= u64::from(restarting) {
                    config = control.config.lock().unwrap().clone();
                    engine = Engine::new(&config.relay);
                    automation = node_core::automation::Automation::default();
                    *control.remote_values.lock().unwrap() =
                        [node_core::automation::RemoteReading::default(); 8];
                    schedules = config.timers.iter().map(|v| v.compile().unwrap()).collect();
                    was_on = false;
                    control.restart_at.store(0, Ordering::Release);
                }
            }
            if let Some(date) = node_core::schedule::local_now(config.network.utc_offset_hours) {
                for (i, t) in schedules.iter().enumerate() {
                    timer_states[i] = t.active(date);
                }
                let on = schedules
                    .iter()
                    .enumerate()
                    .any(|(i, t)| timer_states[i] && t.relay() == Some(0));
                if on != was_on {
                    engine.apply(
                        &node_core::protocol::command(
                            if on {
                                node_core::command::Action::On
                            } else {
                                node_core::command::Action::Off
                            },
                            node_core::command::Source::Automation,
                        ),
                        now,
                    );
                    was_on = on;
                }
            }
            for packet in receiver.try_iter().take(4) {
                let result = engine.apply(&packet.command, now);
                control.snapshot.lock().unwrap().relay = engine.snapshot();
                if let Some(reply) = packet.reply {
                    let _ = reply.try_send(result);
                }
            }
            let sources = RuntimeSnapshot {
                relay: engine.snapshot(),
                timers: timer_states,
                ..Default::default()
            };
            control.automation_tick(&mut automation, &config, &mut engine, &sources, false);
            *control.snapshot.lock().unwrap() = RuntimeSnapshot {
                relay: engine.snapshot(),
                timers: timer_states,
                uptime_seconds: (now / 1000) as u32,
                ttl_seconds: engine.ttl_seconds(),
                tta_seconds: config.relay.tta_seconds,
                automation: automation.status(),
                ..Default::default()
            };
            thread::sleep(Duration::from_millis(10));
        }
    });
    let port = std::env::var("SMARTCONFIG_PREVIEW_PORT")
        .unwrap_or_else(|_| "8090".into())
        .parse::<u16>()?;
    let server = Server::http(("127.0.0.1", port))?;
    println!("Local UI preview: http://127.0.0.1:{port}/ (simulated I/O; user / preview-local)");
    for mut request in server.incoming_requests() {
        let authorization = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("Authorization"))
            .map(|h| h.value.as_str());
        if !web::authorized(authorization, &runtime.config.lock().unwrap()) {
            request.respond(
                Response::from_string("Authentication required")
                    .with_status_code(StatusCode(401))
                    .with_header(
                        Header::from_bytes(
                            "WWW-Authenticate",
                            "Basic realm=\"SmartConfig Preview\"",
                        )
                        .unwrap(),
                    ),
            )?;
            continue;
        }
        if request.method().as_str() == "POST"
            && !request
                .headers()
                .iter()
                .any(|h| h.field.equiv("X-SmartConfig") && h.value.as_str() == "1")
        {
            request.respond(
                Response::from_string("Same-origin request required").with_status_code(403),
            )?;
            continue;
        }
        let upload = request.method().as_str() == "POST"
            && request.url().split('?').next() == Some("/api/files/content");
        let restore = request.method().as_str() == "POST"
            && request.url().split('?').next() == Some("/api/restore");
        let image =
            request.method().as_str() == "POST" && matches!(request.url(), "/update" | "/updatefs");
        let limit = if restore || image {
            node_core::backup::MAX_BACKUP_BYTES
        } else if upload {
            node_core::filesystem::MAX_FILE_BYTES
        } else {
            8192
        };
        if request.body_length().unwrap_or(0) > limit {
            request.respond(Response::from_string("Request too large").with_status_code(413))?;
            continue;
        }
        let uri = request.url().to_owned();
        let mut body = Vec::new();
        if !upload && !restore && !image {
            request.as_reader().take(8193).read_to_end(&mut body)?;
        }
        let reply = if image {
            web::Reply::error(400,"Preview cannot flash hardware. Use an ESP32 application or LittleFS image on the board.")
        } else if restore {
            let length = request.body_length().unwrap_or(0);
            node_core::backup::restore(&runtime, &uri, length, request.as_reader(), |_, _| Ok(()))
        } else if uri == "/api/maintenance" {
            web::Reply::json(
                200,
                json!({"otaSupported":true,"otaSlotBytes":1638400,"filesystemBytes":node_core::filesystem::CAPACITY,"ready":true,"busy":false,"simulated":true}),
            )
        } else if upload {
            let length = request.body_length().unwrap_or(0);
            web::upload_file(&runtime, &uri, length, request.as_reader())
        } else if request.url() == "/api/heap" {
            web::Reply::json(
                200,
                json!({"heap":0,"heapMin":0,"heapMaxBlk":0,"simulated":true}),
            )
        } else {
            web::handle(
                &runtime,
                request.method().as_str(),
                request.url(),
                &body,
                |_, _| Ok(()),
            )
        };
        let mut response = Response::from_data(reply.body.into_bytes())
            .with_status_code(reply.status)
            .with_header(Header::from_bytes("Content-Type", reply.mime).unwrap())
            .with_header(Header::from_bytes("Cache-Control", "no-store").unwrap());
        for (key, value) in reply.headers {
            response.add_header(Header::from_bytes(key, value).unwrap());
        }
        request.respond(response)?;
    }
    Ok(())
}
