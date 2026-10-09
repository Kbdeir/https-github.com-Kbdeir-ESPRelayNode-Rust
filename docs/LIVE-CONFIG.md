# Live Configuration

Non-Wi-Fi saves persist to NVS and notify the running control/services without a
board reboot. Control refresh uses a non-blocking settings lock. Changed input
pulls, mappings, relay TTL/TTA, schedules, rules and remote definitions are applied
at the next control cycle. MQTT client/subscriptions and Modbus listeners/peers
are retired and replaced only for relevant edits; existing bounded transactions
may finish first. SNTP changes recreate its client independently of Wi-Fi/HTTP.

Planned worker retirement disarms its health guard. Unexpected exits still latch
a fault. Old MQTT callbacks/Modbus readings are rejected by service revision;
affected remote write leases and cached readings are invalidated. Changes do not
clear the output interlock. MQTT message generations persist across replacement
clients so old packet IDs cannot collide with a new client's commands.

Only SSID/password edits schedule an automatic settings reboot. Explicit Reboot
uses authenticated POST with the same-origin header and inhibits outputs. Backup
restore, factory reset and OTA still reboot intentionally. Changing the web
password applies to HTTP immediately; the AP password changes on Wi-Fi restart.

## Deployment

Upload `C:\esp\rn\application-ota.bin` with Firmware Maintenance's application
uploader, and `C:\esp\rn\littlefs.bin` with its filesystem uploader. Both are
needed for the reboot control and fixed Live Readings heap display. Filesystem
upload replaces web files, not NVS settings; back up customized web files first.
No partition migration or factory reset is needed.

## Verification

Host regressions cover every settings-save route, failed persistence, live
TTL/TTA behavior, non-blocking control refresh, source invalidation, rule-state
preservation, worker health retirement, reboot POST requirements, and real TCP
Modbus port/unit/allowlist changes and repeated disable/re-enable.
Browser checks use a loopback simulator, including ON preservation across a
non-Wi-Fi save and immediate expiry after shortening TTL, and numeric heap
display/Reboot layout at desktop and mobile widths.

The physical board has not been updated or actuated for this change. Live MQTT
reconfiguration, GPIO mode changes and heap/stack endurance still require
protected-bench hardware verification; passing host tests is not five-year
qualification.

Factory-reset Wi-Fi credentials come from the ignored private build bootstrap,
not hardcoded published source. The board retains its configured reset network.

Verified 2026-10-09: 121 host tests, strict host Clippy, ESP32 release build and
LittleFS mount/asset checks pass. Playwright checks all 12 pages at 1440px and
390px widths, live saves, explicit reboot, numeric heap values and unchanged
reference CSS. No NVS reset or partition-layout change is included.
