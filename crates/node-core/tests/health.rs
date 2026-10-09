use node_core::health::{Health, Service, SERVICE_TIMEOUT_SECONDS};
use std::sync::atomic::Ordering;

#[test]
fn disabled_services_never_request_recovery() {
    let health = Health::default();
    assert_eq!(health.recovery_due(5 * 365 * 24 * 3600), None);
    assert!(health
        .status(1_000_000)
        .iter()
        .all(|status| !status.expected && status.age_seconds == 0));
}

#[test]
fn stalled_services_expire_at_the_deadline() {
    for service in Service::ALL {
        let health = Health::default();
        health.expect(service, 100);
        assert_eq!(health.fault(100 + SERVICE_TIMEOUT_SECONDS - 1), None);
        assert_eq!(health.fault(100 + SERVICE_TIMEOUT_SECONDS), Some(service));
        health.progress(service, 160);
        assert_eq!(health.fault(160), None);
    }
}

#[test]
fn stopped_task_failure_is_latched_even_if_progress_is_updated() {
    let health = Health::default();
    let guard = health.watch(Service::Network, 0);
    assert_eq!(health.fault(0), None);
    drop(guard);
    health.progress(Service::Network, 10);
    assert_eq!(health.fault(10), Some(Service::Network));
    assert!(health.status(10)[0].failed);
}

#[test]
fn recovery_waits_for_minimum_boot_interval_but_failure_blocks_health_promotion() {
    let health = Health::default();
    health.expect(Service::ModbusServer, 0);
    health.failed(Service::ModbusServer);
    assert_eq!(health.fault(30), Some(Service::ModbusServer));
    assert_eq!(health.recovery_due(59), None);
    assert_eq!(health.recovery_due(60), Some(Service::ModbusServer));
}

#[test]
fn concurrent_future_heartbeat_does_not_look_like_a_stall() {
    let health = Health::default();
    health.expect(Service::Network, 101);
    assert_eq!(health.fault(100), None);
    assert_eq!(health.status(100)[0].age_seconds, 0);
}

#[test]
fn seconds_wrap_is_handled_without_false_health() {
    let health = Health::default();
    health.expect(Service::Network, u32::MAX - 20);
    assert_eq!(health.fault(10), None);
    assert_eq!(health.status(10)[0].age_seconds, 31);
    assert_eq!(health.fault(39), Some(Service::Network));
}

#[test]
fn reconnect_progress_stays_healthy_after_49_days_and_five_years() {
    let health = Health::default();
    for service in Service::ALL {
        health.expect(service, 0);
    }
    for seconds in [49 * 24 * 3600, 365 * 24 * 3600, 5 * 365 * 24 * 3600] {
        for service in Service::ALL {
            health.progress(service, seconds);
        }
        assert_eq!(health.recovery_due(seconds + 5), None);
        assert!(health
            .status(seconds + 5)
            .iter()
            .all(|status| status.age_seconds == 5));
    }
}

#[test]
fn one_busy_service_cannot_feed_another_stalled_service() {
    let health = Health::default();
    health.expect(Service::Network, 0);
    health.expect(Service::ModbusServer, 0);
    health.progress(Service::Network, 100);
    assert_eq!(health.fault(100), Some(Service::ModbusServer));
}

#[test]
fn fault_schedules_recovery_once_without_waiting_for_service_locks() {
    let (runtime, _) = node_core::runtime::Runtime::new(
        node_core::config::Config::default(),
        "abcdef".into(),
        "00:00:00:ab:cd:ef".into(),
    );
    runtime.health.expect(Service::Network, 0);
    let _held_config = runtime.config.lock().unwrap();
    assert_eq!(runtime.check_service_health(59_999), None);
    assert_eq!(runtime.check_service_health(60_000), Some(Service::Network));
    assert_eq!(runtime.restart_at.load(Ordering::Acquire), 63);
    assert_eq!(runtime.check_service_health(61_000), None);
    assert_eq!(runtime.restart_at.load(Ordering::Acquire), 63);
}

#[test]
fn service_failure_cannot_delay_an_existing_settings_or_ota_restart() {
    let (runtime, _) = node_core::runtime::Runtime::new(
        node_core::config::Config::default(),
        "abcdef".into(),
        "00:00:00:ab:cd:ef".into(),
    );
    runtime.health.expect(Service::Network, 0);
    runtime.health.failed(Service::Network);
    runtime.restart_at.store(61, Ordering::Release);
    assert_eq!(runtime.check_service_health(60_000), None);
    assert_eq!(runtime.restart_at.load(Ordering::Acquire), 61);
}
