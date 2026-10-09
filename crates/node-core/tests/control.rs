use node_core::{
    command::{Action, Command, Outcome, Source},
    config::{Config, InputConfig, InputMode, RelayConfig, MAX_TIMER_SECONDS},
    engine::Engine,
    input::{input_event, Debouncer},
};
use serde_json::json;

fn engine(ttl: u32, tta: u32) -> Engine {
    Engine::new(&RelayConfig {
        ttl_seconds: ttl,
        tta_seconds: tta,
        ..Default::default()
    })
}

fn command(action: Action) -> Command {
    Command::local(action)
}

#[test]
fn boot_is_off_without_starting_timers() {
    let mut engine = engine(30, 5);
    engine.tick(60_000);
    assert!(!engine.snapshot().on);
    assert_eq!(engine.snapshot().ttl_remaining_seconds, 0);
    assert_eq!(engine.snapshot().tta_remaining_seconds, 0);
}

#[test]
fn ttl_turns_off_at_deadline_even_without_network_events() {
    let mut engine = engine(2, 0);
    engine.apply(&command(Action::On), 100);
    engine.tick(2099);
    assert!(engine.snapshot().on);
    engine.tick(2100);
    assert!(!engine.snapshot().on);
}

#[test]
fn repeated_on_does_not_extend_ttl() {
    let mut engine = engine(2, 0);
    engine.apply(&command(Action::On), 0);
    engine.apply(&command(Action::On), 1500);
    engine.tick(2000);
    assert!(!engine.snapshot().on);
}

#[test]
fn zero_ttl_does_not_expire() {
    let mut engine = engine(0, 0);
    engine.apply(&command(Action::On), 0);
    engine.tick(1_000_000);
    assert!(engine.snapshot().on);
}

#[test]
fn tta_delays_input_activation_then_starts_ttl() {
    let mut engine = engine(2, 3);
    engine.apply(&command(Action::DelayedOn), 0);
    engine.tick(2999);
    assert!(!engine.snapshot().on);
    engine.tick(3000);
    assert!(engine.snapshot().on);
    assert_eq!(engine.snapshot().ttl_remaining_seconds, 2);
    engine.tick(5000);
    assert!(!engine.snapshot().on);
}

#[test]
fn late_tick_cannot_extend_tta_or_ttl() {
    let mut engine = engine(2, 3);
    engine.apply(&command(Action::DelayedOn), 0);
    engine.tick(7000);
    assert!(!engine.snapshot().on);
}

#[test]
fn off_cancels_pending_activation() {
    let mut engine = engine(2, 3);
    engine.apply(&command(Action::DelayedOn), 0);
    engine.apply(&command(Action::Off), 1000);
    engine.tick(3000);
    assert!(!engine.snapshot().on);
}

#[test]
fn zero_tta_is_immediate() {
    let mut engine = engine(0, 0);
    engine.apply(&command(Action::DelayedOn), 10);
    assert!(engine.snapshot().on);
}

#[test]
fn mqtt_on_bypasses_physical_input_tta() {
    let mut engine = engine(0, 30);
    let mut request = command(Action::On);
    request.source = Source::Mqtt;
    engine.apply(&request, 0);
    assert!(engine.snapshot().on);
}

#[test]
fn duplicate_toggle_does_not_reexecute() {
    let mut engine = engine(0, 0);
    let mut request = command(Action::Toggle);
    request.source = Source::Mqtt;
    request.id = Some(42);
    assert_eq!(engine.apply(&request, 0), Outcome::Applied);
    assert_eq!(engine.apply(&request, 20), Outcome::Duplicate);
    assert!(engine.snapshot().on);
}

#[test]
fn command_id_namespaces_are_separate_by_source() {
    let mut engine = engine(0, 0);
    let mut request = command(Action::Toggle);
    request.id = Some(42);
    engine.apply(&request, 0);
    request.source = Source::Http;
    assert_eq!(engine.apply(&request, 10), Outcome::Applied);
    assert!(!engine.snapshot().on);
}

#[test]
fn expired_and_invalid_commands_do_not_change_output() {
    let mut engine = engine(0, 0);
    let mut request = command(Action::On);
    request.expires_at_ms = Some(10);
    assert_eq!(engine.apply(&request, 10), Outcome::Expired);
    request.expires_at_ms = None;
    request.relay = 1;
    assert_eq!(engine.apply(&request, 11), Outcome::InvalidRelay);
    assert!(!engine.snapshot().on);
}

#[test]
fn interlock_cancels_timers_and_blocks_activation() {
    let mut engine = engine(5, 5);
    engine.apply(&command(Action::DelayedOn), 0);
    engine.apply(&command(Action::EmergencyOff), 100);
    assert_eq!(
        engine.apply(&command(Action::On), 200),
        Outcome::Interlocked
    );
    engine.tick(10_000);
    assert!(!engine.snapshot().on);
    engine.apply(&command(Action::ClearInterlock), 10_000);
    assert!(!engine.snapshot().on);
}

#[test]
fn remote_source_cannot_clear_interlocks() {
    let mut engine = engine(0, 0);
    engine.apply(&command(Action::EmergencyOff), 0);
    let mut request = command(Action::ClearInterlock);
    request.source = Source::Http;
    assert_eq!(engine.apply(&request, 10), Outcome::Unauthorized);
    assert!(engine.snapshot().interlocked);
}

#[test]
fn changing_ttl_does_not_restart_elapsed_time() {
    let mut engine = engine(10, 0);
    engine.apply(&command(Action::On), 0);
    engine.apply(&command(Action::SetTtl(2)), 3000);
    assert!(!engine.snapshot().on);
    assert_eq!(
        engine.apply(&command(Action::SetTtl(MAX_TIMER_SECONDS + 1)), 4000),
        Outcome::InvalidTimer
    );
}

#[test]
fn debounce_rejects_bounce_and_emits_once_after_ten_ms() {
    let mut debounce = Debouncer::new(true);
    assert_eq!(debounce.update(false, 0), None);
    assert_eq!(debounce.update(true, 5), None);
    assert_eq!(debounce.update(false, 7), None);
    assert_eq!(debounce.update(false, 16), None);
    assert_eq!(debounce.update(false, 17), Some(false));
    assert_eq!(debounce.update(false, 50), None);
}

#[test]
fn copy_input_uses_raw_high_low_polarity() {
    let config = InputConfig {
        mode: InputMode::CopyToRelay,
        relay: Some(0),
        ..Default::default()
    };
    assert_eq!(
        input_event(&config, true, false).unwrap().action,
        Some(Action::On)
    );
    assert_eq!(
        input_event(&config, false, true).unwrap().action,
        Some(Action::Off)
    );
}

#[test]
fn mapped_toggle_uses_falling_edge_and_can_cancel_tta() {
    let config = InputConfig {
        mode: InputMode::RelayToggle,
        relay: Some(0),
        ..Default::default()
    };
    assert!(input_event(&config, true, false).is_none());
    assert_eq!(
        input_event(&config, false, false).unwrap().action,
        Some(Action::DelayedOn)
    );
    assert_eq!(
        input_event(&config, false, true).unwrap().action,
        Some(Action::Off)
    );
}

#[test]
fn unbound_toggle_and_temperature_never_operate_relay() {
    let config = InputConfig {
        mode: InputMode::RelayToggle,
        ..Default::default()
    };
    assert!(input_event(&config, false, false).is_none());
    let config = InputConfig {
        mode: InputMode::Temperature,
        ..Default::default()
    };
    assert!(input_event(&config, false, false).is_none());
}

#[test]
fn normal_and_toggle_inputs_are_telemetry_only() {
    let config = InputConfig {
        mode: InputMode::Normal,
        ..Default::default()
    };
    assert_eq!(input_event(&config, true, false).unwrap().action, None);
    let config = InputConfig {
        mode: InputMode::Toggle,
        ..Default::default()
    };
    assert!(input_event(&config, true, false).is_none());
    assert!(input_event(&config, false, false).unwrap().publish_toggle);
}

#[test]
fn empty_config_defaults_to_off_unmapped_inputs() {
    let config = Config::parse(b"{}").unwrap();
    assert!(config.inputs.iter().all(|input| input.relay.is_none()));
    assert_eq!(config.relay.ttl_seconds, 0);
    assert!(!config.network.mqtt_enabled);
    assert!(Config::parse(br#"{"network":{}}"#).is_ok());
}

#[test]
fn schema_unknown_fields_and_oversized_documents_are_rejected() {
    assert!(Config::parse(br#"{"schema":2}"#).is_err());
    assert!(Config::parse(br#"{"relay_gpio":2}"#).is_err());
    assert!(Config::parse(&vec![b' '; 8193]).is_err());
}

#[test]
fn timer_limits_and_invalid_relay_mappings_are_rejected() {
    let mut config = Config::default();
    config.relay.ttl_seconds = MAX_TIMER_SECONDS + 1;
    assert!(config.validate().is_err());
    config.relay.ttl_seconds = 0;
    config.inputs[0].mode = InputMode::RelayToggle;
    config.inputs[0].relay = Some(1);
    assert!(config.validate().is_err());
}

#[test]
fn wildcard_and_feedback_loop_topics_are_rejected() {
    let mut config = Config::default();
    config.relay.command_topic = "test/#".into();
    assert!(config.validate().is_err());
    config.relay.command_topic = "test/on".into();
    config.relay.state_topic = "test/on".into();
    assert!(config.validate().is_err());
}

#[test]
fn legacy_strings_and_numbers_import_with_gpio_mapping() {
    let config = Config::from_legacy(
        &json!({"PhyLoc":"lab", "I1MODE":"3", "MQTT_B_PRT":"1883", "mqttUser":"null"}),
        &json!({"RELAYNB":0, "ttl":"10", "tta":2, "PUB_TOPIC1":"/home/test/Coils/C0"}),
        &json!({"I1":"33", "R1":"0"}),
    )
    .unwrap();
    assert_eq!(config.relay.ttl_seconds, 10);
    assert_eq!(config.relay.tta_seconds, 2);
    assert_eq!(config.inputs[0].mode, InputMode::RelayToggle);
    assert_eq!(config.inputs[0].relay, Some(0));
    assert!(config.network.mqtt_username.is_empty());
}

#[test]
fn legacy_overflow_and_wrong_board_mapping_fail_import() {
    assert!(Config::from_legacy(&json!({}), &json!({"ttl":"4294967296"}), &json!({})).is_err());
    assert!(Config::from_legacy(&json!({}), &json!({}), &json!({"I1":18,"R1":1})).is_err());
    assert!(Config::from_legacy(&json!({"MQTT_B_PRT":65536}), &json!({}), &json!({})).is_err());
}
