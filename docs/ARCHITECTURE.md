# Industrial ESP32 Rust architecture roadmap

Target: 10–50 original ESP32 / NodeMCU-32S devices on managed Wi-Fi.

## Phase 0 — Boot
- ESP-IDF Rust startup and serial heartbeat.

## Phase 1 — Wi-Fi and MQTT
- Wi-Fi station, reconnect with bounded exponential backoff.
- MQTT publish/subscribe, Last Will and Testament, deduplicated command IDs.
- Bounded event queues and offline behavior.

## Phase 2 — Bidirectional Modbus TCP
- Concurrent server and client roles on each controller.
- Explicit register ownership and read/write permissions.
- Bounded peer connections and request timeouts.
- Validate MBAP header and protocol framing.

## Phase 3 — Web configuration
- Authenticated configuration API and static web interface.
- Validate and stage settings before commit.
- NVS persistence, versioning and recovery.

## Phase 4 — Local control
- Single-writer relay state machine.
- Debounced inputs, TTL/TTA timers and local interlocks.
- Defined safe states after boot, disconnect and watchdog recovery.

## Phase 5 — Industrial reliability
- Task watchdog, network health metrics, reconnect tests.
- OTA with rollback, soak tests and fault injection.
- TLS where supported and segmented Modbus network access.

## Core invariants
1. No network callback writes a physical output directly.
2. Local safety behavior never depends on broker or Wi-Fi availability.
3. Commands are validated, bounded, acknowledged and deduplicated.
4. The original C++ repository is never modified.

## Important
All phases after Phase 0 are specifications, not implemented features.
