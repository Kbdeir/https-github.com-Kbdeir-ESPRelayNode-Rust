const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const base = process.env.SMARTCONFIG_BOARD_URL;
assert.ok(base, 'Set SMARTCONFIG_BOARD_URL');
const root = path.resolve(__dirname, '..');
const config = JSON.parse(fs.readFileSync(path.join(root, 'config.local.json'), 'utf8'));
const credentials = config.management;
const headers = {Authorization: 'Basic ' + Buffer.from(credentials.username + ':' + credentials.password).toString('base64')};
async function get(uri) {
  const response = await fetch(base + uri, {headers, signal: AbortSignal.timeout(60000)});
  const bytes = Buffer.from(await response.arrayBuffer());
  assert.equal(response.status, 200, uri + ': HTTP ' + response.status);
  return {response, bytes};
}
(async () => {
  const before = JSON.parse((await get('/api/status')).bytes);
  const maintenance = JSON.parse((await get('/api/maintenance')).bytes);
  assert.equal(maintenance.otaSupported, true);
  assert.equal(maintenance.otaSlotBytes, 1638400);
  assert.equal(maintenance.filesystemBytes, 786432);
  const backup = await get('/api/backup?includeWeb=1');
  assert.ok(backup.response.headers.get('content-disposition').includes('smartconfig-backup.json'));
  const archive = JSON.parse(backup.bytes);
  assert.equal(archive._backup_version, 1);
  assert.equal(archive._chipid, before.node);
  assert.equal(archive._includesWeb, true);
  const native = JSON.parse(Buffer.from(archive['/rust-config.json'], 'base64'));
  assert.ok(native.schema === 1 && native.timers.length === 4 && native.automation.length === 4 && native.remote_sensors.length === 8, 'Backup must contain complete typed controller settings');
  const listing = JSON.parse((await get('/api/files/list')).bytes);
  for (const entry of listing.files) {
    assert.ok(typeof archive[entry.name] === 'string', 'Backup missing ' + entry.name);
    const raw = await get('/api/files/content?path=' + encodeURIComponent(entry.name));
    assert.ok(Buffer.from(archive[entry.name], 'base64').equals(raw.bytes), 'Backup file bytes changed: ' + entry.name);
  }
  const application = fs.readFileSync(path.join(process.env.CARGO_TARGET_DIR || 'C:/esp/rn', 'application-ota.bin'));
  const firmware = await get('/api/firmware/download');
  assert.equal(firmware.bytes.length, maintenance.otaSlotBytes);
  assert.ok(firmware.bytes.subarray(0, application.length).equals(application), 'Firmware download differs from deployed image');
  const filesystem = await get('/api/filesystem/download');
  assert.equal(filesystem.bytes.length, maintenance.filesystemBytes);
  const publicBytes = (await get('/GetConfig.json')).bytes;
  const publicConfig = JSON.parse(publicBytes);
  assert.ok(publicConfig.pass === '' && publicConfig.mqttPass === '', 'Password fields must remain blank');
  assert.ok(!publicBytes.toString().includes(native.management.password), 'Management password appeared in ordinary configuration');
  for (const uri of ['/api/backup', '/api/firmware/download', '/api/filesystem/download', '/FirmwareMaintenance.html', '/Backup.html']) {
    const response = await fetch(base + uri, {signal: AbortSignal.timeout(15000)});
    await response.arrayBuffer();
    assert.equal(response.status, 401, 'Unauthenticated maintenance route: ' + uri);
  }
  const after = JSON.parse((await get('/api/status')).bytes);
  assert.ok(after.runtime.uptime_seconds >= before.runtime.uptime_seconds, 'Board restarted during downloads');
  console.log(JSON.stringify({result:'Read-only board maintenance checks passed', assets:listing.files.length, backupBytes:backup.bytes.length, firmwareBytes:firmware.bytes.length, filesystemBytes:filesystem.bytes.length, uptimeBefore:before.runtime.uptime_seconds, uptimeAfter:after.runtime.uptime_seconds, heap:JSON.parse((await get('/api/heap')).bytes)}));
})().catch(error => { console.error(error.message); process.exitCode = 1; });
