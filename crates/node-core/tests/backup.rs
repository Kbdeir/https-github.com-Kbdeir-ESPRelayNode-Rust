use base64::{engine::general_purpose::STANDARD, Engine as _};
use node_core::{
    backup,
    config::{Config, InputMode},
    filesystem::FileStore,
    runtime::Runtime,
    web,
};
use serde_json::{json, Value};
use std::{
    fs, io,
    path::PathBuf,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
};

static NEXT: AtomicU32 = AtomicU32::new(0);
struct Fixture {
    root: PathBuf,
    runtime: Arc<Runtime>,
    store: Arc<FileStore>,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "rust-backup-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let mut config = Config::default();
        config.management.enabled = true;
        config.management.password = "private-password".into();
        config.network.wifi_password = "private-wifi".into();
        config.network.mqtt_password = "private-mqtt".into();
        config.location = "saved location".into();
        let (runtime, _) = Runtime::new(config, "abcdef".into(), "00:00:00:ab:cd:ef".into());
        let store = Arc::new(FileStore::new(&root, None));
        *runtime.files.lock().unwrap() = Some(store.clone());
        Self {
            root,
            runtime,
            store,
        }
    }
    fn restore(&self, bytes: &[u8], query: &str) -> web::Reply {
        backup::restore(&self.runtime, query, bytes.len(), bytes, |_, _| Ok(()))
    }
    fn export(&self, web: bool) -> Vec<u8> {
        backup::download(&self.runtime, web).body.into_bytes()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}
fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut value =
        json!({"_backup_version":1,"_chipid":"123456","_includesWeb":true,"_files":files.len()});
    for (name, bytes) in files {
        value[*name] = json!(STANDARD.encode(bytes));
    }
    serde_json::to_vec(&value).unwrap()
}

#[test]
fn native_settings_roundtrip_includes_secrets_without_leaking_to_read_config() {
    let fixture = Fixture::new();
    let data = fixture.export(false);
    let parsed: Value = serde_json::from_slice(&data).unwrap();
    assert_eq!(parsed["_files"], 1);
    assert_eq!(parsed["_includesWeb"], false);
    let native = STANDARD
        .decode(parsed[backup::CONFIG_FILE].as_str().unwrap())
        .unwrap();
    let config = Config::parse(&native).unwrap();
    assert_eq!(config.network.wifi_password, "private-wifi");
    assert_eq!(config.network.mqtt_password, "private-mqtt");
    fixture.runtime.config.lock().unwrap().location = "changed".into();
    assert_eq!(
        fixture
            .restore(&data, "/api/restore?chipIdMode=keep&restoreWeb=0")
            .status,
        200
    );
    assert_eq!(
        fixture.runtime.config.lock().unwrap().location,
        "saved location"
    );
    let read = web::handle(&fixture.runtime, "GET", "/GetConfig.json", &[], |_, _| {
        Ok(())
    })
    .body
    .into_bytes();
    assert!(!String::from_utf8(read).unwrap().contains("private-"));
}
#[test]
fn large_backup_streams_raw_assets_in_bounded_chunks() {
    let fixture = Fixture::new();
    let bytes = vec![0xa5; 100_000];
    fs::write(fixture.root.join("page.html.gz"), &bytes).unwrap();
    let reply = backup::download(&fixture.runtime, true);
    let mut output = Vec::new();
    reply
        .body
        .write_to(|chunk| {
            assert!(chunk.len() <= 768);
            output.extend_from_slice(chunk);
            Ok(())
        })
        .unwrap();
    let parsed: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(
        STANDARD
            .decode(parsed["/page.html.gz"].as_str().unwrap())
            .unwrap(),
        bytes
    );
    assert_eq!(parsed["_files"], 2);
    assert!(reply
        .body
        .write_to(|_| Err(io::Error::other("transport closed")))
        .is_err());
}
#[test]
fn web_restore_replaces_web_set_but_not_nvs_or_json_artifacts() {
    let fixture = Fixture::new();
    fs::write(fixture.root.join("old.html"), b"old").unwrap();
    fs::write(fixture.root.join("keep.json"), b"settings artifact").unwrap();
    let bytes = archive(&[
        ("/new.html", b"%PhyLoc% stays raw"),
        ("/binary.gz", &[0, 255, 13, 10]),
    ]);
    assert_eq!(
        fixture.restore(&bytes, "/api/restore?restoreWeb=1").status,
        200
    );
    assert!(!fixture.root.join("old.html").exists());
    assert_eq!(
        fs::read(fixture.root.join("new.html")).unwrap(),
        b"%PhyLoc% stays raw"
    );
    assert_eq!(
        fs::read(fixture.root.join("binary.gz")).unwrap(),
        [0, 255, 13, 10]
    );
    assert!(fixture.root.join("keep.json").exists());
    assert_eq!(
        fixture.runtime.config.lock().unwrap().location,
        "saved location"
    );
    assert!(!fixture.root.join(".restore").exists());
}
#[test]
fn invalid_or_interrupted_backup_never_changes_live_files_or_settings() {
    for bytes in [b"{}".to_vec(),b"{\"_backup_version\":2}".to_vec(),b"{\"_backup_version\":1,\"_backup_version\":1}".to_vec(),archive(&[("/../escape",b"bad")]),b"{\"_backup_version\":1,\"_includesWeb\":true,\"/ok.html\":\"YWJj\",\"/bad.html\":\"not base64\"}".to_vec(),b"{\"_backup_version\":1,\"_includesWeb\":true,\"/ok.html\":\"YWJj\"".to_vec()] {
        let fixture=Fixture::new();fs::write(fixture.root.join("old.html"),b"old").unwrap();
        assert_ne!(fixture.restore(&bytes,"/api/restore?restoreWeb=1").status,200);
        assert_eq!(fs::read(fixture.root.join("old.html")).unwrap(),b"old");
        assert!(!fixture.root.join("ok.html").exists());
        assert_eq!(fixture.runtime.restart_at.load(Ordering::Acquire),0);
        assert!(!fixture.root.join(".restore").exists());
    }
}
#[test]
fn metadata_names_sizes_queries_and_content_length_are_bounded() {
    let fixture = Fixture::new();
    let oversized_name = format!("{{\"{}\":1}}", "x".repeat(20000));
    assert_ne!(
        fixture
            .restore(oversized_name.as_bytes(), "/api/restore")
            .status,
        200
    );
    let bytes = fixture.export(false);
    assert_eq!(
        backup::restore(
            &fixture.runtime,
            "/api/restore",
            bytes.len() + 1,
            bytes.as_slice(),
            |_, _| Ok(())
        )
        .status,
        400
    );
    assert_eq!(
        fixture
            .restore(&bytes, "/api/restore?chipIdMode=bad")
            .status,
        400
    );
    assert_eq!(
        fixture.restore(&bytes, "/api/restore?restoreWeb=2").status,
        400
    );
    assert_eq!(
        fixture
            .restore(&bytes, "/api/restore?restoreWeb=0&restoreWeb=1")
            .status,
        400
    );
    assert_ne!(
        fixture
            .restore(&[bytes, b"trailing data".to_vec()].concat(), "/api/restore")
            .status,
        200
    );
    let oversized = archive(&[(
        "/large.html",
        &vec![b'x'; node_core::filesystem::MAX_FILE_BYTES + 1],
    )]);
    assert_ne!(
        fixture
            .restore(&oversized, "/api/restore?restoreWeb=1")
            .status,
        200
    );
}
#[test]
fn persistence_failure_is_transactional_and_does_not_schedule_restart() {
    let fixture = Fixture::new();
    let bytes = fixture.export(false);
    fixture.runtime.config.lock().unwrap().location = "changed".into();
    let reply = backup::restore(
        &fixture.runtime,
        "/api/restore",
        bytes.len(),
        bytes.as_slice(),
        |_, _| Err("NVS full".into()),
    );
    assert_eq!(reply.status, 500);
    assert_eq!(fixture.runtime.config.lock().unwrap().location, "changed");
    assert_eq!(fixture.runtime.restart_at.load(Ordering::Acquire), 0);
}
#[test]
fn id_modes_rewrite_all_topic_references_but_never_passwords() {
    for (mode, expected) in [
        ("keep", "123456"),
        ("replace", "abcdef"),
        ("original", "abcdefabcdef"),
    ] {
        let fixture = Fixture::new();
        fixture.runtime.config.lock().unwrap().relay.command_topic =
            "/home/Controller123456/C0/Controller112233445566".into();
        fixture.runtime.config.lock().unwrap().network.wifi_password = "Controller123456".into();
        let mut value: Value = serde_json::from_slice(&fixture.export(false)).unwrap();
        value["_chipid"] = json!("abcdefabcdef");
        let bytes = serde_json::to_vec(&value).unwrap();
        assert_eq!(
            fixture
                .restore(&bytes, &format!("/api/restore?chipIdMode={mode}"))
                .status,
            200
        );
        let current = fixture.runtime.config.lock().unwrap();
        assert!(current
            .relay
            .command_topic
            .starts_with(&format!("/home/Controller{expected}/C0/Controller")));
        if mode != "keep" {
            assert!(current.relay.command_topic.ends_with(expected));
        }
        assert_eq!(current.network.wifi_password, "Controller123456");
    }
    assert_eq!(
        backup::rewrite_controller_ids("Controllernot-an-id/Controller1234567", "abcdef"),
        "Controllernot-an-id/Controller1234567"
    );
}
#[test]
fn reset_keeps_auth_and_schedules_disabled_without_changing_web_files() {
    let fixture = Fixture::new();
    fs::write(fixture.root.join("page.html"), b"unchanged").unwrap();
    {
        let mut current = fixture.runtime.config.lock().unwrap();
        current.timers[0].enabled = true;
        current.timers[0].time_from = "10:15".into();
        current.automation[0].enabled = true;
        current.automation[0].cooldown = 42;
    }
    assert_eq!(backup::reset(&fixture.runtime, |_, _| Ok(())).status, 200);
    let current = fixture.runtime.config.lock().unwrap();
    assert_eq!(current.management.password, "private-password");
    assert_eq!(current.location, "Not configured yet");
    assert_eq!(current.network.ssid, fixture.runtime.factory_wifi.0);
    assert_eq!(current.network.wifi_password, "private-wifi");
    assert_eq!(current.network.mqtt_host, "192.168.1.1");
    assert!(!current.network.mqtt_enabled);
    assert!(!current.modbus.enabled);
    assert_eq!(current.timers[0].time_from, "10:15");
    assert_eq!(current.automation[0].cooldown, 42);
    assert!(current.timers.iter().all(|timer| !timer.enabled));
    assert!(current.automation.iter().all(|rule| !rule.enabled));
    assert_eq!(
        fs::read(fixture.root.join("page.html")).unwrap(),
        b"unchanged"
    );
}
#[test]
fn reset_persists_generated_topics_and_reloads_them_in_the_configuration_api() {
    let fixture = Fixture::new();
    fixture.runtime.config.lock().unwrap().relay.command_topic =
        "/home/Controller123456/old-command".into();
    let mut saved = Vec::new();
    let response = web::handle(
        &fixture.runtime,
        "POST",
        "/api/resetconfig",
        &[],
        |new, old| {
            assert_eq!(
                old.relay.command_topic,
                "/home/Controller123456/old-command"
            );
            saved = serde_json::to_vec(new).unwrap();
            Ok(())
        },
    );
    assert_eq!(response.status, 200);
    let reloaded = Config::parse(&saved).unwrap();
    assert_eq!(reloaded.relay.ttl_seconds, 0);
    assert_eq!(reloaded.relay.tta_seconds, 0);
    assert_eq!(reloaded.network.timeserver, "162.159.200.123");
    assert_eq!(reloaded.network.utc_offset_hours, 2);
    assert_eq!(reloaded.network.mqtt_port, 1883);
    assert_eq!(reloaded.network.mqtt_keepalive_seconds, 30);
    assert!(reloaded.network.mqtt_username.is_empty());
    assert!(reloaded.network.mqtt_password.is_empty());
    assert_eq!(reloaded.modbus.port, 502);
    assert_eq!(reloaded.modbus.unit, 1);
    assert_eq!(reloaded.management.password, "private-password");
    let (runtime, _) = Runtime::new(reloaded, "abcdef".into(), "00:00:00:ab:cd:ef".into());
    let read = web::handle(&runtime, "GET", "/GetConfig.json", &[], |_, _| {
        panic!("configuration GET must not save")
    });
    let json: Value = serde_json::from_slice(&read.body.into_bytes()).unwrap();
    for (key, suffix) in [
        ("PUB_TOPIC1", "/Coils/C0"),
        ("STATE_PUB_TOPIC", "/Coils/State/C0"),
        ("TTL_PUB_TOPIC", "/sts/VTTL0"),
        ("i_ttl_PUB_TOPIC", "/i/TTL0"),
        ("CURR_TTL_PUB_TOPIC", "/sts/CURRVTTL0"),
    ] {
        assert_eq!(json[key], format!("/home/Controllerabcdef{suffix}"));
    }
    for (index, key) in node_core::management::INPUT_TOPICS.iter().enumerate() {
        assert_eq!(
            json[*key],
            format!("/home/Controllerabcdef/INS/sts/IN{}", index + 1)
        );
        assert_eq!(json[format!("I{}MODE", index + 1)], 2);
    }
    assert_eq!(json["pass"], "");
    assert_eq!(json["mqttPass"], "");
    assert!(fixture.runtime.restart_at.load(Ordering::Acquire) > 0);
}
#[test]
fn factory_defaults_are_board_specific_and_do_not_change_fail_safe_defaults() {
    for id in ["2aaaf8", "240AC42AAAF8"] {
        let config = Config::factory_defaults(id).unwrap();
        assert_eq!(
            config.relay.command_topic,
            format!("/home/Controller{id}/Coils/C0")
        );
        assert!(config
            .inputs
            .iter()
            .all(|input| input.mode == InputMode::Normal && input.relay.is_none()));
        config.validate().unwrap();
        assert!(config.network.ssid.is_empty());
        assert!(config.network.wifi_password.is_empty());
    }
    for id in ["", "../../", "12345", "1234567", "zzzzzz"] {
        assert!(Config::factory_defaults(id).is_err());
    }
    let safe = Config::default();
    assert!(safe.relay.command_topic.is_empty());
    assert!(safe.network.ssid.is_empty());
    assert!(safe
        .inputs
        .iter()
        .all(|input| input.mode == InputMode::None));
}

#[test]
fn reset_uses_private_build_wifi_defaults_not_live_edited_credentials() {
    let mut config = Config::default();
    config.network.ssid = "edited-network".into();
    config.network.wifi_password = "edited-password".into();
    config.management.enabled = true;
    config.management.password = "private-password".into();
    let (runtime, _) = Runtime::new_with_factory_wifi(
        config,
        "abcdef".into(),
        "00:00:00:ab:cd:ef".into(),
        ("factory-network".into(), "factory-password".into()),
    );
    assert_eq!(backup::reset(&runtime, |_, _| Ok(())).status, 200);
    let current = runtime.config.lock().unwrap();
    assert_eq!(current.network.ssid, "factory-network");
    assert_eq!(current.network.wifi_password, "factory-password");
    assert_eq!(current.management.password, "private-password");
}
#[test]
fn failed_reset_and_pending_restart_do_not_replace_settings() {
    let fixture = Fixture::new();
    let original = serde_json::to_vec(&*fixture.runtime.config.lock().unwrap()).unwrap();
    assert_eq!(
        backup::reset(&fixture.runtime, |_, _| Err("NVS write failed".into())).status,
        500
    );
    assert_eq!(
        serde_json::to_vec(&*fixture.runtime.config.lock().unwrap()).unwrap(),
        original
    );
    assert_eq!(fixture.runtime.restart_at.load(Ordering::Acquire), 0);
    fixture.runtime.restart_at.store(42, Ordering::Release);
    assert_eq!(
        backup::reset(&fixture.runtime, |_, _| panic!(
            "must not persist during a pending restart"
        ))
        .status,
        409
    );
    assert_eq!(
        serde_json::to_vec(&*fixture.runtime.config.lock().unwrap()).unwrap(),
        original
    );
    assert_eq!(fixture.runtime.restart_at.load(Ordering::Acquire), 42);
}
#[test]
fn interrupted_commit_recovers_old_assets_and_removes_new_files() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.root.join(".restore")).unwrap();
    fs::write(fixture.root.join(".restore/0.old"), b"old page").unwrap();
    fs::write(fixture.root.join("page.html"), b"new page").unwrap();
    fs::write(fixture.root.join("added.html"), b"added").unwrap();
    fs::write(fixture.root.join(".restore-journal.json"),serde_json::to_vec(&json!([{"name":"/page.html","had_old":true,"replace":true},{"name":"/added.html","had_old":false,"replace":true}])).unwrap()).unwrap();
    backup::recover(&fixture.store).unwrap();
    assert_eq!(
        fs::read(fixture.root.join("page.html")).unwrap(),
        b"old page"
    );
    assert!(!fixture.root.join("added.html").exists());
    assert!(!fixture.root.join(".restore").exists());
}
#[test]
fn get_routes_cannot_reset_restore_or_upload_and_missing_native_config_is_explicit() {
    let fixture = Fixture::new();
    for uri in ["/api/restore", "/api/resetconfig", "/update", "/updatefs"] {
        assert_eq!(
            web::handle(&fixture.runtime, "GET", uri, &[], |_, _| panic!(
                "must not persist"
            ))
            .status,
            405
        );
    }
    let bytes = archive(&[("/config.json", b"{}")]);
    let reply = fixture.restore(&bytes, "/api/restore");
    assert_eq!(reply.status, 400);
    assert!(String::from_utf8(reply.body.into_bytes())
        .unwrap()
        .contains("import-config.cmd"));
}
