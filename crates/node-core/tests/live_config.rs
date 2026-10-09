use node_core::{
    command::{Action, Command},
    config::{Config, InputMode},
    engine::Engine,
    health::{Health, Service},
    runtime::Runtime,
    web,
};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;

fn runtime() -> std::sync::Arc<Runtime> {
    let mut c = Config::default();
    c.network.ssid = "test-network".into();
    c.management.password = "test-password".into();
    Runtime::new(c, "abcdef".into(), "00:00:00:ab:cd:ef".into()).0
}

fn save(r: &std::sync::Arc<Runtime>, path: &str, body: &[u8]) -> Value {
    let reply = web::handle(r, "POST", path, body, |_, _| Ok(()));
    let status = reply.status;
    let value: Value = serde_json::from_slice(&reply.body.into_bytes()).unwrap();
    assert_eq!(status, 200, "{path}: {value}");
    assert_eq!(value["restart"], false);
    assert_eq!(r.restart_at.load(Ordering::Acquire), 0);
    value
}

#[test]
fn system_relay_mapping_inputs_timers_rules_and_remotes_save_without_reboot() {
    let r = runtime();
    for (path, body) in [
        (
            "/Apply.html",
            "PhyLoc=updated&timeserver=192.168.1.2&ntptz=2",
        ),
        (
            "/ApplyRelay.html",
            "RELAYNB=0&ttl=60&tta=10&PUB_TOPIC1=new%2Fcommand",
        ),
        (
            "/InputsConfig.json",
            r#"{"I1MODE":3,"I2MODE":0,"I3MODE":0,"I4MODE":0,"I5MODE":0,"I6MODE":0,"I12_STS_PTP":"new/input","I14_STS_PTP":"","I03_STS_PTP":"","I04_STS_PTP":"","I05_STS_PTP":"","I06_STS_PTP":""}"#,
        ),
        ("/ApplyIRMap.html", "I1=33&R1=0"),
        (
            "/savetimer.html",
            "TNumber=1&TRelay=0&TMTYPEedit=2&TFrom=08%3A00&TTo=09%3A00",
        ),
        (
            "/api/automation/rule",
            "id=1&enabled=0&c0source=7&c0value1=1",
        ),
        (
            "/api/automation/remote",
            "id=1&transport=0&topic=new%2Fsensor",
        ),
    ] {
        save(&r, path, body.as_bytes());
    }
    let mut revision = 0;
    let c = r.config_update(&mut revision).unwrap();
    assert_eq!(c.location, "updated");
    assert_eq!(c.relay.ttl_seconds, 60);
    assert_eq!(c.inputs[0].mode, InputMode::RelayToggle);
    assert_eq!(c.inputs[0].relay, Some(0));
    assert_eq!(c.timers[0].time_from, "08:00");
    assert!(r.config_update(&mut revision).is_none());
}

#[test]
fn mqtt_credentials_enable_disable_and_modbus_changes_notify_their_workers() {
    let r = runtime();
    save(&r, "/Apply.html", b"MQTT_Active=1&MQTT_BROKER=192.168.1.2&MQTT_B_PRT=1884&mqttUser=operator&mqttPass=new-password");
    assert_eq!(r.mqtt_revision.load(Ordering::Acquire), 1);
    assert_eq!(r.modbus_revision.load(Ordering::Acquire), 0);
    save(
        &r,
        "/api/modbus",
        &serde_json::to_vec(&json!({
            "enabled":true,"port":1502,"unit":5,"allowed_clients":["*"],"peers":[]
        }))
        .unwrap(),
    );
    assert_eq!(r.modbus_revision.load(Ordering::Acquire), 1);
    save(&r, "/Apply.html", b"PhyLoc=updated");
    assert_eq!(r.mqtt_revision.load(Ordering::Acquire), 2);
    assert!(!r.config.lock().unwrap().network.mqtt_enabled);
}

#[test]
fn wifi_changes_still_schedule_a_safe_reboot_but_unchanged_wifi_does_not() {
    let r = runtime();
    save(&r, "/Apply.html", b"ssid=test-network&pass=&PhyLoc=changed");
    let reply = web::handle(
        &r,
        "POST",
        "/Apply.html",
        b"ssid=new-network&pass=new-password",
        |_, _| Ok(()),
    );
    assert_eq!(reply.status, 200);
    let value: Value = serde_json::from_slice(&reply.body.into_bytes()).unwrap();
    assert_eq!(value["restart"], true);
    assert!(r.restart_at.load(Ordering::Acquire) > 0);
}

#[test]
fn failed_persistence_does_not_notify_workers_or_change_active_config() {
    let r = runtime();
    let old = r.config.lock().unwrap().clone();
    let reply = web::handle(
        &r,
        "POST",
        "/ApplyRelay.html",
        b"ttl=40&PUB_TOPIC1=new%2Fcommand",
        |_, _| Err("NVS failed".into()),
    );
    assert_eq!(reply.status, 500);
    assert!(old == *r.config.lock().unwrap());
    assert_eq!(r.config_revision.load(Ordering::Acquire), 0);
    assert_eq!(r.mqtt_revision.load(Ordering::Acquire), 0);
    assert_eq!(r.modbus_revision.load(Ordering::Acquire), 0);
}

#[test]
fn typed_settings_and_web_credentials_apply_without_restart() {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let r = runtime();
    let mut c = r.config.lock().unwrap().clone();
    c.management.username = "new-user".into();
    c.management.password = "new-password".into();
    c.location = "typed update".into();
    save(&r, "/api/config", &serde_json::to_vec(&c).unwrap());
    let old = format!("Basic {}", STANDARD.encode("user:test-password"));
    let new = format!("Basic {}", STANDARD.encode("new-user:new-password"));
    let current = r.config.lock().unwrap();
    assert!(!web::authorized(Some(&old), &current));
    assert!(web::authorized(Some(&new), &current));
    assert_eq!(current.location, "typed update");
    assert_eq!(r.mqtt_revision.load(Ordering::Acquire), 0);
    assert_eq!(r.modbus_revision.load(Ordering::Acquire), 0);
}

#[test]
fn controller_refresh_never_waits_for_the_settings_lock() {
    let r = runtime();
    save(&r, "/ApplyRelay.html", b"ttl=40");
    let held = r.config.lock().unwrap();
    let mut revision = 0;
    assert!(r.config_update(&mut revision).is_none());
    assert_eq!(revision, 0);
    drop(held);
    assert_eq!(
        r.config_update(&mut revision).unwrap().relay.ttl_seconds,
        40
    );
}

#[test]
fn live_ttl_change_preserves_on_time_and_expires_immediately_if_overdue() {
    let mut c = Config::default();
    c.relay.ttl_seconds = 30;
    let mut e = Engine::new(&c.relay);
    e.apply(&Command::local(Action::On), 1000);
    c.relay.ttl_seconds = 20;
    e.configure(&c.relay, 11_000);
    assert!(e.snapshot().on);
    assert_eq!(e.snapshot().ttl_elapsed_seconds, 10);
    assert_eq!(e.snapshot().ttl_remaining_seconds, 10);
    c.relay.ttl_seconds = 5;
    e.configure(&c.relay, 11_000);
    assert!(!e.snapshot().on);
}

#[test]
fn live_tta_change_preserves_delay_start_and_never_clears_interlock() {
    let mut c = Config::default();
    c.relay.tta_seconds = 20;
    let mut e = Engine::new(&c.relay);
    e.apply(&Command::local(Action::DelayedOn), 1000);
    c.relay.tta_seconds = 10;
    e.configure(&c.relay, 6000);
    assert_eq!(e.snapshot().tta_remaining_seconds, 5);
    e.apply(&Command::local(Action::EmergencyOff), 7000);
    c.relay.tta_seconds = 0;
    e.configure(&c.relay, 9000);
    assert!(e.snapshot().interlocked);
    assert!(!e.snapshot().on);
}

#[test]
fn topic_only_saves_do_not_change_runtime_ttl_or_reboot() {
    let r = runtime();
    let mut c = r.config.lock().unwrap().clone();
    c.relay.ttl_seconds = 60;
    let mut e = Engine::new(&c.relay);
    e.apply(&Command::local(Action::SetTtl(120)), 0);
    let old = c.clone();
    c.relay.command_topic = "new/command".into();
    assert_eq!(c.relay.ttl_seconds, old.relay.ttl_seconds);
    assert!(c.mqtt_changed(&old));
    assert!(!c.wifi_changed(&old));
    assert_eq!(e.ttl_seconds(), 120);
    c.relay.tta_seconds = 10;
    e.configure_changed(&c.relay, &old.relay, 1000);
    assert_eq!(
        e.ttl_seconds(),
        120,
        "TTA edit must preserve a runtime Modbus TTL override"
    );
}

#[test]
fn remote_source_save_invalidates_old_readings_and_write_leases() {
    let r = runtime();
    r.config.lock().unwrap().automation[0].conditions[0].source =
        node_core::automation::Source::Remote1;
    r.remote_values.lock().unwrap()[0] = node_core::automation::RemoteReading {
        value: 1.0,
        seen_ms: Some(0),
    };
    r.write_permit_until[0].store(100, Ordering::Release);
    save(
        &r,
        "/api/automation/remote",
        b"id=1&transport=0&topic=changed%2Fsensor",
    );
    assert!(r.remote_values.lock().unwrap()[0].seen_ms.is_none());
    assert_eq!(r.write_permit_until[0].load(Ordering::Acquire), 0);
    assert!(r.write_revision[0].load(Ordering::Acquire) > 0);
}

#[test]
fn planned_worker_retirement_is_not_a_watchdog_failure() {
    let h = Health::default();
    for _ in 0..20 {
        let guard = h.watch(Service::Mqtt, 100);
        guard.finish();
        assert_eq!(h.fault(1000), None);
        assert!(!h.status(1000)[3].failed);
    }
    let _running = h.watch(Service::Mqtt, 1000);
    assert_eq!(h.fault(1059), None);
    assert_eq!(h.fault(1060), Some(Service::Mqtt));
}

#[test]
fn reboot_requires_post_and_cannot_delay_an_existing_restart() {
    let r = runtime();
    assert_eq!(
        web::handle(&r, "GET", "/api/reboot", b"", |_, _| Ok(())).status,
        405
    );
    assert_eq!(r.restart_at.load(Ordering::Acquire), 0);
    assert_eq!(
        web::handle(&r, "POST", "/api/reboot", b"", |_, _| Ok(())).status,
        200
    );
    let deadline = r.restart_at.load(Ordering::Acquire);
    assert!(deadline > 0);
    assert_eq!(
        web::handle(&r, "POST", "/api/reboot", b"", |_, _| Ok(())).status,
        200
    );
    assert_eq!(r.restart_at.load(Ordering::Acquire), deadline);
}
