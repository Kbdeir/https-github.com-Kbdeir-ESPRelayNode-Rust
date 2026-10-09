# Automation Port

Reference: `C:\Users\kbdeir\Documents\PlatformIO\Projects\ESP-IDF-ESPRelayNode8266`.
Its Automation page CSS and layout are retained. Four rules and eight remote
sensors are available at `/Automation.html`; all requests require HTTP Basic.
Rule/sensor saves validate, persist to NVS, and apply live without a board restart.
Changed source definitions invalidate stale readings and affected queued writes;
unchanged rules retain their edge/hold/cooldown state.
Existing schema-1 configurations load with rules disabled and sensors empty.

## Rules

Two conditions use greater-than, less-than, inclusive-between or numeric equality
(absolute tolerance 0.001), combined with AND/OR. The second source is optional;
both configured sources must be available even for OR. Sources include local
debounced digital inputs 1-3, relay state and remote sensors 1-8. Local temperature
and current sources remain unavailable until real drivers are implemented.

Rules support On, Off and local Toggle; on-change or periodic triggers; optional
timer 1-4 gate; hold and cooldown times in seconds. Closed gates count as false:
On rules clear OFF and Off rules clear ON, subject to hold/cooldown. On-change
fires once per stable result, including the first available result. Periodic
rules require at least a one-second cooldown and cannot Toggle. Local OFF has
priority over rule ON in the same control pass; it is not a persistent interlock.

The output is local relay 0 or an IPv4 Modbus coil. Remote Toggle is prohibited.
With send-clear enabled, the inverse action is sent when the condition clears.
With send-clear disabled, unsent demand is canceled and the remote output is
left unchanged. Missing or stale data cancels unsent demand without issuing a
local or remote OFF. This reference behavior is not a safety shutdown policy.

## Remote Sensors

MQTT: exact topic, optional top-level JSON key, scalar number or on/off/boolean
text. Payloads are at most 512 bytes with bounded JSON depth. Topics cannot
duplicate each other or local command topics. Retained and fragmented values
are rejected, an intentional difference from the reference to avoid treating
old retained actuator-driving data as fresh. Invalid messages do not refresh age.

Modbus: IPv4 host, port, unit, zero-based address and FC1/2/3/4. Registers support
unsigned/signed 16/32-bit and float32 values, ABCD/CDAB/BADC/DCBA order, scale and
offset. Poll interval is 5-300 seconds. Nonfinite values are rejected. Failed
polls retain the last value until its five-minute freshness limit expires.

## Delivery and Limits

Local actions pass through the existing engine; only control owns GPIO25.
Remote FC5 writes use one latest-demand slot per rule. A failed write retries
after five seconds only while its revision and control-renewed lease remain
valid. The worker verifies the reply echoes the requested address/value and
blocks writes to this board's STA/AP addresses. Transactions are serialized
with bounded connect/write and whole-response deadlines. A request already
sent over TCP cannot be undone when demand changes.

Condition status and remote pending/failure/acknowledged state are separate.
Queue acceptance is not proof of remote actuation; Modbus acknowledgement is
not physical contact feedback. The deployed broker is currently unreachable
on another network, so actual MQTT and remote-controller acceptance still
require connectivity and safe actuator testing. Host tests and preview checks
do not qualify this firmware for unattended industrial use.
