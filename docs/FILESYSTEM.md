# LittleFS Web Assets

All HTML configuration/live/help pages and `rust-ui.js` are loaded from the
LittleFS VFS mount `/littlefs`. No page is embedded in the Rust executable and
there is no embedded fallback or boot-time asset seeding. Missing/unreadable
storage produces 503 while authenticated management APIs remain available.
Mount failure never automatically formats the partition.

The current 4 MiB layout retains NVS at 0x9000 and PHY at 0xf000. OTA metadata
occupies 0x10000-0x11fff; two 0x190000-byte application slots start at 0x20000
and 0x1b0000. LittleFS occupies 0x340000-0x3fffff (768 KiB). The prior single-app
layout had a 1 MiB filesystem at 0x300000 and is not OTA-capable. Migration
requires both application and matching filesystem flashes before RESET. It
preserves NVS but replaces web files. See [MAINTENANCE.md](MAINTENANCE.md).

## Build and Flash

`compile.cmd` builds the application. `build-filesystem.cmd` independently packs
`web/` using pinned littlefs-python 0.19.0, block size 4096, size from `partitions.csv`,
on-disk name limit 255 (the API restricts paths to 63 bytes). The script installs
the image tool in an isolated environment under the build target directory on
first use, then mounts and verifies the generated image and every asset. The
installed PlatformIO mklittlefs produces a 32-byte on-disk name limit, so it is
not used for this port's image.

For the initial migration, enter download mode on COM3 and run:

```powershell
.\flash.cmd -Port COM3 -Baud 115200 -ManualBoot -NoMonitor -InitializeOta
.\flash-filesystem.cmd -Port COM3 -Baud 115200 -ManualBoot
```

Keep the board in download mode between these two commands. After both finish,
release BOOT and press RESET. Neither command erases NVS. The filesystem command
replaces the complete filesystem, including any user-uploaded files. Normal
application-only flashes preserve LittleFS and select ota_0 by resetting OTA
metadata; flash web assets only when explicitly
replacing them. Back up modified pages before flashing another filesystem image.

## File Manager

`/Files.html` retains the ESP-IDF-ESPRelayNode8266 reference styling and provides
file/storage listing, filtering, selection, text editing, upload, raw download,
rename and delete. HTTP Basic applies to all routes. POST requires
`X-SmartConfig: 1`; GET cannot mutate files. Filesystem edits do not update the
typed NVS controller configuration and do not restart or actuate the relay.

Paths are flat absolute names (ASCII letters/digits, underscore, hyphen, dot),
up to 63 bytes excluding the leading slash. Traversal, subdirectories, hidden
files and symlinks are rejected. At most 128 files and 256 KiB per upload are
accepted. Uploads use 768-byte chunks and a hidden temporary file; an incomplete
upload is removed without replacing the old destination. Space is reserved for
both the old file and temporary replacement. Rename refuses existing destinations.

Plain HTML renders template tokens while streaming with bounded buffers, including
tokens spanning read boundaries. Raw file downloads do not substitute tokens.
Static `.gz` alternatives are served with gzip encoding only when the plain file
is absent. Token-bearing HTML must remain uncompressed; the page checks uppercase
and lowercase token names. Editing/deleting executable web assets is privileged
and can break the UI. Restore them through the filesystem flash utility.

The preview uses a separate temporary directory copied from `web/`, never edits
the source assets and does not access board storage. Its storage usage is a block
estimate; firmware usage comes from `esp_littlefs_info`. Moving flash-resident
pages to LittleFS removes them from the executable, but is not a remedy for the
separate task-stack RAM budget. Power-loss testing and a long-duration soak remain
required before production deployment.

## Hardware Acceptance

On 2026-10-09, the application and initial image were deployed to the 4 MiB
ESP32 on COM3, preserving its NVS configuration. LittleFS mounted successfully
with 12 assets occupying 249856 bytes. All 10 management pages passed authenticated
desktop/mobile browser checks, including File Manager, with unchanged reference
CSS. Forty concurrent page requests completed without HTTP 503 or a restart;
free heap measured 79392 bytes before and 79616 bytes after. HTTP stack margin
was 23268 bytes; control-task margin was 12780 bytes.

`tools/verify-board-files.cjs` verified a 105600-byte scratch upload, replacement,
raw download, rename and deletion against the board. It removed the scratch file
and confirmed that the original asset listing was unchanged. This test does not
change controller settings or actuate outputs. `tools/verify-ui.cjs` switches to
read-only checks when `SMARTCONFIG_BOARD_URL` is set; `tools/stress-board.cjs`
checks uptime and heap during concurrent read-only traffic. These checks use
credentials from the private local configuration without logging them.
