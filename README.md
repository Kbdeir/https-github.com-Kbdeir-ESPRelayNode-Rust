# ESPRelayNode-Rust

Independent experimental Rust firmware for the **original ESP32 / NodeMCU-32S**, developed in VS Code using Cargo, ESP-IDF and FreeRTOS.

The existing [ESPRelayNode C++ repository](https://github.com/Kbdeir/ESPRelayNode) is a reference only and will not be modified.

## Current status

**Single-relay network port:** GPIO2 LED blink, heartbeat, GPIO25 relay control, debounced inputs, TTL/TTA, Wi-Fi with protected setup AP, authenticated HTTP, MQTT, Modbus TCP server/client, four calendar timers, four automation rules and eight MQTT/Modbus remote sensors. Configuration and automation pages retain their reference styling. Configuration writes use NVS and restart with the relay OFF. Large HTML pages stream without allocating full-page copies.

All web pages/JS now reside in a separate 768 KiB LittleFS partition. The reference
File Manager supports upload, edit, download, rename and delete. Initial deployment
requires both application and filesystem flashes; see [docs/FILESYSTEM.md](docs/FILESYSTEM.md).

Firmware Maintenance and Backup & Restore use the original reference styling.
Authenticated application OTA, filesystem-image replacement, binary downloads,
streaming settings/web backups and defaults reset are implemented. OTA requires
the new dual-slot layout and matching rollback-enabled bootloader. See
[docs/MAINTENANCE.md](docs/MAINTENANCE.md) before the one-time COM3 migration.

Network services require explicit activation and a management password. Local temperature/current drivers, VPN, HTTPS, signed firmware, multi-relay profiles and a progress watchdog supervisor remain pending. This is not yet an industrially qualified controller. See [docs/NETWORK.md](docs/NETWORK.md) and [docs/AUTOMATION.md](docs/AUTOMATION.md) for behavior, security limits and hardware acceptance.

The local `SmartConfig - AI` checkout is the base reference. See [docs/MIGRATION.md](docs/MIGRATION.md) for file mappings and compatibility differences, and [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the implementation sequence.

## Local Windows utilities

This checkout includes local helper scripts for compile, flash and serial monitor from `C:\Users\kbdeir\Documents\PlatformIO\Projects\SmartConfig-RUST`.

One-time tool setup, after installing Rust with rustup:

```powershell
.\setup-tools.cmd
```

Build firmware:

```powershell
.\compile.cmd
```

Flash an ESP32 and open the serial monitor:

```powershell
.\flash.cmd -Port COM3
```

The current board needs a manual reset after flashing. Wait for `Flash complete`, then press RESET. The serial monitor reads UART directly without attempting a bootloader handshake or toggling reset lines. Close it before the next flash so COM3 is available.

If automatic bootloader entry is unavailable, hold BOOT/GPIO0 during RESET, release BOOT, then run `.\flash.cmd -Port COM3 -Baud 115200 -ManualBoot`. After `Flash complete`, press RESET with BOOT released.

Open only the serial monitor:

```powershell
.\monitor.cmd -Port COM3
```

Useful options:

```powershell
.\compile.cmd -Profile debug
.\flash.cmd -Port COM3 -Baud 921600 -MonitorBaud 115200 -NoMonitor
.\monitor.cmd -Port COM3 -Baud 115200
```

VS Code tasks are also available for setup, release compile, flash/monitor and monitor-only workflows. Build outputs are written to `C:\esp\rn` by default to avoid ESP-IDF path-length failures on Windows; override this with `CARGO_TARGET_DIR` if needed.

## Toolchain notes

The project is Cargo/ESP-IDF based, not PlatformIO based. The original ESP32 uses the Xtensa target `xtensa-esp32-espidf`, configured in `.cargo/config.toml`. `espup` installs the Espressif Rust toolchain, `ldproxy` links the firmware, and `espflash` flashes and monitors the board.

If you prefer WSL2, install Rust inside WSL2, run `cargo install espup --locked && espup install`, load `source ~/export-esp.sh`, install `ldproxy` and `espflash`, then build with `cargo build --release` and flash with `cargo run --release` after attaching USB with `usbipd-win`.

Verify installed ESP-IDF, Rust and dependency versions against the current official [esp-idf-template](https://github.com/esp-rs/esp-idf-template) before hardware testing.

## Core tests and reference configuration

```powershell
.\test-core.cmd
.\import-config.cmd
.\compile.cmd
```

The importer reads `data/config.json`, `data/relay0.json` and `data/IRMAP.json` from the local C++ project. It writes the ignored `config.local.json` and refuses to overwrite an existing file. It does not import sensors, schedules, VPN, or persisted ON states. Credentials in that local file are embedded in subsequent firmware builds; a sanitized schema is available in `config.example.json`.

For an authorized LAN deployment, use `.\import-config.cmd -EnableNetwork -AllowAnyModbus` instead of the plain import. This enables management, generates a private web/setup-AP password, and enables Modbus with the explicit `*` client allowlist. Omit `-AllowAnyModbus` for web/MQTT only; configure individual client IPv4 addresses before enabling Modbus. An existing local file must be edited rather than overwritten. Valid NVS settings take precedence over the build bootstrap.

Web login credentials are in `config.local.json` under `management`; they are not printed to UART or returned by read APIs. HTTP Basic does not encrypt traffic, and Modbus has no authentication. Use a trusted, isolated LAN and do not expose either service to the internet.

## Local UI Preview

Run `.\preview.cmd` for a loopback-only simulator, normally at `http://127.0.0.1:8090/`. Preview credentials are `user` / `preview-local`, unrelated to board credentials. It shares the firmware router and original pages but does not operate GPIO or persist to the board. `tools/verify-ui.cjs` checks desktop/mobile pages with Playwright and compares original CSS blocks.

Without a local configuration, inputs are unmapped and the relay boots OFF. For local relay-toggle control, set an input's `mode` to `relay_toggle` and `relay` to `0`; input indices follow GPIO33, 16, 17, 32, 26, 27. `copy_to_relay` preserves the reference's raw HIGH/LOW polarity. Temperature inputs are reserved but their drivers are pending.

## Roadmap

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Safety

This is experimental firmware, not a certified safety controller. Do not connect hazardous actuators before hardware-specific output defaults, interlocks and recovery tests are validated.
