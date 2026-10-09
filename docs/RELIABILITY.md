# Unattended Recovery

The target is five years of dependable service with automatic recovery, not a
guarantee of uninterrupted process uptime. This implementation is not yet
hardware-qualified for that target. The approved recovery policy is relay OFF
and automatic reboot; physical relay-contact feedback is not implemented.

## Control and Watchdog

ESP-IDF's task watchdog is configured for 10 seconds, panic and reboot, with
both CPU idle tasks monitored. The actual control task subscribes after input
and configuration initialization. Only a completed control cycle feeds its
subscription. A different task cannot keep a blocked control loop alive.

The ESP32-specific watchdog timeout hook is placed in IRAM. It writes the
GPIO25 clear register directly, without allocation, logging, locks or flash
calls, before the task-watchdog panic path. Normal software restarts also have
a shutdown hook that drives GPIO25 LOW. Boot initializes the relay OFF before
reading configuration. A returned control/initialization error attempts OFF
and restarts after a 60-second delay rather than abandoning the control task.

This is software recovery, not a certified independent output interlock. An
external pull-down/driver must hold the active-HIGH relay OFF while GPIO25 is
reset or unconfigured. A stuck CPU, power fault, failed relay or welded contacts
requires appropriate hardware protection; GPIO state is not contact feedback.
The interrupt watchdog and brownout detector remain enabled.

## Service Progress

Network, enabled Modbus server/client, and enabled MQTT adapter have separate
atomic progress timestamps. An unexpected task exit is latched; 60 seconds
without progress is a fault. The control loop schedules the existing three-second
relay-OFF reboot path only after at least 60 seconds of boot uptime. Recovery
does not acquire service/configuration mutexes or postpone an existing settings
or OTA restart. Failed services also block OTA startup-health promotion.

The 60-second minimum limits rapid reboot cycles; it is not an adaptive persistent
boot-loop escape or safe-mode implementation. A persistent internal failure can
still produce repeated recovery reboots and must be diagnosed.

Absent Wi-Fi, failed broker connections and unreachable Modbus peers are not
faults while their worker loops continue making progress. Existing Wi-Fi/MQTT
reconnect and bounded Modbus transactions remain in effect. Supervision measures
task progress, not end-to-end broker delivery or physical actuation. HTTP is
not independently probed; a live network loop alone does not prove every HTTP
handler is responsive.

`GET /api/status` includes `health`: each service's name, expected/failed flags
and seconds since progress. Reset reason is logged at startup. Heartbeats and
supervision do not write NVS, so this addition does not introduce periodic flash
wear. Persistent crash history and MQTT outbox/load endurance still need work.

## Verification and Qualification

Host tests check inactive services, timeout boundaries, exited-task latching,
concurrent newer timestamps, wrap handling, isolated progress, recovery cooldown,
and atomic scheduling without service locks or overwrite of existing reboots.
Timestamps beyond 49 days and five years are simulated; this is not a real soak.
The ESP32 image must be built and its linked ISR checked for IRAM placement and
absence of flash/function calls. The generated SDK configuration must show
`CONFIG_ESP_TASK_WDT_PANIC=y` and a 10-second timeout.

Before unattended deployment:

1. On a protected bench, use a temporary instrumented build to stall control
   and separately stop/stall each service. Measure GPIO25 and actual relay
   contacts through timeout/reset and confirm settings survive. Do not leave
   fault-injection controls in production. OpenOCD/JTAG can disable watchdogs;
   debugger halting is not a valid watchdog acceptance test.
2. Repeat AP/broker outages, reconnects, malformed traffic, slow HTTP clients,
   socket saturation and high-rate Modbus/MQTT traffic. Confirm outages do not
   cause needless restarts and heap/stack margins do not trend downward.
3. Repeat application/filesystem OTA with watchdogs enabled, including startup
   rollback and controlled power interruptions during NVS/filesystem writes.
4. Run 24-hour, multi-day and multi-week mixed-load soaks on multiple boards.
   Record reset reasons, heap minima/largest blocks, stack margins, queue drops,
   reconnects, and contact behavior. Investigate every unexplained restart.
5. Qualify supply/surge/EMI protection, temperature, enclosure and relay switching
   life against the actual load, switching rate and component specifications.

These hardware tests have not been performed for the recovery-enabled image.
The recovery-only change did not require a filesystem update. The subsequent
live-configuration/UI update requires both application and LittleFS images;
see [LIVE-CONFIG.md](LIVE-CONFIG.md) for current deployment instructions.

Recovery-only build verification (2026-10-09, before the live-configuration update):
all 105 host tests, strict host Clippy, formatting
and ESP32 release compilation pass. The application is 1585712 bytes in a
1638400-byte slot (52688 bytes remaining). ELF inspection places the 16-byte
watchdog hook at `0x400810a4` in `.iram0.text`; its two literal loads also point
into IRAM, and it contains a GPIO clear write and return with no function calls.
The generated SDK configuration confirms the resetting 10-second watchdog.
The image has not been flashed by this task.

Implementation follows the [ESP-IDF v5.3.3 watchdog documentation](https://docs.espressif.com/projects/esp-idf/en/v5.3.3/esp32/api-reference/system/wdts.html).
