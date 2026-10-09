# SmartConfig C++ to Rust Migration

Reference: `C:\Users\kbdeir\Documents\PlatformIO\Projects\SmartConfig - AI`.
Destination: `C:\Users\kbdeir\Documents\PlatformIO\Projects\SmartConfig-RUST`.
Assessment date: 2026-10-09. The reference source and hardware mappings are the
behavioral baseline; improvements must be recorded rather than silently changing
the protocol or physical behavior.

## Active Reference Profile

`src/defines.h` selects `PROFILE_SINGLE_RELAY`. Its template selects `HWESP32`,
`_ACS712_`, `_AUTOMATION_RULES_`, `_REMOTE_SENSORS_`, and `HA_DISCOVERY`. Compile-time
support does not imply runtime activation: JSON settings also enable/disable
individual functions. Other profiles include single-relay automation, water
pressure, solar voltmeter, and double-relay without energy monitoring.

The Rust firmware now includes opt-in Wi-Fi, MQTT, authenticated HTTP/web,
Modbus server/client, four calendar schedules, four automation rules and eight
MQTT/Modbus remote sensors. Local sensor drivers remain pending. Automation
uses the additional ESP-IDF-ESPRelayNode8266 checkout as its behavioral and UI
reference. See [NETWORK.md](NETWORK.md) and [AUTOMATION.md](AUTOMATION.md).

## File Mapping

| C++ reference | Rust destination | Status |
| --- | --- | --- |
| `main.cpp` startup/local loop | `src/main.rs`, `src/app.rs` | Local foundation implemented |
| `defines.h`, selected template, `ConfigParams.h` pin definitions | `crates/node-core/src/lib.rs`, `src/app.rs` | Current single-relay board implemented |
| `RelayClass.cpp/h` | `crates/node-core/src/engine.rs`, `command.rs` | OFF boot, state, TTL, TTA, interlock, bounded dedup implemented |
| `InputClass.cpp/h`, `InputsArray.h` | `crates/node-core/src/input.rs`, `src/app.rs` | Digital modes/debounce implemented; temperature reserved |
| `JSONConfig.cpp/h`, `ConfigParams.cpp/h` | Core `config.rs`, `src/storage.rs` | Typed schema, selective import, NVS read/write and backup |
| `data/config.json`, `relay0.json`, `IRMAP.json` | `import-config.cmd`, ignored `config.local.json` | Selective single-relay importer implemented |
| `KSBWiFiHelper.cpp/h`, `Pings.cpp/h` | `src/services.rs` | STA/protected AP, status/scan/reconnect; progress health pending |
| `MQTT_Processes.cpp/h`, `HADiscovery.cpp/h` | `src/mqtt.rs`, core `protocol.rs` | Commands/state/TTL/input topics, LWT, reconnect; HA discovery pending |
| `Modbus.cpp/h`, `ModbusIP.cpp/h` | `src/modbus.rs`, core `modbus.rs` | Server, peer/remote polling and rule-driven FC5 coil writes |
| `AsyncHTTP_Helper.cpp/h`, `data/*.html` | `src/http.rs`, core `web.rs`, `management.rs`, `web/` | Original pages/styles, Basic auth, settings save; unsupported controls disabled |
| `TimerClass.cpp/h`, `Scheduletimer.cpp/h`, calendar timers | Core `schedule.rs`, `src/app.rs` | Four specific/daily/weekly/monthly timers; monotonic TTL/TTA retained |
| `AutomationRules.cpp/h`, `RemoteSensorConfig.cpp/h`, ESP8266 `data/Automation.html` | Core `automation.rs`, `runtime.rs`, MQTT/Modbus adapters, `web/Automation.html` | Four rules/eight remote sensors implemented; hardware acceptance pending |
| `ACS712.cpp/h`, `ACS_Helper.cpp/h` | Future ADC/current measurement and trip logic | Pending |
| `TempSensor.cpp/h`, `TempConfig.cpp/h` | Future OneWire/DS18B20 driver and control logic | Pending |
| `CT_ProcessPower.cpp/h`, `EmonLib.cpp/h`, `ADS11x5Config.cpp/h` | Future measurement modules | Pending |
| `TLPressureSensor.cpp/h`, `HSTConfig.cpp/h`, `WaterFlowSensor.cpp/h` | Future profile-specific measurement modules | Pending |
| `SR04_SERIAL.cpp/h`, ultrasonic/inverter/stepper paths | Future feature modules | Pending; inactive in current profile |
| `KSBAsyncNTP.cpp/h`, `KSBNTP.cpp/h`, `digitalClockDisplay.cpp/h` | `src/services.rs`, core schedule/web | SNTP and fixed UTC offset; DST pending |
| `WireGuardManager.cpp/h`, `WireGuardConfig.cpp/h` | Future validated WireGuard integration | Pending |
| `SerialLog.cpp/h` | ESP-IDF logging, future diagnostics controls | Startup/control logs implemented; web log settings pending |
| `ExecOTA.cpp/h`, update routes | ESP-IDF dual-slot OTA, rollback and staged LittleFS update | Implemented; signing and power-loss qualification pending |
| OLED, mesh, HomeKit, Alexa | Optional modules after core validation | Pending; HomeKit disabled in reference platform configuration |

## Compatibility Contracts

Relay HIGH means ON. Input-copy mode writes the raw debounced HIGH/LOW value,
not an inverted pressed value. Pull-ups therefore mean a released input can
turn ON a mapped copy-mode relay when it changes; no initial boot event is emitted.
Modes 0/1/2/3/4/5 map to none/toggle/normal/relay-toggle/copy/temperature.

Normal input mode produces telemetry on both edges. Toggle mode produces a
telemetry `tog` on falling edges. Only mapped relay-toggle/copy modes operate
relay 0. Temperature-mode GPIO33/GPIO16 are left untouched by digital acquisition.

Legacy `IRMAP` entries identify input GPIO numbers and zero-based relay numbers,
not input indices. Unknown GPIOs, other relays, and contradictory mappings are
rejected by the importer. Legacy string/number timer representations are supported.

Configured MQTT command/state/TTL/input topic names and live `on/off/tog` parsing
are implemented. Broad wildcard subscription, per-relay LWT, broker health echo
and Home Assistant discovery remain pending. Availability uses a new node topic.

## Intentional Changes

| Behavior | Reference | Rust foundation |
| --- | --- | --- |
| GPIO ownership | Several interfaces ultimately call `mdigitalWrite` | Only control loop writes relay GPIO |
| Repeated ON | Can retrigger change handling/timers | Idempotent; does not extend TTL |
| Pending TTA cancellation | Several independent timer/callback paths | OFF clears pending activation |
| TTA = 0 | FreeRTOS timer callback can run on its next 1-second tick | Immediate activation |
| Timer adjustment | Mixed software/FreeRTOS counters | Absolute monotonic deadlines |
| Configuration parsing | Loose string/number/default conversions | Typed schema and range/mapping validation |
| Boot restoration | Persistent/retained state paths exist | Always OFF; restoration intentionally deferred |
| Shared command/state topic echo | Counter-based suppression in MQTT callback | State-only publication; conflicting topics rejected |
| Retained MQTT commands | May restore broker state | Rejected; boot remains OFF |
| Actuator/settings GET requests | Some mutations over GET | Authenticated POST only, with custom header |
| Timer pause | May retain scheduled ON output | Active schedule transitions OFF; manual ON without an active schedule is unchanged |
| Input mode 0 save | JavaScript fallback could change 0 to normal | Preserved as disabled |

The interlock latch is core logic only. No current sensor, physical safety input,
or network policy automatically trips it yet.

## Local Validation

Run `test-core.cmd` for hardware-independent regression tests using stable Rust.
Run `compile.cmd` for the original ESP32 release image. A host CI workflow runs
the same core tests on Linux without building ESP-IDF.

Run `import-config.cmd` to convert the three reference JSON files into an ignored
bootstrap configuration. It refuses to overwrite an existing destination and
does not print credentials. The next build validates and embeds that file.
Absent a local file, the firmware uses default unmapped inputs and relay OFF.
Existing valid NVS settings have precedence over build bootstrap settings.

Hardware acceptance still required: verify HIGH/LOW polarity on the actual relay,
exercise each input/mode, compare TTL/TTA timing, and check OFF after reset/error.
Network outage/load, two-node, watchdog, OTA and multi-day hardware tests remain
required. Host/browser tests do not establish those guarantees.

The current board is on COM3 and requires manual reset after flashing. Notify the
operator when writing finishes, then monitor the normal boot without DTR/RTS reset
attempts. The control loop uses `FreeRtos::delay_ms(10)` rather than a sub-tick
standard-library sleep; a 5 ms sleep on this target was observed to starve IDLE0
and trigger task watchdog warnings.

`sdkconfig.defaults` allocates a 32 KiB main-task stack. The default 3584-byte
stack overflowed at the first scheduler yield after Rust configuration parsing
and control initialization. Heartbeats report the main task's minimum remaining
stack in bytes; input and configuration stress tests must also check that margin.
The expanded automation configuration overflowed the previous 16 KiB startup
stack. Main and HTTP now each budget 32 KiB for nested configuration parsing;
`/api/heap` also reports the HTTP task's minimum remaining stack.
