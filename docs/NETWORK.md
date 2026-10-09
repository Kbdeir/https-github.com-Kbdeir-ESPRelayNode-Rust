# LAN Services

## Activation and Access

`management.enabled` and a configured password are required to start network
services. Import with `-EnableNetwork` to copy reference Wi-Fi/MQTT settings and
generate a private password in ignored `config.local.json`. `-AllowAnyModbus`
also enables TCP port 502 with `allowed_clients: ["*"]`. Otherwise configure
individual client IPv4 addresses before enabling Modbus. Valid NVS settings
take precedence over the embedded bootstrap; flashing does not erase them.

UART prints the connected station IP, not passwords. Browse to that IP; Basic
login uses the `management` credentials in the private local file. A WPA2 setup
AP `SmartConfig-<node-id>` stays available at `192.168.71.1` using the same private
password (first 63 bytes if necessary). Production AP provisioning policy is pending.

HTTP Basic and MQTT are not encrypted in this port. Modbus has no authentication
and allows actuator writes from permitted clients. Use an isolated trusted LAN,
never port-forward these services. Do not publish credential-bearing build artifacts.

## Web and HTTP

Original pages: `/`, `/RelayConfig.html`, `/InputsConfig.html`,
`/Input_Relays_Map.html`, `/Timer1?GetTimer=1` (slots 1-4),
`/LiveReadings.html`, `/LiveReadingsRest.html`. `/ModbusConfig.html` adds matching
server/peer settings. `/Automation.html` ports the ESP-IDF-ESPRelayNode8266 rule
and remote-sensor page. Original style blocks are unchanged. Unsupported local
sensor drivers, VPN and OTA controls are unavailable, not simulated as working.
The multi-board REST view needs a future CORS/gateway design for cross-board access.
All pages and JS are served from LittleFS. `/Files.html` manages the partition;
see [FILESYSTEM.md](FILESYSTEM.md) for deployment, limits and recovery.

All firmware requests require Basic authentication. POST also requires
`X-SmartConfig: 1`, supplied by the pages. GET cannot change output/settings.
Bodies/configurations are limited to 8192 bytes.
HTML is streamed in chunks no larger than 768 bytes. Under low free heap or
contiguous-block pressure, HTTP returns 503 instead of starting another response.
JSON nesting is limited before parsing. These bounds reduce resource risk;
they do not make every standard-library allocation infallible.

| Endpoint | Behavior |
| --- | --- |
| GET `/api/status` | Runtime/queue/network status |
| GET `/api/heap` | Actual ESP32 heap diagnostics |
| GET `/api/relay/state` | Relay 0 output, remaining TTL/TTA |
| POST `/api/relay` | JSON `{"relay":0,"action":"on"}`; `off`, `toggle` also accepted |
| GET `/GetConfig.json`, `/InputsConfig.json` | Legacy shape; secret fields blank |
| POST `/InputsConfig.json` | Validated input settings |
| POST `/api/config` | Complete typed configuration including secrets |
| GET/POST `/api/modbus` | Server/peer status / validated settings update |
| GET `/api/automation` | Four rules, eight remote sensors, condition and delivery status |
| POST `/api/automation/rule`, `/api/automation/remote` | Validated form updates; NVS save and live apply |
| POST `/api/reboot` | Explicit relay-OFF reboot; also available beside Save |
| GET `/api/files/list`, `/api/files/content?path=...` | Storage listing / raw streamed download |
| POST `/api/files/content?path=...`, `/api/files/rename`, `/api/files/delete` | Atomic streamed upload / form rename/delete; no NVS changes |
| GET `/WifiStatus.json`, `/MqttStatus.json` | Connection status |
| GET `/WifiScan.json?start=1` | Start scan; poll without query for results |
| GET `/TimerStatus.json`, `/api/live` | Timer/live compatibility data |
| POST `/TimerControl.json` | Form `relay=0&action=pause` or `resume` |

Original form-save routes work. Blank secret fields retain current credentials.
Successful management saves persist and apply live without resetting the board.
Only changes to Wi-Fi SSID/password schedule a three-second relay-OFF reboot.
Failed saves leave active config and worker revisions unchanged. The control loop
adopts settings at its next tick without waiting on the HTTP/NVS settings lock.
TTL/TTA edits retain the original activation time; shortening an expired TTL
switches the relay OFF. Unchanged rules retain their hold/cooldown and edge state.
Changes to remote source definitions discard cached values and cancel affected
queued writes. MQTT and Modbus workers retire and are joined before replacements
start, so protocol edits may briefly reconnect (an in-flight Modbus transaction
has bounded connect/response deadlines). Disabled protocols release task stacks.
SNTP server and timezone changes apply live. Web authentication changes immediately;
the setup AP retains its boot password until the next Wi-Fi restart.
Factory reset, backup restore and OTA retain their intentional reboot behavior.
Live Readings polls the actual `/api/heap` diagnostics after sensor reads rather
than expecting absent heap fields in the legacy configuration JSON.
Pause clears schedule demand,
transitioning an active schedule OFF; manual ON without an active schedule is
unchanged. Resume evaluates the current window without replaying missed time.

## MQTT

The importer retains relay command/state, TTL command/value, elapsed and input
topic names. Command payloads are `on`, `off`, `tog`, including uppercase forms;
TTL is decimal seconds 0-604800. Retained, oversized and fragmented commands
are rejected. State is not published to the command topic.

State/TTL/elapsed snapshots are retained QoS 1. Input events and acknowledgements
are non-retained QoS 1. Availability is `smartconfig/<node-id>/availability`,
retained online/offline with offline LWT. Online follows subscription acknowledgements.
Command outcomes use `smartconfig/<node-id>/ack`; `persisted` is null for ordinary
actions and true/false for applied MQTT TTL updates. TTL activates before storage
completes; failed persistence does not undo it. Bounded buffers can drop outcomes,
so acknowledgements are not durable guarantees. Disconnected events are cleared,
not replayed. Modbus TTL writes are runtime-only; HTTP and MQTT TTL settings persist.
Home Assistant discovery, structured commands and MQTT TLS remain pending.
Configured remote-sensor topics are also subscribed; payloads are scalar values
or a top-level JSON key. Retained/fragmented sensor messages are rejected and
invalid values do not refresh freshness. See [AUTOMATION.md](AUTOMATION.md).

## Modbus Map Version 1

Defaults: TCP 502, unit 1. Offsets are zero-based; uint32 values use high word
first. Four connections and four read-only peer targets are supported. Partial
frames and stalled sockets have bounded waits.

| Object | Offset | Value/access |
| --- | --- | --- |
| Coil | 0 | Relay 0; FC1 read, FC5/FC15 single-coil write |
| Discrete inputs | 0-5 | GPIO33,16,17,32,26,27; read-only |
| Input registers | 0-1 | Remaining TTL seconds, uint32 |
| Input registers | 2-3 | Remaining TTA seconds, uint32 |
| Input registers | 4-5 | Elapsed ON seconds, uint32 |
| Input register | 6 | Input validity bitmask |
| Holding register | 0 | Map version 1, read-only |
| Holding registers | 1-2 | Runtime TTL; atomic FC16 pair write |
| Holding registers | 3-4 | Configured TTA, read-only |
| Holding register | 5 | Interlocked boolean, read-only |
| Holding register | 6 | Input validity bitmask, read-only |

Half-register TTL writes and unsupported addresses return exceptions. Writes
become control commands, acknowledged after control accepts them. Peers read a
configured coil using FC1 and expose valid/stale status. Eight additional remote
sensors support FC1/2/3/4 and typed/scaled register values. Rules can queue FC5
coil writes to other IPv4 controllers. The client serializes bounded transactions,
retries current writes and cancels superseded demand; it refuses its own STA/AP
addresses. No remote Toggle or read-modify-write operation is supported.

## Hardware Acceptance

1. Enter COM3 download mode manually and flash without automatic reset.
2. After completion release BOOT, press RESET, verify OFF boot, heartbeat,
   no watchdog/stack faults, station IP and protected setup AP.
3. Check unauthorized HTTP rejection, authenticated pages, redacted status and
   read-only Modbus map/version. Confirm broker online/subscriptions.
4. With actuators made safe, exercise inputs, ON/OFF, TTL/TTA, schedule boundaries,
   overlap/pause and local OFF under remote traffic; reject retained MQTT ON.
5. Interrupt Wi-Fi/broker and stall clients while local inputs/TTL operate.
   Measure stack/heap under traffic and verify two-node read polling.
6. Inject invalid configs and NVS power loss; validate recovery, then perform
   24-hour and multi-day soak testing before production use.
