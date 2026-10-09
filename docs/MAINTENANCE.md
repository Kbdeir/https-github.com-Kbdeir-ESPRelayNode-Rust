# Firmware Maintenance and Backup & Restore

`/FirmwareMaintenance.html` and `/Backup.html` retain the SmartConfig - AI HTML/CSS
and controls. The Modbus link is an ordinary Tools-list entry on the main page;
it is no longer appended to the MQTT panel. Pages remain entirely in LittleFS.

## Initial Migration

The 4 MiB board needs two inactive/active OTA slots rather than overwriting the
running application. `partitions.csv` provides two 1638400-byte slots, OTA metadata
and 768 KiB LittleFS. NVS/PHY offsets are unchanged. Size-priority Rust optimization
and LTO are enabled; the build must fit each slot. The matching ESP-IDF bootloader
has rollback enabled and is explicitly passed to espflash, rather than using its
built-in bootloader. Back up edited web files before replacing the filesystem.

Enter download mode on COM3, then run:

```powershell
.\flash.cmd -Port COM3 -Baud 115200 -ManualBoot -NoMonitor -InitializeOta
.\flash-filesystem.cmd -Port COM3 -Baud 115200 -ManualBoot
```

Keep download mode between commands; press RESET only after both finish. Serial
flashes select ota_0 by erasing OTA metadata. Initial migration also erases ota_1.
Neither command erases controller NVS. The filesystem command replaces all web
files; an application-only flash preserves the filesystem after migration.

## Maintenance Operations

All endpoints require HTTP Basic authentication. Every mutation requires POST,
`X-SmartConfig: 1` and a fixed Content-Length; transfer-encoded uploads are rejected.
Firmware/filesystem uploads send raw `application/octet-stream`, not multipart.
The page retains its upload-progress controls but sends the selected File directly.
Errors permit retry and do not falsely report a reboot or successful update.

- `GET /api/maintenance`: slot size, filesystem size, readiness and busy state.
- `GET /api/firmware/download`: running application partition, including padding.
- `GET /api/filesystem/download`: complete raw LittleFS partition image.
- `POST /update`: application binary only, not ELF or a merged bootloader/partition
  image. ESP-IDF validates the inactive-slot image before activation. An interrupted
  upload aborts without changing boot selection. The controller inhibits relay and
  automation output during the upload and reboots with relay OFF on success.
- `POST /updatefs`: exact-size LittleFS image. Stage in the inactive app slot,
  mount read-only without formatting, and verify required pages before committing
  a CRC-protected NVS recovery marker. After reboot, before mounting live LittleFS,
  install and verify the staged image. A power interruption during copying retains
  the marker so the install retries. This consumes the inactive application copy;
  it does not activate that slot as firmware. The running app is not overwritten.

Uploads are gated until 30 seconds of control operation and successful filesystem
and HTTP startup. Pending OTA images are then marked valid; an earlier restart
allows bootloader rollback. This is a startup health check, not a watchdog or an
industrial qualification. Never power-cycle deliberately during a field update.
No signature or secure-boot checks are configured: authenticated operators may
install any compatible ESP32 application. Crash-dump storage is not enabled, and
the maintenance page reports that explicitly rather than claiming a dump was read.

## Backup and Restore

`GET /api/backup?includeWeb=0|1` streams the reference version-1 JSON envelope:
`_backup_version`, `_chipid`, `_includesWeb`, `_files`, and base64 file entries.
The virtual `/rust-config.json` contains the complete active NVS configuration,
including passwords, Wi-Fi, MQTT, Modbus, timers, rules and remote sensors. It is
never written into public web storage. Other filesystem JSON files are included;
`includeWeb=1` includes all filesystem assets. Keep downloaded backups private.
Ordinary status/configuration APIs remain redacted. Response chunks are <=768 bytes.

`POST /api/restore?chipIdMode=keep|replace|original&restoreWeb=0|1` accepts up to
2 MiB, with at most 128 filesystem entries, bounded metadata and 256 KiB entries.
Struson parses JSON and base64 decoding streams into bounded buffers; no complete
backup, file string or web page is materialized in device RAM. Duplicate keys,
invalid paths, unsupported versions, corrupt base64, trailing data and incomplete
uploads are rejected before live changes.

Settings restore validates `/rust-config.json` and persists it through the existing
NVS recovery mechanism. Keep mode leaves topics unchanged. Replace/original modes
rewrite all 6- or 12-hex-character `Controller<ID>` topic references to the live
board ID or backup metadata ID. Passwords and unrelated strings are never rewritten.
Original C++ setting files cannot be silently applied to the Rust schema; use
`import-config.cmd` for supported legacy settings first. Arbitrary filesystem JSON
artifacts are not active NVS settings and are left untouched during restore.

Web-only restore does not modify NVS or JSON files. It stages all selected non-JSON
assets, validates the complete envelope, then installs the restored web set and
removes obsolete non-JSON assets. It requires space for old and staged assets.
Preserve IDs in web-only mode so binary/gzip content remains byte-exact. A synced,
atomically published journal retains replaced/removed originals; failed or
interrupted commits roll back at runtime or the next filesystem mount. Hidden
staging files never appear in File Manager. Successful restore schedules a reboot
with relay OFF; validation or persistence failure does not schedule a restart.

`POST /api/resetconfig` resets controller settings while retaining schedules/rules
but disabling them, preserves the authenticated management login and all web files,
and disables MQTT/Modbus. It regenerates the five relay-0 MQTT topics and six input
telemetry topics as `/home/Controller<live node ID>/...`, matching the reference
`saveRelayDefaultConfig()` and `saveDefaultConfig()` path names. Inputs return to
Normal mode with no relay mappings; relay TTL/TTA return to zero. Supported system
defaults match SmartConfig - AI: location `Not configured yet`, Wi-Fi from the
private build bootstrap, broker `192.168.1.1:1883`, keepalive 30 seconds, NTP
`162.159.200.123`, and UTC offset +2. The setup AP remains available at
`192.168.71.1`; station Wi-Fi attempts that privately configured network. No real
Wi-Fi password is stored in the published factory-default source. This intentional
factory-reset profile is separate from fail-safe `Config::default()`, which stays
empty/unmapped for recovery. It does not erase NVS wholesale or change the hardware
node ID. Unsupported C++ peripherals do not get synthetic configuration files.

## Verification

Host tests cover secret-preserving settings round trips, bounded raw-asset backup,
web-set replacement, malformed/interrupted input, size/name/query limits, NVS
failure, topic-ID modes, defaults reset and journal rollback. The browser verifier
checks all 12 pages at desktop/mobile widths, unchanged reference CSS, the Modbus
link position, backup download/restore and unsuccessful upload retry. Its preview
mutations are loopback-only; board mode is read-only.

Hardware acceptance on COM3 (2026-10-09): the migrated board at `10.92.222.190`
passed both authorized HTTP OTA workflows using the same application and LittleFS
images. Both rebooted automatically, passed the 30-second startup health check,
and preserved controller NVS settings byte-for-byte. The serial capture recorded
only the two intentional software resets, with no panic, stack overflow or watchdog
reset. MQTT connection timeouts remain expected while its broker is unreachable.
Post-update checks verified all 14 assets, complete backups, authenticated routes,
and byte-identical running application downloads. Host coverage totals 92 passing
tests; all 12 board pages were checked at desktop and mobile widths. Hardware
settings restore/reset, forced rollback and deliberate power-loss fault injection
remain untested; successful OTA installation is not evidence for those cases.

Factory-reset correction (2026-10-09): 95 host tests pass, including generated
relay/input topics surviving persistence and configuration-API reload, retention
of disabled schedules/login/web files, and transactional NVS-save failure. The
loopback browser test presses the reset button and verifies populated topic paths,
reference network values, and disabled retained schedules after simulated reboot.
Strict host Clippy and formatting checks pass. The revised ESP32 application is
1580896 bytes and fits its 1638400-byte OTA slot; the 14-asset LittleFS image is
verified. This correction has not yet been flashed or reset-tested on hardware.

Design follows the [ESP-IDF OTA API](https://docs.espressif.com/projects/esp-idf/en/v5.3.3/esp32/api-reference/system/ota.html)
and uses [Struson's streaming reader](https://docs.rs/struson/0.7.2/struson/).
