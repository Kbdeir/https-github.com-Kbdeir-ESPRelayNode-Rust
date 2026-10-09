use serde::Serialize;

use crate::command::{Action, Command, Outcome, Source};
use crate::config::{RelayConfig, MAX_TIMER_SECONDS};

#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
pub struct Snapshot {
    pub on: bool,
    pub interlocked: bool,
    pub ttl_elapsed_seconds: u32,
    pub ttl_remaining_seconds: u32,
    pub tta_remaining_seconds: u32,
}

pub struct Engine {
    state: Snapshot,
    ttl_seconds: u32,
    tta_seconds: u32,
    on_since_ms: Option<u64>,
    off_at_ms: Option<u64>,
    on_at_ms: Option<u64>,
    recent_ids: [Option<(Source, u64)>; 32],
    next_id: usize,
}

impl Engine {
    pub fn new(config: &RelayConfig) -> Self {
        Self {
            state: Snapshot::default(),
            ttl_seconds: config.ttl_seconds,
            tta_seconds: config.tta_seconds,
            on_since_ms: None,
            off_at_ms: None,
            on_at_ms: None,
            recent_ids: [None; 32],
            next_id: 0,
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.state
    }
    pub fn ttl_seconds(&self) -> u32 {
        self.ttl_seconds
    }

    pub fn configure(&mut self, config: &RelayConfig, now_ms: u64) {
        self.configure_timers(config.ttl_seconds, config.tta_seconds, now_ms);
    }

    pub fn configure_changed(&mut self, config: &RelayConfig, previous: &RelayConfig, now_ms: u64) {
        let ttl = if config.ttl_seconds != previous.ttl_seconds {
            config.ttl_seconds
        } else {
            self.ttl_seconds
        };
        self.configure_timers(ttl, config.tta_seconds, now_ms);
    }

    fn configure_timers(&mut self, ttl_seconds: u32, tta_seconds: u32, now_ms: u64) {
        self.ttl_seconds = ttl_seconds;
        if let Some(started) = self.on_since_ms {
            self.off_at_ms = (self.ttl_seconds > 0)
                .then(|| started.saturating_add(u64::from(self.ttl_seconds) * 1000));
        }
        // Preserve delayed-ON's original start; saves must not restart its clock.
        if let Some(deadline) = self.on_at_ms {
            let started = deadline.saturating_sub(u64::from(self.tta_seconds) * 1000);
            self.on_at_ms = Some(started.saturating_add(u64::from(tta_seconds) * 1000));
        }
        self.tta_seconds = tta_seconds;
        self.tick(now_ms);
    }

    pub fn apply(&mut self, command: &Command, now_ms: u64) -> Outcome {
        self.tick(now_ms);
        if command.relay != 0 {
            return Outcome::InvalidRelay;
        }
        if command
            .expires_at_ms
            .is_some_and(|deadline| now_ms >= deadline)
        {
            return Outcome::Expired;
        }
        if let Some(id) = command.id {
            if self.recent_ids.contains(&Some((command.source, id))) {
                return Outcome::Duplicate;
            }
        }
        if matches!(
            command.action,
            Action::ClearInterlock | Action::EmergencyOff
        ) && !matches!(command.source, Source::LocalInput | Source::Automation)
        {
            return Outcome::Unauthorized;
        }
        if self.state.interlocked
            && matches!(
                command.action,
                Action::On | Action::DelayedOn | Action::Toggle
            )
        {
            return Outcome::Interlocked;
        }
        match command.action {
            Action::On => self.set_output(true, now_ms),
            Action::Off => self.set_output(false, now_ms),
            Action::Toggle => self.set_output(!self.state.on && self.on_at_ms.is_none(), now_ms),
            Action::DelayedOn => {
                if !self.state.on && self.on_at_ms.is_none() {
                    if self.tta_seconds == 0 {
                        self.set_output(true, now_ms);
                    } else {
                        self.on_at_ms =
                            Some(now_ms.saturating_add(u64::from(self.tta_seconds) * 1000));
                    }
                }
            }
            Action::SetTtl(seconds) => {
                if seconds > MAX_TIMER_SECONDS {
                    return Outcome::InvalidTimer;
                }
                self.ttl_seconds = seconds;
                if let Some(started) = self.on_since_ms {
                    self.off_at_ms =
                        (seconds > 0).then(|| started.saturating_add(u64::from(seconds) * 1000));
                }
            }
            Action::EmergencyOff => {
                self.state.interlocked = true;
                self.set_output(false, now_ms);
            }
            Action::ClearInterlock => self.state.interlocked = false,
        }
        if let Some(id) = command.id {
            self.recent_ids[self.next_id] = Some((command.source, id));
            self.next_id = (self.next_id + 1) % self.recent_ids.len();
        }
        self.tick(now_ms);
        Outcome::Applied
    }

    pub fn tick(&mut self, now_ms: u64) {
        if self.on_at_ms.is_some_and(|deadline| now_ms >= deadline) {
            let started = self.on_at_ms.unwrap();
            self.set_output(true, started);
        }
        if self.off_at_ms.is_some_and(|deadline| now_ms >= deadline) {
            self.set_output(false, now_ms);
        }
        self.state.ttl_elapsed_seconds = self.on_since_ms.map_or(0, |started| {
            (now_ms.saturating_sub(started) / 1000).min(u64::from(u32::MAX)) as u32
        });
        self.state.ttl_remaining_seconds = remaining(self.off_at_ms, now_ms);
        self.state.tta_remaining_seconds = remaining(self.on_at_ms, now_ms);
    }

    fn set_output(&mut self, on: bool, now_ms: u64) {
        self.on_at_ms = None;
        if self.state.on == on {
            return;
        }
        self.state.on = on;
        self.on_since_ms = on.then_some(now_ms);
        self.off_at_ms = (on && self.ttl_seconds > 0)
            .then(|| now_ms.saturating_add(u64::from(self.ttl_seconds) * 1000));
    }
}

fn remaining(deadline: Option<u64>, now_ms: u64) -> u32 {
    deadline.map_or(0, |deadline| {
        deadline.saturating_sub(now_ms).div_ceil(1000) as u32
    })
}
