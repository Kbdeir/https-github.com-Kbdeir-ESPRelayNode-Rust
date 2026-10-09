use anyhow::Result;
use esp_idf_svc::hal::{
    delay::FreeRtos,
    gpio::{AnyIOPin, IOPin, Input, PinDriver, Pull},
    peripherals::Peripherals,
    task::watchdog::{TWDTConfig, TWDTDriver},
};
use node_core::{
    command::{Action, Source},
    protocol::command,
    runtime::{Runtime, RuntimeSnapshot},
};
use node_core::{
    command::{Command, Outcome},
    config::InputMode,
    engine::Engine,
    input::{input_event, Debouncer},
    CONTROL_TICK_MS, INPUT_GPIOS,
};
use std::sync::atomic::Ordering;

pub fn run() -> Result<()> {
    let peripherals = Peripherals::take()?;
    let mut relay = PinDriver::output(peripherals.pins.gpio25)?;
    relay.set_low()?;
    crate::recovery::initialize()?;

    // Keep the output OFF even if configuration or input initialization fails.
    let result = (|| -> Result<()> {
        let mut config = crate::storage::load()?;
        let nvs = esp_idf_svc::nvs::EspDefaultNvsPartition::take()?;
        crate::firmware::recover_filesystem(&nvs)?;
        let mut mac = [0u8; 6];
        esp_idf_svc::sys::esp!(unsafe {
            esp_idf_svc::sys::esp_read_mac(
                mac.as_mut_ptr(),
                esp_idf_svc::sys::esp_mac_type_t_ESP_MAC_WIFI_STA,
            )
        })?;
        let mac_string = mac
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(":");
        let node_id = format!("{:02x}{:02x}{:02x}", mac[3], mac[4], mac[5]);
        let (runtime, receiver) = Runtime::new_with_factory_wifi(
            config.clone(),
            node_id,
            mac_string,
            crate::storage::factory_wifi()?,
        );
        let _network = if config.management.enabled {
            Some(crate::services::start(
                peripherals.modem,
                runtime.clone(),
                nvs,
            )?)
        } else {
            None
        };
        let mut led = PinDriver::output(peripherals.pins.gpio2)?;
        led.set_high()?;
        let pins: [AnyIOPin; 6] = [
            peripherals.pins.gpio33.downgrade(),
            peripherals.pins.gpio16.downgrade(),
            peripherals.pins.gpio17.downgrade(),
            peripherals.pins.gpio32.downgrade(),
            peripherals.pins.gpio26.downgrade(),
            peripherals.pins.gpio27.downgrade(),
        ];
        let mut inputs: [Option<PinDriver<'static, AnyIOPin, Input>>; 6] =
            std::array::from_fn(|_| None);
        let mut debouncers: [Debouncer; 6] = std::array::from_fn(|_| Debouncer::new(true));
        let mut input_values = [false; 6];
        let mut valid_mask = 0;
        for (index, pin) in pins.into_iter().enumerate() {
            let mut driver = PinDriver::input(pin)?;
            if matches!(
                config.inputs[index].mode,
                InputMode::None | InputMode::Temperature
            ) {
                if config.inputs[index].mode == InputMode::Temperature {
                    log::warn!(
                        "GPIO{} reserved for temperature; driver pending",
                        INPUT_GPIOS[index]
                    );
                }
                inputs[index] = Some(driver);
                continue;
            }
            driver.set_pull(Pull::Up)?;
            debouncers[index] = Debouncer::new(driver.is_high());
            input_values[index] = driver.is_high();
            valid_mask |= 1 << index;
            inputs[index] = Some(driver);
        }

        let mut engine = Engine::new(&config.relay);
        let mut config_revision = 0;
        let mut schedules = config
            .timers
            .iter()
            .map(|t| t.compile())
            .collect::<Result<Vec<_>, _>>()
            .map_err(anyhow::Error::msg)?;
        let mut timer_states = [false; 4];
        let mut automation = node_core::automation::Automation::default();
        let mut next_automation_ms = 0;
        let mut next_schedule_ms = 0;
        let mut schedule_on = false;
        let mut next_led_ms = 1000;
        let mut next_heartbeat_ms = 0;
        let mut previous = engine.snapshot();
        let mut watchdog = TWDTDriver::new(peripherals.twdt, &TWDTConfig::default())?;
        let mut control_watch = watchdog.watch_current_task()?;
        log::info!("ESPRelayNode-Rust: control boot OK; GPIO25 relay OFF");
        log::info!("Status LED: GPIO2, toggle interval 1000 ms");
        log::info!("Control tick: {CONTROL_TICK_MS} ms; input debounce: 10 ms");

        loop {
            let now_ms = runtime.now_ms();
            let now_seconds = (now_ms / 1000) as u32;
            if let Some(updated) = runtime.config_update(&mut config_revision) {
                if updated.relay.ttl_seconds != config.relay.ttl_seconds
                    || updated.relay.tta_seconds != config.relay.tta_seconds
                {
                    engine.configure_changed(&updated.relay, &config.relay, now_ms);
                }
                if updated.automation != config.automation
                    || updated.remote_sensors != config.remote_sensors
                    || updated.inputs != config.inputs
                    || updated.timers != config.timers
                {
                    let changed = automation.configure(&updated, &config);
                    for (index, changed) in changed.into_iter().enumerate() {
                        if changed {
                            runtime.cancel_remote_write(index);
                        }
                    }
                    next_automation_ms = 0;
                }
                if updated.timers != config.timers
                    || updated.network.utc_offset_hours != config.network.utc_offset_hours
                {
                    schedules = updated
                        .timers
                        .iter()
                        .map(|t| t.compile())
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(anyhow::Error::msg)?;
                    next_schedule_ms = 0;
                }
                for index in 0..inputs.len() {
                    if updated.inputs[index] != config.inputs[index] {
                        let driver = inputs[index].as_mut().unwrap();
                        let active = !matches!(
                            updated.inputs[index].mode,
                            InputMode::None | InputMode::Temperature
                        );
                        driver.set_pull(if active { Pull::Up } else { Pull::Floating })?;
                        debouncers[index] = Debouncer::new(driver.is_high());
                        input_values[index] = driver.is_high();
                        if active {
                            valid_mask |= 1 << index;
                        } else {
                            valid_mask &= !(1 << index);
                        }
                    }
                }
                config = updated;
                log::info!("Configuration applied live; revision {config_revision}");
            }
            if let Some(service) = runtime.check_service_health(now_ms) {
                log::error!(
                    "{} lost progress; relay OFF and automatic reboot",
                    service.name()
                );
            }
            let mut local_off = false;
            engine.tick(now_ms);
            if now_ms >= next_schedule_ms {
                let now = node_core::schedule::local_now(config.network.utc_offset_hours);
                for (index, timer) in schedules.iter().enumerate() {
                    timer_states[index] = now.is_some_and(|date| timer.active(date));
                }
                let on = !runtime.schedule_paused.load(Ordering::Relaxed)
                    && schedules
                        .iter()
                        .enumerate()
                        .any(|(i, t)| t.relay() == Some(0) && timer_states[i]);
                if on != schedule_on {
                    engine.apply(
                        &command(
                            if on { Action::On } else { Action::Off },
                            Source::Automation,
                        ),
                        now_ms,
                    );
                    local_off = !on;
                    schedule_on = on;
                }
                next_schedule_ms = now_ms + 1000;
            }
            for index in 0..inputs.len() {
                if valid_mask & (1 << index) == 0 {
                    continue;
                }
                let Some(driver) = inputs[index].as_ref() else {
                    continue;
                };
                let Some(high) = debouncers[index].update(driver.is_high(), now_ms) else {
                    continue;
                };
                input_values[index] = high;
                let state = engine.snapshot();
                if let Some(event) = input_event(
                    &config.inputs[index],
                    high,
                    state.on || state.tta_remaining_seconds > 0,
                ) {
                    log::info!(
                        "Input GPIO{}: {}; mode {:?}",
                        INPUT_GPIOS[index],
                        if high { "HIGH" } else { "LOW" },
                        config.inputs[index].mode
                    );
                    if let Some(action) = event.action {
                        local_off |= action == Action::Off;
                        engine.apply(&Command::local(action), now_ms);
                    }
                    if let Ok(mut events) = runtime.input_events.try_lock() {
                        if events.len() < 16 {
                            events.push_back((index, high));
                        } else {
                            runtime.overflows.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
            let restarting = runtime.restart_at.load(Ordering::Acquire) > 0
                || runtime.maintenance.load(Ordering::Acquire);
            if now_ms >= next_automation_ms || restarting {
                let sources = RuntimeSnapshot {
                    relay: engine.snapshot(),
                    inputs: input_values,
                    input_valid_mask: valid_mask,
                    timers: timer_states,
                    ..Default::default()
                };
                runtime.automation_tick(&mut automation, &config, &mut engine, &sources, local_off);
                next_automation_ms = now_ms + 100;
            }
            if restarting {
                engine.apply(&Command::local(Action::Off), now_ms);
            }
            for _ in 0..4 {
                let Ok(envelope) = receiver.try_recv() else {
                    break;
                };
                let outcome = if restarting
                    || (local_off
                        && !matches!(envelope.command.action, Action::Off | Action::SetTtl(_)))
                {
                    Outcome::Unauthorized
                } else {
                    engine.apply(&envelope.command, now_ms)
                };
                if engine.snapshot().on {
                    relay.set_high()?;
                } else {
                    relay.set_low()?;
                }
                if let Ok(mut snapshot) = runtime.snapshot.try_lock() {
                    snapshot.relay = engine.snapshot();
                }
                if let Some(reply) = envelope.reply {
                    let _ = reply.try_send(outcome);
                }
                if let Ok(mut outcomes) = runtime.outcomes.try_lock() {
                    if outcomes.len() < 16 {
                        outcomes.push_back((envelope.command, outcome));
                    }
                }
                if outcome != Outcome::Applied {
                    log::warn!("Command rejected: {outcome:?}");
                }
            }
            let state = engine.snapshot();
            if state.on != previous.on {
                if state.on {
                    relay.set_high()?;
                } else {
                    relay.set_low()?;
                }
                log::info!("Relay 0: {}", if state.on { "ON" } else { "OFF" });
            }
            previous = state;
            if let Ok(mut snapshot) = runtime.snapshot.try_lock() {
                *snapshot = RuntimeSnapshot {
                    relay: state,
                    inputs: input_values,
                    input_valid_mask: valid_mask,
                    timers: timer_states,
                    ttl_seconds: engine.ttl_seconds(),
                    tta_seconds: config.relay.tta_seconds,
                    stack_margin_bytes: unsafe {
                        esp_idf_svc::sys::uxTaskGetStackHighWaterMark(std::ptr::null_mut())
                    },
                    uptime_seconds: (now_ms / 1000) as u32,
                    automation: automation.status(),
                };
            }
            if restarting
                && runtime.restart_at.load(Ordering::Acquire) > 0
                && !state.on
                && now_ms / 1000 >= u64::from(runtime.restart_at.load(Ordering::Acquire))
            {
                unsafe { esp_idf_svc::sys::esp_restart() };
            }
            if now_ms >= 30_000
                && runtime.network_ready.load(Ordering::Acquire)
                && runtime.health.fault(now_seconds).is_none()
                && !runtime.ota_confirmed.load(Ordering::Acquire)
            {
                if let Err(error) = crate::firmware::confirm_running() {
                    log::error!("OTA health confirmation failed: {error}");
                } else {
                    runtime.ota_confirmed.store(true, Ordering::Release);
                }
            }
            if now_ms >= next_led_ms {
                led.toggle()?;
                next_led_ms = now_ms + 1000;
            }
            if now_ms >= next_heartbeat_ms {
                // A null task handle measures the current task; IDF reports bytes.
                let stack_margin =
                    unsafe { esp_idf_svc::sys::uxTaskGetStackHighWaterMark(std::ptr::null_mut()) };
                log::info!("ESPRelayNode-Rust heartbeat; relay={}, ttl_remaining={}, tta_remaining={}, queue_overflows={}, stack_margin_bytes={stack_margin}",
                    state.on, state.ttl_remaining_seconds, state.tta_remaining_seconds, runtime.overflows.load(Ordering::Relaxed));
                next_heartbeat_ms = now_ms + 10_000;
            }
            // Yield at least one scheduler tick so the idle-task watchdog is serviced.
            control_watch.feed()?;
            FreeRtos::delay_ms(CONTROL_TICK_MS as u32);
        }
    })();
    if let Err(error) = relay.set_low() {
        log::error!("Relay OFF driver error: {error}");
        unsafe {
            std::ptr::write_volatile(
                esp_idf_svc::sys::GPIO_OUT_W1TC_REG as *mut u32,
                1 << node_core::RELAY_GPIO,
            )
        };
    }
    result
}
