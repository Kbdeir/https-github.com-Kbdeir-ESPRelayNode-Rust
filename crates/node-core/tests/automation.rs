use node_core::{
    automation::*, command::Action, config::Config, management, runtime::RuntimeSnapshot, web,
};

fn fixture() -> (Config, RuntimeSnapshot, [RemoteReading; 8]) {
    let mut c = Config::default();
    c.remote_sensors[0].topic = "test/value".into();
    c.automation[0] = Rule {
        enabled: true,
        conditions: [
            Condition {
                source: Source::Remote1,
                op: Operator::Greater,
                value1: 10.0,
                ..Default::default()
            },
            Condition::default(),
        ],
        ..Default::default()
    };
    (
        c,
        RuntimeSnapshot::default(),
        [RemoteReading {
            value: 20.0,
            seen_ms: Some(0),
        }; 8],
    )
}
fn tick(
    a: &mut Automation,
    c: &Config,
    s: &RuntimeSnapshot,
    r: &[RemoteReading; 8],
    now: u64,
) -> Step {
    a.tick(
        now,
        c,
        &Sources {
            snapshot: s,
            remote: r,
            gates: s.timers,
        },
    )[0]
}
#[test]
fn change_rules_fire_both_edges_once() {
    let (c, s, mut r) = fixture();
    let mut a = Automation::default();
    assert_eq!(tick(&mut a, &c, &s, &r, 0).action, Some(Action::On));
    a.accepted(0, 0);
    assert_eq!(tick(&mut a, &c, &s, &r, 1000).action, None);
    r[0].value = 0.0;
    assert_eq!(tick(&mut a, &c, &s, &r, 2000).action, Some(Action::Off));
    a.accepted(0, 2000);
    assert_eq!(tick(&mut a, &c, &s, &r, 3000).action, None);
}

#[test]
fn live_save_preserves_unchanged_rules_edges_hold_and_cooldown() {
    let (c, s, r) = fixture();
    let mut a = Automation::default();
    assert_eq!(tick(&mut a, &c, &s, &r, 0).action, Some(Action::On));
    a.accepted(0, 0);
    let mut updated = c.clone();
    updated.location = "new location".into();
    updated.automation[1].hold = 5;
    updated.remote_sensors[0].label = "new display label".into();
    assert_eq!(a.configure(&updated, &c), [false, true, false, false]);
    assert_eq!(tick(&mut a, &updated, &s, &r, 1000).action, None);
}

#[test]
fn live_remote_source_change_reinitializes_only_dependent_rules() {
    let (c, s, r) = fixture();
    let mut a = Automation::default();
    tick(&mut a, &c, &s, &r, 0);
    a.accepted(0, 0);
    let mut updated = c.clone();
    updated.remote_sensors[0].topic = "different/value".into();
    assert_eq!(a.configure(&updated, &c), [true, false, false, false]);
    assert_eq!(
        tick(&mut a, &updated, &s, &r, 1000).action,
        Some(Action::On)
    );
}
#[test]
fn hold_resets_on_bounce_and_cooldown_spans_edges() {
    let (mut c, s, mut r) = fixture();
    c.automation[0].hold = 2;
    c.automation[0].cooldown = 5;
    let mut a = Automation::default();
    assert_eq!(tick(&mut a, &c, &s, &r, 0).action, None);
    r[0].value = 0.0;
    tick(&mut a, &c, &s, &r, 1000);
    r[0].value = 20.0;
    tick(&mut a, &c, &s, &r, 1500);
    assert_eq!(tick(&mut a, &c, &s, &r, 3499).action, None);
    assert_eq!(tick(&mut a, &c, &s, &r, 3500).action, Some(Action::On));
    a.accepted(0, 3500);
    r[0].value = 0.0;
    tick(&mut a, &c, &s, &r, 4000);
    assert_eq!(tick(&mut a, &c, &s, &r, 6000).action, None);
    assert_eq!(tick(&mut a, &c, &s, &r, 8500).action, Some(Action::Off));
}
#[test]
fn periodic_cooldown_is_bounded_and_toggle_rejected() {
    let (mut c, s, r) = fixture();
    c.automation[0].trigger = Trigger::Periodic;
    c.automation[0].cooldown = 2;
    let mut a = Automation::default();
    assert_eq!(tick(&mut a, &c, &s, &r, 0).action, Some(Action::On));
    a.accepted(0, 0);
    assert_eq!(tick(&mut a, &c, &s, &r, 1999).action, None);
    assert_eq!(tick(&mut a, &c, &s, &r, 2000).action, Some(Action::On));
    c.automation[0].action = RuleAction::Toggle;
    assert!(c.validate().is_err());
    c.automation[0].action = RuleAction::On;
    c.automation[0].cooldown = 0;
    assert!(c.validate().is_err());
}
#[test]
fn stale_and_missing_sources_cancel_pending_without_local_action() {
    let (c, s, r) = fixture();
    let mut a = Automation::default();
    assert!(tick(&mut a, &c, &s, &r, 300000).action.is_some());
    let step = tick(&mut a, &c, &s, &r, 300001);
    assert!(step.cancel_remote);
    assert!(!step.permit_remote);
    assert_eq!(step.action, None);
    assert!(!a.status()[0].available);
}
#[test]
fn both_sources_required_even_with_or() {
    let (mut c, s, r) = fixture();
    c.automation[0].logic = Logic::Or;
    c.automation[0].conditions[1].source = Source::Temperature1;
    let mut a = Automation::default();
    assert_eq!(tick(&mut a, &c, &s, &r, 100).action, None);
    assert!(!a.status()[0].available);
}
#[test]
fn between_equal_and_timer_gate_follow_reference() {
    let (mut c, mut s, r) = fixture();
    c.automation[0].conditions[0] = Condition {
        source: Source::Remote1,
        op: Operator::Between,
        value1: 20.0,
        value2: 30.0,
    };
    c.automation[0].gate = 1;
    let mut a = Automation::default();
    assert_eq!(tick(&mut a, &c, &s, &r, 0).action, Some(Action::Off));
    s.timers[0] = true;
    assert_eq!(tick(&mut a, &c, &s, &r, 1000).action, Some(Action::On));
    c.automation[0].conditions[0].op = Operator::Equal;
    assert_eq!(
        tick(&mut Automation::default(), &c, &s, &r, 0).action,
        Some(Action::On)
    );
    c.automation[0].conditions[0].value2 = 19.0;
    c.automation[0].conditions[0].op = Operator::Between;
    assert!(c.validate().is_err());
}
#[test]
fn remote_clear_policy_cancels_old_demand() {
    let (mut c, s, mut r) = fixture();
    c.automation[0].modbus = Some(CoilTarget {
        host: "192.168.1.2".into(),
        send_clear: false,
        ..Default::default()
    });
    let mut a = Automation::default();
    assert_eq!(tick(&mut a, &c, &s, &r, 0).action, Some(Action::On));
    a.accepted(0, 0);
    r[0].value = 0.0;
    let step = tick(&mut a, &c, &s, &r, 1000);
    assert!(step.cancel_remote);
    assert_eq!(step.action, None);
    c.automation[0].modbus.as_mut().unwrap().send_clear = true;
    assert_eq!(tick(&mut a, &c, &s, &r, 2000).action, Some(Action::Off));
    c.automation[0].action = RuleAction::Toggle;
    assert!(c.validate().is_err());
}
#[test]
fn mqtt_scalars_json_and_resource_bounds() {
    for p in [b"on".as_slice(), b"true", b"yes", br#""ON""#] {
        assert_eq!(remote_payload(p, ""), Ok(1.0));
    }
    assert_eq!(
        remote_payload(br#"{"temperature":"21.5"}"#, "temperature"),
        Ok(21.5)
    );
    for p in [
        b"NaN".as_slice(),
        b"inf",
        br#"{"value":[]} "#,
        b"not-a-number",
    ] {
        assert!(remote_payload(p, "value").is_err());
    }
    assert!(remote_payload(&[b'1'; 513], "").is_err());
    assert!(remote_payload(b"[[[[[[[[[1]]]]]]]]]", "").is_err());
    assert!(remote_payload(br#"{"bad":1}"#, "missing").is_err());
}
#[test]
fn modbus_types_orders_scaling_and_nonfinite() {
    let mut m = RemoteModbus {
        value_type: ValueType::F32,
        ..Default::default()
    };
    for (order, words) in [
        (0, [0x41a0, 0]),
        (1, [0, 0x41a0]),
        (2, [0xa041, 0]),
        (3, [0, 0xa041]),
    ] {
        m.word_order = order;
        assert_eq!(m.decode(&words), Ok(20.0));
    }
    m.word_order = 0;
    m.scale = 2.0;
    m.offset = 1.0;
    assert_eq!(m.decode(&[0x41a0, 0]), Ok(41.0));
    assert!(m.decode(&[0x7fc0, 0]).is_err());
    m.value_type = ValueType::I16;
    m.scale = 1.0;
    m.offset = 0.0;
    assert_eq!(m.decode(&[0xffff]), Ok(-1.0));
    m.value_type = ValueType::I32;
    assert_eq!(m.decode(&[0xffff, 0xfffe]), Ok(-2.0));
    m.value_type = ValueType::U32;
    assert_eq!(m.decode(&[1, 1]), Ok(65537.0));
}
#[test]
fn remote_configuration_validation_and_forms() {
    let mut c = Config::default();
    let f = management::form(
        b"id=1&transport=1&host=192.168.1.2&function=3&valueType=5&scale=2&offset=-1",
    )
    .unwrap();
    c = management::update_automation(&c, "/api/automation/remote", &f).unwrap();
    assert_eq!(c.remote_sensors[0].modbus.value_type, ValueType::F32);
    let f = management::form(b"id=1&enabled=1&c0source=20&c0op=3&c0value1=on").unwrap();
    c = management::update_automation(&c, "/api/automation/rule", &f).unwrap();
    assert_eq!(c.automation[0].conditions[0].value1, 1.0);
    let f = management::form(b"id=9&transport=0").unwrap();
    assert!(management::update_automation(&c, "/api/automation/remote", &f).is_err());
    c.remote_sensors[0].modbus.function = 1;
    assert!(c.validate().is_err());
    assert!(
        serde_json::to_vec(&Config::default()).unwrap().len() < node_core::config::MAX_CONFIG_BYTES
    );
    assert!(serde_json::from_str::<Condition>(r#"{"value1":1e100}"#).is_err());
}
#[test]
fn large_pages_stream_without_materializing_a_response() {
    let (c, s, _) = fixture();
    let (r, _) = node_core::runtime::Runtime::new(c, "test".into(), "test".into());
    *r.files.lock().unwrap() = Some(std::sync::Arc::new(node_core::filesystem::FileStore::new(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../web"),
        None,
    )));
    let reply = web::handle(&r, "GET", "/", b"", |_, _| panic!());
    assert!(matches!(reply.body, web::Body::Asset(_)));
    let mut bytes = 0;
    reply
        .body
        .write_to(|chunk| {
            assert!(chunk.len() <= 768);
            bytes += chunk.len();
            Ok::<_, std::io::Error>(())
        })
        .unwrap();
    assert!(bytes > 32000);
    let context = management::RenderContext {
        config: &Config::default(),
        slot: 1,
        state: s.relay,
        time: "<script>%RSTATE%",
        uptime: 0,
        active: false,
    };
    let mut text = Vec::new();
    management::render_stream("width:50%; %UNKNOWN% %systemtime% %TNBT%", &context, |v| {
        text.extend_from_slice(v);
        Ok::<_, ()>(())
    })
    .unwrap();
    assert_eq!(
        String::from_utf8(text).unwrap(),
        "width:50%; %UNKNOWN% &lt;script&gt;%RSTATE% 1"
    );
}
#[test]
fn streaming_stops_on_transport_failure() {
    let c = Config::default();
    let context = management::RenderContext {
        config: &c,
        slot: 1,
        state: Default::default(),
        time: "now",
        uptime: 0,
        active: false,
    };
    let mut calls = 0;
    assert!(management::render_stream(&"x".repeat(4000), &context, |_| {
        calls += 1;
        Err::<(), _>("disconnected")
    })
    .is_err());
    assert_eq!(calls, 1);
}

#[test]
fn nesting_limits_preserve_quoted_braces() {
    assert!(node_core::config::check_json_depth(br#"{"label":"[[[[[["}"#, 1).is_ok());
    let body = "[".repeat(17) + "0" + &"]".repeat(17);
    assert!(Config::parse(body.as_bytes()).is_err());
    let (r, _) = node_core::runtime::Runtime::new(Config::default(), "test".into(), "test".into());
    assert_eq!(
        web::handle(&r, "POST", "/api/config", body.as_bytes(), |_, _| panic!()).status,
        400
    );
}
#[test]
fn automation_saves_are_transactional_and_start_disabled() {
    let (r, _) = node_core::runtime::Runtime::new(Config::default(), "test".into(), "test".into());
    let reply = web::handle(
        &r,
        "POST",
        "/api/automation/rule",
        b"id=1&enabled=1&c0source=7&c0op=3&c0value1=1",
        |_, _| Err("NVS failed".into()),
    );
    assert_eq!(reply.status, 500);
    assert!(!r.config.lock().unwrap().automation[0].enabled);
    let reply = web::handle(&r, "GET", "/api/automation", b"", |_, _| panic!());
    let value: serde_json::Value = serde_json::from_slice(&reply.body.into_bytes()).unwrap();
    assert_eq!(value["rules"].as_array().unwrap().len(), 4);
    assert_eq!(value["remotes"].as_array().unwrap().len(), 8);
    assert_eq!(value["rules"][0]["enabled"], false);
}

#[test]
fn runtime_local_off_and_interlock_reject_rule_on_without_consuming_edge() {
    let (c, s, values) = fixture();
    let (r, _) = node_core::runtime::Runtime::new(c.clone(), "test".into(), "test".into());
    *r.remote_values.lock().unwrap() = values;
    let mut engine = node_core::engine::Engine::new(&c.relay);
    let mut a = Automation::default();
    r.automation_tick(&mut a, &c, &mut engine, &s, true);
    assert!(!engine.snapshot().on);
    engine.apply(
        &node_core::protocol::command(Action::EmergencyOff, node_core::command::Source::LocalInput),
        r.now_ms(),
    );
    r.automation_tick(&mut a, &c, &mut engine, &s, false);
    assert!(!engine.snapshot().on);
    engine.apply(
        &node_core::protocol::command(
            Action::ClearInterlock,
            node_core::command::Source::LocalInput,
        ),
        r.now_ms(),
    );
    r.automation_tick(&mut a, &c, &mut engine, &s, false);
    assert!(engine.snapshot().on);
}

#[test]
fn runtime_coalesces_remote_demand_and_cancels_it_on_missing_source_or_restart() {
    use std::sync::atomic::Ordering;
    let (mut c, s, values) = fixture();
    c.automation[0].modbus = Some(CoilTarget {
        host: "192.168.1.2".into(),
        ..Default::default()
    });
    let (r, _) = node_core::runtime::Runtime::new(c.clone(), "test".into(), "test".into());
    *r.remote_values.lock().unwrap() = values;
    let mut engine = node_core::engine::Engine::new(&c.relay);
    let mut a = Automation::default();
    r.automation_tick(&mut a, &c, &mut engine, &s, false);
    let first = r.remote_writes.lock().unwrap()[0];
    assert!(first.pending && first.on);
    assert!(!engine.snapshot().on);
    r.automation_tick(&mut a, &c, &mut engine, &s, false);
    assert_eq!(first.revision, r.remote_writes.lock().unwrap()[0].revision);
    r.remote_values.lock().unwrap()[0].seen_ms = None;
    r.automation_tick(&mut a, &c, &mut engine, &s, false);
    assert!(!r.remote_writes.lock().unwrap()[0].pending);
    assert_ne!(first.revision, r.write_revision[0].load(Ordering::Acquire));
    assert_eq!(r.write_permit_until[0].load(Ordering::Acquire), 0);
    *r.remote_values.lock().unwrap() = values;
    r.automation_tick(&mut a, &c, &mut engine, &s, false);
    assert!(r.remote_writes.lock().unwrap()[0].pending);
    r.restart_at.store(1, Ordering::Release);
    r.automation_tick(&mut a, &c, &mut engine, &s, false);
    assert!(!r.remote_writes.lock().unwrap()[0].pending);
    assert_eq!(r.write_permit_until[0].load(Ordering::Acquire), 0);
}
