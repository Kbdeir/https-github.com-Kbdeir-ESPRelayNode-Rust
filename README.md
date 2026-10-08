# ESPRelayNode-Rust

Independent experimental Rust firmware for the **original ESP32 / NodeMCU-32S**, developed in VS Code using Cargo, ESP-IDF and FreeRTOS.

The existing [ESPRelayNode C++ repository](https://github.com/Kbdeir/ESPRelayNode) is a reference only and will not be modified.

## Current status

**Phase 0 only:** ESP-IDF startup, logging and serial heartbeat. MQTT, Modbus TCP, web configuration, I/O and OTA are planned, **not implemented**. The firmware has not yet been compiled or tested on hardware.

## Windows development environment

Recommended: VS Code + WSL2 Ubuntu + rust-analyzer.

1. Install Rust with rustup inside WSL2.
2. Install Espressif tooling: `cargo install espup --locked && espup install`.
3. Load the environment: `source ~/export-esp.sh`.
4. Install utilities: `cargo install ldproxy --locked` and `cargo install espflash --locked`.
5. Build: `cargo build --release`.
6. Flash: `cargo run --release` (after attaching the USB device to WSL with usbipd-win).

The original ESP32 uses the Xtensa target `xtensa-esp32-espidf`. Verify installed ESP-IDF, Rust and dependency versions against the current official [esp-idf-template](https://github.com/esp-rs/esp-idf-template) before building.

## Roadmap

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Safety

This is experimental firmware, not a certified safety controller. Do not connect hazardous actuators before hardware-specific output defaults, interlocks and recovery tests are validated.
