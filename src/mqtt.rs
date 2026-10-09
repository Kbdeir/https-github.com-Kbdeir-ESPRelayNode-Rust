use anyhow::Result;
use esp_idf_svc::{
    handle::RawHandle,
    mqtt::client::{EspMqttClient, LwtConfiguration, MqttClientConfiguration, QoS},
    sys,
};
use node_core::{
    command::{Action, Outcome, Source},
    config::{Config, InputMode},
    health::Service,
    runtime::{Envelope, Runtime},
};
use std::{
    ffi::c_void,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

struct Context {
    runtime: Arc<Runtime>,
    config: Config,
    revision: u32,
    generation: AtomicU32,
    subscribed: AtomicU32,
}
// IDF owns event slices only during this callback. Context lives until client destruction.
unsafe extern "C" fn event(
    context: *mut c_void,
    _base: sys::esp_event_base_t,
    _id: i32,
    data: *mut c_void,
) {
    if context.is_null() || data.is_null() {
        return;
    }
    let ctx = &*(context as *const Context);
    if ctx.runtime.mqtt_revision.load(Ordering::Acquire) != ctx.revision {
        return;
    }
    let ev = &*(data as *const sys::esp_mqtt_event_t);
    match ev.event_id {
        sys::esp_mqtt_event_id_t_MQTT_EVENT_CONNECTED => {
            ctx.subscribed.store(0, Ordering::Relaxed);
            let generation = ctx
                .runtime
                .mqtt_generation
                .fetch_add(1, Ordering::AcqRel)
                .wrapping_add(1);
            ctx.generation.store(generation, Ordering::Release);
            ctx.runtime.mqtt_connected.store(true, Ordering::Release);
        }
        sys::esp_mqtt_event_id_t_MQTT_EVENT_DISCONNECTED => {
            ctx.runtime.mqtt_connected.store(false, Ordering::Release);
            ctx.runtime.mqtt_ready.store(false, Ordering::Release);
        }
        sys::esp_mqtt_event_id_t_MQTT_EVENT_SUBSCRIBED => {
            ctx.subscribed.fetch_add(1, Ordering::Release);
        }
        sys::esp_mqtt_event_id_t_MQTT_EVENT_DATA => {
            if ev.retain
                || ev.current_data_offset != 0
                || ev.data_len != ev.total_data_len
                || !(1..=512).contains(&ev.data_len)
                || !(1..=192).contains(&ev.topic_len)
                || ev.data.is_null()
                || ev.topic.is_null()
            {
                return;
            }
            let topic = std::slice::from_raw_parts(ev.topic as *const u8, ev.topic_len as usize);
            let payload = std::slice::from_raw_parts(ev.data as *const u8, ev.data_len as usize);
            if let Ok(topic) = std::str::from_utf8(topic) {
                for (index, sensor) in ctx.config.remote_sensors.iter().enumerate() {
                    if sensor.transport == node_core::automation::Transport::Mqtt
                        && !sensor.topic.is_empty()
                        && sensor.topic == topic
                    {
                        if let Ok(value) =
                            node_core::automation::remote_payload(payload, &sensor.json_key)
                        {
                            if let Ok(mut readings) = ctx.runtime.remote_values.try_lock() {
                                if ctx.runtime.mqtt_revision.load(Ordering::Acquire) != ctx.revision
                                {
                                    return;
                                }
                                readings[index] = node_core::automation::RemoteReading {
                                    value,
                                    seen_ms: Some(ctx.runtime.now_ms()),
                                };
                            }
                        }
                        return;
                    }
                }
                if let Ok(mut command) =
                    node_core::protocol::mqtt_command(&ctx.config.relay, topic, payload, ev.retain)
                {
                    if ev.msg_id > 0 {
                        command.id = Some(
                            (u64::from(ctx.generation.load(Ordering::Acquire)) << 32)
                                | ev.msg_id as u64,
                        );
                    }
                    command.expires_at_ms = Some(ctx.runtime.now_ms() + 1000);
                    let _ = ctx.runtime.enqueue(Envelope {
                        command,
                        reply: None,
                    });
                }
            }
        }
        _ => {}
    }
}
pub fn start(
    runtime: Arc<Runtime>,
    nvs: esp_idf_svc::nvs::EspDefaultNvsPartition,
    revision: u32,
) -> Result<thread::JoinHandle<()>> {
    let enabled = runtime.config.lock().unwrap().network.mqtt_enabled;
    if enabled {
        runtime
            .health
            .expect(Service::Mqtt, (runtime.now_ms() / 1000) as u32);
    }
    Ok(thread::Builder::new()
        .name("mqtt-adapter".into())
        .stack_size(12288)
        .spawn(move || {
            let guard = enabled.then(|| {
                runtime
                    .health
                    .watch(Service::Mqtt, (runtime.now_ms() / 1000) as u32)
            });
            while runtime.mqtt_revision.load(Ordering::Acquire) == revision {
                runtime
                    .health
                    .progress(Service::Mqtt, (runtime.now_ms() / 1000) as u32);
                match run(runtime.clone(), &nvs, revision) {
                    Ok(()) => break,
                    Err(e) => {
                        runtime.mqtt_connected.store(false, Ordering::Release);
                        runtime.mqtt_ready.store(false, Ordering::Release);
                        log::warn!("MQTT adapter retry after error: {e}");
                        for _ in 0..50 {
                            if runtime.mqtt_revision.load(Ordering::Acquire) != revision {
                                break;
                            }
                            thread::sleep(Duration::from_millis(100));
                        }
                    }
                }
            }
            runtime.mqtt_connected.store(false, Ordering::Release);
            runtime.mqtt_ready.store(false, Ordering::Release);
            if let Some(guard) = guard {
                guard.finish();
            }
        })?)
}
fn run(
    runtime: Arc<Runtime>,
    nvs: &esp_idf_svc::nvs::EspDefaultNvsPartition,
    revision: u32,
) -> Result<()> {
    let config = runtime.config.lock().unwrap().clone();
    if !config.network.mqtt_enabled {
        return Ok(());
    }
    while !runtime.wifi_status.lock().unwrap()["connected"]
        .as_bool()
        .unwrap_or(false)
    {
        if runtime.mqtt_revision.load(Ordering::Acquire) != revision {
            return Ok(());
        }
        runtime
            .health
            .progress(Service::Mqtt, (runtime.now_ms() / 1000) as u32);
        thread::sleep(Duration::from_millis(250));
    }
    let client_id = format!("SmartConfig-{}", runtime.node_id);
    let availability = format!("smartconfig/{}/availability", runtime.node_id);
    let ack_topic = format!("smartconfig/{}/ack", runtime.node_id);
    let uri = format!(
        "mqtt://{}:{}",
        config.network.mqtt_host, config.network.mqtt_port
    );
    let mut ctx = Box::new(Context {
        runtime: runtime.clone(),
        config: config.clone(),
        revision,
        generation: AtomicU32::new(0),
        subscribed: AtomicU32::new(0),
    });
    let mut client = EspMqttClient::new_cb(
        &uri,
        &MqttClientConfiguration {
            client_id: Some(&client_id),
            username: (!config.network.mqtt_username.is_empty())
                .then_some(config.network.mqtt_username.as_str()),
            password: (!config.network.mqtt_password.is_empty())
                .then_some(config.network.mqtt_password.as_str()),
            keep_alive_interval: Some(Duration::from_secs(u64::from(
                config.network.mqtt_keepalive_seconds,
            ))),
            reconnect_timeout: Some(Duration::from_secs(5)),
            network_timeout: Duration::from_secs(2),
            task_stack: 8192,
            buffer_size: 1024,
            out_buffer_size: 1024,
            lwt: Some(LwtConfiguration {
                topic: &availability,
                payload: b"offline",
                qos: QoS::AtLeastOnce,
                retain: true,
            }),
            ..Default::default()
        },
        |_| {},
    )?;
    // Let IDF's newly spawned task enter its running state before stopping it.
    thread::sleep(Duration::from_millis(20));
    sys::esp!(unsafe { sys::esp_mqtt_client_stop(client.handle()) })?;
    sys::esp!(unsafe {
        sys::esp_mqtt_client_register_event(
            client.handle(),
            sys::esp_mqtt_event_id_t_MQTT_EVENT_ANY,
            Some(event),
            (&mut *ctx as *mut Context).cast(),
        )
    })?;
    sys::esp!(unsafe { sys::esp_mqtt_client_start(client.handle()) })?;
    let topics: Vec<_> = [&config.relay.command_topic, &config.relay.ttl_command_topic]
        .into_iter()
        .chain(
            config
                .remote_sensors
                .iter()
                .filter(|s| s.transport == node_core::automation::Transport::Mqtt)
                .map(|s| &s.topic),
        )
        .filter(|v| !v.is_empty())
        .collect();
    let mut generation = 0;
    let mut last_state = None;
    let mut last_publish = 0u64;
    while runtime.mqtt_revision.load(Ordering::Acquire) == revision {
        let connected = runtime.mqtt_connected.load(Ordering::Acquire);
        if connected {
            let current = ctx.generation.load(Ordering::Acquire);
            if current != generation {
                for topic in &topics {
                    client.subscribe(topic, QoS::AtLeastOnce)?;
                }
                generation = current;
                last_state = None;
                last_publish = 0;
            }
            if ctx.subscribed.load(Ordering::Acquire) >= topics.len() as u32 {
                if !runtime.mqtt_ready.swap(true, Ordering::AcqRel) {
                    client.publish(&availability, QoS::AtLeastOnce, true, b"online")?;
                    log::info!("MQTT connected; command subscriptions ready");
                }
                let state = runtime.read();
                let now = runtime.now_ms();
                if last_state != Some(state.relay.on) || now.saturating_sub(last_publish) >= 10000 {
                    for (topic, value) in [
                        (
                            &config.relay.state_topic,
                            if state.relay.on {
                                "on".into()
                            } else {
                                "off".into()
                            },
                        ),
                        (&config.relay.ttl_topic, state.ttl_seconds.to_string()),
                        (
                            &config.relay.elapsed_topic,
                            state.relay.ttl_elapsed_seconds.to_string(),
                        ),
                    ] {
                        if !topic.is_empty() {
                            client.publish(topic, QoS::AtLeastOnce, true, value.as_bytes())?;
                        }
                    }
                    last_state = Some(state.relay.on);
                    last_publish = now;
                }
                let input_events: Vec<_> = runtime.input_events.lock().unwrap().drain(..).collect();
                for (index, high) in input_events {
                    let input = &config.inputs[index];
                    if input.topic.is_empty() {
                        continue;
                    }
                    let payload = match input.mode {
                        InputMode::Toggle if !high => Some("tog"),
                        InputMode::Normal | InputMode::RelayToggle | InputMode::CopyToRelay => {
                            Some(if high { "on" } else { "off" })
                        }
                        _ => None,
                    };
                    if let Some(payload) = payload {
                        client.publish(
                            &input.topic,
                            QoS::AtLeastOnce,
                            false,
                            payload.as_bytes(),
                        )?;
                    }
                }
                let outcomes: Vec<_> = runtime.outcomes.lock().unwrap().drain(..).collect();
                for (command, outcome) in outcomes {
                    let mut persisted = None;
                    if command.source == Source::Mqtt && outcome == Outcome::Applied {
                        if let Action::SetTtl(seconds) = command.action {
                            persisted = Some(true);
                            let mut current = runtime.config.lock().unwrap();
                            if current.relay.ttl_seconds != seconds {
                                let mut candidate = current.clone();
                                candidate.relay.ttl_seconds = seconds;
                                match crate::storage::save(&candidate, &current, nvs) {
                                    Ok(()) => {
                                        runtime.config_saved(&candidate, &current);
                                        *current = candidate;
                                    }
                                    Err(_) => {
                                        persisted = Some(false);
                                        log::warn!(
                                            "MQTT TTL persistence failed; runtime TTL still active"
                                        );
                                    }
                                }
                            }
                        }
                    }
                    let payload = serde_json::to_vec(
                        &serde_json::json!({"id":command.id,"source":command.source,"outcome":outcome,"on":state.relay.on,"persisted":persisted}),
                    )?;
                    client.publish(&ack_topic, QoS::AtLeastOnce, false, &payload)?;
                }
            }
        } else {
            // Do not replay input events or actuator acknowledgements after reconnect.
            runtime.input_events.lock().unwrap().clear();
            runtime.outcomes.lock().unwrap().clear();
        }
        runtime
            .health
            .progress(Service::Mqtt, (runtime.now_ms() / 1000) as u32);
        thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}
