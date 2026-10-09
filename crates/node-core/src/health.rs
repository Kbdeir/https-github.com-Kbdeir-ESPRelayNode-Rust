use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub const SERVICE_TIMEOUT_SECONDS: u32 = 60;
pub const RECOVERY_MIN_UPTIME_SECONDS: u32 = 60;

fn age(now: u32, last: u32) -> u32 {
    let elapsed = now.wrapping_sub(last);
    // A concurrent heartbeat may be newer than the caller's sampled clock.
    if elapsed > u32::MAX / 2 {
        0
    } else {
        elapsed
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    Network,
    ModbusServer,
    ModbusClient,
    Mqtt,
}
impl Service {
    pub const ALL: [Self; 4] = [
        Self::Network,
        Self::ModbusServer,
        Self::ModbusClient,
        Self::Mqtt,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::ModbusServer => "modbus-server",
            Self::ModbusClient => "modbus-client",
            Self::Mqtt => "mqtt-adapter",
        }
    }
}

#[derive(Default)]
struct Progress {
    expected: AtomicBool,
    failed: AtomicBool,
    last_seconds: AtomicU32,
}

#[derive(Default)]
pub struct Health {
    services: [Progress; 4],
}

#[derive(Serialize)]
pub struct ServiceStatus {
    pub name: &'static str,
    pub expected: bool,
    pub failed: bool,
    pub age_seconds: u32,
}
pub struct ServiceGuard<'a> {
    health: &'a Health,
    service: Service,
    finished: bool,
}
impl ServiceGuard<'_> {
    pub fn finish(mut self) {
        self.health.services[self.service as usize]
            .expected
            .store(false, Ordering::Release);
        self.finished = true;
    }
}
impl Drop for ServiceGuard<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.health.failed(self.service);
        }
    }
}
impl Health {
    pub fn watch(&self, service: Service, now_seconds: u32) -> ServiceGuard<'_> {
        self.expect(service, now_seconds);
        ServiceGuard {
            health: self,
            service,
            finished: false,
        }
    }
    pub fn expect(&self, service: Service, now_seconds: u32) {
        let progress = &self.services[service as usize];
        progress.last_seconds.store(now_seconds, Ordering::Release);
        progress.expected.store(true, Ordering::Release);
    }
    pub fn progress(&self, service: Service, now_seconds: u32) {
        self.services[service as usize]
            .last_seconds
            .store(now_seconds, Ordering::Release);
    }
    pub fn failed(&self, service: Service) {
        self.services[service as usize]
            .failed
            .store(true, Ordering::Release);
    }
    pub fn fault(&self, now_seconds: u32) -> Option<Service> {
        Service::ALL.into_iter().find(|service| {
            let progress = &self.services[*service as usize];
            progress.expected.load(Ordering::Acquire)
                && (progress.failed.load(Ordering::Acquire)
                    || age(now_seconds, progress.last_seconds.load(Ordering::Acquire))
                        >= SERVICE_TIMEOUT_SECONDS)
        })
    }
    pub fn recovery_due(&self, now_seconds: u32) -> Option<Service> {
        if now_seconds < RECOVERY_MIN_UPTIME_SECONDS {
            None
        } else {
            self.fault(now_seconds)
        }
    }
    pub fn status(&self, now_seconds: u32) -> [ServiceStatus; 4] {
        Service::ALL.map(|service| {
            let progress = &self.services[service as usize];
            let expected = progress.expected.load(Ordering::Acquire);
            ServiceStatus {
                name: service.name(),
                expected,
                failed: progress.failed.load(Ordering::Acquire),
                age_seconds: if expected {
                    age(now_seconds, progress.last_seconds.load(Ordering::Acquire))
                } else {
                    0
                },
            }
        })
    }
}
