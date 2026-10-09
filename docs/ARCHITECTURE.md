# Industrial ESP32 Rust Architecture

Target: original ESP32 / NodeMCU-32S, starting with the single-relay profile.
The local SmartConfig - AI checkout is read-only. This remains an experimental
port, not a qualified 10-50-node industrial deployment.

## Ownership and Services

The host-testable node-core library supplies the engine, schema, schedules,
MQTT parsing, automation rules, Modbus map and shared HTTP router. Firmware adapters are in
src/app.rs, services.rs, http.rs, mqtt.rs, modbus.rs and storage.rs. The preview
shares the router/original assets but simulates hardware.

Only the control loop drives GPIO25. Debounced local inputs run before remote
traffic. A 16-entry remote queue is nonblocking and drained at most four commands
per pass. Local OFF suppresses remote ON in that pass; it is not a lasting
interlock. An engine interlock latch exists, but sensor-driven trips are pending.

HTTP/Modbus wait for control acknowledgements in worker tasks. MQTT callbacks
only validate/copy bounded messages and enqueue commands. No socket or NVS
operation runs on the GPIO loop. Snapshot/event updates use nonblocking locks
from control; bounded buffers may drop outcomes, not block GPIO acquisition.

## Hardware and Timing

| Function | GPIO | Behavior |
| --- | --- | --- |
| LED | 2 | Alternates every 1000 ms |
| Relay 0 | 25 | HIGH = ON; starts OFF |
| Inputs 1-6 | 33,16,17,32,26,27 | Pull-ups, 10 ms debounce |
| Configuration button | 4 | Reserved; provisioning pending |

Control yields with FreeRtos::delay_ms(10). A shorter standard-library sleep
previously starved IDLE0. Main stack is 32 KiB; ten-second heartbeats report its
minimum remaining stack. GPIO errors attempt OFF; panic/power-loss recovery
requires hardware tests.

TTL/TTA use monotonic deadlines. TTA applies to physical toggle activation, not
direct remote ON. Repeated ON does not extend TTL. OFF cancels pending TTA and
reboot never restores ON. Calendar schedules use SNTP with a fixed UTC offset;
they remain inactive before plausible clock synchronization.

Four specific/daily/weekly/monthly windows are OR-combined for relay 0. Edges
issue commands, so manual OFF is not repeatedly overridden in an unchanged
window. Pause clears schedule demand, transitioning an active schedule OFF;
it does not cancel manual ON when no schedule was active. Resume evaluates the
current window rather than replaying missed time.

## Bounds

| Service | Configured limits |
| --- | --- |
| Control | 32 KiB stack; 16 remote queued; 4 drained/pass |
| Wi-Fi manager | 16 KiB stack; retries 1-60 seconds plus node jitter |
| HTTP | 32 KiB stack; 4 sockets; body/config 8192 bytes; streaming HTML chunks 768 bytes |
| MQTT | 12 KiB worker; 8 KiB IDF task; command payload 512 bytes |
| Modbus server | 8 KiB stack; 4 sockets; 260-byte frame limit |
| Modbus client | 8 KiB stack; 4 legacy peers, 8 remote sensors, 4 coalesced rule writes |
| Telemetry | 16 input events and 16 outcomes; no offline replay |

These are bounds, not measured worst-case latency guarantees. Peer operations
are serialized; a failed peer can delay other reads but not local control.
There is no task-per-peer or full mesh. Valid/stale peer status is explicit.
Automation evaluates every 100 ms in control, using debounced inputs, output
state and finite cached remote values. Workers alone perform remote I/O. Each
rule has one coalesced write slot, a revision and a short control-renewed lease;
superseded, stale or restarting demand cannot start another remote transaction.
Already-transmitted commands cannot be recalled. See [AUTOMATION.md](AUTOMATION.md).

## Protocol Contract

Adapters force command sources. Remote commands expire after one second in the
queue. IDs use a source-namespaced 32-entry dedup ring, current boot only; this
is not durable exactly-once execution. Legacy MQTT on/off/tog and TTL commands
are supported. Retained/oversized/fragmented commands are rejected. State is not
published onto command topics. Availability has an offline LWT; online follows
subscription acknowledgements. Disconnected input/outcome events are discarded.

Modbus uses rmodbus 0.12.2. Coil 0 remains relay 0; unsupported addresses return
exceptions. The implemented version-1 register map and service endpoints are
documented in [NETWORK.md](NETWORK.md).

## Configuration and Recovery

Typed schema 1 rejects unknown fields, invalid mappings, feedback topics and
out-of-range timers. Body/config size is 8192 bytes; TTL/TTA maximum seven days.
GPIO ownership is fixed by profile, not remotely selectable.

Load order is NVS relay_node/config, then relay_node/backup, then private build
bootstrap if neither record exists. Unrecoverable invalid records select safe
defaults. Valid NVS takes precedence over newly flashed bootstrap settings.

Saving validates, writes the previous configuration to the backup blob, then
writes the active blob. RAM configuration changes only after successful storage.
Non-Wi-Fi management saves persist and apply live through revisioned control and
service handoff. Only Wi-Fi credential changes or explicit Reboot schedule the
three-second restart with control holding OFF. See [LIVE-CONFIG.md](LIVE-CONFIG.md).
MQTT TTL updates persist outside control; Modbus TTL updates are runtime-only.

This is not staged candidate health verification or automatic network rollback.
The protected setup AP can recover unreachable station settings. NVS
interruption/recovery still needs fault injection and power-loss testing.

## Security and Remaining Work

Services require explicit management activation and a configured password.
HTTP Basic protects all pages/APIs; POST also requires a custom same-origin
header. Read APIs, rendered pages and UART do not disclose passwords. There is
no HTTPS or authenticated cross-board browser CORS support. Modbus has an
explicit IPv4 allowlist; deliberately configuring * permits any client. MQTT
currently uses imported broker credentials without TLS transport configuration.

Remaining: local sensor drivers/trip detection, Home Assistant discovery, VPN,
multi-relay profiles, signed OTA, TLS,
boot-session command identities, and full configuration health promotion.
Reported relay state is software/GPIO state, not measured relay-contact feedback.

Acceptance requires actual input/polarity/TTL/TTA checks, service-loss testing,
stack/heap load measurements, two-node peer reads, NVS power-loss injection,
and 24-hour followed by multi-day soak tests. Host/browser tests alone do not
establish hardware reliability. See [MIGRATION.md](MIGRATION.md).

Control progress is now subscribed to a resetting 10-second task watchdog.
Network/Modbus/MQTT worker progress is independently supervised; fatal stalls
request OFF-and-reboot recovery. See [RELIABILITY.md](RELIABILITY.md) for scope,
hardware limitations and the still-pending qualification tests.
