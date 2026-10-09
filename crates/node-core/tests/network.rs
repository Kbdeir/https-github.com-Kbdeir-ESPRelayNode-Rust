use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::NaiveDateTime;
use node_core::{
    command::{Action, Source},
    config::Config,
    engine::Engine,
    management, modbus, protocol,
    schedule::Schedule,
    web,
};
use serde_json::json;

fn at(value: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S").unwrap()
}
#[test]
fn mqtt_rejects_retained_and_unsubscribed_commands() {
    let mut c = Config::default();
    c.relay.command_topic = "node/cmd".into();
    assert!(protocol::mqtt_command(&c.relay, "node/cmd", b"ON", true).is_err());
    assert!(protocol::mqtt_command(&c.relay, "node/state", b"ON", false).is_err());
    assert_eq!(
        protocol::mqtt_command(&c.relay, "node/cmd", b"tog", false)
            .unwrap()
            .source,
        Source::Mqtt
    );
}
#[test]
fn mqtt_checks_payload_and_timer_bounds() {
    let mut c = Config::default();
    c.relay.command_topic = "cmd".into();
    c.relay.ttl_command_topic = "ttl".into();
    assert!(protocol::mqtt_command(&c.relay, "cmd", &[b'x'; 513], false).is_err());
    assert!(protocol::mqtt_command(&c.relay, "ttl", b"604801", false).is_err());
    assert_eq!(
        protocol::mqtt_command(&c.relay, "ttl", b"60", false)
            .unwrap()
            .action,
        Action::SetTtl(60)
    );
}
#[test]
fn schedules_do_not_run_when_disabled() {
    assert!(!Schedule::default()
        .compile()
        .unwrap()
        .active(at("2026-10-09 00:00:30")));
}
#[test]
fn daily_overnight_anchors_to_start_day() {
    let s = Schedule {
        enabled: true,
        date_from: "2026-10-09".into(),
        date_to: "2026-10-09".into(),
        time_from: "22:00".into(),
        time_to: "06:00".into(),
        ..Schedule::default()
    }
    .compile()
    .unwrap();
    assert!(s.active(at("2026-10-10 05:59:59")));
    assert!(!s.active(at("2026-10-10 06:00:00")));
    assert!(!s.active(at("2026-10-09 01:00:00")));
}
#[test]
fn weekly_overnight_uses_previous_weekday() {
    let mut days = [false; 7];
    days[5] = true;
    let s = Schedule {
        enabled: true,
        kind: 3,
        weekdays: days,
        time_from: "23:00".into(),
        duration_minutes: 120,
        ..Schedule::default()
    }
    .compile()
    .unwrap();
    assert!(s.active(at("2026-10-10 00:30:00")));
    assert!(!s.active(at("2026-10-11 00:30:00")));
}
#[test]
fn monthly_skips_short_months() {
    let s = Schedule {
        enabled: true,
        kind: 4,
        month_day: 31,
        time_from: "08:00".into(),
        time_to: "09:00".into(),
        ..Schedule::default()
    }
    .compile()
    .unwrap();
    assert!(s.active(at("2026-10-31 08:30:00")));
    assert!(!s.active(at("2026-11-30 08:30:00")));
}
#[test]
fn specific_timer_spans_dates() {
    let s = Schedule {
        enabled: true,
        kind: 1,
        date_from: "2026-10-09".into(),
        date_to: "2026-10-10".into(),
        time_from: "22:00".into(),
        time_to: "01:00".into(),
        ..Schedule::default()
    }
    .compile()
    .unwrap();
    assert!(s.active(at("2026-10-10 00:59:59")));
    assert!(!s.active(at("2026-10-10 01:00:00")));
}
#[test]
fn malformed_schedules_are_rejected() {
    let mut s = Schedule {
        date_from: "2026-02-30".into(),
        ..Schedule::default()
    };
    assert!(s.validate().is_err());
    s = Schedule::default();
    s.enabled = true;
    s.kind = 3;
    assert!(s.validate().is_err());
    s.duration_minutes = u32::MAX;
    assert!(s.validate().is_err());
}
#[test]
fn settings_preserve_blank_secrets_and_validate_timers() {
    let mut c = Config::default();
    c.network.wifi_password = "secret123".into();
    let f =
        management::form(b"ssid=network&pass=&ntptz=4&MQTT_B_PRT=1883&MQTT_KeepAliveSeconds=30")
            .unwrap();
    let changed = management::update_config(&c, "/Apply.html", &f).unwrap();
    assert_eq!(changed.network.wifi_password, "secret123");
    let bad = management::form(b"ttl=604801").unwrap();
    assert!(management::update_config(&c, "/ApplyRelay.html", &bad).is_err());
}
#[test]
fn forms_reject_duplicate_keys() {
    assert!(management::form(b"ttl=10&ttl=20").is_err());
}
#[test]
fn passwords_never_appear_in_read_config() {
    let mut c = Config::default();
    c.network.wifi_password = "secret123".into();
    c.network.mqtt_password = "mqtt-secret".into();
    c.management.password = "web-secret".into();
    let value =
        management::legacy_config(&c, Default::default(), 0, "test", "test", "now").to_string();
    for secret in ["secret123", "mqtt-secret", "web-secret"] {
        assert!(!value.contains(secret));
    }
}
#[test]
fn template_escapes_untrusted_values() {
    let c = Config::default();
    let rendered = management::render(
        "%systemtime% %TNBT% %TIMER_RELAY_OPTIONS%",
        &c,
        1,
        Default::default(),
        "<script>",
        0,
        false,
    );
    assert!(!rendered.contains("<script>"));
    assert!(!rendered.contains("%TNBT%"));
}
#[test]
fn input_mode_zero_survives_json_save() {
    let mut value = json!({});
    for (i, key) in management::INPUT_TOPICS.iter().enumerate() {
        value[format!("I{}MODE", i + 1)] = json!(0);
        value[*key] = json!("");
    }
    let c = management::update_inputs(&Config::default(), &value).unwrap();
    assert_eq!(management::mode_number(&c.inputs[0].mode), 0);
}
#[test]
fn modbus_requires_allowlisted_ipv4_clients() {
    let mut c = Config::default();
    c.modbus.enabled = true;
    assert!(c.validate().is_err());
    c.modbus.allowed_clients.push("192.168.1.10".into());
    assert!(c.validate().is_ok());
    c.modbus.allowed_clients[0] = "any".into();
    assert!(c.validate().is_err());
}
#[test]
fn auth_requires_exact_credentials() {
    let mut c = Config::default();
    c.management.password = "test-password".into();
    let header = format!("Basic {}", STANDARD.encode("user:test-password"));
    assert!(web::authorized(Some(&header), &c));
    assert!(!web::authorized(Some("Basic invalid"), &c));
    assert!(!web::authorized(None, &c));
}
#[test]
fn modbus_relay_write_uses_engine_and_preserves_transaction() {
    let mut engine = Engine::new(&Config::default().relay);
    let request = [0x12, 0x34, 0, 0, 0, 6, 1, 5, 0, 0, 0xff, 0];
    let reply = modbus::respond(
        &request,
        1,
        &node_core::runtime::RuntimeSnapshot {
            relay: engine.snapshot(),
            ..Default::default()
        },
        |c| {
            assert_eq!(c.source, Source::Modbus);
            engine.apply(&c, 0);
            Ok(())
        },
    )
    .unwrap();
    assert!(engine.snapshot().on);
    assert_eq!(reply, request);
}
#[test]
fn modbus_rejects_bad_header_length_and_unit() {
    assert!(modbus::frame_length(&[0, 1, 0, 1, 0, 6, 1]).is_err());
    assert!(modbus::frame_length(&[0, 1, 0, 0, 1, 0, 1]).is_err());
    let mut request = [0, 1, 0, 0, 0, 6, 2, 1, 0, 0, 0, 1];
    assert!(modbus::respond(&request, 1, &Default::default(), |_| panic!()).is_err());
    request[6] = 1;
    assert!(modbus::respond(&request[..11], 1, &Default::default(), |_| panic!()).is_err());
}
#[test]
fn modbus_unknown_coil_returns_exception() {
    let request = [0, 1, 0, 0, 0, 6, 1, 1, 0, 1, 0, 1];
    let reply = modbus::respond(&request, 1, &Default::default(), |_| panic!()).unwrap();
    assert_eq!(&reply[7..], &[0x81, 2]);
}
#[test]
fn modbus_invalid_coil_value_cannot_actuate() {
    let request = [0, 1, 0, 0, 0, 6, 1, 5, 0, 0, 0, 1];
    let reply = modbus::respond(&request, 1, &Default::default(), |_| panic!()).unwrap();
    assert_eq!(&reply[7..], &[0x85, 3]);
}
#[test]
fn get_never_mutates_relay_or_configuration() {
    let (r, _) = node_core::runtime::Runtime::new(Config::default(), "test".into(), "test".into());
    assert_eq!(
        web::handle(
            &r,
            "GET",
            "/RelayConfig.html?RELAYACTION=ON",
            b"",
            |_, _| panic!()
        )
        .status,
        405
    );
    assert_eq!(
        web::handle(&r, "GET", "/Apply.html?ssid=x", b"", |_, _| panic!()).status,
        405
    );
}

#[test]
fn network_services_require_explicit_activation() {
    let mut c = Config::default();
    assert!(!c.management.enabled);
    assert!(!c.modbus.enabled);
    c.management.enabled = true;
    assert!(c.validate().is_err());
    c.management.password = "long-private-password".into();
    c.modbus.enabled = true;
    c.modbus.allowed_clients = vec!["*".into()];
    assert!(c.validate().is_ok());
}

#[test]
fn modbus_ttl_write_is_atomic_and_bounded() {
    let request = [0, 1, 0, 0, 0, 11, 1, 16, 0, 1, 0, 2, 4, 0, 1, 0, 0];
    let mut calls = 0;
    let reply = modbus::respond(&request, 1, &Default::default(), |c| {
        calls += 1;
        assert_eq!(c.action, Action::SetTtl(65536));
        assert_eq!(c.source, Source::Modbus);
        Ok(())
    })
    .unwrap();
    assert_eq!(calls, 1);
    assert_eq!(&reply[7..], &[16, 0, 1, 0, 2]);
    let mut too_large = request;
    too_large[13..].copy_from_slice(&604801u32.to_be_bytes());
    let reply = modbus::respond(&too_large, 1, &Default::default(), |_| panic!()).unwrap();
    assert_eq!(&reply[7..], &[0x90, 3]);
    let half = [0, 1, 0, 0, 0, 6, 1, 6, 0, 1, 0, 1];
    let reply = modbus::respond(&half, 1, &Default::default(), |_| panic!()).unwrap();
    assert_eq!(&reply[7..], &[0x86, 2]);
}

#[test]
fn modbus_map_reports_version_and_validity() {
    let snapshot = node_core::runtime::RuntimeSnapshot {
        input_valid_mask: 0x3f,
        ..Default::default()
    };
    let request = [0, 1, 0, 0, 0, 6, 1, 3, 0, 0, 0, 7];
    let reply = modbus::respond(&request, 1, &snapshot, |_| panic!()).unwrap();
    assert_eq!(&reply[7..11], &[3, 14, 0, 1]);
    assert_eq!(&reply[21..23], &[0, 0x3f]);
    let write = [0, 1, 0, 0, 0, 6, 1, 6, 0, 0, 0, 2];
    let reply = modbus::respond(&write, 1, &snapshot, |_| panic!()).unwrap();
    assert_eq!(&reply[7..], &[0x86, 2]);
}

#[test]
fn failed_config_commit_leaves_active_config_untouched() {
    let (r, _) = node_core::runtime::Runtime::new(Config::default(), "test".into(), "test".into());
    let reply = web::handle(&r, "POST", "/Apply.html", b"ssid=candidate", |_, _| {
        Err("NVS failure".into())
    });
    assert_eq!(reply.status, 500);
    assert_eq!(r.config.lock().unwrap().network.ssid, "");
    assert_eq!(r.restart_at.load(std::sync::atomic::Ordering::Relaxed), 0);
}

#[test]
fn invalid_http_actions_never_enqueue_commands() {
    let (r, queue) =
        node_core::runtime::Runtime::new(Config::default(), "test".into(), "test".into());
    assert_eq!(
        web::handle(
            &r,
            "POST",
            "/api/relay",
            br#"{"action":"invalid"}"#,
            |_, _| panic!()
        )
        .status,
        400
    );
    assert_eq!(
        web::handle(
            &r,
            "GET",
            "/TimerControl.json?action=pause",
            b"",
            |_, _| panic!()
        )
        .status,
        405
    );
    assert!(queue.try_recv().is_err());
}

#[test]
fn all_read_api_responses_redact_passwords() {
    let mut c = Config::default();
    c.network.wifi_password = "private-wifi-secret".into();
    c.network.mqtt_password = "private-mqtt-secret".into();
    c.management.password = "private-web-secret".into();
    let (r, _) = node_core::runtime::Runtime::new(c, "test".into(), "test".into());
    *r.files.lock().unwrap() = Some(std::sync::Arc::new(node_core::filesystem::FileStore::new(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../web"),
        None,
    )));
    for path in [
        "/GetConfig.json",
        "/InputsConfig.json",
        "/api/status",
        "/api/live",
        "/api/modbus",
        "/MqttStatus.json",
        "/config.html",
    ] {
        let reply = web::handle(&r, "GET", path, b"", |_, _| panic!());
        assert_eq!(reply.status, 200);
        let body = String::from_utf8(reply.body.into_bytes()).unwrap();
        for secret in [
            "private-wifi-secret",
            "private-mqtt-secret",
            "private-web-secret",
        ] {
            assert!(!body.contains(secret), "secret exposed at {path}");
        }
    }
}

#[test]
fn timer_countdown_and_overlapping_windows() {
    let first = Schedule {
        enabled: true,
        time_from: "08:00".into(),
        time_to: "10:00".into(),
        ..Schedule::default()
    }
    .compile()
    .unwrap();
    let second = Schedule {
        enabled: true,
        time_from: "09:00".into(),
        time_to: "11:00".into(),
        ..Schedule::default()
    }
    .compile()
    .unwrap();
    assert_eq!(first.countdown(at("2026-10-09 07:59:00")).0, 60);
    assert_eq!(first.countdown(at("2026-10-09 09:59:00")).1, 60);
    let now = at("2026-10-09 10:00:00");
    assert!(!first.active(now));
    assert!(second.active(now));
    assert!([first, second].iter().any(|timer| timer.active(now)));
}
