const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const base = process.env.SMARTCONFIG_BOARD_URL;
assert.ok(base, 'Set SMARTCONFIG_BOARD_URL to the board URL');
const root = path.resolve(__dirname, '..');
const credentials = JSON.parse(fs.readFileSync(path.join(root, 'config.local.json'), 'utf8')).management;
const headers = {Authorization: 'Basic ' + Buffer.from(credentials.username + ':' + credentials.password).toString('base64')};
const pages = ['/', '/Automation.html', '/LiveReadingsRest.html', '/RelayConfig.html'];
async function get(uri) {
  const response = await fetch(base + uri, {headers, signal: AbortSignal.timeout(15000)});
  const body = await response.text();
  assert.ok([200, 503].includes(response.status), `${uri}: HTTP ${response.status}`);
  return {status: response.status, body};
}
async function json(uri) {
  const response = await get(uri);
  assert.equal(response.status, 200, uri);
  return JSON.parse(response.body);
}
(async () => {
  const before = await json('/api/status');
  const heapBefore = await json('/api/heap');
  let served = 0, busy = 0, bytes = 0;
  for (let round = 0; round < 10; round++) {
    const replies = await Promise.all(pages.map(get));
    for (const reply of replies) {
      if (reply.status === 503) { busy++; continue; }
      served++;
      bytes += Buffer.byteLength(reply.body);
      assert.ok(reply.body.includes('</html>'), 'Truncated page');
    }
    const status = await json('/api/status');
    assert.ok(status.runtime.uptime_seconds >= before.runtime.uptime_seconds, 'Board restarted during page traffic');
    console.log(`Round ${round + 1}/10: uptime ${status.runtime.uptime_seconds}s`);
  }
  const after = await json('/api/status');
  const heapAfter = await json('/api/heap');
  assert.ok(served > 0, 'No pages served');
  console.log(JSON.stringify({served, busy, bytes, uptimeBefore: before.runtime.uptime_seconds, uptimeAfter: after.runtime.uptime_seconds, heapBefore, heapAfter}, null, 2));
})().catch(error => { console.error(error.message); process.exitCode = 1; });
