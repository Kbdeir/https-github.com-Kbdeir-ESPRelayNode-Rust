const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const base = process.env.SMARTCONFIG_BOARD_URL;
assert.ok(base, 'Set SMARTCONFIG_BOARD_URL');
assert.equal(process.env.SMARTCONFIG_ALLOW_OTA_TEST, '1', 'OTA tests reboot the board; require explicit authorization');
const credentials = JSON.parse(fs.readFileSync(path.join(__dirname, '..', 'config.local.json'), 'utf8')).management;
const headers = {Authorization: 'Basic ' + Buffer.from(credentials.username + ':' + credentials.password).toString('base64')};
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
async function json(uri) {
  const response = await fetch(base + uri, {headers, signal: AbortSignal.timeout(5000)});
  assert.equal(response.status, 200, uri);
  return response.json();
}
async function settings() {
  return Buffer.from((await json('/api/backup'))['/rust-config.json'], 'base64');
}
async function upload(uri, filename, original) {
  assert.ok((await json('/api/maintenance')).ready, 'Startup health check must pass first');
  const before = await json('/api/status');
  const bytes = fs.readFileSync(filename);
  console.log(`Uploading ${path.basename(filename)}: ${bytes.length} bytes`);
  const response = await fetch(base + uri, {method: 'POST', headers: {...headers, 'X-SmartConfig': '1', 'Content-Type': 'application/octet-stream'}, body: bytes, signal: AbortSignal.timeout(120000)});
  const message = await response.text();
  assert.ok(response.status === 200 && message.startsWith('OK'), `${uri}: HTTP ${response.status}: ${message}`);
  const deadline = Date.now() + 120000;
  let rebootObserved = false;
  while (Date.now() < deadline) {
    await sleep(1000);
    try {
      const state = await json('/api/status');
      if (state.runtime.uptime_seconds < before.runtime.uptime_seconds) rebootObserved = true;
      if (rebootObserved && !state.restart_pending && (await json('/api/maintenance')).ready) {
        assert.ok((await settings()).equals(original), 'OTA changed NVS controller settings');
        assert.equal((await json('/api/files/list')).files.length, 14);
        console.log(`${uri}: automatic reboot and 30-second health confirmation passed; NVS unchanged`);
        return;
      }
    } catch (error) {
      if (error.message.includes('OTA changed') || error.code === 'ERR_ASSERTION') throw error;
    }
  }
  throw new Error(`${uri}: no healthy reboot observed`);
}
(async () => {
  const original = await settings();
  const target = process.env.CARGO_TARGET_DIR || 'C:/esp/rn';
  await upload('/update', path.join(target, 'application-ota.bin'), original);
  await upload('/updatefs', path.join(target, 'littlefs.bin'), original);
  console.log('Both hardware OTA workflows passed; original controller settings preserved.');
})().catch(error => {console.error(error.message);process.exitCode=1;});
