use crate::{
    command::{Command, Outcome},
    config::Config,
    engine::Snapshot,
    COMMAND_QUEUE_CAPACITY,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{sync_channel, Receiver, SyncSender},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

pub struct Envelope {
    pub command: Command,
    pub reply: Option<SyncSender<Outcome>>,
}
#[derive(Clone, Default, Serialize)]
pub struct RuntimeSnapshot {
    pub relay: Snapshot,
    pub inputs: [bool; 6],
    pub input_valid_mask: u16,
    pub timers: [bool; 4],
    pub ttl_seconds: u32,
    pub tta_seconds: u32,
    pub stack_margin_bytes: u32,
    pub uptime_seconds: u32,
    pub automation: [crate::automation::RuleStatus; 4],
}
#[derive(Clone, Copy, Default, Serialize)]
pub struct RemoteWrite {
    pub pending: bool,
    pub on: bool,
    pub acknowledged: Option<bool>,
    pub failed: bool,
    pub revision: u32,
    pub due_ms: u64,
}
pub struct Runtime {
    pub health: crate::health::Health,
    pub files: Mutex<Option<Arc<crate::filesystem::FileStore>>>,
    pub config: Mutex<Config>,
    pub config_revision: AtomicU32,
    pub mqtt_revision: AtomicU32,
    pub mqtt_generation: AtomicU32,
    pub modbus_revision: AtomicU32,
    pub snapshot: Mutex<RuntimeSnapshot>,
    pub commands: SyncSender<Envelope>,
    pub wifi_status: Mutex<Value>,
    pub wifi_scan: Mutex<Value>,
    pub scan_requested: AtomicBool,
    pub mqtt_connected: AtomicBool,
    pub mqtt_ready: AtomicBool,
    pub overflows: AtomicU32,
    pub restart_at: AtomicU32,
    pub maintenance: AtomicBool,
    pub network_ready: AtomicBool,
    pub ota_confirmed: AtomicBool,
    pub peer_status: Mutex<Vec<Value>>,
    pub input_events: Mutex<VecDeque<(usize, bool)>>,
    pub outcomes: Mutex<VecDeque<(Command, Outcome)>>,
    pub schedule_paused: AtomicBool,
    pub remote_values: Mutex<[crate::automation::RemoteReading; 8]>,
    pub remote_writes: Mutex<[RemoteWrite; 4]>,
    pub write_revision: [AtomicU32; 4],
    pub write_permit_until: [AtomicU32; 4],
    pub boot: Instant,
    pub node_id: String,
    pub mac: String,
    pub factory_wifi: (String, String),
}
impl Runtime {
    pub fn cancel_remote_write(&self, index: usize) {
        self.write_permit_until[index].store(0, Ordering::Release);
        self.write_revision[index].fetch_add(1, Ordering::AcqRel);
        if let Ok(mut writes) = self.remote_writes.try_lock() {
            writes[index] = RemoteWrite::default();
        }
    }

    // Called while holding config, only after durable persistence succeeds.
    pub fn config_saved(&self, candidate: &Config, previous: &Config) {
        if candidate == previous {
            return;
        }
        if candidate.modbus_changed(previous) {
            self.modbus_revision.fetch_add(1, Ordering::AcqRel);
        }
        if candidate.mqtt_changed(previous) {
            self.mqtt_revision.fetch_add(1, Ordering::AcqRel);
        }
        for i in 0..4 {
            if candidate.rule_changed(previous, i) {
                self.write_permit_until[i].store(0, Ordering::Release);
                self.write_revision[i].fetch_add(1, Ordering::AcqRel);
            }
        }
        if candidate.remote_sensors != previous.remote_sensors {
            let mut values = self.remote_values.lock().unwrap();
            for i in 0..8 {
                let a = &candidate.remote_sensors[i];
                let b = &previous.remote_sensors[i];
                if a.transport != b.transport
                    || a.topic != b.topic
                    || a.json_key != b.json_key
                    || a.modbus != b.modbus
                {
                    values[i] = crate::automation::RemoteReading::default();
                }
            }
        }
        self.config_revision.fetch_add(1, Ordering::Release);
    }

    pub fn config_update(&self, revision: &mut u32) -> Option<Config> {
        if self.config_revision.load(Ordering::Acquire) == *revision {
            return None;
        }
        // Never wait for an HTTP/NVS writer on the watchdog-protected control loop.
        let config = self.config.try_lock().ok()?;
        *revision = self.config_revision.load(Ordering::Acquire);
        Some(config.clone())
    }

    pub fn new(config: Config, node_id: String, mac: String) -> (Arc<Self>, Receiver<Envelope>) {
        let factory_wifi = (
            config.network.ssid.clone(),
            config.network.wifi_password.clone(),
        );
        Self::new_with_factory_wifi(config, node_id, mac, factory_wifi)
    }

    pub fn new_with_factory_wifi(
        config: Config,
        node_id: String,
        mac: String,
        factory_wifi: (String, String),
    ) -> (Arc<Self>, Receiver<Envelope>) {
        let (commands, receiver) = sync_channel(COMMAND_QUEUE_CAPACITY);
        (
            Arc::new(Self {
                health: crate::health::Health::default(),
                files: Mutex::new(None),
                config: Mutex::new(config),
                config_revision: AtomicU32::new(0),
                mqtt_revision: AtomicU32::new(0),
                mqtt_generation: AtomicU32::new(0),
                modbus_revision: AtomicU32::new(0),
                snapshot: Mutex::new(RuntimeSnapshot::default()),
                commands,
                wifi_status: Mutex::new(
                    json!({"connected":false,"ip":"0.0.0.0","gateway":"0.0.0.0","dns1":"0.0.0.0","dns2":"0.0.0.0"}),
                ),
                wifi_scan: Mutex::new(json!({"scanning":false,"count":0,"networks":[]})),
                scan_requested: AtomicBool::new(false),
                mqtt_connected: AtomicBool::new(false),
                mqtt_ready: AtomicBool::new(false),
                overflows: AtomicU32::new(0),
                restart_at: AtomicU32::new(0),
                maintenance: AtomicBool::new(false),
                network_ready: AtomicBool::new(false),
                ota_confirmed: AtomicBool::new(false),
                peer_status: Mutex::new(Vec::new()),
                input_events: Mutex::new(VecDeque::with_capacity(16)),
                outcomes: Mutex::new(VecDeque::with_capacity(16)),
                schedule_paused: AtomicBool::new(false),
                remote_values: Mutex::new([crate::automation::RemoteReading::default(); 8]),
                remote_writes: Mutex::new([RemoteWrite::default(); 4]),
                write_revision: std::array::from_fn(|_| AtomicU32::new(0)),
                write_permit_until: std::array::from_fn(|_| AtomicU32::new(0)),
                boot: Instant::now(),
                node_id,
                mac,
                factory_wifi,
            }),
            receiver,
        )
    }
    pub fn now_ms(&self) -> u64 {
        self.boot.elapsed().as_millis() as u64
    }
    pub fn check_service_health(&self, now_ms: u64) -> Option<crate::health::Service> {
        let now_seconds = (now_ms / 1000) as u32;
        let service = self.health.recovery_due(now_seconds)?;
        self.restart_at
            .compare_exchange(0, now_seconds + 3, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        Some(service)
    }
    pub fn submit(&self, mut command: Command) -> Result<Outcome, String> {
        command.expires_at_ms = Some(self.now_ms() + 1000);
        let (reply, receiver) = sync_channel(1);
        self.enqueue(Envelope {
            command,
            reply: Some(reply),
        })?;
        receiver
            .recv_timeout(Duration::from_millis(1200))
            .map_err(|_| "control acknowledgement timed out".into())
    }
    pub fn enqueue(&self, envelope: Envelope) -> Result<(), String> {
        self.commands.try_send(envelope).map_err(|_| {
            self.overflows.fetch_add(1, Ordering::Relaxed);
            "command queue full".into()
        })
    }
    pub fn read(&self) -> RuntimeSnapshot {
        self.snapshot.lock().unwrap().clone()
    }
    pub fn automation_tick(
        &self,
        automation: &mut crate::automation::Automation,
        config: &Config,
        engine: &mut crate::engine::Engine,
        sources: &RuntimeSnapshot,
        local_off: bool,
    ) {
        let now = self.now_ms();
        let restarting = self.restart_at.load(Ordering::Acquire) != 0
            || self.maintenance.load(Ordering::Acquire);
        let remote = self
            .remote_values
            .try_lock()
            .map(|v| *v)
            .unwrap_or_default();
        let gates = std::array::from_fn(|i| {
            sources.timers[i]
                && config.timers[i].enabled
                && !self.schedule_paused.load(Ordering::Relaxed)
        });
        let steps = automation.tick(
            now,
            config,
            &crate::automation::Sources {
                snapshot: sources,
                remote: &remote,
                gates,
            },
        );
        for (i, step) in steps.iter().enumerate() {
            if step.cancel_remote || restarting {
                self.write_revision[i].fetch_add(1, Ordering::AcqRel);
                if let Ok(mut writes) = self.remote_writes.try_lock() {
                    writes[i].pending = false;
                }
            }
            self.write_permit_until[i].store(
                if step.permit_remote && !restarting {
                    (now / 1000) as u32 + 3
                } else {
                    0
                },
                Ordering::Release,
            );
            let Some(action) = step.action else {
                continue;
            };
            if restarting || (local_off && action != crate::command::Action::Off) {
                continue;
            }
            let accepted = if config.automation[i].modbus.is_some() {
                if let Ok(mut writes) = self.remote_writes.try_lock() {
                    let on = action == crate::command::Action::On;
                    if !writes[i].pending || writes[i].on != on {
                        writes[i] = RemoteWrite {
                            pending: true,
                            on,
                            revision: self.write_revision[i].load(Ordering::Acquire),
                            ..Default::default()
                        };
                    }
                    true
                } else {
                    false
                }
            } else {
                engine.apply(
                    &crate::protocol::command(action, crate::command::Source::Automation),
                    now,
                ) == Outcome::Applied
            };
            if accepted {
                automation.accepted(i, now);
            }
        }
    }
}
