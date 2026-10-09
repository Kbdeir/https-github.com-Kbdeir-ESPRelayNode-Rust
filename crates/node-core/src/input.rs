use crate::command::Action;
use crate::config::{InputConfig, InputMode};
use crate::INPUT_DEBOUNCE_MS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputEvent {
    pub high: bool,
    pub action: Option<Action>,
    pub publish_toggle: bool,
}

pub struct Debouncer {
    stable: bool,
    candidate: bool,
    candidate_since_ms: u64,
}

impl Debouncer {
    pub fn new(initial_high: bool) -> Self {
        Self {
            stable: initial_high,
            candidate: initial_high,
            candidate_since_ms: 0,
        }
    }

    pub fn update(&mut self, high: bool, now_ms: u64) -> Option<bool> {
        if high != self.candidate {
            self.candidate = high;
            self.candidate_since_ms = now_ms;
        }
        if self.candidate != self.stable
            && now_ms.saturating_sub(self.candidate_since_ms) >= INPUT_DEBOUNCE_MS
        {
            self.stable = self.candidate;
            return Some(self.stable);
        }
        None
    }
}

pub fn input_event(
    config: &InputConfig,
    high: bool,
    relay_on_or_pending: bool,
) -> Option<InputEvent> {
    let (action, publish_toggle) = match config.mode {
        InputMode::Normal => (None, false),
        InputMode::Toggle if !high => (None, true),
        InputMode::CopyToRelay if config.relay == Some(0) => {
            (Some(if high { Action::On } else { Action::Off }), false)
        }
        InputMode::RelayToggle if !high && config.relay == Some(0) => (
            Some(if relay_on_or_pending {
                Action::Off
            } else {
                Action::DelayedOn
            }),
            true,
        ),
        _ => return None,
    };
    Some(InputEvent {
        high,
        action,
        publish_toggle,
    })
}
